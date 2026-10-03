//! Configuration: the global file, overridden field by field by the project file.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::input::Mode;
use crate::project::STATE_DIR;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub command: String,
    pub args: Vec<String>,
    /// Added to the environment Parolsh passes to the agent.
    pub env: BTreeMap<String, String>,
    /// The agent's own session mode id, set on every new conversation.
    /// `None` keeps the agent's default.
    pub mode: Option<String>,
    /// The agent's own config options (`effort`, `model`, ...), by id, set
    /// on every new conversation.
    pub options: BTreeMap<String, OptionValue>,
}

/// A config option value: the id of a choice, or on/off.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    Bool(bool),
    Text(String),
}

impl OptionValue {
    /// Reads a value typed by the user: `true`/`false`, or a choice id.
    pub fn parse(text: &str) -> Self {
        match text {
            "true" => Self::Bool(true),
            "false" => Self::Bool(false),
            other => Self::Text(other.to_string()),
        }
    }
}

impl std::fmt::Display for OptionValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Text(value) => f.write_str(value),
        }
    }
}

/// When to import the environment the user's shell sets up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellEnv {
    /// When Parolsh was not started from a shell.
    #[default]
    Auto,
    Always,
    Never,
}

/// How markdown links are shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    /// Clickable text, followed by the URL.
    #[default]
    Both,
    /// Clickable text only (OSC 8): the URL is lost where it is unsupported.
    Clickable,
    /// Underlined text followed by the URL, nothing clickable.
    Inline,
}

/// How the agent's reasoning ("thinking") is displayed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingDisplay {
    /// Its latest line in the status line.
    #[default]
    Status,
    /// Only "Thinking" in the status line.
    Hidden,
    /// Printed dim in the scrollback.
    Show,
}

/// What happens to the lines of your `!commands` (never their output).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Commands {
    /// Saved in the history for you (`#redraw`, `#resume`, `#audit`), and
    /// not returned by the tools the agent searches it with.
    #[default]
    Private,
    /// Saved, and returned to the agent too.
    Shared,
    /// Not saved: only `#audit` of the current run has them.
    Off,
}

/// How the input prompt is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptStyle {
    /// The agent, the directory and `❯`.
    #[default]
    Parolsh,
    /// The user's Starship prompt (`starship prompt`).
    Starship,
    /// Only `❯`.
    Minimal,
}

impl PromptStyle {
    pub const ALL: [Self; 3] = [Self::Parolsh, Self::Starship, Self::Minimal];

    pub fn name(self) -> &'static str {
        match self {
            Self::Parolsh => "parolsh",
            Self::Starship => "starship",
            Self::Minimal => "minimal",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.name() == name)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    /// Command line that runs `!command`; the command is appended as the last argument.
    pub shell: Vec<String>,
    pub prompt: PromptStyle,
    /// Where plain text goes when Parolsh starts.
    pub input: Mode,
    pub thinking: ThinkingDisplay,
    /// Render the answers' markdown on ANSI terminals.
    pub markdown: bool,
    pub links: LinkStyle,
    pub shell_env: ShellEnv,
    pub default_agent: Option<String>,
    pub agents: BTreeMap<String, Agent>,
    /// How many entries `#audit` keeps; 0 keeps none.
    pub audit_entries: usize,
    /// How many exchanges `#redraw` shows again.
    pub redraw_exchanges: usize,
    /// Days an idle session stays in the history; 0 keeps no history.
    pub history_days: u32,
    /// Whether `!command` lines are saved in the history, and for whom.
    pub commands: Commands,
}

/// One config file as written on disk: every field is optional so a project
/// file can override only what it needs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    shell: Option<Vec<String>>,
    prompt: Option<PromptStyle>,
    input: Option<Mode>,
    thinking: Option<ThinkingDisplay>,
    markdown: Option<bool>,
    links: Option<LinkStyle>,
    shell_env: Option<ShellEnv>,
    default_agent: Option<String>,
    #[serde(default)]
    agents: BTreeMap<String, AgentFile>,
    audit_entries: Option<usize>,
    redraw_exchanges: Option<usize>,
    history_days: Option<u32>,
    commands: Option<Commands>,
    /// Before `commands`: true is `shared`, false is `off`.
    save_commands: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentFile {
    command: Option<String>,
    args: Option<Vec<String>>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    mode: Option<String>,
    #[serde(default)]
    options: BTreeMap<String, OptionValue>,
}

impl ConfigFile {
    fn read(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// A project file comes with the directory, often from a cloned
    /// repository: it may change how Parolsh looks and which configured agent
    /// it uses, never what is executed.
    fn check_project(&self) -> Result<()> {
        // What is kept about your sessions is yours to choose, not a
        // repository's: it could give your commands to the agent.
        for (key, set) in [
            ("shell", self.shell.is_some()),
            ("history_days", self.history_days.is_some()),
            ("commands", self.commands.is_some()),
            ("save_commands", self.save_commands.is_some()),
        ] {
            if set {
                bail!("`{key}` can only be set in the global configuration");
            }
        }
        for (name, agent) in &self.agents {
            if agent.command.is_some() || agent.args.is_some() || !agent.env.is_empty() {
                bail!(
                    "agent `{name}`: `command`, `args` and `env` can only be set in the global configuration"
                );
            }
        }
        Ok(())
    }

    /// Applies a project file, already checked, on top of this one.
    fn merge(mut self, over: Self) -> Self {
        self.prompt = over.prompt.or(self.prompt);
        self.input = over.input.or(self.input);
        self.thinking = over.thinking.or(self.thinking);
        self.markdown = over.markdown.or(self.markdown);
        self.links = over.links.or(self.links);
        self.shell_env = over.shell_env.or(self.shell_env);
        self.default_agent = over.default_agent.or(self.default_agent);
        self.audit_entries = over.audit_entries.or(self.audit_entries);
        self.redraw_exchanges = over.redraw_exchanges.or(self.redraw_exchanges);
        for (name, agent) in over.agents {
            let base = self.agents.entry(name).or_default();
            base.mode = agent.mode.or(base.mode.take());
            base.options.extend(agent.options);
        }
        self
    }
}

impl Config {
    /// Loads `$XDG_CONFIG_HOME/parolsh/config.toml` and, when inside a
    /// project, `<root>/.parolsh/config.toml` on top of it.
    pub fn load(project_root: Option<&Path>) -> Result<Self> {
        let project = project_root.map(project_path);
        Self::load_files(global_path().as_deref(), project.as_deref())
    }

    /// Loads one file on its own, as if it were the global configuration.
    #[cfg(test)]
    pub fn load_file(path: &Path) -> Result<Self> {
        Self::load_files(Some(path), None)
    }

    fn load_files(global: Option<&Path>, project: Option<&Path>) -> Result<Self> {
        let mut file = global
            .map(ConfigFile::read)
            .transpose()?
            .unwrap_or_default();
        if let Some(path) = project {
            let over = ConfigFile::read(path)?;
            over.check_project()
                .with_context(|| format!("invalid {}", path.display()))?;
            file = file.merge(over);
        }
        Self::resolve(file)
    }

    fn resolve(file: ConfigFile) -> Result<Self> {
        let shell = file
            .shell
            .unwrap_or_else(|| vec!["bash".to_string(), "-ic".to_string()]);
        if shell.is_empty() {
            bail!("`shell` cannot be empty");
        }

        let mut agents = BTreeMap::new();
        for (name, agent) in file.agents {
            let Some(command) = agent.command else {
                bail!("agent `{name}` has no `command`");
            };
            agents.insert(
                name,
                Agent {
                    command,
                    args: agent.args.unwrap_or_default(),
                    env: agent.env,
                    mode: agent.mode,
                    options: agent.options,
                },
            );
        }

        if let Some(name) = &file.default_agent
            && !agents.contains_key(name)
        {
            bail!("`default_agent` is `{name}`, but no agent with that name is configured");
        }

        Ok(Self {
            shell,
            prompt: file.prompt.unwrap_or_default(),
            input: file.input.unwrap_or_default(),
            thinking: file.thinking.unwrap_or_default(),
            markdown: file.markdown.unwrap_or(true),
            links: file.links.unwrap_or_default(),
            shell_env: file.shell_env.unwrap_or_default(),
            default_agent: file.default_agent,
            agents,
            audit_entries: file.audit_entries.unwrap_or(1000),
            redraw_exchanges: file.redraw_exchanges.unwrap_or(5),
            history_days: file.history_days.unwrap_or(90),
            commands: file
                .commands
                .or(file.save_commands.map(|save| match save {
                    true => Commands::Shared,
                    false => Commands::Off,
                }))
                .unwrap_or_default(),
        })
    }
}

/// `<root>/.parolsh/config.toml`.
pub fn project_path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join("config.toml")
}

/// `$XDG_CONFIG_HOME/parolsh/config.toml`, or `~/.config/parolsh/config.toml`.
pub fn global_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("parolsh").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn defaults_without_any_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.toml");

        let config = Config::load_files(Some(&missing), None).unwrap();

        assert_eq!(config.shell, ["bash", "-ic"]);
        assert_eq!(config.prompt, PromptStyle::Parolsh);
        assert_eq!(config.input, Mode::Agent);
        assert_eq!(config.thinking, ThinkingDisplay::Status);
        assert!(config.markdown);
        assert_eq!(config.links, LinkStyle::Both);
        assert_eq!(config.shell_env, ShellEnv::Auto);
        assert_eq!(config.default_agent, None);
        assert!(config.agents.is_empty());
        assert_eq!(config.audit_entries, 1000);
        assert_eq!(config.redraw_exchanges, 5);
        assert_eq!(config.history_days, 90);
        assert_eq!(config.commands, Commands::Private);
    }

    #[test]
    fn project_overrides_global_field_by_field() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(
            tmp.path(),
            "global.toml",
            r#"
            default_agent = "qwen"

            [agents.qwen]
            command = "qwen"
            args = ["--acp"]

            [agents.claude]
            command = "claude-agent-acp"
            mode = "plan"
            "#,
        );
        let project = write(
            tmp.path(),
            "project.toml",
            r#"
            default_agent = "claude"
            prompt = "starship"

            [agents.claude]
            mode = "auto"
            "#,
        );

        let config = Config::load_files(Some(&global), Some(&project)).unwrap();

        assert_eq!(config.shell, ["bash", "-ic"]);
        assert_eq!(config.prompt, PromptStyle::Starship);
        assert_eq!(config.default_agent.as_deref(), Some("claude"));
        assert_eq!(
            config.agents["claude"],
            Agent {
                command: "claude-agent-acp".to_string(),
                args: vec![],
                env: BTreeMap::new(),
                mode: Some("auto".to_string()),
                options: BTreeMap::new(),
            }
        );
        assert_eq!(
            config.agents["qwen"],
            Agent {
                command: "qwen".to_string(),
                args: vec!["--acp".to_string()],
                env: BTreeMap::new(),
                mode: None,
                options: BTreeMap::new(),
            }
        );
    }

    /// A cloned repository must not choose what Parolsh executes.
    #[test]
    fn a_project_cannot_change_what_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(
            tmp.path(),
            "global.toml",
            "[agents.claude]\ncommand = \"claude-agent-acp\"\n",
        );
        let cases = [
            ("shell = [\"./evil\"]\n", "`shell`"),
            ("[agents.claude]\ncommand = \"./evil\"\n", "agent `claude`"),
            ("[agents.claude]\nargs = [\"--evil\"]\n", "agent `claude`"),
            (
                "[agents.claude]\nenv = { LD_PRELOAD = \"./evil.so\" }\n",
                "agent `claude`",
            ),
            // A new agent would need a command: it cannot be defined either.
            (
                "[agents.evil]\nmode = \"plan\"\n",
                "agent `evil` has no `command`",
            ),
            ("default_agent = \"evil\"\n", "`default_agent` is `evil`"),
            // Nor what is kept about your sessions.
            ("commands = \"shared\"\n", "`commands`"),
            ("save_commands = true\n", "`save_commands`"),
            ("history_days = 3650\n", "`history_days`"),
        ];

        for (i, (text, message)) in cases.iter().enumerate() {
            let project = write(tmp.path(), &format!("project{i}.toml"), text);
            let error = Config::load_files(Some(&global), Some(&project)).unwrap_err();
            assert!(format!("{error:#}").contains(message), "{text}: {error:#}");
        }
    }

    /// `save_commands`, from before `commands`, is still read; `commands`
    /// wins when both are there.
    #[test]
    fn commands_can_be_private_shared_or_off() {
        let tmp = tempfile::tempdir().unwrap();
        let cases = [
            ("commands = \"private\"\n", Commands::Private),
            ("commands = \"shared\"\n", Commands::Shared),
            ("commands = \"off\"\n", Commands::Off),
            ("save_commands = true\n", Commands::Shared),
            ("save_commands = false\n", Commands::Off),
            (
                "save_commands = true\ncommands = \"private\"\n",
                Commands::Private,
            ),
        ];

        for (i, (text, expected)) in cases.iter().enumerate() {
            let path = write(tmp.path(), &format!("config{i}.toml"), text);
            assert_eq!(
                Config::load_file(&path).unwrap().commands,
                *expected,
                "{text}"
            );
        }
    }

    #[test]
    fn options_and_thinking_merge_like_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(
            tmp.path(),
            "global.toml",
            r#"
            thinking = "hidden"
            input = "agent"

            [agents.claude]
            command = "claude-agent-acp"
            options = { effort = "low", model = "sonnet", fast = true }
            "#,
        );
        let project = write(
            tmp.path(),
            "project.toml",
            r#"
            thinking = "show"
            input = "shell"
            markdown = false
            links = "inline"

            [agents.claude]
            options = { effort = "high" }
            "#,
        );

        let config = Config::load_files(Some(&global), Some(&project)).unwrap();

        assert_eq!(config.thinking, ThinkingDisplay::Show);
        assert_eq!(config.input, Mode::Shell);
        assert!(!config.markdown);
        assert_eq!(config.links, LinkStyle::Inline);
        assert_eq!(
            config.agents["claude"].options,
            BTreeMap::from([
                ("effort".to_string(), OptionValue::Text("high".into())),
                ("fast".to_string(), OptionValue::Bool(true)),
                ("model".to_string(), OptionValue::Text("sonnet".into())),
            ])
        );
    }

    #[test]
    fn option_values_typed_by_the_user() {
        assert_eq!(OptionValue::parse("true"), OptionValue::Bool(true));
        assert_eq!(OptionValue::parse("low"), OptionValue::Text("low".into()));
        assert_eq!(OptionValue::Bool(false).to_string(), "false");
    }

    #[test]
    fn an_agent_needs_a_command() {
        let tmp = tempfile::tempdir().unwrap();
        let project = write(
            tmp.path(),
            "project.toml",
            "[agents.claude]\nmode = \"plan\"\n",
        );

        let error = Config::load_files(None, Some(&project)).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("agent `claude` has no `command`")
        );
    }

    #[test]
    fn default_agent_must_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(tmp.path(), "global.toml", "default_agent = \"codex\"\n");

        let error = Config::load_files(Some(&global), None).unwrap_err();

        assert!(error.to_string().contains("`default_agent` is `codex`"));
    }

    /// Every setting in config.sample.toml, uncommented, must load: the
    /// sample cannot drift from the real configuration.
    #[test]
    fn the_sample_configuration_is_valid() {
        let sample = include_str!("../config.sample.toml");
        let uncommented: String = sample
            .lines()
            .filter_map(|line| line.strip_prefix("# "))
            .filter(|line| {
                line.starts_with('[')
                    || line.split_once(" = ").is_some_and(|(key, _)| {
                        key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                    })
            })
            .map(|line| format!("{line}\n"))
            .collect();
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "config.toml", &uncommented);

        let config = Config::load_files(Some(&path), None).unwrap();

        let names: Vec<&str> = config.agents.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "claude", "codex", "gemini", "goose", "kilo", "openai", "qwen"
            ]
        );
        assert_eq!(config.default_agent.as_deref(), Some("claude"));
        assert_eq!(config.agents["codex"].mode.as_deref(), Some("agent"));
    }

    #[test]
    fn prompt_styles_by_name() {
        for style in PromptStyle::ALL {
            assert_eq!(PromptStyle::from_name(style.name()), Some(style));
            let config: ConfigFile =
                toml::from_str(&format!("prompt = \"{}\"", style.name())).unwrap();
            assert_eq!(config.prompt, Some(style));
        }
        assert_eq!(PromptStyle::from_name("ps1"), None);
    }

    #[test]
    fn rejects_invalid_values() {
        let tmp = tempfile::tempdir().unwrap();
        let cases = [
            "shell = []\n",
            "unknown_key = 1\n",
            "prompt = \"ps1\"\n",
            "thinking = \"loud\"\n",
            "links = \"never\"\n",
            "shell_env = \"sometimes\"\n",
            "input = \"bash\"\n",
            "audit_entries = -1\n",
            "[agents.qwen]\ncommand = \"qwen\"\noptions = { effort = 3 }\n",
            // The old mapping keys are not accepted any more.
            "[agents.qwen]\ncommand = \"qwen\"\npermission_mode = \"normal\"\n",
            "[agents.qwen]\ncommand = \"qwen\"\nmode = 1\n",
        ];

        for (i, text) in cases.iter().enumerate() {
            let path = write(tmp.path(), &format!("case{i}.toml"), text);
            assert!(
                Config::load_files(Some(&path), None).is_err(),
                "accepted: {text}"
            );
        }
    }
}
