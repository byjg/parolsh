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

## What a project can change

A project file comes with the directory, often from a repository you cloned,
so it can change how Parolsh looks and which configured agent it uses, but
never what is executed. These keys are **global only**:

- `shell`
- `agents.<name>.command`, `agents.<name>.args` and `agents.<name>.env`
- `history_days` and `save_commands`: what is kept about your sessions is
  yours to choose, not a repository's

A project can only refer to agents defined in the global file: it cannot add
one. A project file that sets a global-only key is reported as an error and
nothing is started:

```text
parolsh: invalid /home/joao/projects/wallet/.parolsh/config.toml: `shell` can only be set in the global configuration
```

To use other variables or arguments in one project, define a second agent in
the global file (for example `qwen-mini` with its own `env`) and pick it with
the project's `default_agent`.

## The first run creates it

When `~/.config/parolsh/config.toml` does not exist, Parolsh writes it from
its built-in sample, with the [agents](agents.md) it finds on `PATH` enabled
and the first of them as `default_agent` (see
[First run](getting-started.md#first-run)). The sample holds every option and
a ready block for each agent; the ones not installed stay commented out.

`#config` shows the files in use and whether they exist:

```text
[claude] ~/projects/wallet ✦ #config
Global:  /home/joao/.config/parolsh/config.toml
Project: /home/joao/projects/wallet/.parolsh/config.toml  (not found)
#config sample prints every option, with a block for each agent.
```

To start over, delete the file and restart Parolsh, or copy the sample by
hand. It is printed by `#config sample`, and also installed with the package:

```bash
cp /usr/share/doc/parolsh/config.sample.toml ~/.config/parolsh/config.toml

# Installed with Homebrew:
cp "$(brew --prefix)/share/parolsh/config.sample.toml" ~/.config/parolsh/config.toml
``` The sample is also in
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
| `input` | `"agent"` | Where plain text goes when Parolsh starts: `agent` or `shell`. `parolsh --input=agent\|shell` overrides it. See [Shell mode](input-routing.md#shell-mode). |
| `default_agent` | none | Agent used when Parolsh starts. Must name a configured agent. `#agent <name>` switches until you leave Parolsh. |
| `agents.<name>.command` | required | Program that speaks ACP over stdio |
| `agents.<name>.args` | `[]` | Arguments for `command` |
| `agents.<name>.env` | `{}` | Environment variables added for the agent, such as a base URL or a model. See [API keys](agents.md#api-keys) before putting a key here. |
| `agents.<name>.mode` | the agent's default | The agent's own session mode id, set on every new conversation. See [Modes](agents.md#modes). |
| `agents.<name>.options` | `{}` | The agent's own config options (`effort`, `model`, ...), set on every new conversation. See [Options](agents.md#options). |
| `markdown` | `true` | Render the answers' markdown on ANSI terminals: `**bold**`, `` `code` `` in cyan, code blocks, `#` headings and `-` bullets, with the markers hidden. `false` prints the raw text. |
| `shell_env` | `"auto"` | When to import the environment your shell sets up (`~/.profile`, `~/.bashrc`): `auto` (when Parolsh was not started from a shell), `always`, `never`. See [Shell environment](#shell-environment). |
| `links` | `"both"` | How markdown links are shown: `both` (clickable text followed by the URL), `clickable` (clickable text only), `inline` (underlined text followed by the URL). See [links](agents.md#during-a-turn). |
| `audit_entries` | `1000` | How many entries [`#audit`](input-routing.md#audit) keeps, the oldest dropped first. `0` keeps none. |
| `redraw_exchanges` | `5` | How many exchanges [`#redraw`](input-routing.md#showing-the-conversation-again) and `Ctrl+L` show again, and how many are shown when a session is resumed. |
| `history_days` | `90` | Days an idle session stays in the [history of sessions](input-routing.md#the-history-of-sessions). `0` saves nothing. Global only. |
| `save_commands` | `false` | Also save `!command` and `!bash` lines in the history. Global only. |
| `thinking` | `"status"` | How the agent's reasoning is displayed: `status` (its latest line in the status line), `hidden` (only "Thinking"), `show` (printed dim, before the answer). |

`shell` only affects commands you type with `!`. `parolsh -c` always uses a
plain `bash -c`.

`options` merges key by key: a project can change one option without
repeating the others.

## Prompt

| `prompt` | Looks like |
|---|---|
| `parolsh` (default) | `[claude] ~/projects/wallet ✦ ` |
| `minimal` | `✦ ` |
| `starship` | Your [Starship](https://starship.rs) prompt, from `~/.config/starship.toml` |

The `parolsh` prompt shows:

- in brackets, the agent that answers text you type; `[no agent]` when none
  is running (none configured, or it stopped);
- the directory your commands run in, with `~` for your home, and only its
  last two directories: `~/Projects/opensource/byjg/parolsh` shows as
  `~/…/byjg/parolsh`. After a `cd` moved them away from the agent, the
  agent's directory goes in the brackets: `[claude ~/…/byjg/parolsh] /tmp`
  (see [`!command`](input-routing.md#command));
- where plain text goes: `✦` to the agent, `❯` to the shell (see
  [Shell mode](input-routing.md#shell-mode)). It is red when the last `!`
  command, agent turn or `#` command failed. `minimal` shows only this
  symbol.

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
`PAROLSH_AGENT` and `PAROLSH_MODE` for it while the agent runs, so an
`env_var` module in `~/.config/starship.toml` shows them only inside Parolsh:

```toml
[env_var.PAROLSH_AGENT]
format = "with [🤖 $env_value]($style) "
style = "bold purple"
```

`PAROLSH_INPUT` is `agent` or `shell`: where plain text goes (see
[Shell mode](input-routing.md#shell-mode)). Starship runs in the directory of
your commands; after a `cd` moved them away from the agent,
`PAROLSH_AGENT_DIR` is the agent's directory.

Your bash `PS1` is not used: Parolsh is not bash and cannot evaluate it.
Starship is configured outside the shell, which is why it works in both.

If `starship` is missing or fails, Parolsh prints why and uses the `parolsh`
prompt for the rest of the session. Without an ANSI terminal, `starship` also
falls back to `parolsh`.

## Shell environment

Started from a terminal, Parolsh inherits everything your shell set up: the
`PATH` of nvm (where npm installs the agents), exported API keys, proxies.
Started any other way (a desktop launcher, a terminal profile that runs
`parolsh` directly, an IDE), it does not, and an agent installed with npm
fails with *cannot start `claude-agent-acp`: not found on PATH*.

Parolsh cannot read `~/.profile` or `~/.bashrc` itself: they are shell
scripts. Instead it asks your shell to run them, `bash -ilc 'env -0'` (with
the program of the `shell` setting), and imports the environment it prints,
except `TERM`, `PWD` and `SHLVL`. The shell runs without a terminal and with a
5-second limit, so a startup file that waits for input cannot block Parolsh;
whatever the startup files print is ignored.

| `shell_env` | Imports the environment |
|---|---|
| `auto` (default) | only when the process that started Parolsh is not a shell |
| `always` | on every start (about 0.2 s) |
| `never` | never |

If the import fails, Parolsh says why after the banner and keeps the
environment it was given.

## Other files

| Path | Content |
|---|---|
| `~/.local/state/parolsh/history` (or `$XDG_STATE_HOME/parolsh/history`) | Input history, last 1000 lines |
| `/usr/share/doc/parolsh/config.sample.toml` | The commented sample configuration (Linux packages) |
| `$(brew --prefix)/share/parolsh/config.sample.toml` | The same, installed with Homebrew |
