---
sidebar_key: parolsh
tags: [ai, rust, shell, acp]
---

# Parolsh

[![Build Status](https://github.com/byjg/parolsh/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/byjg/parolsh/actions/workflows/build.yml)
[![Opensource ByJG](https://img.shields.io/badge/opensource-byjg-success.svg)](http://opensource.byjg.com)
[![GitHub source](https://img.shields.io/badge/Github-source-informational?logo=github)](https://github.com/byjg/parolsh/)
[![GitHub license](https://img.shields.io/github/license/byjg/parolsh.svg)](https://opensource.byjg.com/license/)
[![GitHub release](https://img.shields.io/github/release/byjg/parolsh.svg)](https://github.com/byjg/parolsh/releases/)

**Speak to your terminal. Natural language first.**

Parolsh is a shell where natural language is the primary command language.
What you type goes to an AI agent through the
[Agent Client Protocol (ACP)](https://agentclientprotocol.com), and Bash stays
one `!` away.

```text
wallet ❯ list the 10 most recently modified files     ← the agent
wallet ❯ !git status                                  ← bash
wallet ❯ !bash                                        ← a full Bash session
wallet ❯ #cd ~/projects/billing                       ← Parolsh itself
```

It runs inside your usual terminal emulator, prints to the normal scrollback
(no full-screen UI), and works with any ACP agent: Claude Code, Codex, Qwen
Code, Gemini CLI, OpenCode and others.

The name comes from Esperanto *parol-* (to speak) + *sh*.

:::warning Early development
Parolsh talks to one ACP agent: answers stream as they arrive, permission
requests are asked in the terminal, and `Ctrl+C` cancels a turn. `#agent <name>`
switches agents. Not there yet: `#resume`, sending `!command` output to the
agent, and Markdown formatting.
:::

## Input

Every line is routed by its first character. Nothing guesses whether you typed
English or a command.

| You type | What happens |
|---|---|
| `text` | Sent to the agent |
| `/command` | Sent to the agent unchanged (the agent's own slash commands) |
| `!command` | Run with `bash -ic`, in the current directory |
| `!bash` | Interactive Bash session; `exit` returns to Parolsh |
| `#command` | Parolsh control command, see `#help` |

See [Input routing](docs/input-routing.md).

## Install

Add the [ByJG package repository](https://opensource.byjg.com/docs/packages),
then:

```bash
# Debian/Ubuntu
sudo apt install parolsh

# Fedora/RHEL
sudo dnf install parolsh
```

Packages are built for `amd64` and `arm64`, and need glibc 2.28 or newer
(Debian 10+, Ubuntu 20.04+, RHEL 8+).

## Documentation

- [Getting started](docs/getting-started.md)
- [Agents](docs/agents.md)
- [Input routing](docs/input-routing.md)
- [Projects](docs/projects.md)
- [Configuration](docs/configuration.md)

## Build from source

Requires Rust 1.88 or newer.

```bash
cargo build --release
./target/release/parolsh
```

Run the checks the CI runs:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

MIT

----
[Open source ByJG](http://opensource.byjg.com)
