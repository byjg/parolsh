---
sidebar_position: 6
---

# Troubleshooting

Find the message you see below.

## The agent cannot start

*cannot start `claude-agent-acp`: not found on PATH*: see
[When an agent cannot start](agents.md#when-an-agent-cannot-start).

## The agent stopped

```text
parolsh: <reason>
parolsh: the agent stopped. Run #new to start it again.
```

The agent's process ended: it crashed, was killed, or closed the connection.
The line before it is the reason, as Parolsh got it: usually
`Process exited with exit status: 1: <the end of what the agent printed on
stderr>`, or `the agent closed the connection`. The conversation is
lost: `#new` starts the agent again with a new one, in the current directory.

## A turn failed

```text
parolsh: <the agent's message>: { ...details... }
```

The agent answered your message with an error instead of an answer. The
agent is still running and the conversation continues: send another message.
The JSON after the message is the agent's own detail, useful to find the
cause in its documentation. The prompt shows status 1 for that turn.

### Tool-call loop protection (Qwen Code)

```text
parolsh: Tool-call loop protection stopped this turn. The session is still available; send a more specific instruction to continue.: {
  "code": "LOOP_DETECTED",
  "errorKind": "loop_detected",
  "loopType": "invalid_tool_params_stagnation"
}
```

Qwen Code watches its model for loops, and stops the turn when it finds one.
The model is looping, not Parolsh or Qwen Code. `loopType` says what it did:

| `loopType` | The model... |
|---|---|
| `consecutive_identical_tool_calls` | repeated the same tool call with the same arguments |
| `global_tool_call_duplicate` | repeated the same tool call across the turn |
| `alternating_tool_call_pattern` | alternated between the same two tool calls |
| `action_stagnation` | kept calling the same tool without progress |
| `shell_command_stagnation` | repeated similar shell commands without progress |
| `read_file_loop` | read files too many times in a row without progress |
| `invalid_tool_params_stagnation` | kept sending invalid tool arguments without correcting them |
| `repeated_tool_execution_failure` | kept hitting the same tool failure after a reminder |
| `chanting_identical_sentences` | repeated the same sentence in its answer or reasoning |
| `repetitive_thoughts` | repeated the same reasoning thought |
| `turn_tool_call_cap` | reached the per-turn tool-call limit |

What to do:

- Send a more specific message: name the file, the command or the next step.
  The model is told the turn was stopped and tries another approach.
- Use a stronger model. Smaller models send invalid tool arguments and
  repeat calls more often.
- `turn_tool_call_cap`: an explicit `model.maxToolCallsPerTurn` in
  `~/.qwen/settings.json` is a hard cap. Remove it to use Qwen Code's
  adaptive default, or raise it (`0` disables it).
- `model.skipLoopDetection: true` in `~/.qwen/settings.json` disables the
  other checks, except `consecutive_identical_tool_calls`,
  `shell_command_stagnation`, `global_tool_call_duplicate`,
  `invalid_tool_params_stagnation` and `repeated_tool_execution_failure`,
  which are always on.

Checked against Qwen Code 0.24.5.
