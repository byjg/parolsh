//! The interactive loop: read a line, route it, act on it.

use anyhow::{Context, Result};
use reedline::{
    ColumnarMenu, Emacs, ExternalPrinter, FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder,
    Reedline, ReedlineEvent, ReedlineMenu, Signal, default_emacs_keybindings,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::acp::AgentHandle;
use crate::audit::{self, Actor, Audit};
use crate::complete::ShellCompleter;
use crate::config::{Agent, Config, OptionValue, PromptStyle};
use crate::input::{Input, Mode, SharedMode, route};
use crate::title::Title;
use crate::{config, hints, project, setup, shell, shellenv, turn, ui};

const HELP: &str = "\
Input:
  <text>          ask the agent (natural language); a shell command when locked
  ?<text>         ask the agent, locked or not
  /<command>      forwarded unchanged to the agent
  !<command>      run a shell command (bash -ic)
  !+<command>     run it and send its output with your next message
  !bash           open an interactive Bash session
  !               lock: plain text goes to bash (prompt ❯)
  ?               unlock: plain text goes to the agent (prompt ✦)
  #<command>      Parolsh control command

Control commands:
  #help           show this help
  #new            start a new conversation
  #cd <path>      switch to another directory or project
  #agent [list]   show the active agent, or list the configured ones
  #agent <name>   switch to another agent (new conversation)
  #config [sample] show the configuration files, or print the full sample
  #options        show the agent's options (effort, model, ...)
  #options <id> <value>  set one until you leave Parolsh
  #project [init] show the project root, or create .parolsh/ here
  #prompt [name]  show the prompt style, or switch: parolsh, starship, minimal
  #audit          what ran here, what the agent got, and what it did
  #exit           leave Parolsh";

pub struct App {
    /// Parolsh's directory: the agent's and the project's, set by `#cd`.
    cwd: PathBuf,
    /// Where shell commands run: `cwd` until a command ends in another
    /// directory (`cd`), and again after `#cd`.
    shell_cwd: PathBuf,
    /// `shell_cwd`, shared with the Tab completion.
    completion_cwd: Arc<Mutex<PathBuf>>,
    /// Where plain text goes, shared with the colors, hints and completion.
    mode: SharedMode,
    project_root: Option<PathBuf>,
    config: Config,
    /// Name of the agent in use: `default_agent`, until `#agent <name>`.
    active: Option<String>,
    agent: Option<AgentHandle>,
    /// Set by `#prompt <name>`, or to fall back when Starship fails.
    prompt_override: Option<PromptStyle>,
    /// Shown once after the banner: what the first run did, or why the
    /// shell environment could not be loaded.
    notices: Vec<String>,
    /// Outputs of `!+` commands, sent with the next message.
    shared: Vec<Shared>,
    /// Exit code and duration of the last command, for the prompt.
    last_status: i32,
    last_duration: Duration,
    /// What happened in this run, for `#audit`.
    audit: Audit,
    /// Prints what the agent does between turns above the prompt.
    printer: ExternalPrinter<String>,
    /// Makes the prompt return, for the agent's questions between turns.
    interrupt: Arc<AtomicBool>,
    /// The terminal's title, while `run` runs on an ANSI terminal.
    title: Option<Title>,
}

enum Flow {
    Continue,
    Exit,
}

impl App {
    /// `notices` are shown after the banner, with the first run's.
    /// `input` is `--input`: where plain text goes at start, over the
    /// configuration's `input`.
    pub fn new(cwd: PathBuf, mut notices: Vec<String>, input: Option<Mode>) -> Result<Self> {
        // Before loading: the first run may write the global configuration.
        let search_path = std::env::var("PATH").ok();
        notices.extend(
            config::global_path().and_then(|path| setup::first_run(&path, search_path.as_deref())),
        );
        let project_root = project::find_root(&cwd);
        let config = Config::load(project_root.as_deref())?;
        let audit = Audit::new(config.audit_entries);
        let mut app = Self {
            completion_cwd: Arc::new(Mutex::new(cwd.clone())),
            mode: SharedMode::default(),
            shell_cwd: cwd.clone(),
            cwd,
            project_root,
            active: config.default_agent.clone(),
            config,
            agent: None,
            prompt_override: None,
            shared: Vec::new(),
            notices,
            last_status: 0,
            last_duration: Duration::ZERO,
            audit,
            printer: ExternalPrinter::new(PRINTER_LINES),
            interrupt: Arc::new(AtomicBool::new(false)),
            title: None,
        };
        // Without an agent, plain text can only go to the shell.
        app.mode.set(match (&app.active, input) {
            (None, _) => Mode::Shell,
            (Some(_), Some(input)) => input,
            (Some(_), None) => app.config.input,
        });
        app.start_agent();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        // Bracketed paste: a pasted text with line breaks is one input, not
        // one Enter per line.
        let mut editor = Reedline::create()
            .use_bracketed_paste(true)
            .with_external_printer(self.printer.clone())
            .with_break_signal(self.interrupt.clone());
        if ui::is_ansi() {
            // Tips while typing: the color and a hint of where the line goes.
            editor = editor
                .with_highlighter(Box::new(hints::InputHighlighter {
                    mode: self.mode.clone(),
                }))
                .with_hinter(Box::new(hints::PrefixHinter {
                    mode: self.mode.clone(),
                }));
            editor = self.with_completion(editor);
        }
        if let Some(path) = history_path() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            editor = editor.with_history(Box::new(FileBackedHistory::with_file(1000, path)?));
        }
        if ui::is_ansi() {
            self.print_banner();
        }
        for notice in self.notices.drain(..) {
            println!("{notice}");
        }
        self.title = Title::start(self.place());

        loop {
            if let Some(title) = &self.title {
                title.set_place(self.place());
                title.set_agent(self.agent.as_ref().map(AgentHandle::activity));
            }
            let prompt = self.prompt();
            match self.read_line(&mut editor, &prompt)? {
                Signal::Success(line) => {
                    if let Flow::Exit = self.handle(route(&line, self.mode.get())) {
                        return Ok(());
                    }
                }
                Signal::CtrlC => continue,
                Signal::CtrlD => return Ok(()),
                _ => continue,
            }
        }
    }

    /// Reads a line, showing meanwhile what the agent does between turns.
    /// The agent's questions are answered after the prompt returns.
    fn read_line(&mut self, editor: &mut Reedline, prompt: &ui::Prompt) -> Result<Signal> {
        let Some(agent) = &self.agent else {
            return Ok(editor.read_line(prompt)?);
        };
        let display = self.display();
        let stop = AtomicBool::new(false);
        let repaint_signal = editor.repaint_signal();
        let repaint = move || repaint_signal.request_repaint();
        let (signal, watched) = std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                turn::watch(
                    agent,
                    display,
                    &self.printer,
                    &repaint,
                    &stop,
                    &self.interrupt,
                    &mut self.audit,
                )
            });
            let signal = editor.read_line(prompt);
            stop.store(true, Ordering::SeqCst);
            (signal, watcher.join().unwrap_or_default())
        });
        // A question that came with the Enter key would break the next prompt.
        self.interrupt.store(false, Ordering::SeqCst);

        let late = std::iter::from_fn(|| self.printer.get_line());
        let lines: Vec<String> = late.chain(watched.lines).collect();
        if !lines.is_empty() || watched.request.is_some() || watched.stopped {
            // Below the line the prompt was on.
            println!();
        }
        for line in lines {
            println!("{line}");
        }
        if let Some(request) = watched.request {
            turn::answer(request, &mut self.audit);
        }
        if watched.stopped {
            self.audit.add(Actor::Parolsh, "the agent stopped");
            eprintln!("parolsh: the agent stopped. Run #new to start it again.");
            self.agent = None;
        }
        Ok(signal?)
    }

    fn display(&self) -> turn::Display {
        turn::Display {
            thinking: self.config.thinking,
            markdown: self.config.markdown,
            links: self.config.links,
        }
    }

    /// The agent's directory, for the title until the agent gives one.
    fn place(&self) -> String {
        ui::short_path(&self.cwd, home().as_deref())
    }

    fn handle(&mut self, input: Input) -> Flow {
        let started = Instant::now();
        // Commands may set their own title.
        let _held = match &input {
            Input::Shell(_) | Input::Bash | Input::Share(_) => self.title.as_ref().map(Title::hold),
            _ => None,
        };
        let status = match input {
            Input::Empty => return Flow::Continue,
            Input::Agent(text) => self.ask(text),
            Input::Shell(line) => {
                let code = report(shell::run(&self.config.shell, &line, &self.shell_cwd).map(
                    |(code, dir)| {
                        self.move_shell(dir);
                        code
                    },
                ));
                self.audit.add(
                    Actor::User,
                    format!("!{} · exit {code}", audit::excerpt(&line, 120)),
                );
                code
            }
            Input::Bash => {
                let code = report(shell::run_foreground(shell::bash(&self.shell_cwd)));
                self.audit.add(Actor::User, format!("!bash · exit {code}"));
                code
            }
            Input::Share(line) => self.share(line),
            Input::Lock(mode) => {
                self.mode.set(mode);
                0
            }
            Input::Control { name, args } => match self.control(&name, &args) {
                Some(status) => status,
                None => return Flow::Exit,
            },
        };
        self.last_status = status;
        self.last_duration = started.elapsed();
        Flow::Continue
    }

    /// The prompt for the next line, in the configured style.
    fn prompt(&mut self) -> ui::Prompt {
        let style = self.prompt_override.unwrap_or(self.config.prompt);
        let ansi = ui::is_ansi();
        let home = home();
        let context = ui::PromptContext {
            cwd: &self.shell_cwd,
            agent_cwd: (self.shell_cwd != self.cwd).then_some(self.cwd.as_path()),
            status: self.last_status,
            duration: self.last_duration,
            mode: self.mode.get(),
            // Only a running agent answers.
            agent: self.agent.as_ref().and(self.banner_agent()),
        };
        let mut fell_back = false;
        let prompt = match style {
            PromptStyle::Minimal => ui::Prompt::minimal(&context, ansi),
            // Starship prints ANSI colors: only on an ANSI terminal.
            PromptStyle::Starship if ansi => match ui::Prompt::starship("starship", &context) {
                Ok(prompt) => prompt,
                Err(e) => {
                    eprintln!("parolsh: starship: {e}. Using the parolsh prompt.");
                    fell_back = true;
                    ui::Prompt::parolsh(&context, home.as_deref(), ansi)
                }
            },
            _ => ui::Prompt::parolsh(&context, home.as_deref(), ansi),
        };
        if fell_back {
            self.prompt_override = Some(PromptStyle::Parolsh);
        }
        match &self.agent {
            Some(agent) => prompt.with_background(agent.activity(), ansi),
            None => prompt,
        }
    }

    fn options_command(&self, args: &str) -> Result<()> {
        let Some(agent) = &self.agent else {
            anyhow::bail!("{}", no_agent());
        };
        let options = agent.options();
        if args.is_empty() {
            if options.is_empty() {
                println!("The agent offers no options (or its session has not started yet).");
            }
            for option in &options {
                let values: Vec<String> = option
                    .values
                    .iter()
                    .map(|v| {
                        if *v == option.current {
                            format!("{v}*")
                        } else {
                            v.clone()
                        }
                    })
                    .collect();
                println!("  {:<18} {}", option.id, values.join(", "));
            }
            return Ok(());
        }

        let Some((id, value)) = args.split_once(char::is_whitespace) else {
            anyhow::bail!("usage: #options <id> <value>");
        };
        let value = value.trim();
        let Some(option) = options.iter().find(|option| option.id == id) else {
            let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
            anyhow::bail!("no option `{id}`. The agent offers: {}", ids.join(", "));
        };
        if !option.values.iter().any(|v| v == value) {
            anyhow::bail!(
                "`{id}` has no value `{value}`. Values: {}",
                option.values.join(", ")
            );
        }
        agent.set_option(id.to_string(), OptionValue::parse(value));
        println!("{id} = {value}, until you leave Parolsh.");
        Ok(())
    }

    /// `#audit`: the entries of this run, oldest first.
    fn audit_command(&self) {
        let lines = self.audit.lines(ui::width());
        if lines.is_empty() {
            println!("Nothing yet: commands, messages and what the agent does show up here.");
        }
        for line in lines {
            println!("{line}");
        }
    }

    fn config_command(&self, args: &str) -> Result<()> {
        match args {
            "" => {
                let status = |path: &Path| {
                    let state = if path.exists() { "" } else { "  (not found)" };
                    format!("{}{state}", path.display())
                };
                match config::global_path() {
                    Some(path) => println!("Global:  {}", status(&path)),
                    None => println!("Global:  none ($HOME is not set)"),
                }
                match &self.project_root {
                    Some(root) => println!("Project: {}", status(&config::project_path(root))),
                    None => println!("Project: none (#project init creates one here)"),
                }
                println!("#config sample prints every option, with a block for each agent.");
                Ok(())
            }
            "sample" => {
                print!("{}", setup::SAMPLE);
                Ok(())
            }
            _ => anyhow::bail!("usage: #config [sample]"),
        }
    }

    fn prompt_command(&mut self, args: &str) -> Result<()> {
        if args.is_empty() {
            println!(
                "{}",
                self.prompt_override.unwrap_or(self.config.prompt).name()
            );
            return Ok(());
        }
        let Some(style) = PromptStyle::from_name(args) else {
            let names: Vec<&str> = PromptStyle::ALL.iter().map(|style| style.name()).collect();
            anyhow::bail!("no prompt named `{args}`. Available: {}", names.join(", "));
        };
        self.prompt_override = Some(style);
        Ok(())
    }

    /// Runs a `#command`. Returns its status, or `None` to leave Parolsh.
    fn control(&mut self, name: &str, args: &str) -> Option<i32> {
        let result = match name {
            "help" => {
                println!("{HELP}");
                Ok(())
            }
            "exit" => return None,
            "new" => {
                self.new_conversation();
                Ok(())
            }
            "cd" => self.change_dir(args),
            "agent" => self.agent(args),
            "project" => self.project(args),
            "prompt" => self.prompt_command(args),
            "config" => self.config_command(args),
            "options" => self.options_command(args),
            "audit" => {
                self.audit_command();
                return Some(0);
            }
            _ => Err(anyhow::anyhow!("unknown command `#{name}`, see #help")),
        };
        if name != "help" {
            let failed = if result.is_err() { " · failed" } else { "" };
            let line = format!("#{name} {args}");
            self.audit
                .add(Actor::User, format!("{}{failed}", line.trim_end()));
        }
        match result {
            Ok(()) => Some(0),
            Err(e) => {
                eprintln!("parolsh: {e:#}");
                Some(1)
            }
        }
    }

    /// Tab completes `!command` lines, as bash does: a single match goes into
    /// the line, several fill in what they share, then show a menu.
    fn with_completion(&self, editor: Reedline) -> Reedline {
        let names = Arc::new(OnceLock::new());
        let shell = self.config.shell[0].clone();
        let is_bash = Path::new(&shell)
            .file_name()
            .is_some_and(|name| name == "bash");
        if is_bash {
            // In the background: loading ~/.bashrc takes a moment, and Tab
            // works with the PATH commands until it is done.
            let names = names.clone();
            let shell = shell.clone();
            std::thread::spawn(move || {
                if let Ok(list) = shellenv::bash_names(&shell) {
                    let _ = names.set(list);
                }
            });
        }
        let completer = ShellCompleter {
            cwd: self.completion_cwd.clone(),
            names,
            mode: self.mode.clone(),
            bash: is_bash.then_some(shell),
        };
        let mut keybindings = default_emacs_keybindings();
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Tab,
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::Menu("completion_menu".to_string()),
                ReedlineEvent::MenuNext,
            ]),
        );
        keybindings.add_binding(
            KeyModifiers::SHIFT,
            KeyCode::BackTab,
            ReedlineEvent::MenuPrevious,
        );
        let menu = ColumnarMenu::default().with_name("completion_menu");
        editor
            .with_completer(Box::new(completer))
            .with_menu(ReedlineMenu::EngineCompleter(Box::new(menu)))
            .with_edit_mode(Box::new(Emacs::new(keybindings)))
            .with_quick_completions(true)
            .with_partial_completions(true)
    }

    fn change_dir(&mut self, args: &str) -> Result<()> {
        let target = resolve_dir(&self.shell_cwd, args)?;
        let project_root = project::find_root(&target);
        // A different project brings its own config; fail before moving.
        let config = Config::load(project_root.as_deref())?;
        let previous = self.active_agent().cloned();
        // Keep the agent chosen with `#agent` when the new project has it.
        if !self
            .active
            .as_ref()
            .is_some_and(|name| config.agents.contains_key(name))
        {
            self.active = config.default_agent.clone();
        }
        // A project that sets another `input` brings it.
        if config.input != self.config.input {
            self.mode.set(config.input);
        }
        self.config = config;
        self.audit.set_limit(self.config.audit_entries);
        self.cwd = target.clone();
        self.move_shell(Some(target));
        self.project_root = project_root;

        // Same agent: a new conversation in the new directory. Otherwise the
        // project wants another agent (or other settings): restart it.
        match &self.agent {
            Some(agent)
                if previous.as_ref() == self.active_agent()
                    && agent.new_session(self.cwd.clone()) => {}
            _ => self.start_agent(),
        }
        Ok(())
    }

    /// Where the next shell command runs: the directory the last one ended
    /// in, when the shell said.
    fn move_shell(&mut self, dir: Option<PathBuf>) {
        if let Some(dir) = dir {
            *self.completion_cwd.lock().expect("cwd lock") = dir.clone();
            self.shell_cwd = dir;
        }
    }

    /// `!+command`: runs it, keeping its output for the next message.
    fn share(&mut self, line: String) -> i32 {
        if line.is_empty() {
            eprintln!("parolsh: usage: !+<command>, e.g. !+docker ps");
            return 1;
        }
        let cwd = self.shell_cwd.clone();
        match shell::run_shared(&self.config.shell, &line, &cwd, SHARED_LIMIT) {
            Ok((code, captured, dir)) => {
                self.move_shell(dir);
                if code != 0 {
                    eprintln!("exit {code}");
                }
                println!("(output of `{line}` goes with your next message)");
                let kept = audit::size(captured.text.len());
                let last = if captured.truncated { " (its end)" } else { "" };
                self.audit.add(
                    Actor::User,
                    format!(
                        "!+{} · exit {code} · {kept} kept{last}",
                        audit::excerpt(&line, 120)
                    ),
                );
                self.shared.push(Shared {
                    command: line,
                    cwd,
                    code,
                    captured,
                });
                code
            }
            Err(e) => {
                eprintln!("parolsh: {e}");
                127
            }
        }
    }

    /// Sends `text` to the agent, after the outputs shared with `!+`.
    /// Returns the status for the prompt.
    fn ask(&mut self, text: String) -> i32 {
        let Some(agent) = &self.agent else {
            eprintln!("parolsh: {}", no_agent());
            return 1;
        };
        let name = self.active.as_deref().unwrap_or("the agent");
        if !self.shared.is_empty() {
            let bytes = self.shared.iter().map(|s| s.captured.text.len()).sum();
            self.audit.add(
                Actor::Shared,
                format!(
                    "{} output(s), {} → {name}",
                    self.shared.len(),
                    audit::size(bytes)
                ),
            );
        }
        self.audit.add(
            Actor::User,
            format!("→ {name}: {}", audit::excerpt(&text, 80)),
        );
        let mut blocks: Vec<String> = self.shared.drain(..).map(|s| s.block()).collect();
        blocks.push(text);
        match turn::run(agent, blocks, self.display(), &mut self.audit) {
            turn::Outcome::Finished => 0,
            turn::Outcome::Failed => 1,
            turn::Outcome::AgentStopped => {
                turn::drain(agent);
                self.audit.add(Actor::Parolsh, "the agent stopped");
                eprintln!("parolsh: the agent stopped. Run #new to start it again.");
                self.agent = None;
                1
            }
        }
    }

    fn new_conversation(&mut self) {
        // An agent that never ended a cancelled turn would only start the new
        // conversation after it: start the agent again instead.
        let restarted = match &self.agent {
            Some(agent) => agent.abandoned().is_some() || !agent.new_session(self.cwd.clone()),
            None => true,
        };
        if restarted {
            self.start_agent();
        }
        if self.agent.is_some() {
            println!("Started a new conversation.");
        } else {
            eprintln!("parolsh: {}", no_agent());
        }
    }

    /// Stops the running agent, if any, and starts the active one with a new
    /// conversation in the current directory.
    fn start_agent(&mut self) {
        self.agent = None;
        let agent = self
            .active_agent()
            .map(|agent| AgentHandle::start(agent, self.cwd.clone()));
        self.agent = agent;
    }

    fn active_agent(&self) -> Option<&Agent> {
        self.active
            .as_ref()
            .and_then(|name| self.config.agents.get(name))
    }

    fn agent(&mut self, args: &str) -> Result<()> {
        match args {
            "" => match &self.active {
                Some(name) => println!("{name}"),
                None => println!("{}", no_agent()),
            },
            "list" => self.list_agents(),
            name => self.switch_agent(name)?,
        }
        Ok(())
    }

    fn switch_agent(&mut self, name: &str) -> Result<()> {
        if !self.config.agents.contains_key(name) {
            let names: Vec<&str> = self.config.agents.keys().map(String::as_str).collect();
            anyhow::bail!(
                "no agent named `{name}`. Configured: {}",
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            );
        }
        if self.active.as_deref() == Some(name) && self.agent.is_some() {
            println!("Already using {name}.");
            return Ok(());
        }
        self.active = Some(name.to_string());
        self.start_agent();
        println!("Agent changed to {name}. Started a new conversation.");
        Ok(())
    }

    fn list_agents(&self) {
        if self.config.agents.is_empty() {
            println!("No agents configured.");
        }
        for (name, agent) in &self.config.agents {
            let active = self.active.as_deref() == Some(name);
            let command = format!("{} {}", agent.command, agent.args.join(" "));
            let mode = agent.mode.as_deref().unwrap_or("(agent default)");
            println!(
                "{} {name:<10} {:<24} mode: {mode}",
                if active { "*" } else { " " },
                command.trim_end()
            );
            // The modes only the running agent can tell.
            if active && let Some(modes) = self.agent.as_ref().and_then(AgentHandle::modes) {
                let offered: Vec<String> = modes
                    .available
                    .iter()
                    .map(|m| {
                        if *m == modes.current {
                            format!("{m}*")
                        } else {
                            m.clone()
                        }
                    })
                    .collect();
                println!("  {:<10} offers: {}", "", offered.join(", "));
            }
        }
    }

    fn project(&mut self, args: &str) -> Result<()> {
        if args == "init" {
            if !project::init(&self.cwd)? {
                println!("{} already exists.", project::STATE_DIR);
            }
            self.project_root = Some(self.cwd.clone());
            return Ok(());
        }
        match &self.project_root {
            Some(root) => println!("{}", root.display()),
            None => println!("Not in a project. Run #project init to create one here."),
        }
        Ok(())
    }

    fn banner_agent(&self) -> Option<ui::BannerAgent<'_>> {
        self.active.as_deref().map(|name| ui::BannerAgent {
            name,
            mode: self.active_agent().and_then(|agent| agent.mode.as_deref()),
        })
    }

    fn print_banner(&self) {
        let agent = self.banner_agent();
        let home = home();
        print!(
            "{}",
            ui::banner(env!("CARGO_PKG_VERSION"), agent, &self.cwd, home.as_deref())
        );
    }
}

/// Most of a `!+` command's output kept for the agent: the end of it.
const SHARED_LIMIT: usize = 16 * 1024;

/// A `!+` command and its output, waiting for the next message.
struct Shared {
    command: String,
    cwd: PathBuf,
    code: i32,
    captured: shell::Captured,
}

impl Shared {
    /// The text block sent to the agent.
    fn block(&self) -> String {
        let cut = if self.captured.truncated {
            format!(" (only its last {} KB)", SHARED_LIMIT / 1024)
        } else {
            String::new()
        };
        format!(
            "The user ran the shell command `{}` in {} (exit code {}). Its output{cut}:\n```\n{}\n```",
            self.command,
            self.cwd.display(),
            self.code,
            self.captured.text.trim_end()
        )
    }
}

/// Where to go when no agent is configured.
/// Lines the prompt holds for printing; the agent's lines beyond wait.
const PRINTER_LINES: usize = 64;

fn no_agent() -> String {
    match config::global_path() {
        Some(path) => format!(
            "no agent configured. Add one to {} and set default_agent (see #config).",
            path.display()
        ),
        None => "no agent configured (see #config).".to_string(),
    }
}

/// The exit code of a `!` command or `!bash`, printed when it failed.
fn report(result: std::io::Result<i32>) -> i32 {
    match result {
        Ok(0) => 0,
        Ok(code) => {
            eprintln!("exit {code}");
            code
        }
        Err(e) => {
            eprintln!("parolsh: {e}");
            127
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn resolve_dir(cwd: &Path, args: &str) -> Result<PathBuf> {
    let home = home();
    let path = match (args, &home) {
        ("" | "~", Some(home)) => home.clone(),
        (path, Some(home)) if path.starts_with("~/") => home.join(&path[2..]),
        (path, _) => cwd.join(path),
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("cannot change to {}", path.display()))?;
    anyhow::ensure!(path.is_dir(), "{} is not a directory", path.display());
    Ok(path)
}

fn history_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("parolsh").join("history"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tab completes the `#` commands `#help` lists, no more, no fewer.
    #[test]
    fn completion_knows_the_commands_help_lists() {
        // A command with several forms is listed once per form.
        let listed: std::collections::BTreeSet<&str> = HELP
            .lines()
            .skip_while(|line| !line.starts_with("Control commands:"))
            .filter_map(|line| line.trim_start().strip_prefix('#'))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let completed = crate::complete::CONTROL_COMMANDS.into_iter().collect();

        assert_eq!(listed, completed);
    }
}
