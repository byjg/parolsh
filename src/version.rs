//! The version Parolsh shows: `Cargo.toml`'s for a build of the release's
//! tag, and marked otherwise, so a build of work in progress is not taken for
//! the release: `0.6.0-dev (85cb214, dirty)`. `build.rs` asks git.

/// What git knew of the source when it was built.
struct Source<'a> {
    /// The short commit hash; empty when built without git.
    commit: &'a str,
    /// Tracked files were changed.
    dirty: bool,
    /// The commit is the tag of this version, `v<version>`.
    released: bool,
}

const BUILT: Source = Source {
    commit: env!("PAROLSH_GIT_COMMIT"),
    dirty: !env!("PAROLSH_GIT_DIRTY").is_empty(),
    released: !env!("PAROLSH_GIT_RELEASED").is_empty(),
};

/// For `--version`: `0.6.0`, or `0.6.0-dev (85cb214, dirty)`.
pub fn long() -> String {
    describe(env!("CARGO_PKG_VERSION"), &BUILT, true)
}

/// For the banner: `0.6.0`, or `0.6.0-dev`.
pub fn short() -> String {
    describe(env!("CARGO_PKG_VERSION"), &BUILT, false)
}

fn describe(version: &str, source: &Source, details: bool) -> String {
    // Without git there is nothing to tell: a source archive of the release.
    if source.commit.is_empty() || (source.released && !source.dirty) {
        return version.to_string();
    }
    if !details {
        return format!("{version}-dev");
    }
    let dirty = if source.dirty { ", dirty" } else { "" };
    format!("{version}-dev ({}{dirty})", source.commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(commit: &str, dirty: bool, released: bool) -> Source<'_> {
        Source {
            commit,
            dirty,
            released,
        }
    }

    #[test]
    fn a_clean_build_of_the_tag_is_the_plain_version() {
        let tag = source("85cb214", false, true);

        assert_eq!(describe("0.6.0", &tag, true), "0.6.0");
        assert_eq!(describe("0.6.0", &tag, false), "0.6.0");
    }

    #[test]
    fn other_builds_say_dev_with_the_commit_and_dirty() {
        let ahead = source("85cb214", false, false);
        let dirty = source("85cb214", true, false);
        // Changed files on top of the tag are not the release either.
        let dirty_tag = source("85cb214", true, true);

        assert_eq!(describe("0.6.0", &ahead, true), "0.6.0-dev (85cb214)");
        assert_eq!(
            describe("0.6.0", &dirty, true),
            "0.6.0-dev (85cb214, dirty)"
        );
        assert_eq!(
            describe("0.6.0", &dirty_tag, true),
            "0.6.0-dev (85cb214, dirty)"
        );
        assert_eq!(describe("0.6.0", &dirty, false), "0.6.0-dev");
    }

    #[test]
    fn without_git_the_version_is_the_crates() {
        assert_eq!(describe("0.6.0", &source("", false, false), true), "0.6.0");
    }
}
