//! `parolsh mcp`: the history of sessions as an MCP server, for the agent.
//! Parolsh gives it to the agent in every conversation that is saved (not
//! `#new private`), as a program the agent starts and talks to over stdio
//! (newline-delimited JSON-RPC). It reads the database only, and only the
//! sessions of one project: the tools have no way to ask for another.

use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;

use crate::history::History;
use crate::version;

/// The server's name, as the agent shows its tools (`parolsh-history`).
pub const NAME: &str = "parolsh-history";

/// Longest entry text `get_session` returns.
const MAX_ENTRY: usize = 4000;

/// What the tools may return: the sessions of one project, and its plain
/// `!command` lines only when you share them (`commands = "shared"`).
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub project: &'a str,
    pub commands: bool,
}

/// The MCP server Parolsh gives the agent: this program, reading `db` within
/// `scope`. `None` when Parolsh cannot tell where its own program is.
pub fn server(db: &Path, scope: Scope) -> Option<McpServer> {
    let program = std::env::current_exe().ok()?;
    let mut args = vec![
        "mcp".to_string(),
        "--db".to_string(),
        db.display().to_string(),
        "--project".to_string(),
        scope.project.to_string(),
    ];
    if scope.commands {
        args.push("--commands".to_string());
    }
    Some(McpServer::Stdio(
        McpServerStdio::new(NAME, program).args(args),
    ))
}

/// Serves the history in `db`, within `scope`, on stdin and stdout until
/// stdin closes.
pub fn serve(db: &Path, scope: Scope) -> anyhow::Result<()> {
    let history = History::open_read_only(db)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&history, scope, &message),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// The reply to one message; `None` for notifications.
fn handle(history: &History, scope: Scope, message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => json!({
            // The client's version: these tools need nothing newer.
            "protocolVersion": params
                .get("protocolVersion")
                .cloned()
                .unwrap_or(json!("2025-06-18")),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": NAME, "version": version::short()},
            "instructions": "Earlier Parolsh sessions of this project: what the user asked, \
                what you answered, the tools you ran, the outputs the user shared. Search \
                it when the user refers to earlier work, or before redoing something.",
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            match call(history, scope, name, &arguments) {
                Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                Err(e) => json!({
                    "content": [{"type": "text", "text": e.to_string()}],
                    "isError": true,
                }),
            }
        }
        _ => return Some(error(id, -32601, &format!("unknown method `{method}`"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tools() -> Value {
    json!([
        {
            "name": "search_history",
            "description": "Search the earlier sessions of this project (messages, answers, \
                tool calls, shared outputs). Words match any form that starts the same with \
                `word*`; combine with OR, quote phrases. Returns the best matches with the \
                session and position to read more with get_session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words to find, e.g. `retry OR backoff`"},
                    "limit": {"type": "integer", "description": "At most this many matches (default 20)"},
                },
                "required": ["query"],
            },
        },
        {
            "name": "list_sessions",
            "description": "The sessions of this project, newest first: number, date, \
                agent, how many entries, and title.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "description": "At most this many (default 20)"},
                },
            },
        },
        {
            "name": "get_session",
            "description": "The entries of one session, in order, a page at a time: who, \
                what kind, and the full text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": {"type": "integer", "description": "The session number"},
                    "from": {"type": "integer", "description": "First entry, from 0 (default 0)"},
                    "limit": {"type": "integer", "description": "At most this many entries (default 30)"},
                },
                "required": ["session"],
            },
        },
        {
            "name": "commands",
            "description": "Shell commands the user ran in this project, newest first, with \
                their exit code: the ones whose output was shared with you, and the others \
                only when the user chose to share them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "contains": {"type": "string", "description": "Only commands containing this text"},
                    "limit": {"type": "integer", "description": "At most this many (default 30)"},
                },
            },
        },
    ])
}

/// Runs tool `name`, as text for the agent.
fn call(history: &History, scope: Scope, name: &str, arguments: &Value) -> anyhow::Result<String> {
    let project = scope.project;
    let number = |key: &str, default: usize| {
        arguments
            .get(key)
            .and_then(Value::as_u64)
            .map_or(default, |n| n as usize)
    };
    let text = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("");
    let mut out = String::new();
    match name {
        "search_history" => {
            let hits =
                history.search(project, text("query"), number("limit", 20), scope.commands)?;
            if hits.is_empty() {
                out.push_str("No matches.");
            }
            for hit in hits {
                out.push_str(&format!(
                    "session {} · entry {} · {} · {} {}\n  {}\n",
                    hit.session, hit.position, hit.at, hit.actor, hit.kind, hit.snippet
                ));
            }
        }
        "list_sessions" => {
            let sessions = history.sessions(project)?;
            if sessions.is_empty() {
                out.push_str("No sessions saved in this project yet.");
            }
            for session in sessions.into_iter().take(number("limit", 20)) {
                out.push_str(&format!(
                    "session {} · {} · {} · {} entries · {}\n",
                    session.id,
                    session.started,
                    session.agent.as_deref().unwrap_or("-"),
                    session.entries,
                    session.title.as_deref().unwrap_or("").replace('\n', " "),
                ));
            }
        }
        "get_session" => {
            let session = arguments
                .get("session")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow::anyhow!("`session` is required"))?;
            let entries = history
                .entries(project, session)?
                .ok_or_else(|| anyhow::anyhow!("no session {session} in this project"))?;
            let from = number("from", 0);
            let limit = number("limit", 30);
            // A plain `!command` keeps its position, and is left out unless
            // shared.
            let shown = |entry: &&crate::history::Stored| scope.commands || entry.kind != "command";
            for (position, entry) in entries
                .iter()
                .enumerate()
                .skip(from)
                .take(limit)
                .filter(|(_, entry)| shown(entry))
            {
                let mut text: String = entry.text.chars().take(MAX_ENTRY).collect();
                if entry.text.chars().count() > MAX_ENTRY {
                    text.push_str(" […cut]");
                }
                out.push_str(&format!(
                    "[{position}] {} {}{}: {text}\n",
                    entry.actor,
                    entry.kind,
                    if entry.meta.as_object().is_some_and(|meta| !meta.is_empty()) {
                        format!(" {}", entry.meta)
                    } else {
                        String::new()
                    },
                ));
            }
            // Only what may be shown counts: a hidden command is not hinted.
            let more = entries.iter().skip(from + limit).filter(shown).count();
            if more > 0 {
                out.push_str(&format!("({more} more: from = {})\n", from + limit));
            }
        }
        "commands" => {
            let commands = history.commands(
                project,
                text("contains"),
                number("limit", 30),
                scope.commands,
            )?;
            if commands.is_empty() {
                out.push_str("No commands saved.");
            }
            for command in commands {
                let prefix = if command.kind == "capture" { "!+" } else { "!" };
                let exit = command
                    .meta
                    .get("exit")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                out.push_str(&format!(
                    "session {} · {} · {prefix}{} · exit {exit}\n",
                    command.session, command.at, command.text
                ));
            }
        }
        _ => anyhow::bail!("unknown tool `{name}`"),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::SessionStart;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.db");
        let mut history = History::open(&db, 90).unwrap();
        for project in ["/p", "/other"] {
            history.begin(SessionStart {
                project: project.to_string(),
                agent: Some("claude".to_string()),
                cwd: project.to_string(),
            });
            history
                .add(
                    "user",
                    "message",
                    &format!("why retry in {project}?"),
                    &json!({"agent": "claude"}),
                )
                .unwrap();
            history
                .add("agent", "answer", "No backoff between tries.", &json!({}))
                .unwrap();
            history
                .add("user", "capture", "cargo test", &json!({"exit": 101}))
                .unwrap();
            // A plain `!command`: yours, unless you share them.
            history
                .add("user", "command", "make retry-deploy", &json!({"exit": 2}))
                .unwrap();
        }
        (dir, db)
    }

    /// The project's sessions, without the plain `!commands`: the default.
    const PRIVATE: Scope = Scope {
        project: "/p",
        commands: false,
    };
    const SHARED: Scope = Scope {
        project: "/p",
        commands: true,
    };

    fn request(history: &History, method: &str, params: Value) -> Value {
        request_within(history, PRIVATE, method, params)
    }

    fn request_within(history: &History, scope: Scope, method: &str, params: Value) -> Value {
        handle(
            history,
            scope,
            &json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}),
        )
        .unwrap()
    }

    fn tool(history: &History, name: &str, arguments: Value) -> String {
        tool_within(history, PRIVATE, name, arguments)
    }

    fn tool_within(history: &History, scope: Scope, name: &str, arguments: Value) -> String {
        let reply = request_within(
            history,
            scope,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        );
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn initialize_lists_the_tools_and_answers_notifications_with_nothing() {
        let (_dir, db) = fixture();
        let history = History::open_read_only(&db).unwrap();

        let init = request(
            &history,
            "initialize",
            json!({"protocolVersion": "2025-03-26"}),
        );
        assert_eq!(init["id"], 7);
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], NAME);
        let tools = request(&history, "tools/list", json!({}));
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["search_history", "list_sessions", "get_session", "commands"]
        );
        let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(handle(&history, PRIVATE, &notification).is_none());
    }

    #[test]
    fn the_tools_see_this_project_only() {
        let (_dir, db) = fixture();
        let history = History::open_read_only(&db).unwrap();

        let found = tool(&history, "search_history", json!({"query": "retry"}));
        assert!(found.contains("session 1 · entry 0 ·"), "{found}");
        assert!(found.contains("why [retry] in /p?"), "{found}");
        assert!(!found.contains("/other"), "{found}");

        let sessions = tool(&history, "list_sessions", json!({}));
        assert_eq!(sessions.lines().count(), 1, "{sessions}");
        let other = request(
            &history,
            "tools/call",
            json!({"name": "get_session", "arguments": {"session": 2}}),
        );
        assert_eq!(other["result"]["isError"], true);
    }

    #[test]
    fn get_session_pages_the_entries_with_their_details() {
        let (_dir, db) = fixture();
        let history = History::open_read_only(&db).unwrap();

        let page = tool(&history, "get_session", json!({"session": 1, "limit": 2}));
        assert_eq!(
            page,
            "[0] user message {\"agent\":\"claude\"}: why retry in /p?\n\
             [1] agent answer: No backoff between tries.\n\
             (1 more: from = 2)\n"
        );
        let rest = tool(&history, "get_session", json!({"session": 1, "from": 2}));
        assert_eq!(rest, "[2] user capture {\"exit\":101}: cargo test\n");
    }

    #[test]
    fn commands_show_the_exit_code() {
        let (_dir, db) = fixture();
        let history = History::open_read_only(&db).unwrap();

        let commands = tool(&history, "commands", json!({"contains": "cargo"}));
        assert!(
            commands.ends_with("!+cargo test · exit 101\n"),
            "{commands}"
        );
        assert_eq!(commands.lines().count(), 1);
    }

    /// Your plain `!commands` are saved for you, not for the agent: its tools
    /// leave them out, unless you share them.
    #[test]
    fn plain_commands_are_returned_only_when_shared() {
        let (_dir, db) = fixture();
        let history = History::open_read_only(&db).unwrap();
        let search = json!({"query": "retry"});
        let session = json!({"session": 1});

        let private = [
            tool(&history, "search_history", search.clone()),
            tool(&history, "get_session", session.clone()),
            tool(&history, "commands", json!({})),
        ];
        for text in &private {
            assert!(!text.contains("retry-deploy"), "{text}");
        }
        // The `!+` one, sent to the agent when it ran, is still there.
        assert!(
            private[2].contains("!+cargo test · exit 101"),
            "{}",
            private[2]
        );

        let shared = [
            tool_within(&history, SHARED, "search_history", search),
            tool_within(&history, SHARED, "get_session", session),
            tool_within(&history, SHARED, "commands", json!({})),
        ];
        assert!(shared[0].contains("make [retry]-deploy"), "{}", shared[0]);
        assert!(
            shared[1].contains("[3] user command {\"exit\":2}: make retry-deploy"),
            "{}",
            shared[1]
        );
        assert!(
            shared[2].contains("!make retry-deploy · exit 2"),
            "{}",
            shared[2]
        );
    }

    #[test]
    fn the_database_is_not_written() {
        let (_dir, db) = fixture();
        let mut history = History::open_read_only(&db).unwrap();
        history.begin(SessionStart {
            project: "/p".to_string(),
            agent: None,
            cwd: "/p".to_string(),
        });

        assert!(history.add("user", "message", "x", &json!({})).is_err());
    }
}
