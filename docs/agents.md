---
sidebar_position: 2
---

# Agents

Parolsh talks to AI agents through the
[Agent Client Protocol (ACP)](https://agentclientprotocol.com). It never calls
an LLM API itself: the agent does. Each agent keeps its own login, models and
settings (`~/.claude`, `~/.codex`, `~/.gemini`, `~/.qwen`, ...), and Parolsh
does not replace them. Any program that speaks ACP over stdio works.

Each agent is one `[agents.<name>]` table in
`~/.config/parolsh/config.toml`. The package installs a sample with every
agent below, commented out, in `/usr/share/doc/parolsh/config.sample.toml`
(see [Configuration](configuration.md)).

```toml
default_agent = "claude"

[agents.claude]
command = "claude-agent-acp"    # program that speaks ACP
args = []                       # its arguments
env = { }                       # extra environment variables for it
mode = "default"                # the agent's own mode; omit to keep its default
options = { effort = "low" }    # the agent's own config options
```

## Switching agents

`default_agent` is the agent Parolsh starts. To use another one:

```text
[claude] ~/projects/wallet ✦ #agent codex
Agent changed to codex. Started a new conversation.
```

`#agent <name>` stops the running agent and starts the other one, with a new
conversation. It lasts until you leave Parolsh; `default_agent` does not
change. `#agent` prints the agent in use.

`#agent list` shows every configured agent and its `mode`; `*` marks the one
in use. For that one, it also shows the modes the agent offers, with `*` on
the current mode:

```text
* claude     claude-agent-acp         mode: auto
             offers: default, acceptEdits, plan, auto*, bypassPermissions
  codex      codex-acp                mode: agent
```

## Modes

`mode` is the agent's own session mode id, as the agent names it: Parolsh
sets it on every new conversation and does not translate it. Without `mode`,
the agent starts in its own default.

If the agent does not offer that mode, it stays in its current one and
Parolsh prints a notice with the modes it does offer. Some agents (Kilo Code)
have no session modes but a `mode` [option](#options): `mode` sets that
instead.

Whatever the mode, when the agent asks for permission, Parolsh shows the
agent's options and waits for your choice (see
[During a turn](#during-a-turn)).

## Options

Agents offer config options: reasoning effort, model, fast mode, and so on.
`options` sets them, with the agent's own ids and values, on every new
conversation:

```toml
[agents.claude]
command = "claude-agent-acp"
options = { effort = "low", model = "sonnet" }
```

`#options` shows what the running agent offers, with `*` on the current
value, and `#options <id> <value>` changes one until you leave Parolsh (it is
set again after `#new` and `#cd`):

```text
[claude] ~/projects/wallet ✦ #options
  mode               default*, acceptEdits, plan, auto, bypassPermissions
  model              default*, opus[1m], claude-fable-5-1[1m], sonnet, haiku
  effort             default, low*, medium, high, xhigh, max
  fast               on, off*
[claude] ~/projects/wallet ✦ #options model sonnet
model = sonnet, until you leave Parolsh.
```

An option or value the agent does not offer is reported with what it does
offer, and the conversation keeps going. The list can change: Claude drops
`fast` for models without a fast mode. Options are values the agent reports
for your account and version, so check `#options` rather than this page.

`options` merges key by key: a project can change one option.

### Less thinking

No agent turns reasoning off through a standard switch; each has its own
option or setting:

| Agent | Option | Least thinking |
|---|---|---|
| Claude | `effort` | `low` |
| Codex | `reasoning_effort` | `low` |
| Qwen Code | `"reasoning": false` in `~/.qwen/settings.json`, see [Qwen Code](#qwen-code) | off |
| Kilo Code | `effort` | the lowest value `#options` lists |
| OpenCode | `effort`, only for models that have it | the lowest value `#options` lists |
| Gemini CLI, Goose | none through ACP | their own settings |

How Parolsh *displays* the reasoning is separate: `thinking` in the
[configuration](configuration.md#keys) shows it in the status line (default),
hides it, or prints it.

## API keys

Keep keys in your shell environment, for example in `~/.bashrc`:

```bash
export OPENAI_API_KEY=sk-...
```

The agent inherits them through Parolsh. `env` works for keys too, but then
the key sits in plain text in `config.toml`. Use `env` for base URLs, model
names and switches. `env` is only read from the global file, see
[What a project can change](configuration.md#what-a-project-can-change).

## Summary

| Agent | ACP command | Login or key | Modes (agent default in bold) |
|---|---|---|---|
| [Claude](#claude) | `claude-agent-acp` | Claude Code login, `ANTHROPIC_API_KEY` | **`default`**, `acceptEdits`, `plan`, `auto`, `bypassPermissions` |
| [Codex](#codex) | `codex-acp` | ChatGPT login, `OPENAI_API_KEY` | `read-only`, **`agent`**, `agent-full-access` |
| [Gemini CLI](#gemini-cli) | `gemini --acp` | `GEMINI_API_KEY`, Vertex AI | **`default`**, `autoEdit`, `yolo`, `plan` |
| [Qwen Code](#qwen-code) | `qwen --acp` | `~/.qwen/settings.json`, OpenAI-compatible key | `plan`, `default`, `auto-edit`, `auto`, `yolo` |
| [Kilo Code](#kilo-code) | `kilo acp` | `kilo auth login` | none (set in Kilo) |
| [OpenCode](#opencode) | `opencode acp` | `opencode auth login`, free models | none (`mode` option: **`build`**, `plan`) |
| [Any OpenAI-compatible API](#any-openai-compatible-api) | `qwen --acp` | `OPENAI_API_KEY` | Qwen Code's |
| [Goose](#goose) | `goose acp` | provider key | `approve`, `smart_approve`, `chat`, **`auto`** |

:::note Tested
Claude (`claude-agent-acp` 0.81), Codex (`codex-acp` 1.13) and Qwen Code
(0.24) are tested with Parolsh; their modes are the ones they reported. So
are Kilo Code (7.8), OpenCode (2.0.26) and Goose (1.54). Gemini CLI comes
from its documentation and source code as of September 2026. Agents change
their modes between versions: `#agent list` shows what the running one
offers.
:::

## Claude

Claude Code, through the ACP adapter. Needs Node.js 22 or newer.

```bash
npm install -g @agentclientprotocol/claude-agent-acp
```

It uses your Claude Code login, or `ANTHROPIC_API_KEY` when it is set. The
adapter does not log in by itself: without either, every message fails with
*Authentication required*.

To use your Claude subscription (Pro, Max) or Console account, log in once
with the Claude Code CLI:

```bash
npm install -g @anthropic-ai/claude-code   # or another install method of Claude Code
claude auth login                          # or run `claude` and type /login
claude auth status --text                  # check that you are logged in
```

The login is kept in `~/.claude/.credentials.json` on Linux and in the
Keychain on macOS, where the adapter finds it. When `ANTHROPIC_API_KEY` is
also set, the key wins: turns are billed to the API, not to your
subscription.

```toml
[agents.claude]
command = "claude-agent-acp"
mode = "default"
```

| Mode | Behaviour |
|---|---|
| `default` | Asks before edits and commands |
| `acceptEdits` | Edits without asking, commands ask |
| `plan` | Plans only, changes nothing |
| `auto` | Acts without asking; still asks about risky actions |
| `bypassPermissions` | Never asks |

:::note nvm
With Node from nvm, a global npm install lives under the active Node version.
Switching versions hides the command until it is installed again. This
applies to every npm-based agent on this page.
:::

## Codex

OpenAI Codex, through the ACP adapter. `@zed-industries/codex-acp` is the old,
deprecated name.

```bash
npm install -g @agentclientprotocol/codex-acp
```

Login: your ChatGPT account (Codex's usual login in `~/.codex/`), or an API
key in `CODEX_API_KEY` or `OPENAI_API_KEY`.

```toml
[agents.codex]
command = "codex-acp"
mode = "agent"
```

| Mode | Behaviour |
|---|---|
| `read-only` | Reads; asks before anything else |
| `agent` | Codex's default ("auto review"): asks only for actions it flags as unsafe |
| `agent-full-access` | Full access, never asks |

## Gemini CLI

Google's Gemini CLI. Needs Node.js 20 or newer.

```bash
npm install -g @google/gemini-cli
```

Login: `GEMINI_API_KEY` (Google AI Studio), or Vertex AI with
`GOOGLE_GENAI_USE_VERTEXAI=true` and `GOOGLE_API_KEY` (or
`GOOGLE_CLOUD_PROJECT` and `GOOGLE_CLOUD_LOCATION`). `GEMINI_MODEL` picks the
model.

:::warning Google login for individuals
With a personal Google login ("Gemini Code Assist for individuals"), Gemini
CLI 0.61 fails to open a session: *This client is no longer supported for
Gemini Code Assist for individuals… migrate to the Antigravity suite*. The
refusal comes from Google's service, not from Parolsh. Use an API key or
Vertex AI instead.
:::

```toml
[agents.gemini]
command = "gemini"
args = ["--acp"]
mode = "default"
```

| Mode | Behaviour |
|---|---|
| `default` | Asks for approval |
| `autoEdit` | Edits are approved, commands still ask |
| `yolo` | Approves everything |
| `plan` | Read-only |

Older versions use `--experimental-acp` instead of `--acp`.

## Qwen Code

Qwen Code, a fork of Gemini CLI. Needs Node.js 22 or newer.

```bash
npm install -g @qwen-code/qwen-code
```

Qwen Code uses the provider and model configured in `~/.qwen/settings.json`.
The free Qwen login was discontinued in April 2026: it needs an API key. The
simplest is any OpenAI-compatible endpoint, see
[Any OpenAI-compatible API](#any-openai-compatible-api). Anthropic
(`ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`) and Gemini
(`GEMINI_API_KEY`, `GEMINI_MODEL`) keys work too.

```toml
[agents.qwen]
command = "qwen"
args = ["--acp"]
mode = "default"
```

| Mode | Behaviour |
|---|---|
| `plan` | Analyzes only |
| `default` | Asks before file edits and shell commands |
| `auto-edit` | Edits are approved, commands still ask |
| `auto` | A classifier approves safe actions and blocks risky ones |
| `yolo` | Approves everything |

Qwen Code does not always start in `default` (it started in `auto` in our
tests): set `mode` to be sure.

Qwen Code stops a turn when its model loops, with *Tool-call loop protection
stopped this turn*. The conversation continues, see
[Troubleshooting](troubleshooting.md#tool-call-loop-protection-qwen-code).

**Thinking off.** Set `"reasoning": false` in the `model.generationConfig`
of `~/.qwen/settings.json`:

```json
{
  "model": {
    "name": "Qwen/Qwen3.6-35B-A3B",
    "generationConfig": { "reasoning": false }
  }
}
```

- It works with the provider set through environment variables
  (`OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_API_KEY`, or the same in
  `~/.qwen/.env`): no `modelProviders` entry is needed.
- For a Qwen-family model, Qwen Code then sends
  `chat_template_kwargs: {enable_thinking: false}` (vLLM, SGLang) or
  `enable_thinking: false` (DashScope).
- Checked with Qwen Code 0.24.5 and `Qwen/Qwen3.6-35B-A3B` on vLLM: 29
  reasoning chunks for a small question before, none after, same answer.
- A top-level `"enable_thinking": false` in `settings.json` is ignored.

With a [`modelProviders`](https://github.com/QwenLM/qwen-code/blob/main/docs/users/configuration/model-providers.md)
entry, which Qwen Code recommends over environment variables, put
`"reasoning": false` in that entry's `generationConfig` instead: the top-level
`model.generationConfig` is ignored for provider models. Declaring
`"capabilities": { "reasoning": { "profile": "qwen-chat-template" } }` on the
entry also makes Qwen Code offer the `reasoning_effort` option, so
`#options reasoning_effort none` switches thinking per conversation. These two
come from Qwen Code's source and were not checked end to end.

## Kilo Code

Kilo Code's CLI. The npm package ships a native binary, no Node.js version
requirement.

```bash
npm install -g @kilocode/cli
kilo auth login
```

Kilo uses your Kilo account (Kilo Gateway, many models), or a provider you
configure in Kilo itself (`kilo auth`, `~/.config/kilo/kilo.json`).

```toml
[agents.kilo]
command = "kilo"
args = ["acp"]
```

Kilo has no ACP session modes, but a `mode` option with its agents: `ask`
and `plan` (read-only), `code` (its default), `debug` and `orchestrator`.
`mode = "ask"` sets it through that option.

:::warning Permissions are configured in Kilo
None of Kilo's modes acts without asking. Kilo asks according to its own
configuration: set `"permission": "allow"` in `~/.config/kilo/kilo.json` to
let it act without asking, or use its `/auto-approve` setting.
:::

## OpenCode

OpenCode's CLI, a native binary.

```bash
curl -fsSL https://opencode.ai/install | bash
opencode auth login
```

OpenCode uses the providers you log in to with `opencode auth login`. Without
one, it offers its own free models; its default may be one that was retired
(*Model ... has been deprecated*), so name one with the `model` option.
`#options` lists the models it offers.

```toml
[agents.opencode]
command = "opencode"
args = ["acp"]
mode = "build"
options = { model = "opencode/big-pickle" }
```

Like Kilo, OpenCode has no ACP session modes, but a `mode` option with its
agents: `build` (its default) and `plan` (read-only). `mode = "plan"` sets it
through that option. `effort` is only there for models that have it.

:::warning `build` acts without asking
With OpenCode's default configuration, `build` edits files and runs commands
without asking. To be asked first, set it in `~/.config/opencode/opencode.json`
(or the project's `opencode.json`):

```json
{
  "permission": { "edit": "ask", "bash": "ask" }
}
```

Parolsh then shows each request, like any agent's.
:::

The [history tools](input-routing.md#the-agent-can-search-the-history) work
with OpenCode, which shows them as `parolsh-history.search_history` and so on.

OpenCode reports its [context, cost and tokens](#the-context-and-what-it-costs),
compacts with `/compact`, keeps its conversations for
[`#resume`](input-routing.md#resuming-a-session), and copies one for a
[turn sent to the background](#turns-in-the-background).

## Any OpenAI-compatible API

To use an API token directly, with OpenAI, OpenRouter, Azure, a local vLLM or
Ollama, or any endpoint that speaks the OpenAI API, run
[Qwen Code](#qwen-code) with three variables. It switches to that endpoint
when all three are set.

```bash
npm install -g @qwen-code/qwen-code
export OPENAI_API_KEY=sk-...        # in your shell, see "API keys"
```

```toml
[agents.openai]
command = "qwen"
args = ["--acp"]
env = { OPENAI_BASE_URL = "https://api.openai.com/v1", OPENAI_MODEL = "gpt-5" }
mode = "default"
```

Other endpoints only change `env`:

```toml
# OpenRouter
env = { OPENAI_BASE_URL = "https://openrouter.ai/api/v1", OPENAI_MODEL = "anthropic/claude-sonnet-4.5" }

# Local Ollama (use any non-empty OPENAI_API_KEY)
env = { OPENAI_BASE_URL = "http://localhost:11434/v1", OPENAI_MODEL = "qwen3-coder" }
```

The modes are Qwen Code's, see above.

## Goose

Block's open-source agent, with many providers.

```bash
curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | CONFIGURE=false bash
```

It reads its provider from `~/.config/goose/config.yaml` (written by
`goose configure`) or from environment variables. For an OpenAI-compatible
endpoint:

```toml
[agents.goose]
command = "goose"
args = ["acp"]
env = { GOOSE_PROVIDER = "openai", GOOSE_MODEL = "gpt-5", OPENAI_BASE_URL = "https://api.openai.com/v1" }
mode = "approve"
```

| Mode | Behaviour |
|---|---|
| `approve` | Asks before every tool call |
| `smart_approve` | Asks only for sensitive tool calls |
| `chat` | Chat only: no tools at all |
| `auto` | Acts without asking (Goose's default) |

:::note The desktop app is not the CLI
The Goose desktop package puts the app itself on `PATH` as `goose`: with it,
`command = "goose"` opens a window instead of starting the agent. Point
`command` to the CLI it ships, on Linux
`/usr/lib/goose/resources/bin/goose`. It reads the same
`~/.config/goose/config.yaml` as the app.
:::

Goose calls the [history tools](input-routing.md#the-agent-can-search-the-history)
from code it runs, so Parolsh shows them as `execute typescript`, not by
their names. Its permission choices arrive as their ids (`allow_once`,
`reject_always`, ...).

## More agents

Other agents that speak ACP. Configure them the same way; `#agent list` shows
the modes they offer once running.

| Agent | ACP command |
|---|---|
| GitHub Copilot CLI | `copilot --acp` |
| Cursor CLI | `cursor-agent acp` |
| Augment (Auggie) | `auggie --acp` |
| Mistral Vibe | `vibe-acp` |
| Kimi CLI | `kimi acp` |
| Cline | `cline --acp` |

The full list is at
[agentclientprotocol.com](https://agentclientprotocol.com/get-started/agents).

## When an agent cannot start

*cannot start `claude-agent-acp`: not found on PATH* means Parolsh does not
see the agent's program. npm installs agents in a directory only your shell
adds to `PATH` (with nvm, `~/.nvm/versions/node/<version>/bin`), so it happens
when Parolsh was not started from a terminal. `shell_env` (see
[Shell environment](configuration.md#shell-environment)) imports your shell's
environment for that case; `shell_env = "always"` forces it.

## Lifecycle

- **Start:** Parolsh starts the default agent when it starts, and opens a
  conversation in the current directory. This runs in the background, so the
  prompt is ready at once.
- **New conversation:** `#new`, or `#cd` to another directory. `#new private`
  starts one that is not saved in the
  [history](input-routing.md#the-history-of-sessions).
- **Back to an earlier one:** `#resume <n>`, with agents that can (Claude,
  Codex), see [Resuming a session](input-routing.md#resuming-a-session).
- **Switch:** `#agent <name>`, see [Switching agents](#switching-agents).
- **Restart:** `#cd` to a project configured with another agent restarts it.
  If the agent stops (it crashed, or its command does not exist), Parolsh
  prints the reason and `#new` starts it again.
- **Failed turn:** when the agent answers a message with an error (for
  example, Qwen Code's tool-call loop protection), Parolsh prints it and the
  conversation continues: send another message. See
  [Troubleshooting](troubleshooting.md).
- **Exit:** leaving Parolsh stops the agent and everything it started.

## During a turn

The answer is printed as it arrives. On an ANSI terminal it is laid out for
reading, apart from the output of your commands:

```text
[claude] ~/projects/wallet ✦ why is the api container restarting?
✦ The container exits because DATABASE_URL is not set: the entrypoint reads
  it before the compose file's env_file is loaded.

  Move it to the service's environment and restart.

✓ 3 tool calls · 18s
```

- `✦` marks where the answer starts, and its other lines are indented under
  it. After a question from the agent, or a notice, the answer that goes on is
  marked again.
- Lines break between words at the width of your terminal, never in the
  middle of a word. The width is read as the text arrives, so after you resize
  the window the next lines follow it; what is already printed stays as it is
  (`Ctrl+L` shows the conversation again at the new width, see
  [Showing the conversation again](input-routing.md#showing-the-conversation-again)).
- Code blocks and table rows are not broken, and a list item wraps under its
  own text.
- Only the word being received is held back, until its end arrives.

Its markdown is rendered on the fly: `**bold**` in bold, `` `code` `` and code
blocks in cyan, `#` headings bold and underlined, `-` bullets as `•`, with the
markers hidden. A marker cut in half between two chunks waits for the next
one. An unclosed `**` only affects the rest of its line. `markdown = false` in
the [configuration](configuration.md#keys) keeps the raw text, still laid out
as above.

Links `[text](url)` show their text underlined and clickable, followed by the
URL: `text (https://...)`. Most terminals open it with Ctrl+click (GNOME
Terminal, Konsole, kitty, WezTerm, iTerm2, Windows Terminal, tmux 3.4 or
newer); elsewhere the URL after the text is still there to copy. The URL is
not repeated when it is the text itself, as in `<https://...>`. A link is the
only thing held back while it streams, from its `[` to its `)`. `links` in the
configuration shows only the clickable text, or only the text and the URL.

A status line under the answer shows what the agent is doing, redrawn in place
instead of adding lines:

```text
[claude] ~/projects/wallet ✦ why is the API container restarting?
⠹ Running: Terminal · 12s
```

Tool calls only update that line: `Running: <title> (8s)` with the time that
tool has taken, `2 tools running` when there are more, `Failed: <title>` when
one fails, and back to `Thinking` when they are done. An agent that shares
its plan shows the step in progress, `Plan 2/5: <step>`. While the agent
reasons before answering ("thinking"), the line shows the latest bit of its
reasoning, for example `⠸ Thinking: Let me calculate · 3s`; the reasoning
itself is not printed.

When the agent runs background tasks, the line counts them: `· 2 bg` (see
[Between turns](#between-turns)).

After 15 seconds without anything from the agent, the line says for how long
it has been quiet, and after a minute how to cancel:

```text
⠦ Thinking · 376s · quiet 340s · Ctrl+C to cancel
```

A quiet agent may still be working, for example waiting for a slow tool or
its API, since agents send nothing while they wait. To see what it really
sent, see [Logging the agent's messages](troubleshooting.md#logging-the-agents-messages).

When the turn ends, the status line is replaced by a summary, after a blank
line:

```text
✦ The container exits because DATABASE_URL is not set...

✓ 3 tool calls · 18s · 24k of 1M (2%) · $0.35
```

After the time comes what the agent reports of its context: the tokens in it,
how many it holds, and how full that is. Then what the conversation cost so
far, for the agents that say it. See
[The context and what it costs](#the-context-and-what-it-costs).

Without an ANSI terminal (output piped, or `TERM=dumb`), the answer is printed
as the agent sends it: no mark, no indent and no wrapping. There is no status
line either, and each tool call is printed as a `• <title>` line.

When the agent asks for permission, Parolsh shows what it wants to do, then
its options:

```text
Permission requested: Writing to /tmp/notes.txt
  /tmp/notes.txt
  @@ -1 +1,2 @@
   first line
  +second line
  [1] Allow All Edits
  [2] Allow
  [3] Reject
Choose:
```

File edits show as a diff (red and green on an ANSI terminal), text the agent
attached is printed as is, and when the agent attached neither, its tool
input is shown instead. At most 40 lines are shown.

Type the number and press Enter. Anything else rejects once.

### Questions from the agent

Agents can ask you questions while they work:

```text
The agent asks (press Enter to skip a question):
  Which color do you prefer?
    [1] Red
    [2] Blue
  Choose a number, or type your own answer: 2
```

- Type the number of a choice, several numbers separated by commas when more
  than one is allowed, or your own answer when the agent accepts free text.
- With Claude and Codex you can do both: the number, then a note after a
  comma, as in `2, keep the tests`. They send a free-text "Other" field with
  each question; it is where what you type goes, not a question of its own.
- A number that is no choice is asked again, not taken as your answer.
- Enter skips a question. If you skip a question the agent marked as
  required, it gets no answers at all.

Claude and Codex ask through the standard ACP form request (elicitation), and
Qwen Code through its own question tool; Parolsh answers both. Gemini CLI does
not ask questions in ACP mode.

`Ctrl+C` cancels the turn: the status line shows `Cancelling…`, the agent
stops, Parolsh prints `(cancelled)` and returns to the prompt.

`Ctrl+Z` gives you the prompt back and leaves the agent working on the turn:
see [Turns in the background](#turns-in-the-background).

An agent blocked in a request may not confirm the cancel. After 5 seconds, or
at once on a second `Ctrl+C`, Parolsh stops waiting and prints
`(cancelled — the agent did not confirm)`. The agent keeps running, and what
it still sends for that turn is dropped. Your next message waits until that
turn ends (the status line shows `Waiting for the previous turn to stop…`),
and `Ctrl+C` there drops it. `#new` starts the agent again instead of
waiting.

## Between turns

An agent can keep working after its turn ended. Claude, for example, runs long
commands as background tasks, ends the turn ("I'll be notified when it
finishes") and, when the task ends, wakes up on its own to check the result.
Parolsh prints what the agent writes then above the prompt, after a header,
while you type:

```text
[claude] ~/projects/wallet ✦
(the agent, between turns)
• Check batch log
✦ The batch finished: 34 repositories created, none failed.
[claude] ~/projects/wallet ✦ █
```

- Its text is printed line by line; a line it leaves unfinished is printed
  after it pauses briefly. Tool calls are `• <title>` lines, and go to
  [`#audit`](input-routing.md#audit) like the ones in a turn. Its reasoning and
  plan are not shown: the status line only exists during a turn.
- When it asks for permission or asks you questions, the prompt makes way for
  the question. What you were typing comes back after you answer.
- If it stops, Parolsh says so and `#new` starts it again.

Background tasks belong to the agent's process: switching agents (`#agent`)
and leaving Parolsh stop the agent, and the tasks it started with it.

## Turns in the background

A turn can take minutes. `Ctrl+Z` sends it to the background: you get the
prompt back at once, for your commands and for other questions to the agent,
and the turn's answer comes when it is done.

```text
[claude] ~/projects/wallet ✦ compare the code with version 0.6.0
⠹ Reading src/client.rs · 12s                                    (Ctrl+Z)
[1] in the background: compare the code with version 0.6.0
[claude] ~/projects/wallet ✦ what does the retry test check?       1 job
✦ It checks that a timeout is retried three times, with a longer wait
  each time.

✓ 4s · 31k of 1M (3%) · $0.21
[1] done: compare the code with version 0.6.0
✦ Since 0.6.0, the client retries on timeouts, …

✓ 14 tool calls · 185s
[claude] ~/projects/wallet ✦
```

An agent works on one message at a time in a conversation, so the two are
two conversations:

- **The turn keeps the conversation it was in**, and the agent process it
  runs in.
- **You go on in a copy of it**, in a second agent process: everything said
  before that turn's message is in it, the message is not. It takes a second
  or two to start.
- **When the turn ends**, Parolsh shows its answer (what the agent wrote
  after its last tool call) and gives it to the agent you are talking to,
  with your request, along with your next message. Until then that agent
  does not know the request exists, and it never sees the work behind the
  answer: the files read, the commands run.
- What happens after the copy is not shared the other way either: the turn
  in the background knows nothing of what you ask meanwhile.

Several turns can run at once, each sent with `Ctrl+Z`:

| | |
|---|---|
| `#jobs` | Lists them: the request, what the agent is doing, its tool calls and for how long |
| `#fg [n]` | Waits for turn `n`, or the last one, saying the same: `Ctrl+C` cancels it, `Ctrl+Z` goes back to the prompt |
| The prompt | Says how many run, on its right: `2 jobs` |

```text
[claude] ~/projects/wallet ✦ #jobs
[1] compare the code with version 0.6.0 · Running: cargo test (42s) · 10 tool calls · 5:51
```

- **A turn that asks for permission**, or asks you questions, does it at the
  prompt, between two lines that say whose question it is:

  ```text
  ── background turn 1: compare the code with version 0.6.0 ──
  Permission requested: cargo test
    [1] Yes
    [2] No
  Choose: 1
  ── end of background turn 1's question ──
  ```

  It does not interrupt a turn you are in: that turn's status line says
  `job 1 is waiting for you`, and the question comes when it ends. The turn
  in the background waits meanwhile. `#fg` asks it at once.
- **What a cancelled or failed turn wrote** is shown, not given to the
  agent.
- **Leaving stops them.** `#exit` and `Ctrl+D` say what is still running the
  first time, and leave the second time. The same when the agent has
  [background tasks](#background-tasks-and-the-terminal-title) running.
- **A new conversation** (`#new`, `#cd`, `#agent`, `#resume`) leaves them
  running. Their answer is then shown, not given to the new conversation.
- In the [history](input-routing.md#a-turn-in-the-background), the turn is a
  session of its own while it runs, and goes to the end of the session it
  left when it ends.

### How the conversation is copied

It depends on what the agent offers:

| The agent | You go on in | |
|---|---|---|
| Copies a conversation up to a message | A copy that ends before the turn's message | Claude, Codex |
| Copies a whole conversation | A copy: Parolsh tells the agent, with your next message, that your last request is being worked on elsewhere | OpenCode |
| Does not copy | A new conversation, which Parolsh says: `The conversation here is a new one.` | Qwen Code |

The first message of a conversation has nothing before it to copy: you go on
in a new conversation. Kilo Code announces that it copies conversations; it
was not tried.

:::note Not part of ACP
Copying a conversation (`session/fork`) is still marked unstable in ACP, and
copying up to a message is not in it: the adapters of Claude and Codex take
it in a field named after JetBrains' AIR, whoever sends it. No JetBrains
software is involved. If an adapter drops it, Parolsh falls back to the next
row of the table.
:::

Not sent to the background: a `!command` (see
[`!command`](input-routing.md#command)), and a turn being cancelled.

## The context and what it costs

An agent remembers the conversation in its context, which has a size. Claude,
Codex, Qwen Code, Kilo Code and OpenCode report how full it is, and Parolsh
shows it where it does not take room in the prompt:

```text
✓ 3 tool calls · 18s · 24k of 1M (2%) · $0.35
[claude] ~/projects/wallet ✦                                          ctx 82%
```

- **With each turn's summary**: the tokens in the context, its size and the
  percentage.
- **On the right of the prompt**, only when it fills up: `ctx 82%` in yellow
  from 75%, in red from 90%. Below that, nothing.
- **The cost** is the agent's own estimate for the conversation so far, at
  API prices: with a subscription it is not what you are billed. Claude,
  Kilo Code and OpenCode report one, Codex and Qwen Code do not. Parolsh
  computes no price itself.

### Compacting

When the context fills up, the conversation is compacted: the agent replaces
it with a summary and goes on. Claude does it by itself when it gets close to
full, and you can ask for it with the agent's own command, which Parolsh
forwards as typed (see [`/command`](input-routing.md#command-1)): `/compact`
for Claude, Codex, Kilo Code and OpenCode, `/compress` for Qwen Code. What
follows the command is for the agent, as in `/compact keep the decisions about the retry logic`.

```text
[claude] ~/projects/wallet ✦ /compact
Compacted: 28k → 5k tokens (6.8s)

✓ 7s · 5k of 1M (1%) · $0.35
```

- Yours or the agent's, a compaction is said in one line, with the context
  before and after. OpenCode says that it compacted, not the sizes: the line
  is `Compacted (1.2s)`. Qwen Code and Kilo Code say theirs in their answer
  instead (Qwen: `Context compressed (21249 -> ~18491).`; Kilo prints the
  summary it made). Qwen's context is updated with the next turn.
- The context grows again with the next turn: the agent's own instructions
  and tools are counted in it each time.
- **The agent then remembers a summary; Parolsh still has everything.** The
  [history](input-routing.md#the-history-of-sessions) is not compacted:
  `#redraw`, `#audit <n>` and the agent's search of it show the conversation
  as it happened, with a `conversation compacted` line where it was.
- A compacted conversation is [resumed](input-routing.md#resuming-a-session)
  like any other: the agent goes back to the summary and what came after it,
  while the screen shows the exchanges in full. What you see above that line
  the agent may only know in short.

### Background tasks and the terminal title

Claude tells Parolsh which background tasks it runs. While some run, the
prompt shows how many on the right, with the time of the oldest, and the
status line of a turn counts them too:

```text
[claude] ~/projects/wallet ✦                                ⧗ 1 bg · 0:42
⠹ Thinking · 4s · 1 bg
```

On an ANSI terminal, Parolsh also sets the terminal's title, so you can follow
the agent from another tab or window:

```text
parolsh · ~/…/projects/wallet              before the agent names the conversation
parolsh · Fix the batch script             the conversation's title, from the agent
⠹ parolsh · Fix the batch script           a turn is running
⠹ 2 bg · parolsh · Fix the batch script    background tasks are running
```

- The conversation's title is the agent's own (Claude and Codex send one;
  Claude after its first answer), cut to 40 characters. Until then, and after `#new`, the
  title shows the agent's directory.
- The title is left alone while a `!command` runs, since programs such as
  `vim` or `ssh` set their own.
- Parolsh saves the title it found on start and puts it back on exit, on
  terminals with a title stack (xterm, GNOME Terminal and other VTE terminals,
  kitty, WezTerm, tmux). Elsewhere the last title stays after Parolsh ends.

Parolsh also tells the terminal which directory the shell is in (OSC 7), on
start and before every prompt, as bash does on VTE terminals. Terminals that
use it (GNOME Terminal, Tilix, Ptyxis, WezTerm, kitty, foot, iTerm2) open a new
tab or split in that directory, and Tilix no longer warns about a
"configuration issue" on a profile that runs Parolsh. It is the shell's
directory, the one in the prompt, which `!cd` moves.

Only Claude reports background tasks, through the ACP extension of JetBrains'
AIR (`asyncTasks`), which Parolsh asks for. Other agents show no count; their
turns still spin in the title.
