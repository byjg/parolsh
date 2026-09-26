//! Configuration: the global file, overridden field by field by the project file.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

/// How the input prompt is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptStyle {
    /// The directory name and `❯`.
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
    pub thinking: ThinkingDisplay,
    /// Render the answers' markdown on ANSI terminals.
    pub markdown: bool,
    pub links: LinkStyle,
    pub default_agent: Option<String>,
    pub agents: BTreeMap<String, Agent>,
}

/// One config file as written on disk: every field is optional so a project
/// file can override only what it needs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    shell: Option<Vec<String>>,
    prompt: Option<PromptStyle>,
    thinking: Option<ThinkingDisplay>,
    markdown: Option<bool>,
    links: Option<LinkStyle>,
    default_agent: Option<String>,
    #[serde(default)]
    agents: BTreeMap<String, AgentFile>,
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

    fn merge(mut self, over: Self) -> Self {
        self.shell = over.shell.or(self.shell);
        self.prompt = over.prompt.or(self.prompt);
        self.thinking = over.thinking.or(self.thinking);
        self.markdown = over.markdown.or(self.markdown);
        self.links = over.links.or(self.links);
        self.default_agent = over.default_agent.or(self.default_agent);
        for (name, agent) in over.agents {
            let base = self.agents.entry(name).or_default();
            base.command = agent.command.or(base.command.take());
            base.args = agent.args.or(base.args.take());
            base.env.extend(agent.env);
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
        let mut file = ConfigFile::default();
        for path in [global, project].into_iter().flatten() {
            file = file.merge(ConfigFile::read(path)?);
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
            thinking: file.thinking.unwrap_or_default(),
            markdown: file.markdown.unwrap_or(true),
            links: file.links.unwrap_or_default(),
            default_agent: file.default_agent,
            agents,
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
        assert_eq!(config.thinking, ThinkingDisplay::Status);
        assert!(config.markdown);
        assert_eq!(config.links, LinkStyle::Both);
        assert_eq!(config.default_agent, None);
        assert!(config.agents.is_empty());
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
            shell = ["zsh", "-ic"]
            prompt = "starship"

            [agents.claude]
            mode = "auto"
            "#,
        );

        let config = Config::load_files(Some(&global), Some(&project)).unwrap();

        assert_eq!(config.shell, ["zsh", "-ic"]);
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

    #[test]
    fn env_is_merged_key_by_key() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(
            tmp.path(),
            "global.toml",
            r#"
            [agents.qwen]
            command = "qwen"
            env = { OPENAI_BASE_URL = "https://api.openai.com/v1", OPENAI_MODEL = "gpt-5" }
            "#,
        );
        let project = write(
            tmp.path(),
            "project.toml",
            r#"
            [agents.qwen]
            env = { OPENAI_MODEL = "gpt-5-mini" }
            "#,
        );

        let config = Config::load_files(Some(&global), Some(&project)).unwrap();
        let qwen = &config.agents["qwen"];

        assert_eq!(qwen.env["OPENAI_BASE_URL"], "https://api.openai.com/v1");
        assert_eq!(qwen.env["OPENAI_MODEL"], "gpt-5-mini");
    }

    #[test]
    fn options_and_thinking_merge_like_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(
            tmp.path(),
            "global.toml",
            r#"
            thinking = "hidden"

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
            markdown = false
            links = "inline"

            [agents.claude]
            options = { effort = "high" }
            "#,
        );

        let config = Config::load_files(Some(&global), Some(&project)).unwrap();

        assert_eq!(config.thinking, ThinkingDisplay::Show);
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
