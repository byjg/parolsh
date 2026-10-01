---
sidebar_position: 3
---

# Input routing

Parolsh never guesses whether a line is English or a shell command. The first
character decides, always:

| Input | Route |
|---|---|
| `text` | The active ACP agent |
| `?text` | The active ACP agent |
| `/command` | The active ACP agent, unchanged |
| `!command` | The configured shell, default `bash -ic` |
| `!+command` | The same, and its output goes with your next message |
| `!bash` | An interactive Bash session |
| `#command` | Parolsh itself |
| `!` alone | Lock: plain text goes to the shell, see [Shell mode](#shell-mode) |
| `?` alone | Unlock: plain text goes to the agent again |

`find files modified today` is always a question for the agent. To run the
Unix `find`, type `!find . -mtime -1`.

## Shell mode

When you run many commands in a row, `!` alone locks plain text to the
shell, and `?` alone unlocks it. The prompt's last symbol shows where plain
text goes:

```text
[claude] ~/projects/wallet ✦ !
[claude] ~/projects/wallet ❯ git status             ← bash
[claude] ~/projects/wallet ❯ ?why is main behind?   ← the agent
[claude] ~/projects/wallet ❯ ?
[claude] ~/projects/wallet ✦ what changed today?    ← the agent
```

A real session, locked to bash, sharing a diff with the agent, then
unlocked:

![A Parolsh session: ! locks plain text to bash and the prompt changes from ✦ to ❯; git status runs in bash; !+git diff shows the diff and keeps it for the agent; ?what did I change asks Claude, which explains the change; ? unlocks back to ✦](images/shell-mode.png)

| Input | Agent mode `✦` (default) | Shell mode `❯` |
|---|---|---|
| `text` | the agent | the shell, like `!text` |
| `?text` | the agent | the agent |
| `/command` | the agent | the shell (`/usr/bin/ls` runs); `?/command` reaches the agent |
| `!command`, `!+command`, `!bash` | the shell | the same |
| `#command` | Parolsh | Parolsh |

Unlike `!bash`, shell mode stays in Parolsh: the agent is a `?` away, `!+`
shares output with it, and `#` commands work. Each command runs in its own
shell, but a `cd` carries over to the next (see [`!command`](#command)). The
mode lasts until you change it or leave Parolsh.

To start in shell mode, set `input = "shell"` in `config.toml` (see
[Configuration](configuration.md#keys)), or run `parolsh --input=shell`;
`--input` wins over the file. A project's `.parolsh/config.toml` can set it
too: `#cd` into a project whose `input` differs switches to it. Without an
agent configured, Parolsh always starts in shell mode.

## While you type

On an ANSI terminal the line shows where it will go before you press Enter:

| Line | Color |
|---|---|
| `!command`, `!bash` | yellow |
| `!+command` | green |
| `#command` | magenta |
| `/command` | blue |
| text for the agent | the terminal's color |

In shell mode, plain text is a shell command, so it is yellow too. The marker
(`!`, `!+`, `#`, `/`) is bold. While the line is only a marker, a
dim hint says what it does, for example `!+  run a shell command and send its
output with your next message`. The hint is a tip: the right arrow does not
insert it into the line.

### Tab completion

On `!command` and `!+command` lines, and plain lines in shell mode, `Tab`
completes the word at the cursor,
as bash does: a single match goes into the line, several fill in what they
have in common, and the next `Tab` shows them in a menu (`Tab` and
`Shift+Tab` move through it, `Enter` picks one).

- **The command name** completes from the programs on your `PATH` and, when
  the shell is bash, from the aliases, functions and builtins your
  `~/.bashrc` sets up. That list is read once, in the background, when
  Parolsh starts: an alias added later shows up the next time.
- **A command's arguments** complete as bash would, when the shell is bash
  and [bash-completion](https://github.com/scop/bash-completion) is
  installed: git branches and subcommands, ssh hosts, systemctl units,
  `--flags`. It uses the completions bash-completion ships, those in
  `~/.local/share/bash-completion/completions/` and `~/.bash_completion`,
  but not the ones your `~/.bashrc` defines (`complete -C ...`,
  `source <(kubectl completion bash)`): put those in `~/.bash_completion`
  to get them. `!bash` has your full bash completion.
- **Other words**, and arguments bash-completion has nothing for (or takes
  more than half a second to find), complete as file and directory names,
  from the directory your commands run in. `~/` and absolute paths work, hidden
  files show up when the name starts with `.`, and special characters are
  escaped (`My\ Files`).

Text for the agent, `?text`, `/command` and `#command` have no completion.

## `!command`

The command runs as `bash -ic "<command>"`, in the directory the last
command ended in.

- **Your `~/.bashrc` is loaded**, so aliases, functions and shell options work
  as in your normal terminal. The shell is interactive (`-i`) because a plain
  `bash -c` ignores aliases, and the stock Debian/Ubuntu `~/.bashrc` stops
  right away when the shell is not interactive.
- **A `cd` carries over.** When a command ends, its shell tells Parolsh the
  directory it ended in, and the next command runs there: `cd src`,
  `cd - && make`, `pushd`, a `cd` before `exit 1`. Nothing else does:
  `export FOO=1`, a variable or an alias defined in the line is gone when
  the command ends. Use `!bash` for a session that keeps all of its state.
- **The agent stays where it is.** A `cd` moves your commands, not the
  conversation: the agent keeps working in the directory it started in, and
  the prompt shows both while they differ:

  ```text
  [claude] ~/projects/wallet ✦ !cd /var/log
  [claude ~/projects/wallet] /var/log ✦ #cd .       ← brings the agent here
  [claude] /var/log ✦
  ```

  `#cd .` starts a new conversation in the directory of your commands. The
  outputs you share with `!+` say where they ran.
- The directory comes back through the shell's `EXIT` trap, so the
  `shell` must understand `trap` (bash, zsh, dash do). A command that ends
  with `exec`, or is killed, leaves the directory as it was.
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
[claude] ~/projects/wallet ✦ !+docker ps
CONTAINER ID   IMAGE        STATUS
...
(output of `docker ps` goes with your next message)
[claude] ~/projects/wallet ✦ which of these containers looks unhealthy?
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

Opens a real interactive Bash in the directory of your commands, with your full
`~/.bashrc` and completions. `exit` returns to Parolsh, in the directory it was
in before: changes made inside the session stay there.

## Text

Anything that does not start with `!`, `#` or `/` is a question for the
agent, in agent mode (in shell mode, start it with `?`). The answer is printed as it arrives, with a status line showing what
the agent is doing (see [During a turn](agents.md#during-a-turn)). `Ctrl+C`
cancels the turn.

## `/command`

Lines starting with `/` belong to the agent. Parolsh reserves no `/` commands
and forwards the line as typed, so the agent's own slash commands work.

## `#command`

| Command | What it does |
|---|---|
| `#help` | Show the input rules and commands |
| `#new` | Start a new conversation with the agent, in the agent's directory |
| `#cd <path>` | Change Parolsh's directory, for the agent and your commands, and start a new conversation there. `~` and relative paths work, from the directory of your commands, so `#cd .` brings the agent where a `cd` took them; no path means your home directory. Reloads the configuration of the project found there. An agent chosen with `#agent` stays when that project configures it; otherwise the project's `default_agent` is used, and the agent restarts if it changed. |
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
| `#audit` | What happened since Parolsh started, see [`#audit`](#audit) |
| `#exit` | Leave Parolsh (`Ctrl+D` also works) |

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
| `USER` | `!command`, `!+command` and `!bash` with their exit code, your messages (their first line) with the agent they went to, `#` commands, and your answers to the agent's questions |
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
