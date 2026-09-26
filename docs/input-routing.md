---
sidebar_position: 3
---

# Input routing

Parolsh never guesses whether a line is English or a shell command. The first
character decides, always:

| Input | Route |
|---|---|
| `text` | The active ACP agent |
| `/command` | The active ACP agent, unchanged |
| `!command` | The configured shell, default `bash -ic` |
| `!+command` | The same, and its output goes with your next message |
| `!bash` | An interactive Bash session |
| `#command` | Parolsh itself |

`find files modified today` is always a question for the agent. To run the
Unix `find`, type `!find . -mtime -1`.

## While you type

On an ANSI terminal the line shows where it will go before you press Enter:

| Line | Color |
|---|---|
| `!command`, `!bash` | yellow |
| `!+command` | green |
| `#command` | magenta |
| `/command` | blue |
| text for the agent | the terminal's color |

The marker (`!`, `!+`, `#`, `/`) is bold. While the line is only a marker, a
dim hint says what it does, for example `!+  run a shell command and send its
output with your next message`. The hint is a tip: the right arrow does not
insert it into the line.

## `!command`

The command runs as `bash -ic "<command>"` in Parolsh's current directory.

- **Your `~/.bashrc` is loaded**, so aliases, functions and shell options work
  as in your normal terminal. The shell is interactive (`-i`) because a plain
  `bash -c` ignores aliases, and the stock Debian/Ubuntu `~/.bashrc` stops
  right away when the shell is not interactive.
- **Each command is independent.** `!cd /tmp` or `!export FOO=1` change
  nothing in Parolsh: the command runs and returns. Use `#cd` to change
  directory, or `!bash` for a session that keeps its state.
- **Commands stay out of your bash history.** Parolsh sets `HISTFILE=/dev/null`
  for them and keeps its own history.
- A non-zero exit code is printed after the output, for example `exit 1`.
- `Ctrl+C` stops the running command, not Parolsh.
- Parolsh has no job control. With the default `bash -ic`, `Ctrl+Z` is handled
  by that bash, as if you had typed the line in bash: the running program is
  paused (`Stopped`), the rest of the line continues, and the paused program
  ends when the command finishes. With a non-interactive `shell` such as
  `["bash", "-c"]`, Parolsh resumes the command right away. Use `!bash` when
  you need job control.

Loading `~/.bashrc` costs a little time on every command (about 0.17 s on a
typical Ubuntu setup). Anything `~/.bashrc` or `/etc/bash.bashrc` prints also
shows up in the output.

The shell is configurable, see [Configuration](configuration.md):

```toml
shell = ["zsh", "-ic"]
```

## `!+command`

Runs the command like `!command`, shows its output, and keeps it for the
agent: it is sent with your next message, then forgotten.

```text
wallet ❯ !+docker ps
CONTAINER ID   IMAGE        STATUS
...
(output of `docker ps` goes with your next message)
wallet ❯ which of these containers looks unhealthy?
```

The agent answers from that output instead of running the command again.

- Only commands you mark with `!+` are shared: a plain `!command` never
  reaches the agent.
- Several `!+` commands before one message are all sent, in order, each with
  its command, directory and exit code.
- At most the last 16 KB of each output is sent.
- The command's output goes through Parolsh, so programs see a pipe instead
  of a terminal: they print plain text, without colors or pagers. Full-screen
  programs (`vim`, `top`) belong to a plain `!`. Its input is still the
  terminal, so a `sudo` password prompt works.
- `!+` alone shows how to use it.

## `!bash`

Opens a real interactive Bash in the current directory, with your full
`~/.bashrc` and completions. `exit` returns to Parolsh, in the directory it was
in before: changes made inside the session stay there.

## Text

Anything that does not start with `!`, `#` or `/` is a question for the
agent. The answer is printed as it arrives, with a status line showing what
the agent is doing (see [During a turn](agents.md#during-a-turn)). `Ctrl+C`
cancels the turn.

## `/command`

Lines starting with `/` belong to the agent. Parolsh reserves no `/` commands
and forwards the line as typed, so the agent's own slash commands work.

## `#command`

| Command | What it does |
|---|---|
| `#help` | Show the input rules and commands |
| `#new` | Start a new conversation with the agent, in the current directory |
| `#cd <path>` | Change Parolsh's directory and start a new conversation there. `~` and relative paths work; no path means your home directory. Reloads the configuration of the project found there. An agent chosen with `#agent` stays when that project configures it; otherwise the project's `default_agent` is used, and the agent restarts if it changed. |
| `#agent` | Show the agent in use |
| `#agent list` | List configured agents; `*` marks the one in use |
| `#agent <name>` | Switch to another configured agent, with a new conversation. `default_agent` does not change. |
| `#options` | Show the running agent's options, `*` on the current values |
| `#options <id> <value>` | Set one of the agent's options until you leave Parolsh |
| `#config` | Show the configuration files in use, and whether they exist |
| `#config sample` | Print the full sample configuration |
| `#prompt` | Show the prompt style in use |
| `#prompt <name>` | Switch the prompt: `parolsh`, `starship` or `minimal`, until you leave Parolsh |
| `#project` | Show the project root |
| `#project init` | Create `.parolsh/` in the current directory |
| `#exit` | Leave Parolsh (`Ctrl+D` also works) |
