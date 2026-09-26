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
```

## Switching agents

`default_agent` is the agent Parolsh starts. To use another one:

```text
wallet ❯ #agent codex
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
Parolsh prints a notice with the modes it does offer. Agents without session
modes (Kilo Code) ignore `mode`, and Parolsh says so.

Whatever the mode, when the agent asks for permission, Parolsh shows the
agent's options and waits for your choice (see
[During a turn](#during-a-turn)).

## API keys

Keep keys in your shell environment, for example in `~/.bashrc`:

```bash
export OPENAI_API_KEY=sk-...
```

The agent inherits them through Parolsh. `env` works for keys too, but then
the key sits in plain text in `config.toml`, and in a project's
`.parolsh/config.toml` it can end up in git. Use `env` for base URLs, model
names and switches.

## Summary

| Agent | ACP command | Login or key | Modes (agent default in bold) |
|---|---|---|---|
| [Claude](#claude) | `claude-agent-acp` | Claude Code login, `ANTHROPIC_API_KEY` | **`default`**, `acceptEdits`, `plan`, `auto`, `bypassPermissions` |
| [Codex](#codex) | `codex-acp` | ChatGPT login, `OPENAI_API_KEY` | `read-only`, **`agent`**, `agent-full-access` |
| [Gemini CLI](#gemini-cli) | `gemini --acp` | `GEMINI_API_KEY`, Vertex AI | **`default`**, `autoEdit`, `yolo`, `plan` |
| [Qwen Code](#qwen-code) | `qwen --acp` | `~/.qwen/settings.json`, OpenAI-compatible key | `plan`, `default`, `auto-edit`, `auto`, `yolo` |
| [Kilo Code](#kilo-code) | `kilo acp` | `kilo auth login` | none (set in Kilo) |
| [Any OpenAI-compatible API](#any-openai-compatible-api) | `qwen --acp` | `OPENAI_API_KEY` | Qwen Code's |
| [Goose](#goose) | `goose acp` | provider key | `approve`, `smart_approve`, `chat`, **`auto`** |

:::note Tested
Claude (`claude-agent-acp` 0.81), Codex (`codex-acp` 1.13) and Qwen Code
(0.24) are tested with Parolsh; their modes are the ones they reported. Kilo
Code was checked with an ACP handshake. Gemini CLI and Goose come from their
documentation and source code as of September 2026. Agents change their
modes between versions: `#agent list` shows what the running one offers.
:::

## Claude

Claude Code, through the ACP adapter. Needs Node.js 22 or newer.

```bash
npm install -g @agentclientprotocol/claude-agent-acp
```

It reuses your Claude Code login (`~/.claude/`), or uses `ANTHROPIC_API_KEY`
when it is set.

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

:::warning Permissions are configured in Kilo
Kilo has no ACP session modes, so `mode` has no effect. Kilo asks according
to its own configuration: set `"permission": "allow"` in
`~/.config/kilo/kilo.json` to let it act without asking, or use its
`/auto-approve` setting.
:::

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

## More agents

Other agents that speak ACP. Configure them the same way; `#agent list` shows
the modes they offer once running.

| Agent | ACP command |
|---|---|
| OpenCode | `opencode acp` |
| GitHub Copilot CLI | `copilot --acp` |
| Cursor CLI | `cursor-agent acp` |
| Augment (Auggie) | `auggie --acp` |
| Mistral Vibe | `vibe-acp` |
| Kimi CLI | `kimi acp` |
| Cline | `cline --acp` |

The full list is at
[agentclientprotocol.com](https://agentclientprotocol.com/get-started/agents).

## Lifecycle

- **Start:** Parolsh starts the default agent when it starts, and opens a
  conversation in the current directory. This runs in the background, so the
  prompt is ready at once.
- **New conversation:** `#new`, or `#cd` to another directory.
- **Switch:** `#agent <name>`, see [Switching agents](#switching-agents).
- **Restart:** `#cd` to a project configured with another agent restarts it.
  If the agent stops (it crashed, or its command does not exist), Parolsh
  prints the reason and `#new` starts it again.
- **Exit:** leaving Parolsh stops the agent and everything it started.

## During a turn

The answer is printed as it arrives. On an ANSI terminal, a status line under
it shows what the agent is doing, redrawn in place instead of adding lines:

```text
wallet ❯ why is the API container restarting?
⠹ Running: Terminal · 12s
```

Tool calls only update that line. While the agent reasons before answering
("thinking"), the line shows the latest bit of its reasoning, for example
`⠸ Thinking: Let me calculate · 3s`; the reasoning itself is not printed. When
the turn ends, the status line is replaced by a summary:

```text
The container exits because DATABASE_URL is not set...
✓ 3 tool calls · 18s
```

Without an ANSI terminal (output piped, or `TERM=dumb`), there is no status
line and each tool call is printed as a `• <title>` line.

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
- Enter skips a question. If you skip a question the agent marked as
  required, it gets no answers at all.

Claude and Codex ask through the standard ACP form request (elicitation), and
Qwen Code through its own question tool; Parolsh answers both. Gemini CLI does
not ask questions in ACP mode.

`Ctrl+C` cancels the turn: the agent stops, Parolsh prints `(cancelled)` and
returns to the prompt.
