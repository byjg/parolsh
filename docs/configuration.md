---
sidebar_position: 5
---

# Configuration

Parolsh reads two TOML files, both optional:

| File | Scope |
|---|---|
| `~/.config/parolsh/config.toml` (or `$XDG_CONFIG_HOME/parolsh/config.toml`) | Global |
| `<project>/.parolsh/config.toml` | The current [project](projects.md) |

The project file is applied on top of the global one, field by field: it only
needs the values it changes. Unknown keys are an error, so a typo is reported
instead of being ignored.

## Start from the sample

The package installs a sample with every option and a ready block for each
[agent](agents.md), all commented out:

```bash
mkdir -p ~/.config/parolsh
cp /usr/share/doc/parolsh/config.sample.toml ~/.config/parolsh/config.toml

# Installed with Homebrew:
cp "$(brew --prefix)/share/parolsh/config.sample.toml" ~/.config/parolsh/config.toml
```

Uncomment the agents you use and set `default_agent`. The sample is also in
the repository as
[`config.sample.toml`](https://github.com/byjg/parolsh/blob/master/config.sample.toml).

## Example

Global:

```toml
shell = ["bash", "-ic"]
default_agent = "qwen"

[agents.qwen]
command = "qwen"
args = ["--acp"]

[agents.claude]
command = "claude-agent-acp"
mode = "plan"
```

Project:

```toml
default_agent = "claude"

[agents.claude]
mode = "auto"
```

In that project, the default agent is `claude` in its `auto` mode, still
launched as `claude-agent-acp`.

## Keys

| Key | Default | Meaning |
|---|---|---|
| `shell` | `["bash", "-ic"]` | Command line for `!command`; the command is added as the last argument. Must not be empty. |
| `prompt` | `"parolsh"` | Prompt style: `parolsh`, `starship` or `minimal`. See [Prompt](#prompt). |
| `default_agent` | none | Agent used when Parolsh starts. Must name a configured agent. `#agent <name>` switches until you leave Parolsh. |
| `agents.<name>.command` | required | Program that speaks ACP over stdio |
| `agents.<name>.args` | `[]` | Arguments for `command` |
| `agents.<name>.env` | `{}` | Environment variables added for the agent, such as a base URL or a model. See [API keys](agents.md#api-keys) before putting a key here. |
| `agents.<name>.mode` | the agent's default | The agent's own session mode id, set on every new conversation. See [Modes](agents.md#modes). |

`shell` only affects commands you type with `!`. `parolsh -c` always uses a
plain `bash -c`.

`env` merges key by key: a project can change one variable without repeating
the others.

## Prompt

| `prompt` | Looks like |
|---|---|
| `parolsh` (default) | `wallet ❯ ` |
| `minimal` | `❯ ` |
| `starship` | Your [Starship](https://starship.rs) prompt, from `~/.config/starship.toml` |

`#prompt <name>` switches until you leave Parolsh; `#prompt` prints the style
in use.

### Starship

With `prompt = "starship"`, Parolsh runs `starship prompt` before every line,
so the prompt looks as it does in your shell. Starship gets:

- the exit code of the last `!` command or agent turn: its `❯` turns red after
  a failure, as in bash;
- how long it took, for its `cmd_duration` module;
- the terminal width, and the right prompt (`right_format`) when you have one.

Starship does not know Parolsh, but it can show the agent in use. Parolsh sets
`PAROLSH_AGENT` and `PAROLSH_MODE` for it, so an `env_var` module in
`~/.config/starship.toml` shows them only inside Parolsh:

```toml
[env_var.PAROLSH_AGENT]
format = "with [🤖 $env_value]($style) "
style = "bold purple"
```

Your bash `PS1` is not used: Parolsh is not bash and cannot evaluate it.
Starship is configured outside the shell, which is why it works in both.

If `starship` is missing or fails, Parolsh prints why and uses the `parolsh`
prompt for the rest of the session. Without an ANSI terminal, `starship` also
falls back to `parolsh`.

## Other files

| Path | Content |
|---|---|
| `~/.local/state/parolsh/history` (or `$XDG_STATE_HOME/parolsh/history`) | Input history, last 1000 lines |
| `/usr/share/doc/parolsh/config.sample.toml` | The commented sample configuration (Linux packages) |
| `$(brew --prefix)/share/parolsh/config.sample.toml` | The same, installed with Homebrew |
