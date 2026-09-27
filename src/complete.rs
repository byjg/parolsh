//! Tab completion for `!command` and `!+command`: command names from `PATH`
//! and from bash (aliases, functions, builtins), then file names. It knows
//! no command's own arguments (git branches, flags): `!bash` has those.
//! Text for the agent has nothing to complete.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use reedline::{Completer, CompletionResult, Span, Suggestion};

use crate::shellenv::is_executable;

/// Characters that end a word, besides whitespace.
const SEPARATORS: &str = "|;&()<>";

/// Characters a file name needs escaped to stay one word for the shell.
const SPECIAL: &str = " '\"\\()&;|<>$`!*?[]{}#";

pub struct ShellCompleter {
    /// Parolsh's current directory, which `#cd` changes.
    pub cwd: Arc<Mutex<PathBuf>>,
    /// Bash's aliases, functions and builtins, read in the background when
    /// Parolsh starts; not set until then.
    pub names: Arc<OnceLock<Vec<String>>>,
}

impl Completer for ShellCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let cwd = self.cwd.lock().expect("cwd lock").clone();
        let names = self.names.get().map(Vec::as_slice).unwrap_or_default();
        let search_path = std::env::var("PATH").ok();
        CompletionResult::fresh(suggestions(line, pos, &cwd, search_path.as_deref(), names))
    }
}

/// The completions of the word before `pos`, replacing that word.
fn suggestions(
    line: &str,
    pos: usize,
    cwd: &Path,
    search_path: Option<&str>,
    names: &[String],
) -> Vec<Suggestion> {
    let Some(start) = command_start(line).filter(|&start| start <= pos) else {
        return Vec::new();
    };
    let word_start = start + word_start(&line[start..pos]);
    let word = unescape(&line[word_start..pos]);
    let command_position = is_command_position(&line[start..word_start]);
    let mut found = if command_position && !word.contains('/') {
        commands(&word, search_path, names)
    } else {
        files(&word, cwd, command_position)
    };
    for suggestion in &mut found {
        suggestion.span = Span::new(word_start, pos);
    }
    found
}

/// Where the shell command starts: after `!` or `!+`, as `input::route`
/// reads them. `None` for lines that do not go to the shell.
fn command_start(line: &str) -> Option<usize> {
    let command = line.trim_start().strip_prefix('!')?;
    let command = command.trim_start().strip_prefix('+').unwrap_or(command);
    Some(line.len() - command.len())
}

/// The start of the last word of `text`. A backslash keeps the next
/// character in the word (`My\ Files`).
fn word_start(text: &str) -> usize {
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c.is_whitespace() || SEPARATORS.contains(c) {
            start = i + c.len_utf8();
        }
    }
    start
}

/// True when the word after `before` is a command name: the first word, or
/// the first after `|`, `;`, `&` or `(`.
fn is_command_position(before: &str) -> bool {
    before
        .trim_end()
        .chars()
        .last()
        .is_none_or(|c| "|;&(".contains(c))
}

/// Command names starting with `word`. An empty word completes nothing:
/// every command on the system is not a useful list.
fn commands(word: &str, search_path: Option<&str>, names: &[String]) -> Vec<Suggestion> {
    if word.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<String> = names
        .iter()
        .filter(|name| name.starts_with(word))
        .cloned()
        .collect();
    for dir in search_path.map(std::env::split_paths).into_iter().flatten() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        found.extend(entries.flatten().filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (name.starts_with(word) && is_executable(&entry.path())).then_some(name)
        }));
    }
    found.sort();
    found.dedup();
    found
        .into_iter()
        .map(|name| suggestion(escape(&name), true))
        .collect()
}

/// Files and directories whose path starts with `word`. Hidden ones only
/// when the name being typed starts with `.`. In command position, only
/// directories and programs.
fn files(word: &str, cwd: &Path, programs_only: bool) -> Vec<Suggestion> {
    let (dir, prefix) = match word.rfind('/') {
        Some(slash) => word.split_at(slash + 1),
        None => ("", word),
    };
    let Ok(entries) = std::fs::read_dir(directory(dir, cwd)) else {
        return Vec::new();
    };
    let mut found: Vec<Suggestion> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
                return None;
            }
            let path = entry.path();
            let is_dir = path.is_dir();
            if programs_only && !is_dir && !is_executable(&path) {
                return None;
            }
            let value = format!("{}{}", escape(dir), escape(&name));
            Some(if is_dir {
                suggestion(value + "/", false)
            } else {
                suggestion(value, true)
            })
        })
        .collect();
    found.sort_by(|a, b| a.value.cmp(&b.value));
    found
}

/// The directory a typed `dir` part names: `~/` is the home directory,
/// relative paths start at `cwd`.
fn directory(dir: &str, cwd: &Path) -> PathBuf {
    if let Some(rest) = dir.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    cwd.join(dir)
}

fn suggestion(value: String, append_whitespace: bool) -> Suggestion {
    Suggestion {
        value,
        append_whitespace,
        ..Suggestion::default()
    }
}

fn escape(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for c in name.chars() {
        if SPECIAL.contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn unescape(word: &str) -> String {
    let mut plain = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        plain.extend(if c == '\\' { chars.next() } else { Some(c) });
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory with a program `tool`, a program `tidy`, a plain file
    /// `notes.txt`, a hidden `.env`, a file with a space and a directory.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for program in ["tool", "tidy"] {
            let path = dir.path().join(program);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        for file in ["notes.txt", ".env", "My Files.md"] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("src")).unwrap();
        dir
    }

    /// The lines after completing at the end of `line`, one per suggestion.
    fn complete(line: &str, cwd: &Path, search_path: Option<&str>, names: &[&str]) -> Vec<String> {
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        suggestions(line, line.len(), cwd, search_path, &names)
            .into_iter()
            .map(|s| {
                let space = if s.append_whitespace { " " } else { "" };
                format!("{}{}{space}", &line[..s.span.start], s.value)
            })
            .collect()
    }

    #[test]
    fn command_names_come_from_path_and_bash() {
        let dir = fixture();
        let path = dir.path().to_str();

        assert_eq!(
            complete("!t", dir.path(), path, &["type", "tldr-alias"]),
            ["!tidy ", "!tldr-alias ", "!tool ", "!type "]
        );
        assert_eq!(complete("!+to", dir.path(), path, &[]), ["!+tool "]);
        assert_eq!(complete("  ! + to", dir.path(), path, &[]), ["  ! + tool "]);
        assert_eq!(complete("!ls | to", dir.path(), path, &[]), ["!ls | tool "]);
        assert_eq!(
            complete("!true && to", dir.path(), path, &[]),
            ["!true && tool "]
        );
    }

    #[test]
    fn an_empty_command_name_completes_nothing() {
        let dir = fixture();

        assert!(complete("!", dir.path(), dir.path().to_str(), &["ll"]).is_empty());
        assert!(complete("!+ ", dir.path(), dir.path().to_str(), &["ll"]).is_empty());
    }

    #[test]
    fn arguments_are_files_in_the_current_directory() {
        let dir = fixture();

        assert_eq!(
            complete("!cat ", dir.path(), None, &[]),
            [
                "!cat My\\ Files.md ",
                "!cat notes.txt ",
                "!cat src/",
                "!cat tidy ",
                "!cat tool "
            ]
        );
        assert_eq!(complete("!cat .e", dir.path(), None, &[]), ["!cat .env "]);
        assert_eq!(
            complete("!cat My\\ F", dir.path(), None, &[]),
            ["!cat My\\ Files.md "]
        );
        assert_eq!(
            complete("!ls > no", dir.path(), None, &[]),
            ["!ls > notes.txt "]
        );
    }

    #[test]
    fn paths_complete_inside_directories() {
        let dir = fixture();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        let absolute = format!("!cat {}/src/m", dir.path().display());

        assert_eq!(
            complete("!cat src/m", dir.path(), None, &[]),
            ["!cat src/main.rs "]
        );
        assert_eq!(
            complete(&absolute, Path::new("/"), None, &[]),
            [format!("!cat {}/src/main.rs ", dir.path().display())]
        );
        assert!(complete("!cat missing/", dir.path(), None, &[]).is_empty());
    }

    #[test]
    fn a_path_as_command_completes_programs_and_directories() {
        let dir = fixture();

        assert_eq!(
            complete("!./", dir.path(), None, &[]),
            ["!./src/", "!./tidy ", "!./tool "]
        );
    }

    #[test]
    fn only_shell_lines_complete() {
        let dir = fixture();
        let path = dir.path().to_str();

        for line in ["to", "#to", "/to", "why is ! here to"] {
            assert!(
                complete(line, dir.path(), path, &["tool"]).is_empty(),
                "{line}"
            );
        }
    }

    #[test]
    fn completes_the_word_at_the_cursor() {
        let dir = fixture();
        let line = "!cat no | wc";
        let names = Vec::new();

        let found = suggestions(line, "!cat no".len(), dir.path(), None, &names);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, "notes.txt");
        assert_eq!(found[0].span, Span::new(5, 7));
    }
}
