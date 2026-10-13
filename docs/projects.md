---
sidebar_position: 6
---

# Projects

A project is a directory that contains a `.parolsh/` directory, the same way a
Git repository contains `.git/`.

```text
wallet/
├── .git/
├── .parolsh/
│   └── config.toml
└── src/
```

Create one with `#project init`: it creates `.parolsh/` in Parolsh's
directory, the agent's (see [Two directories](#two-directories)). To make
another directory the root, `#cd` there first.

## Discovery

When Parolsh starts, and whenever Parolsh's directory moves (`#cd`, or
resuming a session of another directory), it looks for `.parolsh/` in that
directory and then in each parent directory. The first one found is the project root:

```text
/home/joao/projects/wallet/src/api/.parolsh
/home/joao/projects/wallet/src/.parolsh
/home/joao/projects/wallet/.parolsh      <- found
```

`#project` prints the root, or says that you are not in a project.

## What the project decides

The project root decides which configuration is used:
`.parolsh/config.toml` is loaded on top of the global configuration (see
[Configuration](configuration.md)). It can pick one of your configured agents
and change how Parolsh looks, but it cannot replace the programs you
configured: the shell, the clipboard, and the agents' commands. It can still
change the agents' `mode` and `options`, which decide what an agent may do
without asking. See
[What a project can change](configuration.md#what-a-project-can-change).

It does not change any directory.

## Two directories

Parolsh keeps two directories, which are the same until a `cd` moves one:

| Directory | Moved by | Used for |
|---|---|---|
| Parolsh's, the agent's | `#cd`, and resuming a session of another directory (`#resume`, `parolsh --resume`, `parolsh --continue`) | The agent's conversation, the project and its configuration, `#project init` |
| Your commands' | a `cd` in a `!command` or in shell mode, and whatever moves Parolsh's | `!command`, `!+command`, `!bash`, `@path`, `Tab` |

A `cd` in a shell command does not look for `.parolsh/` again: after
`!cd ../billing`, the project and its configuration are still the ones of
the agent's directory, and `#project` still prints that root. While the two
differ, the prompt shows both (see [Shell commands](input-routing.md#shell-commands)),
and `#cd .` brings Parolsh, the agent and the project to the directory of
your commands.

Agents keep their own memory and history. `.parolsh/` stores only what
Parolsh needs, and never a copy of the agent's conversation.
