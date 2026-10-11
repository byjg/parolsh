---
sidebar_position: 1
---

# Getting started

## Install

Add the [ByJG package repository](https://opensource.byjg.com/docs/packages),
then install the package:

```bash
# Debian/Ubuntu
sudo apt install parolsh

# Fedora/RHEL
sudo dnf install parolsh
```

On macOS (or Linux) with [Homebrew](https://brew.sh):

```bash
brew install byjg/tap/parolsh
```

Homebrew builds Parolsh from source from the
[ByJG tap](https://github.com/byjg/homebrew-tap), installing Rust only for the
build. The sample configuration is then in
`$(brew --prefix)/share/parolsh/config.sample.toml`.

The Linux package installs a single binary, `/usr/bin/parolsh`, and depends
only on `bash`.

## Install an agent

ACP agents are not dependencies: install at least one before the first run.
For example, Claude:

```bash
npm install -g @agentclientprotocol/claude-agent-acp
```

[Agents](agents.md) lists the others (Codex, Gemini CLI, Qwen Code, Kilo
Code, OpenCode, Goose, ...) with their install command and login.

## First run

The first time Parolsh starts, it creates your configuration,
`~/.config/parolsh/config.toml`, from its built-in sample:

- every agent it finds on your `PATH` (`claude-agent-acp`, `codex-acp`,
  `gemini`, `qwen`, `kilo`, `opencode`, `goose`) is already enabled;
- the first one found becomes `default_agent`, in that order;
- everything else stays in the file as commented examples.

```text
Created /home/joao/.config/parolsh/config.toml with claude, qwen. Using claude; switch with #agent <name>.
[claude] ~/projects/wallet ✦
```

If no agent is found, the file is created with everything commented out:
install an agent, then uncomment its block and `default_agent`. Parolsh never
overwrites an existing file; delete it to run the first-run setup again.

`#config` shows which configuration files are used, and `#config sample`
prints the full sample. See [Configuration](configuration.md) for every
option.

## Start it

Run `parolsh` in the directory you want to work in:

```bash
cd ~/projects/wallet
parolsh
```

Parolsh starts the configured agent in the background and opens a new
conversation in that directory, the same way `claude` or `codex` behave when
started inside a folder. The prompt appears right away; a question typed
before the agent is ready waits for it. Each new `parolsh` starts a new,
independent conversation.

On an ANSI terminal, Parolsh greets you with a banner showing the agent, its
mode and the directory:

```text
╭─────────╮     Parolsh x.y.z
│  > _    │     agent: claude (auto)
╰─┬───────╯     project: ~/projects/wallet
  ╱             #help

Speak to your terminal. Natural language first.
[claude] ~/projects/wallet ✦
```

Type `#help` to see the commands and `#exit` (or `Ctrl+D`) to leave.

## First commands

```text
[claude] ~/projects/wallet ✦ what changed today?    ask the agent
[claude] ~/projects/wallet ✦ !git status            run a shell command
[claude] ~/projects/wallet ✦ !+docker ps           run it, and share its output with your next question
[claude] ~/projects/wallet ✦ !bash                  open a Bash session, `exit` to come back
[claude] ~/projects/wallet ✦ !                      lock: plain text goes to bash (❯), `?` unlocks
[claude] ~/projects/wallet ✦ #cd ~/projects/billing switch to another project
[claude] ~/projects/wallet ✦ #project init          mark the current directory as a project
[claude] ~/projects/wallet ✦ #new                   start a new conversation
```

The answer is printed as it arrives. `Ctrl+C` cancels it and returns to the
prompt. `Ctrl+Z` gives you the prompt back while the agent goes on with it,
see [Turns in the background](agents.md#turns-in-the-background).

To start with plain text going to bash, run `parolsh --input=shell` or set
`input = "shell"` in the configuration (see
[Shell mode](input-routing.md#shell-mode)).

To pick up where you left off, `parolsh --continue` goes back to the last
conversation of this project, and `parolsh --resume <n>` to session `n` (see
[Resuming a session](input-routing.md#resuming-a-session)).

`parolsh --version` prints the version. A build that is not the release, from
a later commit or with changed files, says so:

```text
parolsh 0.6.0-dev (85cb214, dirty)
```

## Use it from scripts

`parolsh -c` runs a command with a plain, non-interactive `bash -c`, without
the agent and without loading `~/.bashrc`:

```bash
parolsh -c 'echo "$0 $1"' name first
```

The exit code is the command's exit code.
