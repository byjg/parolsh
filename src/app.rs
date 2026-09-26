//! The interactive loop: read a line, route it, act on it.

use anyhow::{Context, Result};
use reedline::{FileBackedHistory, Reedline, Signal};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::acp::AgentHandle;
use crate::config::{Agent, Config, PromptStyle};
use crate::input::{Input, route};
use crate::{config, project, setup, shell, turn, ui};

const HELP: &str = "\
Input:
  <text>          ask the agent (natural language)
  /<command>      forwarded unchanged to the agent
  !<command>      run a shell command (bash -ic)
  !bash           open an interactive Bash session
  #<command>      Parolsh control command

Control commands:
  #help           show this help
  #new            start a new conversation
  #cd <path>      switch to another directory or project
  #agent [list]   show the active agent, or list the configured ones
  #agent <name>   switch to another agent (new conversation)
  #config [sample] show the configuration files, or print the full sample
  #project [init] show the project root, or create .parolsh/ here
  #prompt [name]  show the prompt style, or switch: parolsh, starship, minimal
  #exit           leave Parolsh";

pub struct App {
    cwd: PathBuf,
    project_root: Option<PathBuf>,
    config: Config,
    /// Name of the agent in use: `default_agent`, until `#agent <name>`.
    active: Option<String>,
    agent: Option<AgentHandle>,
    /// Set by `#prompt <name>`, or to fall back when Starship fails.
    prompt_override: Option<PromptStyle>,
    /// What the first run did, shown once after the banner.
    setup_notice: Option<String>,
    /// Exit code and duration of the last command, for the prompt.
    last_status: i32,
    last_duration: Duration,
}

enum Flow {
    Continue,
    Exit,
}

impl App {
    pub fn new(cwd: PathBuf) -> Result<Self> {
        // Before loading: the first run may write the global configuration.
        let search_path = std::env::var("PATH").ok();
        let setup_notice =
            config::global_path().and_then(|path| setup::first_run(&path, search_path.as_deref()));
        let project_root = project::find_root(&cwd);
        let config = Config::load(project_root.as_deref())?;
        let mut app = Self {
            cwd,
            project_root,
            active: config.default_agent.clone(),
            config,
            agent: None,
            prompt_override: None,
            setup_notice,
            last_status: 0,
            last_duration: Duration::ZERO,
        };
        app.start_agent();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        // Bracketed paste: a pasted text with line breaks is one input, not
        // one Enter per line.
        let mut editor = Reedline::create().use_bracketed_paste(true);
        if let Some(path) = history_path() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            editor = editor.with_history(Box::new(FileBackedHistory::with_file(1000, path)?));
        }
        if ui::is_ansi() {
            self.print_banner();
        }
        if let Some(notice) = self.setup_notice.take() {
            println!("{notice}");
        }

        loop {
            self.report_agent_errors();
            let prompt = self.prompt();
            match editor.read_line(&prompt)? {
                Signal::Success(line) => {
                    if let Flow::Exit = self.handle(route(&line)) {
                        return Ok(());
                    }
                }
                Signal::CtrlC => continue,
                Signal::CtrlD => return Ok(()),
                _ => continue,
            }
        }
    }

    fn handle(&mut self, input: Input) -> Flow {
        let started = Instant::now();
        let status = match input {
            Input::Empty => return Flow::Continue,
            Input::Agent(text) => self.ask(text),
            Input::Shell(line) => run(shell::command(&self.config.shell, &line, &self.cwd)),
            Input::Bash => run(shell::bash(&self.cwd)),
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
        match style {
            PromptStyle::Minimal => ui::Prompt::minimal(),
            // Starship prints ANSI colors: only on an ANSI terminal.
            PromptStyle::Starship if ui::is_ansi() => {
                let context = ui::PromptContext {
                    cwd: &self.cwd,
                    status: self.last_status,
                    duration: self.last_duration,
                    agent: self.banner_agent(),
                };
                match ui::Prompt::starship("starship", &context) {
                    Ok(prompt) => prompt,
                    Err(e) => {
                        eprintln!("parolsh: starship: {e}. Using the parolsh prompt.");
                        self.prompt_override = Some(PromptStyle::Parolsh);
                        ui::Prompt::parolsh(self.label())
                    }
                }
            }
            _ => ui::Prompt::parolsh(self.label()),
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
            _ => Err(anyhow::anyhow!("unknown command `#{name}`, see #help")),
        };
        match result {
            Ok(()) => Some(0),
            Err(e) => {
                eprintln!("parolsh: {e:#}");
                Some(1)
            }
        }
    }

    fn change_dir(&mut self, args: &str) -> Result<()> {
        let target = resolve_dir(&self.cwd, args)?;
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
        self.config = config;
        self.cwd = target;
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

    /// Sends `text` to the agent. Returns the status for the prompt.
    fn ask(&mut self, text: String) -> i32 {
        let Some(agent) = &self.agent else {
            eprintln!("parolsh: {}", no_agent());
            return 1;
        };
        match turn::run(agent, text) {
            turn::Outcome::Finished => 0,
            turn::Outcome::AgentStopped => {
                turn::drain(agent);
                eprintln!("parolsh: the agent stopped. Run #new to start it again.");
                self.agent = None;
                1
            }
        }
    }

    fn new_conversation(&mut self) {
        let restarted = match &self.agent {
            Some(agent) => !agent.new_session(self.cwd.clone()),
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

    fn report_agent_errors(&mut self) {
        if let Some(agent) = &self.agent
            && !turn::drain(agent)
        {
            eprintln!("parolsh: the agent stopped. Run #new to start it again.");
            self.agent = None;
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
        let home = std::env::var_os("HOME").map(PathBuf::from);
        print!(
            "{}",
            ui::banner(env!("CARGO_PKG_VERSION"), agent, &self.cwd, home.as_deref())
        );
    }

    fn label(&self) -> String {
        self.cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.cwd.display().to_string())
    }
}

/// Runs a `!` command or `!bash`. Returns its exit code.
/// Where to go when no agent is configured.
fn no_agent() -> String {
    match config::global_path() {
        Some(path) => format!(
            "no agent configured. Add one to {} and set default_agent (see #config).",
            path.display()
        ),
        None => "no agent configured (see #config).".to_string(),
    }
}

fn run(command: Command) -> i32 {
    match shell::run_foreground(command) {
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

fn resolve_dir(cwd: &Path, args: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
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
