//! Tab completion for `!command`, `!+command` and, in shell mode, plain
//! lines: command names from `PATH`
//! and from bash (aliases, functions, builtins); then each command's own
//! arguments (git branches, ssh hosts) from bash-completion, or file names.
//! `#` lines complete the command's name, and `#cd` a directory. In text
//! for the agent, `@` completes a file or directory from the agent's
//! directory.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use reedline::{Completer, CompletionResult, Span, Suggestion};

use crate::input::{Input, Mode, SharedMode, route};
use crate::shellenv::{self, is_executable};

/// Asks bash-completion for the arguments of a command line, see the script.
const BASH_SCRIPT: &str = include_str!("complete.bash");

/// How long bash-completion may take: the completer runs on each key while
/// the menu is open. Slower, the file names are offered instead.
const BASH_TIMEOUT: Duration = Duration::from_millis(500);

/// Characters that end a word, besides whitespace.
const SEPARATORS: &str = "|;&()<>";

/// Characters a file name needs escaped to stay one word for the shell.
const SPECIAL: &str = " '\"\\()&;|<>$`!*?[]{}#";

/// The `#` commands, as `#help` lists them.
pub const CONTROL_COMMANDS: [&str; 16] = [
    "help", "new", "cd", "agent", "config", "options", "project", "prompt", "audit", "sessions",
    "forget", "resume", "redraw", "jobs", "fg", "exit",
];

pub struct ShellCompleter {
    /// Where shell commands run.
    pub cwd: Arc<Mutex<PathBuf>>,
    /// The agent's directory, for `@path` in text for the agent.
    pub agent_cwd: Arc<Mutex<PathBuf>>,
    /// Bash's aliases, functions and builtins, read in the background when
    /// Parolsh starts; not set until then.
    pub names: Arc<OnceLock<Vec<String>>>,
    pub mode: SharedMode,
    /// The bash that completes arguments, when `shell` is bash.
    pub bash: Option<String>,
}

impl Completer for ShellCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let cwd = self.cwd.lock().expect("cwd lock").clone();
        let agent_cwd = self.agent_cwd.lock().expect("cwd lock").clone();
        let names = self.names.get().map(Vec::as_slice).unwrap_or_default();
        let search_path = std::env::var("PATH").ok();
        let mode = self.mode.get();
        let found = completions(line, pos, mode, &cwd, &agent_cwd, || {
            suggestions(
                line,
                pos,
                mode,
                &cwd,
                search_path.as_deref(),
                names,
                self.bash.as_deref(),
            )
        });
        CompletionResult::fresh(found)
    }
}

/// What Tab completes at `pos`: a `#` line's command name or `#cd`
/// directory, an `@path` in text for the agent, or else the word of a shell
/// line (`shell`).
fn completions(
    line: &str,
    pos: usize,
    mode: Mode,
    cwd: &Path,
    agent_cwd: &Path,
    shell: impl FnOnce() -> Vec<Suggestion>,
) -> Vec<Suggestion> {
    if let Some(found) = control(line, pos, cwd) {
        return found;
    }
    if let Input::Agent(_) = route(line, mode) {
        return mention(line, pos, agent_cwd);
    }
    shell()
}

/// The completions of the word of a shell line before `pos`, replacing that
/// word.
fn suggestions(
    line: &str,
    pos: usize,
    mode: Mode,
    cwd: &Path,
    search_path: Option<&str>,
    names: &[String],
    bash: Option<&str>,
) -> Vec<Suggestion> {
    let Some(start) = command_start(line, mode).filter(|&start| start <= pos) else {
        return Vec::new();
    };
    let word_start = start + word_start(&line[start..pos]);
    let word = unescape(&line[word_start..pos]);
    let command_position = is_command_position(&line[start..word_start]);
    let mut found = if command_position && !word.contains('/') {
        commands(&word, search_path, names)
    } else if let Some(found) = bash.filter(|_| !command_position).and_then(|bash| {
        arguments(
            bash,
            &line[start + segment_start(&line[start..word_start])..pos],
            &word,
            cwd,
        )
    }) {
        found
    } else {
        files(&word, cwd, command_position)
    };
    for suggestion in &mut found {
        suggestion.span = Span::new(word_start, pos);
    }
    found
}

/// The completions of a `#` line: the command's name, then `#cd`'s
/// directory. `None` for other lines.
fn control(line: &str, pos: usize, cwd: &Path) -> Option<Vec<Suggestion>> {
    let start = line.len() - line.trim_start().len();
    let typed = line.get(start..pos)?.strip_prefix('#')?;
    let found = match typed.split_once(char::is_whitespace) {
        None => CONTROL_COMMANDS
            .iter()
            .filter(|name| name.starts_with(typed))
            .map(|name| Suggestion {
                span: Span::new(start, pos),
                ..suggestion(format!("#{name}"), true)
            })
            .collect(),
        // `#cd` takes the rest of the line as the path, unescaped.
        Some(("cd", path)) => {
            let path = path.trim_start();
            let mut found = directories(path, cwd);
            for suggestion in &mut found {
                suggestion.span = Span::new(pos - path.len(), pos);
            }
            found
        }
        Some(_) => Vec::new(),
    };
    Some(found)
}

/// The files and directories an `@path` word at `pos` may name, from the
/// agent's directory. Other words of the agent's text complete nothing.
fn mention(line: &str, pos: usize, agent_cwd: &Path) -> Vec<Suggestion> {
    let start = word_start(&line[..pos]);
    let Some(path) = line[start..pos].strip_prefix('@') else {
        return Vec::new();
    };
    let mut found = files(&unescape(path), agent_cwd, false);
    for suggestion in &mut found {
        suggestion.value.insert(0, '@');
        suggestion.span = Span::new(start, pos);
    }
    found
}

/// Where the shell command starts: after `!` or `!+`, as `input::route`
/// reads them, or at the start of a plain line in shell mode. `None` for
/// lines that do not go to the shell.
fn command_start(line: &str, mode: Mode) -> Option<usize> {
    let trimmed = line.trim_start();
    let command = match trimmed.strip_prefix('!') {
        Some(command) => command,
        None if mode == Mode::Shell && !trimmed.starts_with(['#', '?']) => trimmed,
        None => return None,
    };
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

/// Where the last command of `before` starts: after the last `|`, `;`, `&`
/// or `(`, and its blanks.
fn segment_start(before: &str) -> usize {
    let after = before.rfind(|c| "|;&(".contains(c)).map_or(0, |i| i + 1);
    after + (before[after..].len() - before[after..].trim_start().len())
}

/// The completions bash-completion offers for the last word of `command`,
/// run in `cwd`. `None` when there are none, when it takes longer than
/// `BASH_TIMEOUT`, or when they do not continue `word` (bash splits words
/// at `=` and `:` too): the file names are offered then.
fn arguments(bash: &str, command: &str, word: &str, cwd: &Path) -> Option<Vec<Suggestion>> {
    let (bytes, _) = shellenv::capture(
        bash,
        &["-c", BASH_SCRIPT, "parolsh", command],
        Some(cwd),
        BASH_TIMEOUT,
    )
    .ok()?;
    let mut values: Vec<String> = String::from_utf8_lossy(&bytes)
        .split('\0')
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    if values.is_empty() || values.iter().any(|value| !value.starts_with(word)) {
        return None;
    }
    values.sort();
    values.dedup();
    Some(
        values
            .into_iter()
            .map(|value| {
                // A trailing space is bash-completion's way to say the word
                // is complete; `--option=`, `dir/` and `host:` go on.
                if let Some(value) = value.strip_suffix(' ') {
                    return suggestion(escape(value), true);
                }
                if directory(&value, cwd).is_dir() && !value.ends_with('/') {
                    return suggestion(escape(&value) + "/", false);
                }
                let goes_on = value.ends_with(['/', '=', ':']);
                suggestion(escape(&value), !goes_on)
            })
            .collect(),
    )
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
    let (dir, entries) = entries(word, cwd);
    entries
        .into_iter()
        .filter(|(_, path)| !programs_only || path.is_dir() || is_executable(path))
        .map(|(name, path)| {
            let value = format!("{}{}", escape(dir), escape(&name));
            if path.is_dir() {
                suggestion(value + "/", false)
            } else {
                suggestion(value, true)
            }
        })
        .collect()
}

/// The directories a typed path may go on to, unescaped (`#cd` reads the
/// path as typed).
fn directories(path: &str, cwd: &Path) -> Vec<Suggestion> {
    let (dir, entries) = entries(path, cwd);
    entries
        .into_iter()
        .filter(|(_, path)| path.is_dir())
        .map(|(name, _)| suggestion(format!("{dir}{name}/"), false))
        .collect()
}

/// The entries, sorted by name, of the directory a typed path names whose
/// names start like its last part, with that directory part as typed:
/// `src/m` gives `src/` and `main.rs`. Hidden entries only when the part
/// starts with a dot.
fn entries<'a>(word: &'a str, cwd: &Path) -> (&'a str, Vec<(String, PathBuf)>) {
    let (dir, prefix) = match word.rfind('/') {
        Some(slash) => word.split_at(slash + 1),
        None => ("", word),
    };
    let Ok(entries) = std::fs::read_dir(directory(dir, cwd)) else {
        return (dir, Vec::new());
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let hidden = name.starts_with('.') && !prefix.starts_with('.');
            (name.starts_with(prefix) && !hidden).then(|| (name, entry.path()))
        })
        .collect();
    found.sort();
    (dir, found)
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

pub fn unescape(word: &str) -> String {
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
        complete_in(Mode::Agent, line, cwd, search_path, names)
    }

    fn complete_in(
        mode: Mode,
        line: &str,
        cwd: &Path,
        search_path: Option<&str>,
        names: &[&str],
    ) -> Vec<String> {
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        let shell = || suggestions(line, line.len(), mode, cwd, search_path, &names, None);
        completions(line, line.len(), mode, cwd, cwd, shell)
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

        for line in ["to", "/to", "why is ! here to"] {
            assert!(
                complete(line, dir.path(), path, &["tool"]).is_empty(),
                "{line}"
            );
        }
    }

    #[test]
    fn in_shell_mode_plain_lines_complete() {
        let dir = fixture();
        let path = dir.path().to_str();
        let shell = |line| complete_in(Mode::Shell, line, dir.path(), path, &["tool"]);

        assert_eq!(shell("too"), ["tool "]);
        assert_eq!(shell("cat no"), ["cat notes.txt "]);
        assert_eq!(shell("!too"), ["!tool "]);
        assert!(shell("?to").is_empty());
    }

    #[test]
    fn at_in_text_for_the_agent_completes_paths_from_its_directory() {
        let dir = fixture();
        let agent_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(agent_dir.path().join("src")).unwrap();
        std::fs::write(agent_dir.path().join("src/app.rs"), "").unwrap();
        std::fs::write(agent_dir.path().join("My Notes.md"), "").unwrap();
        let complete = |mode, line: &str| -> Vec<String> {
            completions(
                line,
                line.len(),
                mode,
                dir.path(),
                agent_dir.path(),
                Vec::new,
            )
            .into_iter()
            .map(|s| {
                let space = if s.append_whitespace { " " } else { "" };
                format!("{}{}{space}", &line[..s.span.start], s.value)
            })
            .collect()
        };

        assert_eq!(complete(Mode::Agent, "hi @s"), ["hi @src/"]);
        assert_eq!(complete(Mode::Agent, "hi @src/a"), ["hi @src/app.rs "]);
        assert_eq!(complete(Mode::Agent, "read @My"), ["read @My\\ Notes.md "]);
        assert_eq!(
            complete(Mode::Shell, "?look at @src/"),
            ["?look at @src/app.rs "]
        );
        // Words without `@`, and `@` on shell lines, are not mentions.
        assert!(complete(Mode::Agent, "hi s").is_empty());
        assert!(complete(Mode::Agent, "!cat @s").is_empty());
        assert!(complete(Mode::Shell, "cat @s").is_empty());
    }

    #[test]
    fn hash_lines_complete_the_command_name() {
        let dir = fixture();
        let path = dir.path().to_str();

        assert_eq!(
            complete("#a", dir.path(), path, &[]),
            ["#agent ", "#audit "]
        );
        assert_eq!(complete("  #cd", dir.path(), path, &[]), ["  #cd "]);
        assert_eq!(
            complete("#", dir.path(), path, &[]).len(),
            CONTROL_COMMANDS.len()
        );
        assert!(complete("#to", dir.path(), path, &["tool"]).is_empty());
        // Only `#cd` has arguments to complete.
        assert!(complete("#agent t", dir.path(), path, &["tool"]).is_empty());
        let shell = complete_in(Mode::Shell, "#pr", dir.path(), path, &[]);
        assert_eq!(shell, ["#project ", "#prompt "]);
    }

    #[test]
    fn hash_cd_completes_directories_as_typed() {
        let dir = fixture();
        std::fs::create_dir(dir.path().join("My Projects")).unwrap();
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        std::fs::create_dir(dir.path().join("src/app")).unwrap();

        assert_eq!(
            complete("#cd ", dir.path(), None, &[]),
            ["#cd My Projects/", "#cd src/"]
        );
        assert_eq!(
            complete("#cd  My", dir.path(), None, &[]),
            ["#cd  My Projects/"]
        );
        assert_eq!(
            complete("#cd src/", dir.path(), None, &[]),
            ["#cd src/app/"]
        );
        assert_eq!(complete("#cd .h", dir.path(), None, &[]), ["#cd .hidden/"]);
        assert!(complete("#cd notes", dir.path(), None, &[]).is_empty());
    }

    #[test]
    fn completes_the_word_at_the_cursor() {
        let dir = fixture();
        let line = "!cat no | wc";
        let names = Vec::new();

        let found = suggestions(
            line,
            "!cat no".len(),
            Mode::Agent,
            dir.path(),
            None,
            &names,
            None,
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, "notes.txt");
        assert_eq!(found[0].span, Span::new(5, 7));
    }

    /// A bash that knows a completion for `fake`, then runs the script.
    fn fake_bash(dir: &Path) -> String {
        let path = dir.join("fake-bash");
        std::fs::write(
            &path,
            r#"#!/bin/bash
script=$2; shift 2
exec bash -c '
_fake() {
    case $2 in
        sl*) sleep 5 ;;
        do*) COMPREPLY=("done ") ;;
        s*) COMPREPLY=(src) ;;
        no*) COMPREPLY=(other) ;;
        *) COMPREPLY=($(compgen -W "alpha beta branch --color=" -- "$2")) ;;
    esac
}
complete -F _fake fake
'"$script" "$@"
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn complete_with_bash(line: &str, cwd: &Path) -> Vec<String> {
        let bash = fake_bash(cwd);
        suggestions(line, line.len(), Mode::Agent, cwd, None, &[], Some(&bash))
            .into_iter()
            .map(|s| {
                let space = if s.append_whitespace { " " } else { "" };
                format!("{}{}{space}", &line[..s.span.start], s.value)
            })
            .collect()
    }

    #[test]
    fn arguments_come_from_the_commands_bash_completion() {
        let dir = fixture();

        assert_eq!(
            complete_with_bash("!fake b", dir.path()),
            ["!fake beta ", "!fake branch "]
        );
        assert_eq!(
            complete_with_bash("!ls | fake al", dir.path()),
            ["!ls | fake alpha "]
        );
    }

    #[test]
    fn bash_completion_says_when_a_word_goes_on() {
        let dir = fixture();

        assert_eq!(
            complete_with_bash("!fake --co", dir.path()),
            ["!fake --color="]
        );
        assert_eq!(complete_with_bash("!fake do", dir.path()), ["!fake done "]);
        assert_eq!(complete_with_bash("!fake s", dir.path()), ["!fake src/"]);
    }

    #[test]
    fn without_bash_completion_arguments_are_files() {
        let dir = fixture();

        // No completion for the command, or one that does not continue the
        // word.
        assert_eq!(
            complete_with_bash("!cat no", dir.path()),
            ["!cat notes.txt "]
        );
        assert_eq!(
            complete_with_bash("!fake no", dir.path()),
            ["!fake notes.txt "]
        );
    }

    #[test]
    fn a_slow_bash_completion_gives_way_to_files() {
        let dir = fixture();
        let started = std::time::Instant::now();

        let found = complete_with_bash("!fake sl", dir.path());

        assert!(found.is_empty(), "{found:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
