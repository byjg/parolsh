---
sidebar_position: 6
---

# Security model

Parolsh is a frontend: it runs your shell commands and talks to an agent. It
does not decide what the agent may do. This page says what Parolsh protects,
and what it leaves to the agent.

## Parolsh is not a sandbox

The agent runs as a normal process of your user, with your files, your
network and the environment Parolsh has. Parolsh offers it no tools of its own
(no file access, no terminal through ACP): the agent reads, writes and runs
commands with its own tools, under its own rules.

Whatever limits the agent is the agent's: Claude's permission modes, Codex's
sandbox (`read-only`, `agent`, `agent-full-access`), and so on. Pick the
agent's [mode](agents.md#modes) for the trust you want, and check what that
mode allows in the agent's own documentation.

## Permission prompts come from the agent

When the agent asks before acting, Parolsh shows its request and its options
(see [During a turn](agents.md#during-a-turn)). The agent decides when to ask:
Parolsh does not see what the agent does without asking, and cannot stop it.
In a mode that skips the prompts (`bypassPermissions`, `agent-full-access`),
nothing is asked.

At the prompt, anything other than one of the numbers rejects once.

## `!command` runs as typed

`!command`, `!+command`, `!bash` and plain lines in shell mode go to your
shell exactly as you typed them, with no check and no confirmation, the same
as typing them in bash.

## What reaches the agent

Only what you send:

- the text you type for the agent;
- the output of each `!+command` since your last message: its stdout and
  stderr, the last 16 KB at most, without terminal escape sequences, with the
  command, its directory and its exit code;
- the current directory, when a conversation starts.

The output of `!command` and of `!bash` is never sent. The agent can still
read your files and run commands with its own tools, see
[above](#parolsh-is-not-a-sandbox).

## The agent's environment

The agent inherits Parolsh's environment, which includes the API keys and
other secrets your shell exports. With `shell_env` (see
[Shell environment](configuration.md#shell-environment)), Parolsh imports what
`~/.profile` and `~/.bashrc` set up even when it was not started from a shell.
On top of that, the agent gets its `env` from the global configuration.

Keep secrets the agent does not need out of the environment you start
Parolsh from.

## Project configuration

A `.parolsh/config.toml` comes with the directory, often from a repository
you cloned. It can change how Parolsh looks and pick one of the agents you
configured, but not what is executed: `shell` and the agents' `command`,
`args` and `env` are only read from your global configuration. See
[What a project can change](configuration.md#what-a-project-can-change).

## Looking back

`#audit` lists what ran on your machine, what reached the agent and what the
agent reported doing, for the current run (see
[`#audit`](input-routing.md#audit)).

The [history of sessions](input-routing.md#the-history-of-sessions) keeps more,
on disk: your messages, the outputs you shared, the agent's answers, reasoning
and tool calls, and your answers to its questions, for 90 days by default. It
is `~/.local/state/parolsh/history.db`, readable only by you.

- `!command` lines are not in it unless you set `save_commands = true`, which
  the banner then says.
- `#forget` removes a session, `#new private` keeps a conversation out of it,
  and `history_days = 0` saves nothing.
- A project file cannot change these settings.

The agent gets this history as an MCP server (see
[The agent can search the history](input-routing.md#the-agent-can-search-the-history)),
so **what was saved can reach the agent again**: any agent you use in this
project, of any provider, can read what its searches return from earlier
sessions, including what was first sent to another agent, and the `!command`
lines when `save_commands` is on. It cannot see other projects' sessions. A
`#new private` conversation does not get the server; `history_days = 0` turns
both the history and the server off.

## The ACP log

`PAROLSH_ACP_LOG` writes the whole conversation to a file, including what you
share with `!+`. Parolsh creates it readable only by you; delete it when you
are done (see
[Logging the agent's messages](troubleshooting.md#logging-the-agents-messages)).
