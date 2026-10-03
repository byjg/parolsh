#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["pyte", "pillow"]
# ///
"""Captures docs/images/shell-mode.png: a real Parolsh session.

Runs Parolsh on a pseudo-terminal, emulated with pyte, inside a throwaway
git repository at ~/projects/wallet (a temporary HOME), with the real
`claude-agent-acp`. Types the lines in SESSION, waits for each to finish,
and renders the screen with headless Chrome.

Usage: scripts/screenshot.py [--agent COMMAND] [--out PATH] [--parolsh BINARY]

Without --parolsh, this tree is built: unless it is a clean checkout of the
release's tag, its banner says `-dev`. Give a build of the tag to show the
release's version.

Needs uv, cargo, git, Google Chrome and a logged-in agent. The agent keeps
your real HOME, so its login works; everything else sees the temporary one.
"""
import argparse
import fcntl
import html
import os
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
COLUMNS, ROWS = 100, 60

# What is typed, in order. Each line waits for the prompt to come back.
SESSION = [
    "!",
    "git status --short",
    "!+git diff",
    "?what did I change? Two short sentences.",
    "?",
]

BEFORE = '''def charge(card, amount):
    return gateway.charge(card, amount)
'''

AFTER = '''def charge(card, amount, retries=3):
    for attempt in range(retries):
        try:
            return gateway.charge(card, amount)
        except TimeoutError:
            if attempt == retries - 1:
                raise
'''

# Catppuccin Mocha.
PALETTE = {
    "default": "#cdd6f4", "black": "#45475a", "red": "#f38ba8",
    "green": "#a6e3a1", "brown": "#f9e2af", "yellow": "#f9e2af",
    "blue": "#89b4fa", "magenta": "#cba6f7", "cyan": "#94e2d5",
    "white": "#bac2de", "dim": "#7f849c",
}
BACKGROUND = "#1e1e2e"
PROMPT = re.compile(r"[✦❯] $")


class Terminal(pyte.Screen):
    """A pyte screen that answers the terminal's queries (cursor position)
    on the pseudo-terminal, and shows faint text (SGR 2) as dim."""

    def __init__(self, fd):
        super().__init__(COLUMNS, ROWS)
        self.fd = fd

    def write_process_input(self, data):
        os.write(self.fd, data.encode())

    def select_graphic_rendition(self, *attrs, **kwargs):
        # In an extended color (38 or 48: `38;2;R;G;B`, `38;5;N`) the numbers
        # are not attributes: a 2 there is not "faint".
        if 38 in attrs or 48 in attrs:
            return super().select_graphic_rendition(*attrs, **kwargs)
        dim = 2 in attrs
        attrs = tuple(a for a in attrs if a != 2)
        if attrs or not dim:
            super().select_graphic_rendition(*attrs, **kwargs)
        if dim:
            self.cursor.attrs = self.cursor.attrs._replace(fg="dim")


def make_repo(home):
    repo = home / "projects" / "wallet"
    (repo / "src").mkdir(parents=True)
    # Otherwise Ubuntu's /etc/bash.bashrc prints a sudo tip in a new HOME.
    (home / ".sudo_as_admin_successful").touch()
    git = ["git", "-c", "user.name=Demo", "-c", "user.email=demo@example.com"]
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repo, check=True)
    (repo / "src" / "payments.py").write_text(BEFORE)
    subprocess.run(git + ["add", "."], cwd=repo, check=True)
    subprocess.run(git + ["commit", "-q", "-m", "Charge cards"], cwd=repo, check=True)
    (repo / "src" / "payments.py").write_text(AFTER)
    return repo


def write_config(config_home, agent):
    (config_home / "parolsh").mkdir(parents=True)
    (config_home / "parolsh" / "config.toml").write_text(f'''
shell_env = "never"
default_agent = "claude"

[agents.claude]
command = "{agent}"
mode = "auto"
env = {{ HOME = "{os.environ["HOME"]}" }}
''')


def run_session(parolsh, repo, env):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(repo)
        os.execve(parolsh, [parolsh], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLUMNS, 0, 0))
    screen = Terminal(fd)
    stream = pyte.ByteStream(screen)

    def pump(until, timeout, quiet):
        """Reads until `until()` holds and nothing arrived for `quiet` s."""
        end = time.time() + timeout
        last = time.time()
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                stream.feed(os.read(fd, 65536))
                last = time.time()
            elif time.time() - last > quiet and until():
                return
        sys.exit("timed out; the screen was:\n" + "\n".join(screen.display).rstrip())

    def at_prompt():
        row = screen.display[screen.cursor.y][: screen.cursor.x]
        return bool(PROMPT.search(row))

    pump(at_prompt, 30, 1.0)
    for line in SESSION:
        os.write(fd, line.encode())
        pump(lambda: True, 5, 0.3)
        os.write(fd, b"\r")
        pump(at_prompt, 180, 1.5)
    os.kill(pid, 9)
    return screen


def to_html(screen):
    rows = [screen.buffer[y] for y in range(screen.lines)]
    last = max(y for y in range(screen.lines) if screen.display[y].strip() or y == screen.cursor.y)
    lines = []
    for y in range(last + 1):
        spans, run, style = [], "", None
        for x in range(screen.columns):
            char = rows[y][x]
            cursor = (x, y) == (screen.cursor.x, screen.cursor.y)
            fg = char.fg if char.fg in PALETTE else (f"#{char.fg}" if re.fullmatch(r"[0-9a-f]{6}", char.fg) else "default")
            color = PALETTE.get(fg, fg)
            css = f"color:{color}"
            if char.bold:
                css += ";font-weight:bold"
            if char.underscore:
                css += ";text-decoration:underline"
            if cursor:
                css += f";background:{PALETTE['default']};color:{BACKGROUND}"
            if css != style:
                if run:
                    spans.append(f'<span style="{style}">{html.escape(run)}</span>')
                run, style = "", css
            run += char.data or " "
        spans.append(f'<span style="{style}">{html.escape(run)}</span>')
        lines.append("".join(spans))
    body = "\n".join(lines)
    return f'''<!doctype html>
<meta charset="utf-8">
<style>
  html, body {{ margin: 0; background: transparent; }}
  .window {{ display: inline-block; margin: 24px; border-radius: 10px;
    background: {BACKGROUND}; box-shadow: 0 10px 30px rgba(0,0,0,.45); }}
  .bar {{ padding: 12px 14px 0; }}
  .bar i {{ display: inline-block; width: 12px; height: 12px; margin-right: 6px;
    border-radius: 50%; }}
  pre {{ margin: 0; padding: 12px 18px 18px; color: {PALETTE["default"]};
    font: 14px/1.3 "JetBrains Mono", "Fira Code", "FiraCode Nerd Font Mono",
    "DejaVu Sans Mono", monospace;
    font-variant-ligatures: none; font-feature-settings: "liga" 0, "calt" 0; }}
</style>
<div class="window">
  <div class="bar"><i style="background:#f38ba8"></i><i style="background:#f9e2af"></i><i style="background:#a6e3a1"></i></div>
  <pre>{body}</pre>
</div>
'''


def render(page, out):
    """Screenshots `page` with headless Chrome and crops it to the window."""
    raw = page.with_suffix(".png")
    subprocess.run([
        "google-chrome", "--headless=new", "--disable-gpu", "--hide-scrollbars",
        "--force-device-scale-factor=2", "--default-background-color=00000000",
        "--window-size=1100,1400", f"--screenshot={raw}", page.as_uri(),
    ], check=True, capture_output=True)
    image = Image.open(raw)
    left, top, right, bottom = image.getbbox()
    margin = 20
    image.crop((max(left - margin, 0), max(top - margin, 0), right + margin, bottom + margin)).save(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--agent", default="claude-agent-acp", help="ACP agent command")
    parser.add_argument("--out", default=ROOT / "docs" / "images" / "shell-mode.png", type=Path)
    parser.add_argument("--parolsh", type=Path, help="the binary to run, instead of building this tree")
    args = parser.parse_args()

    if args.parolsh:
        parolsh = str(args.parolsh.resolve())
    else:
        subprocess.run(["cargo", "build", "--quiet"], cwd=ROOT, check=True)
        parolsh = str(ROOT / "target" / "debug" / "parolsh")
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        home = tmp / "home"
        repo = make_repo(home)
        write_config(tmp / "config", args.agent)
        env = {
            "HOME": str(home), "PATH": os.environ["PATH"], "TERM": "xterm-256color",
            # 24-bit colors, for the logo's gradient.
            "COLORTERM": "truecolor",
            "LANG": "C.UTF-8", "XDG_CONFIG_HOME": str(tmp / "config"),
            "XDG_STATE_HOME": str(tmp / "state"),
        }
        screen = run_session(parolsh, repo, env)
        page = tmp / "screen.html"
        page.write_text(to_html(screen))
        args.out.parent.mkdir(parents=True, exist_ok=True)
        render(page, args.out)
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
