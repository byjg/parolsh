#!/usr/bin/env python3
"""Types lines into an interactive bash running on a pseudo-terminal.

bash has job control there, so a program started from it runs as a job, like
Parolsh started from a user's terminal. Prints what the terminal showed,
without escape sequences.

Usage: job_shell.py LINE...
       job_shell.py exec:PROGRAM LINE...

With exec:PROGRAM, PROGRAM runs directly on the pseudo-terminal instead of
bash, like a program started by a desktop launcher (its parent is not a
shell).

A LINE starting with "paste:" is pasted instead of typed: "\\n" becomes a line
break, and, like a terminal, the paste is wrapped in bracketed-paste markers
only if the program turned bracketed paste on. No Enter is added.
"""
import os
import pty
import re
import select
import sys
import time

args = sys.argv[1:]
direct = args[0][len("exec:"):] if args and args[0].startswith("exec:") else None
if direct:
    args = args[1:]

pid, fd = pty.fork()
if pid == 0:
    if direct:
        os.execv(direct, [direct])
    os.execvp("bash", ["bash", "--norc", "--noprofile", "-i"])

output = b""
bracketed = False


def pump(seconds):
    global output
    end = time.time() + seconds
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if not ready:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            return
        output += data
        global bracketed
        if b"\x1b[?2004h" in data:
            bracketed = True
        if b"\x1b[?2004l" in data:
            bracketed = False
        # Answer cursor-position queries like a terminal emulator would.
        for _ in range(data.count(b"\x1b[6n")):
            os.write(fd, b"\x1b[1;1R")


pump(0.5)
for line in args:
    if line.startswith("paste:"):
        text = line[len("paste:"):].replace("\\n", "\n").encode()
        if bracketed:
            text = b"\x1b[200~" + text + b"\x1b[201~"
        os.write(fd, text)
    else:
        os.write(fd, line.encode() + b"\r")
    pump(1.5)
os.kill(pid, 9)

text = output.decode(errors="replace").replace("\r", "")
print(re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[=>78]", "", text))
