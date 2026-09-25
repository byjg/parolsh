---
sidebar_position: 4
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

Create one with `#project init` in the directory you want as the root.

## Discovery

When Parolsh starts, and after every `#cd`, it looks for `.parolsh/` in the
current directory and then in each parent directory. The first one found is
the project root:

```text
/home/joao/projects/wallet/src/api/.parolsh
/home/joao/projects/wallet/src/.parolsh
/home/joao/projects/wallet/.parolsh      <- found
```

`#project` prints the root, or says that you are not in a project.

## What the project decides

The project root decides which configuration is used:
`.parolsh/config.toml` is loaded on top of the global configuration (see
[Configuration](configuration.md)).

It does not change the working directory: that is always the directory where
Parolsh started, or the last `#cd`.

Agents keep their own memory and history. `.parolsh/` stores only what
Parolsh needs, and never a copy of the agent's conversation.
