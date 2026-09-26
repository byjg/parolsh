#!/usr/bin/env python3
"""A minimal ACP agent for tests: newline-delimited JSON-RPC over stdio.

Prompts:
  perm   asks for permission to edit a file (with a diff) and replies with
         the chosen option id
  ask    asks for permission with only raw input (like Qwen's questions)
  think  sends reasoning, then the answer "done"
  slow   replies "working" and waits for session/cancel
  env X  replies the value of the environment variable X
  other  replies "[<session>|<mode>|<cwd>] <text>"

With --no-auto the sessions do not offer the `auto` mode.
"""
import json
import os
import sys

modes = ["default", "plan"] if "--no-auto" in sys.argv else ["default", "plan", "auto"]
sessions = {}  # session id -> {"cwd": ..., "mode": ...}
next_request_id = 1000


def send(message):
    message["jsonrpc"] = "2.0"
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def receive():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


def say(session_id, text):
    send({"method": "session/update", "params": {"sessionId": session_id, "update": {
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": text}}}})


def ask_permission(session_id, tool_call):
    global next_request_id
    next_request_id += 1
    send({"id": next_request_id, "method": "session/request_permission", "params": {
        "sessionId": session_id,
        "toolCall": tool_call,
        "options": [
            {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
        ]}})
    while True:
        message = receive()
        if message.get("id") == next_request_id and "method" not in message:
            outcome = message["result"]["outcome"]
            return outcome.get("optionId", outcome["outcome"])


def prompt(request):
    params = request["params"]
    session_id = params["sessionId"]
    text = params["prompt"][0]["text"]
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
    elif text == "think":
        for chunk in ["Let me ", "think.\n", "Almost there"]:
            send({"method": "session/update", "params": {"sessionId": session_id, "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": chunk}}}})
        say(session_id, "done")
    elif text.startswith("env "):
        say(session_id, os.environ.get(text[4:], "<unset>"))
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
        message = receive()
        method = message.get("method")
        if method == "initialize":
            send({"id": message["id"], "result": {
                "protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}})
        elif method == "session/new":
            session_id = f"s{len(sessions) + 1}"
            sessions[session_id] = {"cwd": message["params"]["cwd"], "mode": "default"}
            send({"id": message["id"], "result": {"sessionId": session_id, "modes": {
                "currentModeId": "default",
                "availableModes": [{"id": m, "name": m} for m in modes]}}})
        elif method == "session/set_mode":
            params = message["params"]
            sessions[params["sessionId"]]["mode"] = params["modeId"]
            send({"id": message["id"], "result": {}})
        elif method == "session/prompt":
            prompt(message)
        elif "id" in message:
            send({"id": message["id"], "error": {"code": -32601, "message": "not supported"}})


main()
