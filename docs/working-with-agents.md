---
sidebar_position: 3
---

# Working with agents

What happens while you talk to an agent: a turn and its status line,
permission requests and questions, turns sent to the background, a second
agent to pair with, and the context the conversation takes. Installing and
configuring each agent is in [Agents](agents.md); finding and resuming
earlier conversations is in [Sessions and history](sessions.md).

## Lifecycle

- **Start:** Parolsh starts the default agent when it starts, and opens a
  conversation in the current directory. This runs in the background, so the
  prompt is ready at once.
- **New conversation:** `#new`, or `#cd` to another directory. `#new private`
  starts one that is not saved in the
  [history](sessions.md#the-history-of-sessions).
- **Back to an earlier one:** `#resume <n>`, with agents that can (Claude,
  Codex, OpenCode), see [Resuming a session](sessions.md#resuming-a-session).
- **Switch:** `#agent <name>`, see [Switching agents](agents.md#switching-agents).
- **Restart:** `#cd` restarts the agent when the project found there changes
  its settings (its `mode` or `options`), and `#agent <name>` starts another
  one. `#cd` does not switch to the project's `default_agent` while the agent
  in use is configured, which it almost always is: agents are defined in the
  global file, so every project has them. The project's `default_agent` is
  only used when Parolsh starts there; to use another agent afterwards, name
  it with `#agent <name>`. If the agent stops (it crashed, or its command does
  not exist), Parolsh prints the reason and `#new` starts it again.
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
  [Showing the conversation again](sessions.md#showing-the-conversation-again)).
- Code blocks and table rows are not broken, and a list item wraps under its
  own text.
- Only the word being received is held back, until its end arrives. When the
  agent goes quiet in the middle of a line, the word is shown anyway, so the
  answer does not look cut short while the turn goes on.

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
⠹ Running: Terminal (9s) · 12s
```

Tool calls only update that line: `Running: <title> (8s)` with the time that
tool has taken, `2 tools running` when there are more, `Failed: <title>` when
one fails, and back to `Thinking` when they are done. While the answer's
text arrives, it says `Writing`. An agent that shares
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
- Only what you type once a question or a permission request is shown
  answers it. What you typed while the agent worked is dropped: an `Allow`
  typed ahead would let the agent do what you had not seen yet.

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
  [`#audit`](sessions.md#audit) like the ones in a turn. Its reasoning and
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
⠹ Running: Read src/client.rs (2s) · 12s                         (Ctrl+Z)
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
  [background tasks](#background-tasks-and-the-terminal-title) or
  [spells](spells.md#spells-in-the-background) running.
- **A new conversation** (`#new`, `#cd`, `#agent`, `#resume`) leaves them
  running. Their answer is then shown, not given to the new conversation.
- In the [history](sessions.md#a-turn-in-the-background), the turn is a
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
[shell commands](input-routing.md#shell-commands)), and a turn being cancelled.

## Pairing two agents

`#pair <agent>` brings in a second agent to answer what the agent you work
with says when it stops: "A, B or C? I recommend A". The second agent knows
the issue and what the agent said last, and looks at the files itself. It
does not see how the agent got there, so what it answers is a second opinion,
not an echo of the first.

```text
[claude] ~/projects/wallet ✦ #pair codex
Pairing with codex: it answers what claude says when it stops.
Your next message is the issue, for both.
[claude ⇄ codex] ~/projects/wallet ✦ the transfer endpoint accepts a negative amount
  ... claude works, and stops ...
I can reject it in the controller (A) or in the Transfer model (B).
I recommend B. Which one?
── codex answers ──
  ... codex reads the code ...
Go with B, and add a test for an amount of zero as well.
Send this to claude? [Y/n], or write to codex:
```

At that question:

| | |
|---|---|
| `Enter`, `y` or `yes` | The answer goes to the agent as its next message, and when the agent stops again the second agent answers again |
| `n` or `no` | Nothing is sent: your next message goes to the agent, as usual. `#copy` has the answer |
| Anything else | Goes to the second agent, which answers again: "why not A?" |

Nothing reaches the agent without your `Enter`, so the two never go on by
themselves. Only what you type once the question is shown counts: what was
typed or pasted before it is dropped.

There are two ways to start, by what you write after the agent's name:

| | |
|---|---|
| `#pair codex` | Your next message is the issue, for both: the agent works on it, and the second agent gets it with the agent's first answer |
| `#pair codex <text>` | The text is for the second agent alone, and it starts at once, with what the agent answered last. The agent is not sent it |

The second form is for an agent that is already at work, or done, and it is
where you say what you want of the second agent:

```text
[claude] ~/projects/wallet ✦ #pair codex review what was done: only what you observe, ask for nothing
Pairing with codex: it answers what claude says when it stops.
Only codex got what you wrote: claude was not sent it.
── codex answers ──
  ... codex reads the diff ...
The amount of zero has no test, and the error message leaks the account id.
Send this to claude? [Y/n], or write to codex:
```

Without a text of yours, the second agent decides what the agent does next:
it answers its question, chooses among its options, or says what is wrong or
missing.

- **The second agent gets** what the agent wrote after its last tool call,
  each time the agent ends a turn. Not its reasoning, its tool calls, or what
  it wrote in between, and not the history of the session: it runs without
  the history server. When the agent has not answered yet in this
  conversation, as right after `#new` or `#resume`, `#pair codex <text>`
  still starts at once, and the second agent looks at the work as it is.
- **It is a conversation of its own**, in a second agent process, in the
  same directory. Its permission requests come to you like the agent's. Any
  configured agent can be the second one, the same one included: give it a
  [mode](agents.md#modes) that does not change files, in an entry of its own:

  ```toml
  [agents.navigator]
  command = "claude-agent-acp"
  mode = "plan"
  ```

- **The prompt names both** while you are pairing: `[claude ⇄ codex]`.
- **When it has nothing more to say** the second agent says so, and you are
  back at the prompt.
- **`#decide`** asks the second agent again about what the agent said last,
  for example after you answered the agent yourself. Not before the issue:
  after `#pair codex`, the second agent gets it with your next message.
- **`#pair off`** stops it, and `#pair` says who the second agent is. A new
  conversation (`#new`, `#agent`, `#cd`, `#resume`) and a turn sent to the
  background with `Ctrl+Z` stop it too, and so do `Ctrl+Z` during the second
  agent's own turn (it does not work in the background) and a second agent
  that stops. Parolsh says why: `Pairing with codex ended: …`.
- **One at a time, with an agent running:** `#pair` while pairing says
  `already pairing: #pair off stops it`. When the second agent's turn is
  cancelled or fails, you are back at the prompt, without the question.

Questions the agent asks in the middle of a turn, as a form or a permission
request, still come to you: see
[Questions from the agent](#questions-from-the-agent).

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
forwards as typed (see [agent slash commands](input-routing.md#agent-slash-commands)): `/compact`
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
  [history](sessions.md#the-history-of-sessions) is not compacted:
  `#redraw`, `#audit <n>` and the agent's search of it show the conversation
  as it happened, with a `conversation compacted` line where it was.
- A compacted conversation is [resumed](sessions.md#resuming-a-session)
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
