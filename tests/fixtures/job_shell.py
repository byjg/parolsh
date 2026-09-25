#!/usr/bin/env python3
"""Types lines into an interactive bash running on a pseudo-terminal.

bash has job control there, so a program started from it runs as a job, like
Parolsh started from a user's terminal. Prints what the terminal showed,
without escape sequences.

Usage: job_shell.py LINE...
"""
import os
import pty
import re
import select
import sys
import time

pid, fd = pty.fork()
if pid == 0:
    os.execvp("bash", ["bash", "--norc", "--noprofile", "-i"])

output = b""


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
        # Answer cursor-position queries like a terminal emulator would.
        for _ in range(data.count(b"\x1b[6n")):
            os.write(fd, b"\x1b[1;1R")


pump(0.5)
for line in sys.argv[1:]:
    os.write(fd, line.encode() + b"\r")
    pump(1.5)
os.kill(pid, 9)

text = output.decode(errors="replace").replace("\r", "")
print(re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[=>78]", "", text))
