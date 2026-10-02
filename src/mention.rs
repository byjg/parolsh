//! `@path` in a message to the agent: the files and directories it names go
//! with the message as ACP resource links, resolved from the agent's
//! directory, so the agent cannot read them from the wrong place.

use std::path::{Path, PathBuf};

use crate::complete::unescape;

/// Characters that may end a sentence after a mention: `see @notes.txt.`
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '"', '\''];

/// The files and directories `text` mentions as `@path`, in order, once
/// each. Relative paths start at `cwd`, `~/` is `home`. Words that name
/// nothing (`@team`) are left out. A backslash keeps a space in the path
/// (`@My\ Files.md`), as Tab completes it.
pub fn mentioned(text: &str, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for word in words(text) {
        let Some(path) = word.strip_prefix('@').filter(|path| !path.is_empty()) else {
            continue;
        };
        let path = unescape(path);
        let trimmed = path.trim_end_matches(TRAILING);
        let resolved = resolve(&path, cwd, home).or_else(|| resolve(trimmed, cwd, home));
        if let Some(resolved) = resolved
            && !found.contains(&resolved)
        {
            found.push(resolved);
        }
    }
    found
}

/// The words of `text`, split on whitespace a backslash does not escape.
fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
            start.get_or_insert(i);
        } else if c.is_whitespace() {
            if let Some(start) = start.take() {
                words.push(&text[start..i]);
            }
        } else {
            start.get_or_insert(i);
        }
    }
    if let Some(start) = start {
        words.push(&text[start..]);
    }
    words
}

/// The existing file or directory `path` names, absolute.
fn resolve(path: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let full = match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => cwd.join(path),
    };
    full.canonicalize().ok()
}

/// The `file://` URI of an absolute path, with the bytes a URI cannot carry
/// percent-encoded.
pub fn uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        for file in ["src/app.rs", "notes.txt", "My Files.md"] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        dir
    }

    fn names(text: &str, cwd: &Path, home: Option<&Path>) -> Vec<String> {
        let root = cwd.canonicalize().unwrap();
        mentioned(text, cwd, home)
            .iter()
            .map(|path| path.strip_prefix(&root).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn mentions_of_existing_paths_are_found_in_order_once() {
        let dir = fixture();

        assert_eq!(
            names(
                "compare @src/app.rs with @notes.txt and @src/app.rs",
                dir.path(),
                None
            ),
            ["src/app.rs", "notes.txt"]
        );
        assert_eq!(names("look in @src", dir.path(), None), ["src"]);
    }

    #[test]
    fn words_that_name_nothing_are_not_mentions() {
        let dir = fixture();

        assert!(names("ask @team or mail me@notes.txt", dir.path(), None).is_empty());
        assert!(names("a lone @ sign", dir.path(), None).is_empty());
    }

    #[test]
    fn punctuation_after_a_mention_is_not_part_of_it() {
        let dir = fixture();

        assert_eq!(
            names("read @notes.txt. Then (see @src/app.rs)", dir.path(), None),
            ["notes.txt", "src/app.rs"]
        );
    }

    #[test]
    fn an_escaped_space_stays_in_the_path() {
        let dir = fixture();

        assert_eq!(
            names("open @My\\ Files.md", dir.path(), None),
            ["My Files.md"]
        );
    }

    #[test]
    fn home_and_absolute_paths_resolve() {
        let dir = fixture();
        let elsewhere = tempfile::tempdir().unwrap();
        let absolute = format!("@{}", dir.path().join("notes.txt").display());

        let notes = vec![dir.path().join("notes.txt").canonicalize().unwrap()];

        assert_eq!(
            mentioned("@~/notes.txt", elsewhere.path(), Some(dir.path())),
            notes
        );
        assert_eq!(mentioned(&absolute, elsewhere.path(), None), notes);
    }

    #[test]
    fn uris_encode_what_a_path_may_hold() {
        assert_eq!(
            uri(Path::new("/tmp/My notes#1.md")),
            "file:///tmp/My%20notes%231.md"
        );
        assert_eq!(uri(Path::new("/a/ação")), "file:///a/a%C3%A7%C3%A3o");
    }
}
