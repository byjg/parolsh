//! Project discovery: the nearest ancestor directory holding `.parolsh/`.

use std::path::{Path, PathBuf};

pub const STATE_DIR: &str = ".parolsh";

pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(STATE_DIR).is_dir())
        .map(Path::to_path_buf)
}

/// Creates `.parolsh/` in `dir`. Returns false when it already existed.
pub fn init(dir: &Path) -> std::io::Result<bool> {
    let state = dir.join(STATE_DIR);
    if state.is_dir() {
        return Ok(false);
    }
    std::fs::create_dir(state)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_nearest_ancestor_with_a_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wallet");
        let deep = root.join("src/api");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(root.join(STATE_DIR)).unwrap();

        assert_eq!(find_root(&deep), Some(root.clone()));
        assert_eq!(find_root(&root), Some(root));
    }

    #[test]
    fn a_state_file_is_not_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(STATE_DIR), "").unwrap();

        assert_eq!(find_root(tmp.path()), None);
    }

    #[test]
    fn init_creates_the_state_dir_once() {
        let tmp = tempfile::tempdir().unwrap();

        assert!(init(tmp.path()).unwrap());
        assert!(!init(tmp.path()).unwrap());
        assert_eq!(find_root(tmp.path()), Some(tmp.path().to_path_buf()));
    }
}
