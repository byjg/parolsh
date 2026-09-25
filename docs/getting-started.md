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

The package installs a single binary, `/usr/bin/parolsh`, and depends only on
`bash`. ACP agents are not dependencies: install the one you want to use and
configure it, see [Agents](agents.md).

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
