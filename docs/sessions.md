---
sidebar_position: 4
---

# Sessions and history

What happened, now and in earlier sessions: `#audit` lists this run, and
the history keeps each conversation so you can find it, resume it, or let
the agent search it. Typing at the prompt is in
[Input routing](input-routing.md); the agent itself is in
[Working with agents](working-with-agents.md).

## `#audit`

`#audit` lists what happened since Parolsh started, oldest first: what ran on
your machine, what reached the agent, what the agent did, and what you
answered.

```text
[claude] ~/projects/wallet ✦ #audit
   0:12  USER     !kubectl get pods · exit 0
   0:20  USER     !+kubectl logs api · exit 0 · 12 KB kept
   0:31  SHARED   1 output(s), 12 KB → claude
   0:31  USER     → claude: why is the api crashing?
   0:44  AGENT    execute: kubectl describe pod api · completed
   0:52  AGENT    asks: Writing to src/foo.rs
   0:55  USER     → Allow once
   0:58  AGENT    edit: Write src/foo.rs [src/foo.rs] · completed
   1:03  PAROLSH  turn ended · 32s
```

| Who | What |
|---|---|
| `USER` | `!command`, `!+command` and `!bash` with their exit code, your messages (their first line) with the agent they went to, `#` commands (all but `#help`, `#exit`, `#audit`, `#sessions`, `#redraw`, `#jobs`, `#fg`, `#copy` and `#raw`), and your answers to the agent's questions |
| `SHARED` | `!+` output sent with a message, and its size |
| `AGENT` | Tool calls, with their kind, the files the agent names and how they ended; permission requests and questions |
| `PAROLSH` | How each turn ended, a cancel the agent did not confirm, an agent that stopped |

It keeps titles and commands only, the first line of each and at most 200
characters: outputs and answers stay in the scrollback. It is in memory, and
gone when you leave Parolsh; for a full record, see
[Logging the agent's messages](troubleshooting.md#logging-the-agents-messages).
It keeps the last 1000 entries and says how many older ones were dropped;
`audit_entries` in the [configuration](configuration.md#keys) changes that,
and `0` turns it off.
What the agent does without telling (a tool it does not report) is not
there: see [Security model](security.md).

## The history of sessions

Parolsh also saves each conversation in a history, so you can look back at
it after you leave: `history.db` (SQLite) in `$XDG_STATE_HOME/parolsh/`
(`~/.local/state/parolsh/`), readable only by you: one database shared by all
projects.

```text
[claude] ~/projects/wallet ✦ #sessions
*   14  2026-10-02 15:27  claude     23 entries  412k tok    $1.84  Fix the crashing api
    11  2026-09-30 10:02  claude     41 entries  1.2M tok    $4.31  Add retries to the client
[claude] ~/projects/wallet ✦ #audit 11
   0:00  USER     → claude: the client gives up too early
   0:09  AGENT    read: Read src/client.rs [src/client.rs] · completed
   0:15  AGENT    answer: It retries once, with no backoff. …
   ...
```

- A session is one conversation: it starts with Parolsh, `#new`, `#cd` or
  `#agent`, and is saved from its first entry. `#sessions` names it with the
  agent's title (Claude and Codex give one), or your first message.
- `#sessions` also says what each session used, when the agent reports it
  (Claude, Codex, Kilo Code and OpenCode do, Qwen Code does not):
  the tokens of all its turns, and the cost for the agents that give one
  (see [The context and what it costs](working-with-agents.md#the-context-and-what-it-costs)).
  The total counts what was sent, received, reasoned, and read from and
  written to the agent's cache; most of it is cache reads, which cost far less. The four
  are kept apart with each turn, with the model that answered: see
  [Usage in the database](#usage-in-the-database).
- It keeps what `#audit` shows, with the full text instead of a short line,
  and also the agent's answers and reasoning, and the outputs shared with
  `!+`. `#audit <n>` shows answers as one line each; reasoning is not shown.
- **Your `!command` lines are saved for you, not for the agent.** The line and
  its exit code, never its output, typed with `!` or in shell mode: `#redraw`,
  a resumed session and `#audit <n>` show them (`!cargo test · exit 101`), and
  the tools the agent searches the history with leave them out.
  `commands = "shared"` gives them to the agent too (a notice after the
  banner says so), and `commands = "off"` does not save them. A `!+` command is always
  saved and returned to the agent: its output was sent to it.
- Sessions belong to the project (its root, or the agent's directory without
  one): `#sessions` lists this project's, and `#forget` only removes one of
  them. Their numbers are the same everywhere, never given twice: see
  [Finding a session](#finding-a-session) for the other projects'.
- A session idle for more than 90 days is removed when Parolsh starts;
  `history_days` changes that, and `0` saves nothing. `#forget` removes one
  now, and `#new private` starts a conversation that is not saved.

`history_days` and `commands` can only be set in the global
[configuration](configuration.md#keys).

### Finding a session

`#sessions` takes the text to look for, and `all` for every project:

| | |
|---|---|
| `#sessions` | This project's sessions |
| `#sessions <text>` | Those where the text was said, or that it titles |
| `#sessions all` | The sessions of every project |
| `#sessions all <text>` | The same search over every project |

```text
[claude] ~/projects/wallet ✦ #sessions retry
    11  2026-09-30 10:02  claude     41 entries  1.2M tok    $4.31  Add retries to the client
        agent: It [retries] once, with no backoff. …
[claude] ~/projects/wallet ✦ #sessions all falcon
    27  2026-10-02 09:14  codex      18 entries                     billing  Billing rewrite
        you: the rewrite is called [falcon]
    11  2026-09-30 10:02  claude     41 entries  1.2M tok    $4.31  wallet   Add retries to the client
        command: cargo test [falcon]
```

- Under each session, the line the text is in, and whose it is: yours, the
  agent's, or one of your `!command` lines, which are searched too. This
  search is yours, at the prompt: the agent's own
  [search](#the-agent-can-search-the-history) stays in this project and
  leaves your commands out. `#share <n>` gives it a session of another one.
- It matches words, not meanings, and all of them: `retry client` finds the
  entries that have both.
- Newest first, 20 at most; a line says how many more there are.
- With `all`, each session's project, by the name of its directory.
  `#resume` says the whole path before going there.
- `#audit <n>` shows a session of any project: it only looks.
- Parolsh reads its history itself: no agent is asked, and it works without
  one.

### Resuming a session

`#resume <n>` goes back to session `n`: Parolsh moves to its directory and
agent if needed, and asks the agent to reopen that conversation (ACP
`session/resume`, or `session/load` when that is all the agent offers; the
conversation it replays is not printed again). The agent then remembers
everything that was said, and the session goes on in the history.

```text
[claude] ~/projects/wallet ✦ #resume 11
…its last exchanges…
Resumed session 11: the agent remembers that conversation.
[claude] ~/projects/wallet ✦ and the timeout, did we change it?
```

Parolsh first shows the session's last exchanges (`redraw_exchanges`, 5 by
default), so you see where it was. From the command line,
`parolsh --resume 11` starts in that session, and `parolsh --continue` in the
last one of the project, the same way.

`#resume <text>` goes back to the one session of this project that has the
text, as `#sessions <text>` finds it; when several have it, it lists them.
`#resume all <text>` looks in every project.

**A session of another project** is said, and asked about, before anything
moves: going there is what `#cd` does (that project's configuration, maybe
another agent), and it ends the conversation you are in.

```text
[claude] ~/projects/wallet ✦ #resume 27
Session 27: "Billing rewrite", ~/projects/billing, 2026-10-02 09:14.
Go there? [y/N] y
Agent changed to codex. Started a new conversation.
…its last exchanges…
Resumed session 27: the agent remembers that conversation.
[codex] ~/projects/billing ✦
```

The `Agent changed` line only shows when the session was with another agent.
Anything but `y` or `yes` stays where you are (`Staying here.`), and what you
typed before the question was shown does not answer it. `parolsh --resume 27` from another
directory asks the same.

A number is easily mistyped, and the one next to it is a session too:

- `Tab` after `#resume`, `#audit` or `#forget` lists this project's sessions
  with their date, agent and title, to pick one instead.
- `#forget` says which session it is and asks before removing it: that one
  cannot be undone.
- A session whose directory no longer exists, or whose agent is not
  configured any more, is not resumed, and Parolsh says which; `#audit <n>`
  still shows it.

The agent keeps conversations itself (Claude in `~/.claude/projects/`): if it
removed one, it cannot be resumed, and `#audit <n>` still shows what Parolsh
saved:

```text
parolsh: cannot resume session 11: the agent no longer has that conversation. #audit 11 shows what Parolsh saved of it
```

A session where you only ran shell commands has no conversation for the agent
to go back to: an agent keeps one from its first message. `#sessions` names
it by its first command, and `#resume` shows its commands and goes on with it,
the agent starting a new conversation:

```text
[claude] ~/projects/wallet ✦ #sessions
    7  2026-10-03 15:38  claude      2 entries  (commands only) !source .env
[claude] ~/projects/wallet ✦ #resume 7
!source .env · exit 0
!cd infra/ · exit 0
Continued session 7: it has only shell commands, so the agent starts a new conversation.
```

What the commands had set up is not there again: the directory your commands
were in, and what they exported, are not saved.

### A conversation the agent had elsewhere

Parolsh's history only has what happened in Parolsh. A conversation you had
with the agent in its own terminal, or in an editor, is the agent's:
`#sessions agent` lists what the running agent has for this directory, and
`#resume agent <text>` goes back to one.

```text
[claude] ~/projects/wallet ✦ #sessions agent
     ·  2026-10-11 20:12  3062a1f0  Background task handling
     ·  2026-10-07 13:47  9b41c2de  Background turns summary
    14  2026-10-02 15:27  ef501917  Fix the crashing api
A dot: not in Parolsh's history. #resume agent <title or id> opens one.
[claude] ~/projects/wallet ✦ #resume agent task handling
…its last exchanges…
Resumed "Background task handling": the agent remembers that conversation.
It is session 33 from now on, with what the agent replayed of it.
```

- Each line has when the conversation last changed, the start of the agent's
  id for it, and its title. The ones Parolsh has a session for show its
  number; `#resume agent` opens that session.
- `#resume agent <text>` takes part of the title, in any case, or the start
  of the id. When several match, as two conversations with the same title
  do, it lists them: the id tells them apart. After a listing, Tab completes
  to the id.
- **It is recorded when it is opened.** The agent replays the conversation
  (your messages, its answers, its tool calls), and Parolsh saves that as a
  new session: `#redraw`, `#audit <n>`, `#sessions <text>` and `#resume <n>`
  work on it like on any other. Only the conversation you open is taken, when
  you open it.
- A replay has no dates: the session is dated when the agent says the
  conversation last changed, and its usage starts from now.
- The running agent, for this directory. For another agent's, `#agent
  <name>` first. An agent that cannot replay a conversation only reopens it,
  and Parolsh says that it has nothing of what came before; one that does not
  list its conversations says so.

:::warning Not while it is open elsewhere
A conversation that is still open in the agent's own terminal would be
written to by two programs. Parolsh cannot tell: close it there first.
:::

### Showing the conversation again

`#redraw`, or `Ctrl+L`, clears the terminal and prints the last exchanges of
the conversation again from the history, laid out for the terminal's width as
it is now:

```text
[claude] ~/projects/wallet ✦ why is the api container restarting?
• read: Read docker-compose.yml [docker-compose.yml] · completed
✦ The container exits because DATABASE_URL is not set: the entrypoint reads
  it before the compose file's env_file is loaded.

turn ended · 18s
[claude] ~/projects/wallet ✦
```

- Use it after resizing the window: what was already printed is not laid out
  again by itself (see [During a turn](working-with-agents.md#during-a-turn)).
- An exchange is your message and what followed it. `#redraw` shows the last
  5, `#redraw 2` the last 2, and `redraw_exchanges` in the
  [configuration](configuration.md#keys) changes the 5.
- It shows your messages, the answers, and one line for each tool call,
  question, answer of yours and `!+` output (its size, not its text).
- **The whole terminal is cleared, its scrollback too.** The output of your
  earlier `!commands` goes with it: Parolsh does not keep it and cannot print
  it again.
- `Ctrl+L` keeps the line you are typing.
- In a conversation that is not saved (`#new private`, or `history_days = 0`)
  there is nothing to show again: it only clears.

### A turn in the background

A turn sent to the background with `Ctrl+Z` (see
[Turns in the background](working-with-agents.md#turns-in-the-background)) leaves the
session while it runs: its entries, from your message on, are a session of
their own, which `#sessions` lists and `#audit <n>` shows. When the turn ends
they go to the end of the session it left, where it ended, and that session
of its own is gone. If the conversation it left was replaced meanwhile
(`#new`, `#cd`, `#agent`, `#resume`), or you left Parolsh, it stays a session
of its own, which `#resume <n>` can go back to.

### Usage in the database

For a report, what each turn used is in `history.db`, in the `meta` of the
entry that ends it (`kind = 'event'`, `turn ended …`), under `usage`:

| Field | What |
|---|---|
| `input`, `output` | Tokens sent to the model and received from it |
| `thought` | Reasoning tokens, for the agents that count them apart (Kilo Code) |
| `cache_read`, `cache_write` | Tokens read from and written to the agent's cache |
| `models` | The models that answered |
| `cost`, `currency` | What the conversation cost since the turn before, when the agent says it |
| `context`, `context_size` | The tokens in the context after the turn, and its size |

```bash
sqlite3 ~/.local/state/parolsh/history.db "
  SELECT s.id, s.agent, sum(json_extract(e.meta, '\$.usage.input')) AS input,
         sum(json_extract(e.meta, '\$.usage.output')) AS output,
         round(sum(json_extract(e.meta, '\$.usage.cost')), 2) AS cost
  FROM sessions s JOIN entries e ON e.session_id = s.id
  WHERE e.kind = 'event' GROUP BY s.id"
```

A turn the agent starts by itself, after a background task, reports no
tokens: its cost is counted with the next turn you send. A compaction is its
own entry, with `compaction.before` and `compaction.after`.

### The agent can search the history

In every conversation that is saved, the agent gets the history of this
project as an MCP server, `parolsh-history`, so it can look up earlier work
itself when you refer to it, without loading whole sessions:

```text
[claude] ~/projects/wallet ✦ how did we fix the retries last week?
• mcp__parolsh-history__search_history
Last Tuesday we added exponential backoff in src/client.rs …
```

| Tool | What it returns |
|---|---|
| `search_history` | The best matches for some words (SQLite FTS5: `retry OR backoff`, `"a phrase"`, `prefix*`), with the session and entry |
| `list_sessions` | The sessions: number, date, agent, title |
| `get_session` | The entries of one session, a page at a time, with their full text |
| `commands` | The saved shell commands and their exit codes |

Ask in your own words; the agent picks the tool:

| You ask | The agent uses |
|---|---|
| `where did we change the retry logic?` | `search_history` |
| `summarize session #2` | `get_session`, with the number `#sessions` shows |
| `what did we work on last week?` | `list_sessions`, then `get_session` |
| `which commands failed today?` | `commands` (`!+` ones; plain `!command` lines only with `commands = "shared"`) |

```text
[claude] ~/projects/wallet ✦ summarize session #2
• mcp__parolsh-history__get_session
Session #2 was about the billing rewrite, codename Falcon-9 …
```

- Start the line with a word, not with `#`: `#2 summarize` is a Parolsh
  command.
- The search matches words, not meanings. For something exact, give the words:
  `search the history for Falcon-9`.
- If the agent answers without looking, name the tools: `use the
  parolsh-history tools to find …`. Claude, Codex, Kilo Code and OpenCode
  used the tools when asked to search the history; Qwen Code and Goose
  searched the files instead, and used them once they were named. Kilo and
  Qwen ask before each call, like for any tool.

The server is Parolsh itself (`parolsh mcp`), started by the agent. It reads
the history without changing it, and its tools only return this project's
sessions, and those of other projects you shared with the conversation: they
cannot ask for another. A `#new private` conversation does not
get it. To see what the tools return without the agent, see
[Asking the history yourself](troubleshooting.md#asking-the-history-yourself).

### Sharing a session of another project

The agent's tools only return this project's sessions. For one of another
project, the tool says so, and the agent tells you:

```text
[claude] ~/projects/site ✦ can you summarize session 7?
✦ I can't read it: session 7 belongs to another project and hasn't been shared
  here. Type #share 7 at the Parolsh prompt and I'll check it.
[claude] ~/projects/site ✦ #share 7
Session 7: "Inotify limit validation", ~, 2026-10-04 11:30.
The agent of this conversation can now read it with its history tools.
[claude] ~/projects/site ✦ try again
```

- The agent reads it as it reads this project's sessions, a page at a time,
  and its search covers it. Parolsh puts nothing of it in the conversation.
- **For this conversation only**: a new one (`#new`, `#cd`, `#agent`,
  `#resume`) starts without it. The conversation that goes on after a turn
  was [sent to the background](working-with-agents.md#turns-in-the-background) keeps it.
- Your `!command` lines of that session are left out, as this project's are,
  unless `commands = "shared"`.
- `#share` alone lists what is shared. `#sessions all` finds the session's
  number, and `#audit <n>` shows it to you without giving it to the agent.
- Not in a `#new private` conversation, nor with the history off: the agent
  has no history tools there.
