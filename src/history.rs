//! The history of sessions: what Parolsh saw in each conversation, kept in
//! SQLite (`history.db` in Parolsh's state directory), searchable with FTS5.
//! One database for every project; each session belongs to one project.
//! `#audit`, `#sessions` and `#forget` read it.

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id   INTEGER PRIMARY KEY,
    root TEXT NOT NULL UNIQUE
);
-- AUTOINCREMENT: a forgotten session's number is never given to another.
CREATE TABLE IF NOT EXISTS sessions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    agent            TEXT,
    agent_session_id TEXT,
    cwd              TEXT NOT NULL,
    title            TEXT,
    started_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, updated_at);
CREATE TABLE IF NOT EXISTS entries (
    id         INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    at         INTEGER NOT NULL,
    actor      TEXT NOT NULL,
    kind       TEXT NOT NULL,
    text       TEXT NOT NULL,
    meta       TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS entries_session ON entries(session_id, id);
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    text, content='entries', content_rowid='id'
);
CREATE TRIGGER IF NOT EXISTS entries_insert AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS entries_delete AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS entries_update AFTER UPDATE OF text ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO entries_fts(rowid, text) VALUES (new.id, new.text);
END;
";

/// Where and with what a session runs; it is written with its first entry,
/// so a conversation where nothing happened leaves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStart {
    /// The project root, or the agent's directory without one.
    pub project: String,
    pub agent: Option<String>,
    pub cwd: String,
}

/// A session, as `#sessions` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: i64,
    /// Local time, `YYYY-MM-DD HH:MM`.
    pub started: String,
    pub agent: Option<String>,
    /// The agent's title, or the first message.
    pub title: Option<String>,
    pub entries: usize,
}

/// What `#resume` needs of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumable {
    pub agent: Option<String>,
    /// The agent's id for the conversation.
    pub agent_session_id: Option<String>,
    pub cwd: String,
}

/// An entry read back: milliseconds since its session started, and the
/// fields `Audit` wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub at: Duration,
    pub actor: String,
    pub kind: String,
    pub text: String,
    pub meta: serde_json::Value,
}

pub struct History {
    conn: Connection,
    /// The session being written to, once it has an entry.
    current: Option<i64>,
    /// The session to create with the next entry.
    next: Option<SessionStart>,
    title: Option<String>,
    agent_session_id: Option<String>,
}

impl History {
    /// Opens (or creates, readable by the user only) the database at
    /// `path`, and removes the sessions idle for more than `days` days.
    pub fn open(path: &Path, days: u32) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        create_private(path);
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.execute_batch(SCHEMA)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        let history = Self {
            conn,
            current: None,
            next: None,
            title: None,
            agent_session_id: None,
        };
        history.purge(days)?;
        Ok(history)
    }

    /// Goes on writing to session `id` of `project`: `#resume`. False when
    /// there is none.
    pub fn resume(&mut self, project: &str, id: i64) -> rusqlite::Result<bool> {
        let title: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT s.title FROM sessions s JOIN projects p ON p.id = s.project_id
                 WHERE s.id = ?1 AND p.root = ?2",
                params![id, project],
                |row| row.get(0),
            )
            .optional()?;
        let Some(title) = title else {
            return Ok(false);
        };
        self.current = Some(id);
        self.next = None;
        self.title = title;
        Ok(true)
    }

    /// What `#resume` needs of session `id` of `project`.
    pub fn resumable(&self, project: &str, id: i64) -> rusqlite::Result<Option<Resumable>> {
        self.conn
            .query_row(
                "SELECT s.agent, s.agent_session_id, s.cwd
                 FROM sessions s JOIN projects p ON p.id = s.project_id
                 WHERE s.id = ?1 AND p.root = ?2",
                params![id, project],
                |row| {
                    Ok(Resumable {
                        agent: row.get(0)?,
                        agent_session_id: row.get(1)?,
                        cwd: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// Starts a new session; it is written with its first entry.
    pub fn begin(&mut self, start: SessionStart) {
        self.current = None;
        self.next = Some(start);
        self.title = None;
        self.agent_session_id = None;
    }

    /// Stops writing until the next `begin`: `#new private`.
    pub fn pause(&mut self) {
        self.current = None;
        self.next = None;
    }

    /// The agent's title for the conversation.
    pub fn set_title(&mut self, title: Option<String>) {
        if self.title == title {
            return;
        }
        self.title = title;
        if let Some(id) = self.current {
            let _ = self.conn.execute(
                "UPDATE sessions SET title = ?1 WHERE id = ?2",
                params![self.title, id],
            );
        }
    }

    /// The agent's id for the conversation, for resuming it later.
    pub fn set_agent_session_id(&mut self, id: Option<String>) {
        if self.agent_session_id == id {
            return;
        }
        self.agent_session_id = id;
        if let Some(session) = self.current {
            let _ = self.conn.execute(
                "UPDATE sessions SET agent_session_id = ?1 WHERE id = ?2",
                params![self.agent_session_id, session],
            );
        }
    }

    /// The session being written to, if it has started.
    pub fn current(&self) -> Option<i64> {
        self.current
    }

    /// Adds an entry to the current session, creating the session first if
    /// needed. Its id, for `update`; `None` while paused.
    pub fn add(
        &mut self,
        actor: &str,
        kind: &str,
        text: &str,
        meta: &serde_json::Value,
    ) -> rusqlite::Result<Option<i64>> {
        let Some(session) = self.session()? else {
            return Ok(None);
        };
        let now = now();
        self.conn.execute(
            "INSERT INTO entries (session_id, at, actor, kind, text, meta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session, now, actor, kind, text, meta.to_string()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![now, session],
        )?;
        Ok(Some(id))
    }

    /// Changes an entry: a tool call that got a new title or finished.
    pub fn update(
        &mut self,
        id: i64,
        text: &str,
        meta: &serde_json::Value,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE entries SET text = ?1, meta = ?2 WHERE id = ?3",
            params![text, meta.to_string(), id],
        )?;
        Ok(())
    }

    /// The sessions of `project`, newest first.
    pub fn sessions(&self, project: &str) -> rusqlite::Result<Vec<Session>> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, strftime('%Y-%m-%d %H:%M', s.started_at / 1000, 'unixepoch', 'localtime'),
                    s.agent,
                    coalesce(s.title, (SELECT text FROM entries
                                       WHERE session_id = s.id AND kind = 'message'
                                       ORDER BY id LIMIT 1)),
                    (SELECT count(*) FROM entries WHERE session_id = s.id)
             FROM sessions s JOIN projects p ON p.id = s.project_id
             WHERE p.root = ?1
             ORDER BY s.updated_at DESC, s.id DESC",
        )?;
        let rows = statement.query_map([project], |row| {
            Ok(Session {
                id: row.get(0)?,
                started: row.get(1)?,
                agent: row.get(2)?,
                title: row.get(3)?,
                entries: row.get::<_, i64>(4)? as usize,
            })
        })?;
        rows.collect()
    }

    /// The entries of session `id` of `project`, in order; `None` when
    /// `project` has no such session.
    pub fn entries(&self, project: &str, id: i64) -> rusqlite::Result<Option<Vec<Stored>>> {
        let Some(started) = self.started(project, id)? else {
            return Ok(None);
        };
        let mut statement = self.conn.prepare(
            "SELECT at, actor, kind, text, meta FROM entries WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([id], |row| {
            let at: i64 = row.get(0)?;
            let meta: String = row.get(4)?;
            Ok(Stored {
                at: Duration::from_millis(at.saturating_sub(started).max(0) as u64),
                actor: row.get(1)?,
                kind: row.get(2)?,
                text: row.get(3)?,
                meta: serde_json::from_str(&meta).unwrap_or_default(),
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map(Some)
    }

    /// Removes session `id` of `project`. False when there is none.
    pub fn forget(&mut self, project: &str, id: i64) -> rusqlite::Result<bool> {
        if self.started(project, id)?.is_none() {
            return Ok(false);
        }
        self.conn
            .execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        if self.current == Some(id) {
            // Later entries start a new session of the same conversation.
            self.current = None;
        }
        Ok(true)
    }

    /// When session `id` of `project` started, in milliseconds.
    fn started(&self, project: &str, id: i64) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT s.started_at FROM sessions s JOIN projects p ON p.id = s.project_id
                 WHERE s.id = ?1 AND p.root = ?2",
                params![id, project],
                |row| row.get(0),
            )
            .optional()
    }

    /// The current session, created from `next` on first use.
    fn session(&mut self) -> rusqlite::Result<Option<i64>> {
        if let Some(id) = self.current {
            return Ok(Some(id));
        }
        let Some(start) = &self.next else {
            return Ok(None);
        };
        self.conn.execute(
            "INSERT INTO projects (root) VALUES (?1) ON CONFLICT(root) DO NOTHING",
            [&start.project],
        )?;
        let project: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE root = ?1",
            [&start.project],
            |row| row.get(0),
        )?;
        let now = now();
        self.conn.execute(
            "INSERT INTO sessions (project_id, agent, agent_session_id, cwd, title, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                project,
                start.agent,
                self.agent_session_id,
                start.cwd,
                self.title,
                now
            ],
        )?;
        self.current = Some(self.conn.last_insert_rowid());
        Ok(self.current)
    }

    /// Removes the sessions idle for more than `days` days.
    fn purge(&self, days: u32) -> rusqlite::Result<()> {
        let oldest = now() - i64::from(days) * 24 * 60 * 60 * 1000;
        self.conn
            .execute("DELETE FROM sessions WHERE updated_at < ?1", [oldest])?;
        self.conn.execute(
            "DELETE FROM projects WHERE id NOT IN (SELECT project_id FROM sessions)",
            [],
        )?;
        Ok(())
    }
}

/// Creates `path` readable by the user only, if it does not exist: the
/// history holds what was typed and answered. SQLite gives its journal files
/// the same permissions.
fn create_private(path: &Path) {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path);
}

/// Milliseconds since the Unix epoch.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn open(dir: &Path) -> History {
        History::open(&dir.join("history.db"), 90).unwrap()
    }

    fn start(project: &str) -> SessionStart {
        SessionStart {
            project: project.to_string(),
            agent: Some("claude".to_string()),
            cwd: project.to_string(),
        }
    }

    fn texts(history: &History, project: &str, id: i64) -> Vec<String> {
        history
            .entries(project, id)
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|entry| format!("{} {} {}", entry.actor, entry.kind, entry.text))
            .collect()
    }

    #[test]
    fn the_database_is_readable_by_the_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        open(dir.path());

        let mode = std::fs::metadata(dir.path().join("history.db"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_session_is_written_with_its_first_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history.set_title(Some("Fix the batch".to_string()));
        assert!(history.sessions("/p").unwrap().is_empty());

        history.add("user", "message", "hello", &json!({})).unwrap();
        history
            .add("agent", "answer", "hi there", &json!({}))
            .unwrap();

        let sessions = history.sessions("/p").unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Fix the batch"));
        assert_eq!(sessions[0].entries, 2);
        assert_eq!(
            texts(&history, "/p", sessions[0].id),
            ["user message hello", "agent answer hi there"]
        );
    }

    #[test]
    fn without_a_title_a_session_is_named_by_its_first_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history
            .add("parolsh", "event", "turn ended", &json!({}))
            .unwrap();
        history
            .add("user", "message", "why retry?", &json!({}))
            .unwrap();

        assert_eq!(
            history.sessions("/p").unwrap()[0].title.as_deref(),
            Some("why retry?")
        );
    }

    #[test]
    fn sessions_belong_to_their_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/a"));
        history.add("user", "message", "in a", &json!({})).unwrap();
        let a = history.current().unwrap();
        history.begin(start("/b"));
        history.add("user", "message", "in b", &json!({})).unwrap();

        assert_eq!(history.sessions("/a").unwrap().len(), 1);
        assert_eq!(history.sessions("/b").unwrap().len(), 1);
        assert!(history.entries("/b", a).unwrap().is_none());
        assert!(!history.forget("/b", a).unwrap());
    }

    #[test]
    fn paused_sessions_are_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history.pause();

        assert_eq!(
            history
                .add("user", "message", "secret", &json!({}))
                .unwrap(),
            None
        );
        assert!(history.sessions("/p").unwrap().is_empty());
    }

    #[test]
    fn an_entry_can_be_updated_and_the_search_follows() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        let id = history
            .add("agent", "tool", "Read a", &json!({}))
            .unwrap()
            .unwrap();
        history
            .update(id, "Read client.rs", &json!({"finished": true}))
            .unwrap();
        let session = history.current().unwrap();

        assert_eq!(
            texts(&history, "/p", session),
            ["agent tool Read client.rs"]
        );
        assert_eq!(search(&history, "client"), 1);
        assert_eq!(search(&history, "a"), 0);
        let entry = &history.entries("/p", session).unwrap().unwrap()[0];
        assert_eq!(entry.meta, json!({"finished": true}));
    }

    fn search(history: &History, query: &str) -> i64 {
        history
            .conn
            .query_row(
                "SELECT count(*) FROM entries_fts WHERE entries_fts MATCH ?1",
                [query],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn forgetting_a_session_removes_its_entries_from_the_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history
            .add("user", "message", "retry backoff", &json!({}))
            .unwrap();
        let id = history.current().unwrap();

        assert!(history.forget("/p", id).unwrap());
        assert!(history.sessions("/p").unwrap().is_empty());
        assert_eq!(search(&history, "retry"), 0);
        // The conversation goes on in a new session.
        history.add("user", "message", "again", &json!({})).unwrap();
        assert_ne!(history.current(), Some(id));
    }

    #[test]
    fn sessions_idle_too_long_are_removed_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history.add("user", "message", "old", &json!({})).unwrap();
        let old = now() - 91 * 24 * 60 * 60 * 1000;
        history
            .conn
            .execute("UPDATE sessions SET updated_at = ?1", [old])
            .unwrap();
        drop(history);

        let history = open(dir.path());
        assert!(history.sessions("/p").unwrap().is_empty());
        assert_eq!(search(&history, "old"), 0);
    }

    #[test]
    fn a_resumed_session_gets_the_new_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history.set_agent_session_id(Some("abc".to_string()));
        history.add("user", "message", "first", &json!({})).unwrap();
        let first = history.current().unwrap();
        history.begin(start("/p"));

        assert_eq!(
            history.resumable("/p", first).unwrap(),
            Some(Resumable {
                agent: Some("claude".to_string()),
                agent_session_id: Some("abc".to_string()),
                cwd: "/p".to_string(),
            })
        );
        assert!(history.resume("/p", first).unwrap());
        history
            .add("user", "message", "second", &json!({}))
            .unwrap();
        assert_eq!(history.current(), Some(first));
        assert_eq!(history.sessions("/p").unwrap().len(), 1);
        assert!(!history.resume("/other", first).unwrap());
    }

    #[test]
    fn the_agent_session_id_is_kept_for_resuming() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = open(dir.path());
        history.begin(start("/p"));
        history.set_agent_session_id(Some("abc".to_string()));
        history.add("user", "message", "hi", &json!({})).unwrap();

        let id: Option<String> = history
            .conn
            .query_row("SELECT agent_session_id FROM sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(id.as_deref(), Some("abc"));
    }
}
