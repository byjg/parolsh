//! The alert when something ends in the background: a spell, or a turn sent
//! there with Ctrl+Z. You may be looking elsewhere: the line Parolsh prints
//! is not enough.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::Alert;
use crate::ui;

/// The players tried for a sound file, in order.
const PLAYERS: [&str; 2] = ["pw-play", "paplay"];

/// Alerts that something ended after running `took`: not when it took less
/// than `after`, you were still there.
pub fn ended(alert: &Alert, after: Duration, took: Duration) {
    if took < after {
        return;
    }
    match alert {
        Alert::Off => {}
        Alert::Bell => bell(),
        // Without a player, the bell.
        Alert::Sound(path) => {
            if !play(&expand(path, std::env::var_os("HOME").map(PathBuf::from))) {
                bell();
            }
        }
    }
}

/// The terminal's bell: the terminal decides what it does (a sound, a
/// flash, the tab marked).
fn bell() {
    if ui::is_ansi() {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07");
        let _ = out.flush();
    }
}

/// Plays `path` with the first player found, without waiting for it.
fn play(path: &Path) -> bool {
    for player in PLAYERS {
        let started = Command::new(player)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = started {
            std::thread::spawn(move || child.wait());
            return true;
        }
    }
    false
}

/// `~/` is the home directory.
fn expand(path: &Path, home: Option<PathBuf>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_in_the_home_directory_is_found() {
        let home = Some(PathBuf::from("/home/me"));

        assert_eq!(
            expand(Path::new("~/Downloads/beep.mp3"), home.clone()),
            PathBuf::from("/home/me/Downloads/beep.mp3")
        );
        assert_eq!(
            expand(Path::new("/usr/share/sounds/beep.oga"), home.clone()),
            PathBuf::from("/usr/share/sounds/beep.oga")
        );
        // Only a whole `~` component.
        assert_eq!(
            expand(Path::new("~me/beep.mp3"), home),
            PathBuf::from("~me/beep.mp3")
        );
        assert_eq!(
            expand(Path::new("~/beep.mp3"), None),
            PathBuf::from("~/beep.mp3")
        );
    }
}
