//! Tells the build what git knows about the source: the commit, whether
//! files are changed, and whether this commit is the release's tag. The
//! version shown is made from that in `src/version.rs`. Without git (a
//! source archive), all three are empty and the version is `Cargo.toml`'s.

use std::process::Command;

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    // Changed tracked files; untracked ones are not part of the build.
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|changes| !changes.is_empty());
    let released = git(&["describe", "--tags", "--exact-match", "HEAD"])
        .is_some_and(|tag| tag == format!("v{version}"));

    println!("cargo:rustc-env=PAROLSH_GIT_COMMIT={commit}");
    println!(
        "cargo:rustc-env=PAROLSH_GIT_DIRTY={}",
        if dirty { "true" } else { "" }
    );
    println!(
        "cargo:rustc-env=PAROLSH_GIT_RELEASED={}",
        if released { "true" } else { "" }
    );

    // Run again when the commit, the staged files or the tags change. An
    // edit that is not staged is only seen at the next of these.
    println!("cargo:rerun-if-changed=Cargo.toml");
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        for path in ["HEAD", "index", "packed-refs", "refs/tags", "refs/heads"] {
            println!("cargo:rerun-if-changed={dir}/{path}");
        }
    }
}

/// The output of a git command, trimmed; `None` when git or the repository
/// is missing, or the command fails.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
