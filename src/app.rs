//! The interactive loop: read a line, route it, act on it.

use anyhow::{Context, Result};
use reedline::{
    ColumnarMenu, Emacs, ExternalPrinter, FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder,
    Prompt as _, PromptEditMode, Reedline, ReedlineEvent, ReedlineMenu, Signal,
    default_emacs_keybindings,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::acp::{AgentHandle, Block, ForkOf, Forks, Start};
use crate::audit::{self, Actor, Audit, Kind, Record};
use crate::complete::ShellCompleter;
use crate::config::{Agent, Commands, Config, OptionValue, PromptStyle};
use crate::history::{History, Session, SessionStart};
use crate::input::{Input, Mode, SharedMode, route};
use crate::jobs::{self, Job};
use crate::title::Title;
use crate::turn::Ended;
use crate::{
    config, hints, mcp, mention, project, replay, setup, shell, shellenv, turn, ui, version,
};
use agent_client_protocol::schema::v1::McpServer;

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
  #new private    start one that is not saved in the history
  #cd <path>      switch to another directory or project
  #agent [list]   show the active agent, or list the configured ones
  #agent <name>   switch to another agent (new conversation)
  #config [sample] show the configuration files, or print the full sample
  #options        show the agent's options (effort, model, ...)
  #options <id> <value>  set one until you leave Parolsh
  #project [init] show the project root, or create .parolsh/ here
  #prompt [name]  show the prompt style, or switch: parolsh, starship, minimal
  #audit [n]      what ran here, what the agent got, and what it did; n: an earlier session
  #sessions       the earlier sessions of this project, from the history
  #forget [n]     remove a session from the history; the current one without n
  #resume <n>     go back to session n with the agent, when it can (Claude, Codex)
  #redraw [n]     clear the terminal and show the last n exchanges again (Ctrl+L)
  #jobs           the turns sent to the background (Ctrl+Z during a turn)
  #fg [n]         wait for background turn n, or the last one (Ctrl+C cancels it)
  #exit           leave Parolsh";

pub struct App {
    /// Parolsh's directory: the agent's and the project's, set by `#cd`.
    cwd: PathBuf,
    /// Where shell commands run: `cwd` until a command ends in another
    /// directory (`cd`), and again after `#cd`.
    shell_cwd: PathBuf,
    /// What your shell commands changed in their environment (`export`,
    /// `unset`, `source`), applied to the next ones. In memory only, and not
    /// the agent's.
    shell_changes: shell::Changes,
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
    /// The history database, when there is one: the agent gets it as an MCP
    /// server.
    history_db: Option<PathBuf>,
    /// The session to go back to once the banner is printed.
    resume: Option<Resume>,
    /// Turns sent to the background with Ctrl+Z, until they are reported.
    jobs: Vec<Job>,
    /// The number of the next one.
    next_job: usize,
    /// Numbers the conversations: what a background turn answered only goes
    /// back to the one it left.
    conversation: u64,
    /// The current conversation is not saved (`#new private`).
    private: bool,
    /// What background turns answered, for the agent with the next message:
    /// its conversation was copied before them.
    handover: Vec<String>,
    /// Leaving was refused once, because something is still running.
    leaving: bool,
}

/// The session to go back to at start: `--resume <n>`, or `--continue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Session(i64),
    /// The last one of this project.
    Latest,
}

enum Flow {
    Continue,
    Exit,
}

impl App {
    /// `notices` are shown after the banner, with the first run's.
    /// `input` is `--input`: where plain text goes at start, over the
    /// configuration's `input`.
    /// `resume` is `--resume` or `--continue`: the session to go back to.
    pub fn new(
        cwd: PathBuf,
        mut notices: Vec<String>,
        input: Option<Mode>,
        resume: Option<Resume>,
    ) -> Result<Self> {
        // Before loading: the first run may write the global configuration.
        let search_path = std::env::var("PATH").ok();
        notices.extend(
            config::global_path().and_then(|path| setup::first_run(&path, search_path.as_deref())),
        );
        let project_root = project::find_root(&cwd);
        let config = Config::load(project_root.as_deref())?;
        let mut audit = Audit::new(config.audit_entries);
        let mut history_db = None;
        if config.history_days > 0
            && let Some(path) = state_dir().map(|dir| dir.join("history.db"))
        {
            match History::open(&path, config.history_days) {
                Ok(history) => {
                    audit = audit.with_history(history, config.commands != Commands::Off);
                    history_db = Some(path);
                    if config.commands == Commands::Shared {
                        notices.push(
                            "the agent can read your !commands in the history · \
                             commands = \"private\" keeps them to you"
                                .to_string(),
                        );
                    }
                }
                Err(e) => notices.push(format!(
                    "parolsh: cannot open the history {}: {e}; this run is not saved",
                    path.display()
                )),
            }
        }
        let mut app = Self {
            completion_cwd: Arc::new(Mutex::new(cwd.clone())),
            mode: SharedMode::default(),
            shell_cwd: cwd.clone(),
            shell_changes: shell::Changes::default(),
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
            history_db,
            resume,
            jobs: Vec::new(),
            next_job: 1,
            conversation: 0,
            private: false,
            handover: Vec::new(),
            leaving: false,
        };
        // Without an agent, plain text can only go to the shell.
        app.mode.set(match (&app.active, input) {
            (None, _) => Mode::Shell,
            (Some(_), Some(input)) => input,
            (Some(_), None) => app.config.input,
        });
        app.start_agent(false);
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
            keep_private(&path)?;
            editor = editor.with_history(Box::new(FileBackedHistory::with_file(1000, path)?));
        }
        if ui::is_ansi() {
            self.print_banner();
        }
        for notice in self.notices.drain(..) {
            println!("{notice}");
        }
        self.title = Title::start(self.place());
        if let Some(resume) = self.resume.take()
            && let Err(e) = self.resume_at_start(resume)
        {
            eprintln!("parolsh: {e:#}");
        }

        loop {
            if let Some(title) = &self.title {
                title.set_place(self.place());
                title.set_agent(self.agent.as_ref().map(AgentHandle::activity));
            }
            if let (Some(agent), Some(history)) = (&self.agent, self.audit.history_mut()) {
                // Until the agent says, keep what the history has: a resumed
                // session keeps its title.
                let activity = agent.activity().get();
                if activity.title.is_some() {
                    history.set_title(activity.title);
                }
                if activity.session_id.is_some() {
                    history.set_agent_session_id(activity.session_id);
                }
            }
            self.attend_jobs();
            let prompt = self.prompt();
            match self.read_line(&mut editor, &prompt)? {
                // A key bound to a command (Ctrl+L) is a line like any other.
                Signal::Success(line) | Signal::HostCommand(line) => {
                    match self.handle(route(&line, self.mode.get())) {
                        Flow::Exit if self.may_leave() => return Ok(()),
                        Flow::Exit => {}
                        Flow::Continue => self.leaving = false,
                    }
                }
                Signal::CtrlC => continue,
                Signal::CtrlD if self.may_leave() => return Ok(()),
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
                    turn::Flags {
                        stop: &stop,
                        interrupt: &self.interrupt,
                    },
                    &mut self.audit,
                    &mut self.jobs,
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
        let job = self.jobs.iter().any(Job::wants_attention);
        if !lines.is_empty() || watched.request.is_some() || watched.stopped {
            // Below the line the prompt was on.
            println!();
        } else if job && ui::is_ansi() {
            // Only a background turn made the prompt return: its line goes,
            // and what was typed comes back with the next prompt.
            print!("{}", ui::CLEAR_LINE);
        } else if job {
            println!();
        }
        for line in lines {
            println!("{line}");
        }
        if let Some(request) = watched.request {
            turn::answer(request, &mut self.audit);
        }
        if watched.stopped {
            self.audit.add(Record::new(
                Actor::Parolsh,
                Kind::Event,
                "the agent stopped",
            ));
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
                let ran = shell::run(
                    &self.config.shell,
                    &line,
                    &self.shell_cwd,
                    &self.shell_changes,
                );
                let code = report(ran.map(|(code, left)| {
                    self.after_shell(left);
                    code
                }));
                self.audit
                    .add(Record::new(Actor::User, Kind::Command, line).meta(json!({"exit": code})));
                code
            }
            Input::Bash => {
                let bash = shell::bash(&self.shell_cwd, &self.shell_changes);
                let code = report(shell::run_foreground(bash));
                self.audit.add(
                    Record::new(Actor::User, Kind::Command, "bash").meta(json!({"exit": code})),
                );
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
        let prompt = prompt.with_jobs(self.jobs.len());
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
    /// `#audit`: this run, from memory; `#audit <n>`: session `n` of this
    /// project, from the history.
    fn audit_command(&self, args: &str) -> Result<()> {
        let lines = if args.is_empty() {
            let lines = self.audit.lines(ui::width());
            if lines.is_empty() {
                println!("Nothing yet: commands, messages and what the agent does show up here.");
            }
            lines
        } else {
            let id = session_number(args)?;
            let entries = self
                .history()?
                .entries(&self.project_key(), id)?
                .with_context(|| format!("no session {id} in this project, see #sessions"))?;
            audit::stored_lines(&entries, ui::width())
        };
        for line in lines {
            println!("{line}");
        }
        Ok(())
    }

    /// `#sessions`: the sessions of this project in the history, newest
    /// first; `*` marks the current one.
    fn sessions_command(&self) -> Result<()> {
        let history = self.history()?;
        let sessions = history.sessions(&self.project_key())?;
        if sessions.is_empty() {
            println!("No sessions saved in this project yet.");
        }
        let current = history.current();
        // What each used, when an agent reported it: `1.2M tok  $4.31`.
        let used = |session: &Session| {
            let usage = &session.usage;
            let cost = match (usage.cost, &usage.currency) {
                (Some(cost), Some(currency)) => ui::money(cost, currency),
                _ => String::new(),
            };
            match usage.tokens() {
                0 => String::new(),
                tokens => format!("{:>4} tok  {cost:>7}", ui::tokens(tokens)),
            }
        };
        let column = sessions.iter().map(|s| used(s).len()).max().unwrap_or(0);
        let column = if column > 0 { column + 2 } else { 0 };
        for session in sessions {
            // Without a message, the session is named by its first command.
            let title = match (&session.title, &session.first_command) {
                (Some(title), _) => Some(audit::excerpt(title, 60)),
                (None, Some(command)) => {
                    Some(format!("(commands only) !{}", audit::excerpt(command, 44)))
                }
                (None, None) => None,
            };
            let line = format!(
                "{} {:>4}  {}  {:<8} {:>4} entries  {:<column$}{}",
                if current == Some(session.id) {
                    "*"
                } else {
                    " "
                },
                session.id,
                session.started,
                session.agent.as_deref().unwrap_or("-"),
                session.entries,
                used(&session),
                title.unwrap_or_default(),
            );
            println!("{}", audit::excerpt(&line, ui::width()));
        }
        Ok(())
    }

    /// `#forget [n]`: removes session `n`, or the current one, from the
    /// history.
    fn forget_command(&mut self, args: &str) -> Result<()> {
        let project = self.project_key();
        let history = self
            .audit
            .history_mut()
            .context("the history is off (history_days = 0)")?;
        let id = match args {
            "" => history
                .current()
                .context("nothing of this conversation is saved yet")?,
            n => session_number(n)?,
        };
        anyhow::ensure!(
            history.forget(&project, id)?,
            "no session {id} in this project, see #sessions"
        );
        println!("Session {id} removed from the history.");
        Ok(())
    }

    /// `#resume <n>`: goes back to session `n` of this project with its
    /// agent, in its directory, and goes on saving to it.
    fn resume_command(&mut self, args: &str) -> Result<()> {
        self.resume_session(session_number(args)?)
    }

    /// `--resume <n>` or `--continue`. The agent is still starting: the
    /// resume waits for it.
    fn resume_at_start(&mut self, resume: Resume) -> Result<()> {
        let id = match resume {
            Resume::Session(id) => id,
            Resume::Latest => {
                // This run's own session is only written with its first entry.
                self.history()?
                    .sessions(&self.project_key())?
                    .first()
                    .map(|session| session.id)
                    .context("no session saved in this project yet")?
            }
        };
        self.resume_session(id)
    }

    fn resume_session(&mut self, id: i64) -> Result<()> {
        let session = self
            .history()?
            .resumable(&self.project_key(), id)?
            .with_context(|| format!("no session {id} in this project, see #sessions"))?;
        // A session of shell commands only: the agent has no conversation
        // to go back to (it keeps one from its first message), so it goes on
        // with the one it has, and the session with what follows.
        let conversation = session.messages > 0;
        if Path::new(&session.cwd) != self.cwd {
            self.change_dir(&session.cwd)?;
        }
        let other_agent =
            |name: &String| self.active.as_ref() != Some(name) || self.agent.is_none();
        match &session.agent {
            Some(name) if other_agent(name) => {
                // Its agent may be gone from the configuration: only a
                // conversation needs it.
                if conversation || self.config.agents.contains_key(name) {
                    self.switch_agent(name)?;
                }
            }
            Some(_) => {}
            None if conversation => anyhow::bail!("session {id} had no agent"),
            None => {}
        }
        if conversation {
            let agent_session = session.agent_session_id.with_context(|| {
                format!("session {id} cannot be resumed: the agent gave no id for it")
            })?;
            let mcp = self.mcp_servers(false);
            let agent = self.agent.as_ref().with_context(no_agent)?;
            anyhow::ensure!(
                agent.resume(agent_session, self.cwd.clone(), mcp),
                "the agent stopped"
            );
            match turn::wait_resumed(agent) {
                turn::Resumed::Yes => {}
                turn::Resumed::No(why) => anyhow::bail!(
                    "cannot resume session {id}: {why}. #audit {id} shows what Parolsh saved of it"
                ),
                turn::Resumed::AgentStopped(why) => {
                    self.agent = None;
                    anyhow::bail!("{why}. Run #new to start it again.");
                }
            }
        }
        let project = self.project_key();
        if let Some(history) = self.audit.history_mut() {
            history.resume(&project, id)?;
        }
        // Another conversation: what a background turn answers is not its.
        self.conversation += 1;
        self.private = false;
        self.handover.clear();
        // Where the session was, before going on.
        self.show_exchanges(self.config.redraw_exchanges)?;
        if conversation {
            println!("Resumed session {id}: the agent remembers that conversation.");
        } else {
            println!(
                "Continued session {id}: it has only shell commands, so the agent starts a new conversation."
            );
        }
        Ok(())
    }

    /// `#redraw [n]`: clears the terminal, scrollback included, and shows the
    /// last `n` exchanges of the conversation again, laid out at the
    /// terminal's width. Only clears when the conversation is not saved.
    fn redraw_command(&mut self, args: &str) -> Result<()> {
        let count = match args {
            "" => self.config.redraw_exchanges,
            number => number
                .parse()
                .ok()
                .filter(|&count: &usize| count > 0)
                .with_context(|| format!("`{number}` is not a number of exchanges"))?,
        };
        if ui::is_ansi() {
            print!("{}", replay::WIPE);
        }
        self.show_exchanges(count)
    }

    /// Prints the last `count` exchanges of the current session from the
    /// history; nothing when it is not saved.
    fn show_exchanges(&mut self, count: usize) -> Result<()> {
        let project = self.project_key();
        let entries = match self.audit.history() {
            Some(history) => match history.current() {
                Some(id) => history.entries(&project, id)?.unwrap_or_default(),
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let prompt = self.prompt();
        let prompt = format!(
            "{}{}",
            prompt.render_prompt_left(),
            prompt.render_prompt_indicator(PromptEditMode::Default)
        );
        print!(
            "{}",
            replay::render(
                replay::last_exchanges(&entries, count),
                &prompt,
                self.display(),
                ui::is_ansi(),
                turn::text_width(),
            )
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());
        Ok(())
    }

    /// `#new [private]`.
    fn new_command(&mut self, args: &str) -> Result<()> {
        let private = match args {
            "" => false,
            "private" => true,
            other => anyhow::bail!("`#new {other}`: only `#new` or `#new private`"),
        };
        self.new_conversation(private);
        Ok(())
    }

    fn history(&self) -> Result<&History> {
        self.audit
            .history()
            .context("the history is off (history_days = 0)")
    }

    /// The project sessions belong to: its root, or the agent's directory
    /// without one.
    fn project_key(&self) -> String {
        self.project_root
            .as_deref()
            .unwrap_or(&self.cwd)
            .display()
            .to_string()
    }

    /// A new conversation: a new session in the history, unless private.
    fn begin_session(&mut self, private: bool) {
        self.conversation += 1;
        self.private = private;
        // What a background turn answered was for the conversation it left.
        self.handover.clear();
        self.audit.begin(SessionStart {
            project: self.project_key(),
            agent: self.agent.as_ref().and(self.active.clone()),
            cwd: self.cwd.display().to_string(),
        });
        if private {
            self.audit.pause();
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
            "new" => self.new_command(args),

            "forget" => self.forget_command(args),
            "resume" => self.resume_command(args),
            "cd" => self.change_dir(args),
            "agent" => self.agent(args),
            "project" => self.project(args),
            "prompt" => self.prompt_command(args),
            "config" => self.config_command(args),
            "options" => self.options_command(args),
            // Not recorded: looking changes nothing.
            "audit" | "sessions" | "redraw" | "jobs" | "fg" => {
                let result = match name {
                    "audit" => self.audit_command(args),
                    "sessions" => self.sessions_command(),
                    "jobs" => {
                        self.jobs_command();
                        Ok(())
                    }
                    "fg" => self.fg_command(args),
                    _ => self.redraw_command(args),
                };
                return Some(match result {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("parolsh: {e:#}");
                        1
                    }
                });
            }
            _ => Err(anyhow::anyhow!("unknown command `#{name}`, see #help")),
        };
        if name != "help" {
            self.audit.add(
                Record::new(Actor::User, Kind::Control, format!("{name} {args}"))
                    .meta(json!({"failed": result.is_err()})),
            );
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
        // Instead of only clearing the screen: the conversation again, at
        // the terminal's width.
        keybindings.add_binding(
            KeyModifiers::CONTROL,
            KeyCode::Char('l'),
            ReedlineEvent::ExecuteHostCommand("#redraw".to_string()),
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
                    && agent.new_session(self.cwd.clone(), self.mcp_servers(false)) =>
            {
                self.begin_session(false)
            }
            _ => self.start_agent(false),
        }
        Ok(())
    }

    /// Where and with what the next shell command runs: the directory the
    /// last one ended in, when the shell said, and what it changed in its
    /// environment.
    fn after_shell(&mut self, left: shell::Left) {
        self.move_shell(left.dir);
        self.shell_changes = left.changes;
    }

    /// Where the next shell command runs.
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
        let ran = shell::run_shared(
            &self.config.shell,
            &line,
            &cwd,
            &self.shell_changes,
            SHARED_LIMIT,
        );
        match ran {
            Ok((code, captured, left)) => {
                self.after_shell(left);
                if code != 0 {
                    eprintln!("exit {code}");
                }
                println!("(output of `{line}` goes with your next message)");
                self.audit.add(
                    Record::new(Actor::User, Kind::Capture, line.clone()).meta(json!({
                        "exit": code,
                        "kept": captured.text.len(),
                        "truncated": captured.truncated,
                    })),
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
        let kept: usize = self.shared.iter().map(|s| s.captured.text.len()).sum();
        let shared: Vec<String> = self.shared.drain(..).map(|s| s.block()).collect();
        if !shared.is_empty() {
            self.audit.add(
                Record::new(Actor::Shared, Kind::Share, shared.join("\n\n")).meta(json!({
                    "outputs": shared.len(),
                    "bytes": kept,
                    "agent": name,
                })),
            );
        }
        // `@path` starts where your commands run, the directory the prompt
        // shows. The agent reads the word too, from its own directory: when
        // a `cd` took the two apart, it gets the path in full.
        let home = home();
        let files = mention::mentioned(&text, &self.shell_cwd, home.as_deref());
        let sent = match self.shell_cwd == self.cwd {
            true => text.clone(),
            false => mention::in_full(&text, &self.shell_cwd, home.as_deref()),
        };
        let message = self.audit.add(
            Record::new(Actor::User, Kind::Message, text.clone())
                .meta(json!({"agent": name, "files": files.len()})),
        );
        // What background turns answered since the last message comes first.
        let told = std::mem::take(&mut self.handover);
        let mut blocks: Vec<Block> = told.iter().cloned().map(Block::Text).collect();
        blocks.extend(shared.into_iter().map(Block::Text));
        blocks.extend(files.into_iter().map(Block::File));
        blocks.push(Block::Text(sent));
        let display = self.display();
        match turn::run(agent, blocks, display, &mut self.audit, &mut self.jobs) {
            turn::Outcome::Background(turn) => {
                // The copy the conversation goes on in was not told either.
                self.handover = told;
                self.send_to_background(*turn, text, message.row());
                0
            }
            turn::Outcome::Finished => 0,
            turn::Outcome::Failed => 1,
            turn::Outcome::AgentStopped => {
                turn::drain(agent);
                self.audit.add(Record::new(
                    Actor::Parolsh,
                    Kind::Event,
                    "the agent stopped",
                ));
                eprintln!("parolsh: the agent stopped. Run #new to start it again.");
                self.agent = None;
                1
            }
        }
    }

    /// Ctrl+Z during a turn: the turn goes on in the background, with the
    /// agent process and the conversation it has, and the session's entries
    /// from its message on move to a session of their own. The conversation
    /// here goes on in another process: in a copy that ends before that
    /// message when the agent can make one, in a new one otherwise.
    fn send_to_background(&mut self, mut turn: turn::Turn, request: String, message: Option<i64>) {
        let Some(old) = self.agent.take() else {
            return;
        };
        let activity = old.activity().get();
        let start = match (activity.forks, activity.session_id, activity.before_prompt) {
            (Forks::UpTo, Some(session), Some(message)) => Start::Fork(ForkOf {
                session,
                up_to: Some(message),
            }),
            (Forks::Whole, Some(session), _) => Start::Fork(ForkOf {
                session,
                up_to: None,
            }),
            // No copy, or nothing was said before that message.
            _ => Start::New,
        };
        let split = message.and_then(|first| {
            let history = self.audit.history_mut()?;
            history.split(first).ok().flatten()
        });
        turn.move_to(split);
        let job = Job::new(self.next_job, request.clone(), self.conversation, old, turn);
        self.next_job += 1;
        // Over the `^Z` the terminal echoed.
        print!("{}", if ui::is_ansi() { ui::CLEAR_LINE } else { "\r" });
        println!("{}", job.label("in the background"));
        self.jobs.push(job);

        let mcp = self.mcp_servers(self.private);
        let agent = self
            .active_agent()
            .map(|agent| AgentHandle::start_with(agent, self.cwd.clone(), mcp, start.clone()));
        self.agent = agent;
        let Some(agent) = &self.agent else {
            eprintln!("parolsh: {}", no_agent());
            return;
        };
        match &start {
            Start::New => println!("The conversation here is a new one."),
            Start::Fork(of) => match turn::wait_resumed(agent) {
                // A whole copy has the request: the agent would do it again.
                turn::Resumed::Yes if of.up_to.is_none() => self.handover.push(format!(
                    "For your information only: do not answer this or comment on it.\n\n\
                     My last request is being worked on in the background, in another \
                     conversation: do not work on it here. Its answer will follow.\n\n\
                     The request:\n{request}"
                )),
                turn::Resumed::Yes => {}
                turn::Resumed::No(why) => {
                    println!("parolsh: {why}. The conversation here is a new one.");
                }
                turn::Resumed::AgentStopped(why) => {
                    self.agent = None;
                    eprintln!("parolsh: {why}. Run #new to start it again.");
                }
            },
        }
    }

    /// Deals with the background turns that ask something or ended.
    fn attend_jobs(&mut self) {
        for job in &mut self.jobs {
            job.poll(&mut self.audit);
            job.answer(&mut self.audit);
        }
        while let Some(done) = self.jobs.iter().position(|job| job.ended().is_some()) {
            let job = self.jobs.remove(done);
            self.report(job);
        }
    }

    /// A background turn ended: shows its answer, puts its entries at the
    /// end of the session it left, and keeps the answer for the agent, with
    /// the next message. Left as a session of its own when the conversation
    /// here is no longer the one it left.
    fn report(&mut self, job: Job) {
        let Some(ended) = job.ended().cloned() else {
            return;
        };
        println!("{}", job.label(jobs::outcome(&ended)));
        if let Ended::Failed(why) | Ended::AgentStopped(why) = &ended {
            eprintln!("parolsh: {why}");
        }
        let turn = job.turn();
        let answer = turn.answer().trim();
        if !answer.is_empty() {
            let record = Record::new(Actor::Agent, Kind::Answer, answer);
            let (ansi, width) = (ui::is_ansi(), turn::text_width());
            print!("{}", replay::answer(&record, self.display(), ansi, width));
        }
        if ui::is_ansi() {
            let summary = ui::summary(turn.tool_calls(), turn.started().elapsed(), None);
            println!("{summary}");
        }
        let here = job.conversation == self.conversation;
        match (turn.session(), here) {
            (Some(session), true) => {
                if let Some(history) = self.audit.history_mut()
                    && let Err(e) = history.append(session)
                {
                    eprintln!("parolsh: history: {e}");
                }
            }
            (Some(session), false) => {
                println!("It is session {session} in the history: #audit {session} shows it.");
            }
            (None, _) => {}
        }
        // Only an answer: what a cancelled or failed turn wrote is not one.
        if here && !answer.is_empty() && ended == Ended::Finished(None) {
            self.handover.push(handover(&job.request, answer));
        }
    }

    /// `#jobs`: the turns in the background.
    fn jobs_command(&mut self) {
        if self.jobs.is_empty() {
            println!("No turns in the background. Ctrl+Z sends the running one there.");
        }
        for job in &mut self.jobs {
            job.poll(&mut self.audit);
            let line = format!(
                "[{}] {} · {}",
                job.number,
                audit::excerpt(&job.request, 40),
                job.progress()
            );
            println!("{}", audit::excerpt(&line, ui::width()));
        }
    }

    /// `#fg [n]`: waits for background turn `n`, or the last one. Ctrl+C
    /// cancels it, Ctrl+Z goes back to the prompt.
    fn fg_command(&mut self, args: &str) -> Result<()> {
        let number = match args {
            "" => self.jobs.last().map(|job| job.number),
            number => Some(
                number
                    .parse()
                    .with_context(|| format!("`#fg {number}`: not a number"))?,
            ),
        };
        let index = number
            .and_then(|number| self.jobs.iter().position(|job| job.number == number))
            .context("no such turn in the background, see #jobs")?;
        let ansi = ui::is_ansi();
        let clear = || {
            if ansi {
                eprint!("{}", ui::CLEAR_LINE);
            }
        };
        // Keys pressed before do not count.
        turn::interrupt_asked();
        turn::background_asked();
        println!("{}", self.jobs[index].label("waiting for"));
        let mut cancelling = false;
        loop {
            let job = &mut self.jobs[index];
            job.poll(&mut self.audit);
            if job.asks() {
                clear();
                job.answer(&mut self.audit);
                continue;
            }
            if job.ended().is_some() {
                clear();
                break;
            }
            if turn::interrupt_asked() {
                job.cancel();
                cancelling = true;
            }
            if turn::background_asked() {
                clear();
                println!("{}", job.label("in the background"));
                return Ok(());
            }
            if ansi {
                let cancelling = if cancelling { "Cancelling… · " } else { "" };
                let line = format!(
                    "[{}] {cancelling}{}  (Ctrl+Z: back to the prompt, Ctrl+C: cancel it)",
                    job.number,
                    job.progress()
                );
                eprint!(
                    "{}{}",
                    ui::CLEAR_LINE,
                    ui::dim(&audit::excerpt(&line, ui::width()))
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.attend_jobs();
        Ok(())
    }

    /// `#exit` and Ctrl+D do not leave the first time while something is
    /// still running: it would stop with Parolsh.
    fn may_leave(&mut self) -> bool {
        let jobs = self.jobs.len();
        let tasks = self
            .agent
            .as_ref()
            .map_or(0, |agent| agent.activity().get().tasks);
        if self.leaving || (jobs == 0 && tasks == 0) {
            return true;
        }
        self.leaving = true;
        let count = |n: usize, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        let running: Vec<String> = [
            count(
                jobs,
                "turn in the background (#jobs)",
                "turns in the background (#jobs)",
            ),
            count(
                tasks,
                "background task of the agent",
                "background tasks of the agent",
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!(
            "Still running: {}. Leaving stops them: #exit again to leave.",
            running.join(" and ")
        );
        false
    }

    fn new_conversation(&mut self, private: bool) {
        // An agent that never ended a cancelled turn would only start the new
        // conversation after it: start the agent again instead.
        let restarted = match &self.agent {
            Some(agent) => {
                agent.abandoned().is_some()
                    || !agent.new_session(self.cwd.clone(), self.mcp_servers(private))
            }
            None => true,
        };
        if restarted {
            // `start_agent` begins the session.
            self.start_agent(private);
        } else {
            self.begin_session(private);
        }
        match (&self.agent, private) {
            (None, _) => eprintln!("parolsh: {}", no_agent()),
            (Some(_), false) => println!("Started a new conversation."),
            (Some(_), true) => {
                println!("Started a private conversation: it is not saved in the history.")
            }
        }
    }

    /// Stops the running agent, if any, and starts the active one with a new
    /// conversation in the current directory.
    fn start_agent(&mut self, private: bool) {
        self.agent = None;
        let mcp = self.mcp_servers(private);
        let agent = self
            .active_agent()
            .map(|agent| AgentHandle::start(agent, self.cwd.clone(), mcp));
        self.agent = agent;
        self.begin_session(private);
    }

    /// The MCP servers the agent gets in a conversation: the history of this
    /// project, unless the conversation is private or there is no history.
    fn mcp_servers(&self, private: bool) -> Vec<McpServer> {
        match &self.history_db {
            Some(db) if !private => {
                let scope = mcp::Scope {
                    project: &self.project_key(),
                    commands: self.config.commands == Commands::Shared,
                };
                mcp::server(db, scope).into_iter().collect()
            }
            _ => Vec::new(),
        }
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
        self.start_agent(false);
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
            ui::banner(&version::short(), agent, &self.cwd, home.as_deref())
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

/// Lines the prompt holds for printing; the agent's lines beyond wait.
const PRINTER_LINES: usize = 64;

/// Where to go when no agent is configured.
/// What the agent is told of a background turn that ended, with the next
/// message: its conversation is a copy made before it. At most the last
/// `HANDOVER_LIMIT` bytes of the answer.
fn handover(request: &str, answer: &str) -> String {
    let mut start = answer.len().saturating_sub(HANDOVER_LIMIT);
    while !answer.is_char_boundary(start) {
        start += 1;
    }
    let cut = if start > 0 { "(the end of it)\n" } else { "" };
    format!(
        "For your information only: do not answer this or comment on it, unless I ask about \
         it.\n\nWhile we talked, a request of mine was worked on in the background, in another \
         conversation, and it ended. You were not told of it before.\n\n\
         The request:\n{request}\n\nIts answer:\n{cut}{}",
        &answer[start..]
    )
}

/// As much as a `!+` output keeps.
const HANDOVER_LIMIT: usize = 16 * 1024;

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

/// The input history: the lines you typed.
fn history_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("history"))
}

/// Makes `path` readable by the user only, creating it if needed: the input
/// history holds every line typed, `!commands` with their arguments too.
fn keep_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    // One made by an earlier version, with the default permissions.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// `$XDG_STATE_HOME/parolsh`, or `~/.local/state/parolsh`.
fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("parolsh"))
}

/// A session number, as `#sessions` lists them.
fn session_number(text: &str) -> Result<i64> {
    text.parse()
        .ok()
        .filter(|&id: &i64| id > 0)
        .with_context(|| format!("`{text}` is not a session number, see #sessions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_background_answer_is_handed_over_with_its_request_and_cut_at_the_limit() {
        let short = handover("compare with 0.6.0", "Three things changed.");
        assert!(
            short.ends_with(
                "The request:\ncompare with 0.6.0\n\nIts answer:\nThree things changed."
            ),
            "{short}"
        );

        // The end is kept, cut between characters.
        let long = format!("start {} the end", "é".repeat(HANDOVER_LIMIT));
        let cut = handover("write a lot", &long);
        let answer = cut.split_once("Its answer:\n").unwrap().1;
        assert!(
            answer.starts_with("(the end of it)\né"),
            "{}",
            &answer[..40]
        );
        assert!(answer.ends_with("é the end"));
        assert!(answer.len() <= HANDOVER_LIMIT + "(the end of it)\n".len());
        assert!(!cut.contains("start"));
    }

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
