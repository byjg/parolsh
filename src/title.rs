//! The terminal's title: `parolsh` and the conversation's title, or the
//! directory before the agent gave one. While the agent works, a spinner and
//! how many background tasks run: `⠹ 2 bg · parolsh · Fix the batch script`.
//! It shows from another tab or window, where the prompt does not.

use nix::sys::signal::{SigSet, Signal};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::acp::{Activity, ActivityWatch};
use crate::ui;

/// How often the title changes while the agent works: terminals redraw their
/// tab on each change.
const TICK: Duration = Duration::from_millis(500);
/// Longest conversation title shown: tabs are narrow.
const MAX_TITLE: usize = 40;

/// Keeps the terminal's title up to date, and puts the previous one back
/// when dropped.
pub struct Title {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    /// Shown until the agent gives the conversation a title.
    place: Mutex<String>,
    agent: Mutex<Option<ActivityWatch>>,
    /// A command owns the terminal: its title is left alone.
    held: AtomicBool,
    /// The title on screen may not be ours: write it again.
    dirty: AtomicBool,
    stop: AtomicBool,
}

/// Leaves the title to a command while it runs.
pub struct Held(Arc<Shared>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.held.store(false, Ordering::SeqCst);
        self.0.dirty.store(true, Ordering::SeqCst);
    }
}

impl Title {
    /// Saves the terminal's title and starts updating it. Only on an ANSI
    /// terminal.
    pub fn start(place: String) -> Option<Self> {
        if !ui::is_ansi() {
            return None;
        }
        // Saved on the terminal's title stack (xterm, VTE, kitty, WezTerm,
        // tmux); elsewhere this does nothing.
        write("\x1b[22;0t");
        let shared = Arc::new(Shared {
            place: Mutex::new(place),
            ..Default::default()
        });
        let ticking = shared.clone();
        let thread = std::thread::spawn(move || tick(&ticking));
        Some(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// The directory, shown until the agent gives a title.
    pub fn set_place(&self, place: String) {
        if let Ok(mut current) = self.shared.place.lock() {
            *current = place;
        }
    }

    /// The agent whose work is shown, `None` when there is none.
    pub fn set_agent(&self, agent: Option<ActivityWatch>) {
        if let Ok(mut current) = self.shared.agent.lock() {
            *current = agent;
        }
    }

    /// Leaves the title alone until the returned guard is dropped: a command
    /// such as `vim` or `ssh` may set its own.
    pub fn hold(&self) -> Held {
        self.shared.held.store(true, Ordering::SeqCst);
        Held(self.shared.clone())
    }
}

impl Drop for Title {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        write("\x1b[23;0t");
    }
}

fn tick(shared: &Shared) {
    // While a command owns the terminal, Parolsh is in the background, and
    // a write from there would stop it on terminals with `tostop` set.
    let mut ttou = SigSet::empty();
    ttou.add(Signal::SIGTTOU);
    let _ = ttou.thread_block();

    let mut frame = 0;
    let mut shown = String::new();
    while !shared.stop.load(Ordering::SeqCst) {
        if !shared.held.load(Ordering::SeqCst) {
            let activity = shared
                .agent
                .lock()
                .ok()
                .and_then(|agent| agent.as_ref().map(ActivityWatch::get))
                .unwrap_or_default();
            let place = shared
                .place
                .lock()
                .map(|place| place.clone())
                .unwrap_or_default();
            let title = text(frame, &place, &activity);
            if title != shown || shared.dirty.swap(false, Ordering::SeqCst) {
                write(&format!("\x1b]2;{title}\x07"));
                shown = title;
            }
            frame += 1;
        }
        std::thread::park_timeout(TICK);
    }
}

/// The title: `[⠹ ][2 bg · ]parolsh · <title or place>`. The spinner turns
/// while a prompt is in flight or background tasks run.
pub fn text(frame: usize, place: &str, activity: &Activity) -> String {
    let mut title = String::new();
    if activity.busy || activity.tasks > 0 {
        title.push(ui::spinner(frame));
        title.push(' ');
    }
    if activity.tasks > 0 {
        title.push_str(&format!("{} bg · ", activity.tasks));
    }
    title.push_str("parolsh · ");
    match &activity.title {
        Some(name) => title.push_str(&shorten(name)),
        None => title.push_str(place),
    }
    // The agent's title must not end the escape sequence early.
    title.chars().filter(|c| !c.is_control()).collect()
}

fn shorten(title: &str) -> String {
    if title.chars().count() <= MAX_TITLE {
        return title.to_string();
    }
    let cut: String = title.chars().take(MAX_TITLE - 1).collect();
    format!("{}…", cut.trim_end())
}

fn write(sequence: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(sequence.as_bytes());
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(busy: bool, tasks: usize, title: Option<&str>) -> Activity {
        Activity {
            busy,
            tasks,
            oldest: None,
            title: title.map(str::to_string),
        }
    }

    #[test]
    fn an_idle_agent_shows_the_place_until_it_gives_a_title() {
        let place = "~/…/byjg/parolsh";

        assert_eq!(
            text(0, place, &activity(false, 0, None)),
            "parolsh · ~/…/byjg/parolsh"
        );
        assert_eq!(
            text(0, place, &activity(false, 0, Some("Fix the batch"))),
            "parolsh · Fix the batch"
        );
    }

    #[test]
    fn a_working_agent_spins_and_counts_its_background_tasks() {
        let turn = text(0, "~", &activity(true, 0, Some("Fix")));
        let tasks = text(1, "~", &activity(false, 2, Some("Fix")));

        assert_eq!(turn, "⠋ parolsh · Fix");
        assert_eq!(tasks, "⠙ 2 bg · parolsh · Fix");
    }

    #[test]
    fn a_long_title_is_cut_and_control_characters_dropped() {
        let long = "word ".repeat(20);
        let title = text(0, "~", &activity(false, 0, Some(&long)));
        assert!(title.chars().count() <= "parolsh · ".chars().count() + MAX_TITLE);
        assert!(title.ends_with('…'), "{title}");

        let sneaky = text(0, "~", &activity(false, 0, Some("a\x07\x1b]2;b")));
        assert_eq!(sneaky, "parolsh · a]2;b");
    }
}
