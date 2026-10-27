---
sidebar_position: 4.5
---

# Spells

Some operations come back: install a mod on the game server, clean the merged
branches of every repository, trigger a site's build. You do one once, by
hand or with the agent, and the next time you have to remember which host,
which order, and what to check before and after.

A spell keeps it, for any agent: a shell script that does the predictable
part, and instructions for the judgement around it. The agent gets your
spells as an MCP server, `parolsh-spells`, the way it gets the
[history](sessions.md#the-agent-can-search-the-history), so a spell works
the same after `#agent <name>`, with agents that have no skills of their own.

## Asking for one

Say what you want. The agent finds the spell, works out its arguments from
what you said and where you are, and casts it:

```text
[claude] ~/Downloads ✦ install the mod in the vintage story server
• mcp__parolsh-spells__list_spells
• mcp__parolsh-spells__get_spell vs-install-mod
  vsinstrumentsredux_2.2.1.zip is the only mod here; modid vsinstrumentsredux 2.2.1, game 1.22.0 (server 1.22.7).
  The same file is already on the server and loaded at the last start: nothing to do.
```

When you know the name, `#cast` is the shortcut:

| Command | What it does |
|---|---|
| `#cast` | List your spells: name, arguments, description |
| `#cast <name> [args]` | Ask the agent to cast it, with what you typed after the name: `#cast vs-install-mod ./mymod.zip`. It still reads the instructions first. `Tab` completes the name |

## Writing a spell

A spell is a directory in `~/.config/parolsh/spells/` (or
`$XDG_CONFIG_HOME/parolsh/spells/`), named after it:

```text
~/.config/parolsh/spells/
└── vs-install-mod/
    ├── spell.toml
    └── run.sh
```

```toml
# spell.toml
description = "Install a Vintage Story mod (.zip) on my server: stop, copy, start, wait until it runs"
run  = "run.sh"                       # relative to the spell's directory
args = [{ name = "file", description = "Path of the mod .zip" }]
env  = { NIMBUS_CONTEXT = "tinkerboard", VS_SERVICE = "svc-1fdc0de7",
         VS_HOST = "jg@192.168.226.93", VS_DATA = "/vol/100g/vintagestory" }
instructions = """
Before running:
- The file: the one the user names, else the .zip mods in the directory of the user's commands,
  else in ~/Downloads and /tmp. More than one candidate → ask.
- Read modinfo.json: modid, version, dependencies. dependencies.game must not be newer than the
  server (image tag in `nimbus service describe`); missing dependency modids in Mods/ → list, ask.
- Same sha256 already in Mods/ → do not restart; check in the log that it loaded.
  Another version of the same modid → offer to replace it (both would load).
After:
- Failure or timeout: the [Error] lines with the modid and the end of server-main.log; propose
  the alternatives (roll back to the previous zip, another version, a missing dependency).
- scp failed: free space on the volume, SSH to the host; offer to retry.
"""
```

```bash
#!/usr/bin/env bash
# run.sh FILE: stop the server, copy the mod, start it, wait until it runs.
set -euo pipefail
file=$1
nimbus=(nimbus --context "$NIMBUS_CONTEXT")
log=$VS_DATA/Logs/server-main.log

unzip -p "$file" modinfo.json >/dev/null || { echo "not a mod: no modinfo.json"; exit 2; }

"${nimbus[@]}" service stop "$VS_SERVICE"
scp "$file" "$VS_HOST:$VS_DATA/Mods/"
inode=$(ssh "$VS_HOST" stat -c %i "$log")   # the start moves the log to Archive/
"${nimbus[@]}" service start "$VS_SERVICE"

for _ in $(seq 60); do                        # 5 minutes
  sleep 5
  [ "$(ssh "$VS_HOST" stat -c %i "$log" 2>/dev/null)" != "$inode" ] || continue
  ssh "$VS_HOST" grep -q "Dedicated Server now running" "$log" && { echo "running"; exit 0; }
done
echo "timeout: the server did not report running"; ssh "$VS_HOST" tail -50 "$log"; exit 1
```

| Key | |
|---|---|
| `description` | One line: what it does, and on what. The agent picks the spell by it. Required |
| `run` | The script, relative to the spell's directory. It must be executable (`chmod +x run.sh`) |
| `args` | The script's arguments, in order: `name` and `description` each. `cast_spell` must give all of them |
| `env` | Variables added to the script's environment |
| `instructions` | What the agent checks and decides before running, and what it offers when it fails |

A spell needs `run` or `instructions`, or both. With only `instructions`, it
is a procedure the agent follows with its own tools.

- **The script is the predictable part**: the same every time, and it works
  without Parolsh: `env VS_HOST=... ./run.sh mod.zip`.
- **The instructions are the judgement around it**: which file, is it
  compatible, is it already there, what to offer when it fails. The script
  stays simple, and the agent does not improvise the mechanics.
- **The script decides nothing about where files are**: it takes paths as
  arguments. Where to look comes from your words and the directory you are
  in.

A spell whose `spell.toml` cannot be read is listed with the reason, by
`#cast` and by `list_spells`.

## Learning one from a conversation

A spell is usually written right after you did the operation with the agent:

```text
[claude] ~ ✦ save what we just did as a spell to install vintage story mods
```

The agent writes `spell.toml` and `run.sh` with its own file tools, after you
approve the edit as any other. `list_spells`'s description tells it the
format. Read what it wrote: the files are plain text, yours to edit. The
spells are read at each call, so a new one, or one you edited, is there
at once, in the same conversation.

## The tools

| Tool | What it does |
|---|---|
| `list_spells` | The names, arguments and descriptions, and how to write a spell |
| `get_spell(name)` | The directory of your commands, the arguments, the environment, the instructions and the script's text |
| `cast_spell(name, args)` | Runs the script and waits up to 30 s. Returns the exit code and the last 16 KB of its output, stdout and stderr together, as `!+` keeps them; or, still running, its job number and the output so far |
| `spell_job(job)` | How a spell in the background goes: running or ended, for how long, its exit code and its output so far. Without `job`, the jobs of the conversation |
| `stop_spell(job)` | Stops it, with all it started: SIGTERM, then SIGKILL after 5 s |

`cast_spell` runs the script:

- through your configured [`shell`](configuration.md#keys) (`bash -ic` by
  default, so your `~/.bashrc` sets up its `PATH`), as `"$0" "$@"`: each
  argument is its own word, never read by a shell;
- in the directory of your commands, the one the prompt shows, even after a
  `cd` during the conversation. Parolsh keeps it in
  `~/.local/state/parolsh/run/<pid>/cwd` while it runs, and the server reads
  it at each call;
- with empty input, and no terminal: a script cannot ask you anything. Put
  the questions in the instructions: the agent asks you, and passes your
  answer as an argument (`--replace`). `sudo` cannot ask for your password
  either: give the spell a `NOPASSWD` rule in sudoers for the one command it
  needs, or use `sudo -A` with a graphical `SUDO_ASKPASS`.

## Spells in the background

Agents give up on a tool call after a while, and you cannot type while the
agent waits. So `cast_spell` waits 30 s at most: a script still running then
goes on in the background as a job, the agent's turn ends, and you get the
prompt back.

```text
[claude] ~/Downloads ✦ install the mod in the vintage story server
• mcp__parolsh-spells__cast_spell vs-install-mod
  The server is starting: spell job 1. I will tell you when it runs.
[claude] ~/Downloads ✦ how is it going?
• mcp__parolsh-spells__spell_job 1
  Still waiting for "Dedicated Server now running", 1:40 so far.
spell job 1 · vs-install-mod vsinstrumentsredux_2.2.1.zip ended · exit 0 · 2:12
[claude] ~/Downloads ✦ stop it          ← or this, while it runs: stop_spell 1
```

- **Ask how it goes**, or to stop it: the agent has `spell_job` and
  `stop_spell`.
- **When it ends**, Parolsh says so at the prompt, also while you type, and
  tells the agent how it ended, with the end of its output: the agent then
  does what the spell's instructions say for its end (reports, offers what
  to do on a failure). `spell_ended` in the
  [configuration](configuration.md#keys) says when:
  - `turn` (default): at once, in a turn of its own, unless you are typing:
    then with your next message;
  - `turn-immediately`: at once, even while you type: what you typed comes
    back at the next prompt;
  - `tell`: with your next message.
- **`#jobs`** lists the spells still running, with the turns in the
  background, and the prompt counts them: `1 job`.
- **They belong to the conversation.** Only its agent can follow them, so
  `#new`, `#cd`, `#agent <name>` and `#resume` stop them, and so do `#exit`
  and `Ctrl+D`. Each says what still runs the first time, and goes on the
  second:

  ```text
  [claude] ~ ✦ #new
  Still running: spell job 1 · vs-install-mod mod.zip. A new conversation stops it: #new again to go on.
  ```

  Stopping a spell stops all it started (its process group). A job that
  must outlive the conversation starts it its own way and returns, as
  `gh workflow run` does; the agent checks on it afterwards.
- **A script that hangs** (an `ssh` asking about a host key, a `read`) shows
  up as a job that does not end: ask the agent to stop it, then to fix the
  script.

## Where they come from

Spells are read from your configuration directory only. A project's
`.parolsh/` cannot define them, as it cannot replace the shell or the agents'
commands: a cloned repository must not add code the agent may run. See the
[security model](security.md#spells).

The agent gets `parolsh-spells` in every conversation, private ones
included, before you have any spell: spells have nothing to do with the
history, and the first one is learned through it.
It is a server of its own, not more tools on `parolsh-history`, because the
history is read-only: an agent where you allowed `mcp__parolsh-history` as a
whole would otherwise run scripts without asking. Allow `cast_spell` the way
you allow any command the agent runs.
