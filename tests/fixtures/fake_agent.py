#!/usr/bin/env python3
"""A minimal ACP agent for tests: newline-delimited JSON-RPC over stdio.

Prompts:
  perm   asks for permission to edit a file (with a diff) and replies with
         the chosen option id
  ask    asks for permission with only raw input (like Qwen's questions)
  think  sends reasoning, then the answer "done"
  caps   replies the client's elicitation capability, as JSON
  form   sends an elicitation/create form and replies the result, as JSON
  qwen   asks a question the way Qwen Code does and replies the raw result
  opts   replies the session's config option values, as JSON
  bump   changes `effort` to "high" itself and notifies the client
  md     replies markdown, with the markers cut across chunks
  blocks replies the text of the blocks sent before it, as JSON

The prompt is the text of the last block; earlier blocks are context.
  slow   replies "working" and waits for session/cancel
  stuck  replies "working", ignores the first session/cancel and ends on the
         second, like an agent blocked in an API call
  stream sends a text chunk every 20 ms until session/cancel
  fail   answers the prompt with an error, like Qwen's loop protection
  die    prints "boom" on stderr and exits with status 1, without answering
  env X  replies the value of the environment variable X
  warn   prints "fake warning" on stderr and replies "warned"
  tools  starts two tool calls, renames the first, completes it, fails the
         second, sends a plan and a usage update, then replies "done"
  later  replies "started", ends the turn and, half a second later, like
         a background task waking the agent up: starts a tool call and
         writes "background done" and an unfinished line "all good"
  later perm  the same, but asks for permission to edit a file then, and
         writes "chose:<option id>"
  bg     like Claude: titles the session "Fake background work", starts a
         background task (only for clients that ask for them, with JetBrains'
         AIR extension), replies "started" and ends the turn. The task ends
         3 seconds later.
  other  replies "[<session>|<mode>|<cwd>] <text>"

With --no-auto the sessions do not offer the `auto` mode. With --kilo, like
Kilo Code: no session modes, and the mode is a `mode` config option.

Sessions offer config options `effort` (low/high), `fast` (boolean) and
`model` (one/two).
"""
import json
import os
import select
import sys
import time

modes = ["default", "plan"] if "--no-auto" in sys.argv else ["default", "plan", "auto"]
kilo = "--kilo" in sys.argv
sessions = {}  # session id -> {"cwd": ..., "mode": ...}
client_capabilities = {}
next_request_id = 1000
later = []  # (when, action): what the agent does after its turn ended


def send(message):
    message["jsonrpc"] = "2.0"
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def receive():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


def config_options(session):
    def select(option_id, values):
        current = session["mode"] if option_id == "mode" else session["options"][option_id]
        return {"id": option_id, "name": option_id.title(), "type": "select",
                "currentValue": current,
                "options": [{"value": v, "name": v} for v in values]}
    options = [select("effort", ["low", "high"]), select("model", ["one", "two"]),
               {"id": "fast", "name": "Fast", "type": "boolean",
                "currentValue": session["options"]["fast"]}]
    if kilo:
        options.append(select("mode", ["code", "ask"]))
    return options


def say(session_id, text):
    send({"method": "session/update", "params": {"sessionId": session_id, "update": {
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": text}}}})


def ask_client(method, params):
    """Sends a request to the client and returns its result."""
    global next_request_id
    next_request_id += 1
    send({"id": next_request_id, "method": method, "params": params})
    while True:
        message = receive()
        if message.get("id") == next_request_id and "method" not in message:
            return message["result"]


def ask_permission(session_id, tool_call):
    outcome = ask_client("session/request_permission", {
        "sessionId": session_id,
        "toolCall": tool_call,
        "options": [
            {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
        ]})["outcome"]
    return outcome.get("optionId", outcome["outcome"])


def prompt(request):
    params = request["params"]
    session_id = params["sessionId"]
    text = params["prompt"][-1]["text"]
    session = sessions[session_id]
    stop_reason = "end_turn"

    if text == "perm":
        tool_call = {"toolCallId": "t1", "title": "Writing to notes.txt", "kind": "edit",
                     "content": [{"type": "diff", "path": "/tmp/notes.txt",
                                  "oldText": "a\n", "newText": "a\nb\n"}]}
        say(session_id, "chose:" + ask_permission(session_id, tool_call))
    elif text == "ask":
        tool_call = {"toolCallId": "t2", "title": "Ask user 1 question", "content": [],
                     "rawInput": {"questions": [{"question": "Which color?"}]}}
        say(session_id, "chose:" + ask_permission(session_id, tool_call))
    elif text == "caps":
        say(session_id, json.dumps(client_capabilities.get("elicitation")))
    elif text == "form":
        result = ask_client("elicitation/create", {
            "sessionId": session_id, "mode": "form", "message": "Pick a color",
            "requestedSchema": {"type": "object", "properties": {
                "color": {"type": "string", "title": "Color",
                          "oneOf": [{"const": "red", "title": "Red"},
                                    {"const": "blue", "title": "Blue"}]}}}})
        say(session_id, json.dumps(result, sort_keys=True))
    elif text == "qwen":
        questions = [{"question": "Which color?", "header": "Color",
                      "options": [{"label": "Red"}, {"label": "Blue"}]}]
        result = ask_client("session/request_permission", {
            "sessionId": session_id,
            "toolCall": {"toolCallId": "t3", "title": "Ask user 1 question", "content": [],
                         "rawInput": {"questions": questions},
                         "_meta": {"qwenInteractionKind": "user_question",
                                   "qwenQuestions": questions}},
            "options": [{"optionId": "proceed_once", "name": "Submit", "kind": "allow_once"},
                        {"optionId": "cancel", "name": "Cancel", "kind": "reject_once"}]})
        say(session_id, json.dumps(result, sort_keys=True))
    elif text == "opts":
        say(session_id, json.dumps(
            {**session["options"], "mode": session["mode"]}, sort_keys=True))
    elif text == "blocks":
        say(session_id, json.dumps([block.get("text") for block in params["prompt"][:-1]]))
    elif text == "md":
        for chunk in ["- **bo", "ld** and `co", "de`\n", "## Ti", "tle\n"]:
            say(session_id, chunk)
    elif text == "bump":
        session["options"]["effort"] = "high"
        send({"method": "session/update", "params": {"sessionId": session_id, "update": {
            "sessionUpdate": "config_option_update",
            "configOptions": config_options(session)}}})
        say(session_id, "bumped")
    elif text == "think":
        for chunk in ["Let me ", "think.\n", "Almost there"]:
            send({"method": "session/update", "params": {"sessionId": session_id, "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": chunk}}}})
        say(session_id, "done")
    elif text == "fail":
        send({"id": request["id"], "error": {
            "code": -32603, "message": "Tool-call loop protection stopped this turn.",
            "data": {"code": "LOOP_DETECTED"}}})
        return
    elif text == "die":
        print("boom", file=sys.stderr, flush=True)
        sys.exit(1)
    elif text == "tools":
        def update(fields):
            send({"method": "session/update", "params": {"sessionId": session_id,
                                                          "update": fields}})
        update({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read a",
                "kind": "read", "locations": [{"path": "/tmp/a"}]})
        update({"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "Read b"})
        update({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "title": "Read a.rs"})
        update({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "status": "completed"})
        update({"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "failed"})
        update({"sessionUpdate": "plan", "entries": [
            {"content": "Read", "priority": "high", "status": "completed"},
            {"content": "Write tests", "priority": "high", "status": "in_progress"},
            {"content": "Ship", "priority": "low", "status": "pending"}]})
        update({"sessionUpdate": "usage_update", "used": 10, "size": 100})
        say(session_id, "done")
    elif text == "warn":
        print("fake warning", file=sys.stderr, flush=True)
        say(session_id, "warned")
    elif text in ("later", "later perm"):
        say(session_id, "started")

        def wake_up():
            send({"method": "session/update", "params": {"sessionId": session_id, "update": {
                "sessionUpdate": "tool_call", "toolCallId": "b1", "title": "Read log"}}})
            if text == "later perm":
                tool_call = {"toolCallId": "b2", "title": "Writing to notes.txt",
                             "kind": "edit", "content": []}
                say(session_id, "chose:" + ask_permission(session_id, tool_call) + "\n")
            else:
                say(session_id, "background ")
                say(session_id, "done\nall good")
        later.append((time.time() + 0.5, wake_up))
    elif text == "bg":
        def update(fields):
            send({"method": "session/update", "params": {"sessionId": session_id,
                                                          "update": fields}})
        update({"sessionUpdate": "session_info_update", "title": "Fake background work"})
        air = client_capabilities.get("_meta", {}).get("jetbrains", {}).get("air", {})
        if "asyncTasks" in air.get("capabilities", []):
            update({"sessionUpdate": "async_task_spawned", "asyncTaskId": "b9",
                    "name": "sleep 3", "taskType": "local_bash", "canStop": True})
            later.append((time.time() + 3, lambda: update({
                "sessionUpdate": "async_task_state_update", "asyncTaskId": "b9",
                "state": "completed"})))
        say(session_id, "started")
    elif text.startswith("env "):
        say(session_id, os.environ.get(text[4:], "<unset>"))
    elif text == "stuck":
        say(session_id, "working")
        cancels = 0
        while cancels < 2:
            if receive().get("method") == "session/cancel":
                cancels += 1
        stop_reason = "cancelled"
    elif text == "stream":
        while True:
            say(session_id, ".")
            ready, _, _ = select.select([sys.stdin], [], [], 0.02)
            if ready and receive().get("method") == "session/cancel":
                stop_reason = "cancelled"
                break
    elif text == "slow":
        say(session_id, "working")
        while True:
            message = receive()
            if message.get("method") == "session/cancel":
                stop_reason = "cancelled"
                break
    else:
        send({"method": "session/update", "params": {"sessionId": session_id, "update": {
            "sessionUpdate": "tool_call", "toolCallId": "t0", "title": "Read README.md"}}})
        say(session_id, f"[{session_id}|{session['mode']}|{session['cwd']}] ")
        say(session_id, text)

    send({"id": request["id"], "result": {"stopReason": stop_reason}})


def main():
    while True:
        if later:
            when, action = later[0]
            ready, _, _ = select.select([sys.stdin], [], [], max(0, when - time.time()))
            if not ready:
                later.pop(0)
                action()
                continue
        message = receive()
        method = message.get("method")
        if method == "initialize":
            client_capabilities.update(message["params"].get("clientCapabilities", {}))
            send({"id": message["id"], "result": {
                "protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}})
        elif method == "session/new":
            session_id = f"s{len(sessions) + 1}"
            session = sessions[session_id] = {
                "cwd": message["params"]["cwd"], "mode": "code" if kilo else "default",
                "options": {"effort": "high", "model": "one", "fast": False}}
            result = {"sessionId": session_id, "configOptions": config_options(session)}
            if not kilo:
                result["modes"] = {"currentModeId": "default",
                                   "availableModes": [{"id": m, "name": m} for m in modes]}
            send({"id": message["id"], "result": result})
        elif method == "session/set_config_option":
            params = message["params"]
            session = sessions[params["sessionId"]]
            option = next((o for o in config_options(session)
                           if o["id"] == params["configId"]), None)
            value = params["value"]
            allowed = [v["value"] for v in option.get("options", [])] if option else []
            if option is None or (option["type"] == "select" and value not in allowed):
                send({"id": message["id"], "error": {"code": -32602, "message": "invalid"}})
                continue
            if params["configId"] == "mode":
                session["mode"] = value
            else:
                session["options"][params["configId"]] = value
            send({"id": message["id"], "result": {"configOptions": config_options(session)}})
        elif method == "session/set_mode":
            params = message["params"]
            sessions[params["sessionId"]]["mode"] = params["modeId"]
            send({"id": message["id"], "result": {}})
        elif method == "session/prompt":
            prompt(message)
        elif "id" in message:
            send({"id": message["id"], "error": {"code": -32601, "message": "not supported"}})


main()
