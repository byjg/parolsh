//! First run: when there is no global configuration, write one from the
//! built-in sample, with the agents found on `PATH` already enabled.

use std::path::Path;

/// The sample configuration, built into the binary so the first run does not
/// depend on where a package installed the file.
pub const SAMPLE: &str = include_str!("../config.sample.toml");

/// Agents from the sample that can be enabled without the user's data, in
/// order of preference for `default_agent`, with the command to look for.
const KNOWN_AGENTS: [(&str, &str); 6] = [
    ("claude", "claude-agent-acp"),
    ("codex", "codex-acp"),
    ("gemini", "gemini"),
    ("qwen", "qwen"),
    ("kilo", "kilo"),
    ("goose", "goose"),
];

/// Writes the configuration at `path` if it does not exist yet. Returns a
/// message for the user, or `None` when a configuration was already there.
pub fn first_run(path: &Path, search_path: Option<&str>) -> Option<String> {
    if path.exists() {
        return None;
    }
    let found = installed_agents(search_path);
    let text = configuration(&found);
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text));
    if let Err(e) = written {
        return Some(format!("cannot create {}: {e}", path.display()));
    }

    let path = path.display();
    Some(match found.first() {
        Some(default) => format!(
            "Created {path} with {}. Using {default}; switch with #agent <name>.",
            found.join(", ")
        ),
        None => format!(
            "Created {path}. No ACP agent was found on PATH: install one, then \
             uncomment it there (see #config)."
        ),
    })
}

/// Names of the known agents whose command is on `search_path`.
pub fn installed_agents(search_path: Option<&str>) -> Vec<&'static str> {
    KNOWN_AGENTS
        .iter()
        .filter(|(_, command)| crate::shellenv::find_command(command, search_path).is_some())
        .map(|(name, _)| *name)
        .collect()
}

/// The sample with the blocks of `agents` uncommented (except `env` and
/// `options`, whose values are only examples) and `default_agent` set to the
/// first of them.
pub fn configuration(agents: &[&str]) -> String {
    let mut out = String::with_capacity(SAMPLE.len());
    let mut enabling = false;
    for line in SAMPLE.lines() {
        let setting = line.strip_prefix("# ").filter(|rest| is_setting(rest));
        match setting {
            Some(rest) if rest.starts_with("[agents.") => {
                let name = rest.trim_start_matches("[agents.").trim_end_matches(']');
                enabling = agents.contains(&name);
                out.push_str(if enabling { rest } else { line });
            }
            Some(rest) if enabling && !is_example(rest) => out.push_str(rest),
            Some(rest) if rest.starts_with("default_agent ") && !agents.is_empty() => {
                out.push_str(&format!("default_agent = \"{}\"", agents[0]));
            }
            _ => {
                // A blank line or a comment ends an agent's block.
                if setting.is_none() {
                    enabling = false;
                }
                out.push_str(line);
            }
        }
        out.push('\n');
    }
    out
}

/// Settings whose sample values need the user's own data.
fn is_example(line: &str) -> bool {
    line.starts_with("env ") || line.starts_with("options ")
}

/// A commented-out TOML line: a table header or `key = value`.
fn is_setting(line: &str) -> bool {
    line.starts_with('[')
        || line
            .split_once(" = ")
            .is_some_and(|(key, _)| key.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn fake_command(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn finds_installed_agents_in_preference_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fake_command(first.path(), "qwen");
        fake_command(second.path(), "codex-acp");
        // Not executable: not an installed command.
        std::fs::write(first.path().join("gemini"), "").unwrap();
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();

        assert_eq!(installed_agents(path.to_str()), ["codex", "qwen"]);
        assert!(installed_agents(None).is_empty());
    }

    #[test]
    fn enables_the_found_agents_and_sets_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, configuration(&["codex", "goose"])).unwrap();

        let config = Config::load_file(&path).unwrap();

        assert_eq!(config.default_agent.as_deref(), Some("codex"));
        let names: Vec<&str> = config.agents.keys().map(String::as_str).collect();
        assert_eq!(names, ["codex", "goose"]);
        assert_eq!(config.agents["codex"].mode.as_deref(), Some("agent"));
        assert_eq!(config.agents["goose"].args, ["acp"]);
        // The example env and options values stay commented out.
        assert!(config.agents["goose"].env.is_empty());
        assert!(config.agents["codex"].options.is_empty());
    }

    #[test]
    fn without_agents_the_sample_is_unchanged() {
        assert_eq!(configuration(&[]), SAMPLE);
    }

    #[test]
    fn first_run_writes_once() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        fake_command(bin.path(), "claude-agent-acp");
        let path = tmp.path().join("parolsh/config.toml");

        let message = first_run(&path, bin.path().to_str()).unwrap();

        assert!(message.starts_with("Created "), "{message}");
        assert!(message.ends_with("with claude. Using claude; switch with #agent <name>."));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("\n[agents.claude]\ncommand = \"claude-agent-acp\"\n"));

        std::fs::write(&path, "# mine\n").unwrap();
        assert_eq!(first_run(&path, bin.path().to_str()), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
    }

    #[test]
    fn first_run_without_agents_points_to_the_docs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");

        let message = first_run(&path, None).unwrap();

        assert!(
            message.contains("No ACP agent was found on PATH"),
            "{message}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLE);
    }
}
