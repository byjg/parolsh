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
Code, Goose, ...) with their install command and login.

## First run

The first time Parolsh starts, it creates your configuration,
`~/.config/parolsh/config.toml`, from its built-in sample:

- every agent it finds on your `PATH` (`claude-agent-acp`, `codex-acp`,
  `gemini`, `qwen`, `kilo`, `goose`) is already enabled;
- the first one found becomes `default_agent`, in that order;
- everything else stays in the file as commented examples.

```text
Created /home/joao/.config/parolsh/config.toml with claude, qwen. Using claude; switch with #agent <name>.
wallet ❯
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
  ┏━┓┏━┓┏━┓┏━┓╻  ┏━┓╻ ╻
  ┣━┛┣━┫┣┳┛┃ ┃┃  ┗━┓┣━┫
  ╹  ╹ ╹╹┗╸┗━┛┗━╸┗━┛╹ ╹  0.1.0
  Speak to your terminal. Natural language first.
  agent: claude (auto) · ~/projects/wallet · #help
wallet ❯
```

Type `#help` to see the commands and `#exit` (or `Ctrl+D`) to leave.

## First commands

```text
wallet ❯ what changed today?    ask the agent
wallet ❯ !git status            run a shell command
wallet ❯ !bash                  open a Bash session, `exit` to come back
wallet ❯ #cd ~/projects/billing switch to another project
wallet ❯ #project init          mark the current directory as a project
wallet ❯ #new                   start a new conversation
```

The answer is printed as it arrives. `Ctrl+C` cancels it and returns to the
prompt.

## Use it from scripts

`parolsh -c` runs a command with a plain, non-interactive `bash -c`, without
the agent and without loading `~/.bashrc`:

```bash
parolsh -c 'echo "$0 $1"' name first
```

The exit code is the command's exit code.
