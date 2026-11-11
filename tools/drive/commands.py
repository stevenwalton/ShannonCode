"""Run every `:` command the manual names and report what the user sees.

The class this hunts is a command that does nothing and says nothing -- a dead
command reads as a broken one. A refusal is a PASS: it told you why.
"""
import sys, os, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from term import start

_T = tempfile.mkdtemp(prefix="shannon-cmd-")
_ED = os.path.join(_T, "e.sh")
with open(_ED, "w") as f:
    f.write('#!/bin/sh\ncp "$1" %s\nexit 0\n' % os.path.join(_T, "view.txt"))
os.chmod(_ED, 0o755)
ENV = {"EDITOR": _ED, "VISUAL": _ED}

# Commands that legitimately end the session; driven last, one per session.
EXITS = ["quit", "q", "qall", "quitall", "qa", "x", "wq"]

CMDS = [
    "help", "help window", "keymap", "keymap normal", "keymap bogus",
    "graph", "messages", "mes", "analyze ls -la", "analyze", "config",
    "source", "so /nonexistent.toml", "reload", "rel", "scriptnames", "scr",
    "unsource", "uns /nonexistent.toml",
    "set", "set all", "set tui.scrolloff=3", "se tui.scrolloff?",
    "set tui.prompt.height=15", "set bogus=1", "set secrets.extra=x",
    "verbose set tui.scrolloff?", "verb", "verbose help",
    "model", "model opus", "model nonesuch", "secrets", "secret",
    "new", "clone", "open", "open deadbeef", "continue", "reply",
    "child", "child do a thing", "delete", "hide", "tool",
    "split", "vs", "sg", "vsg", "only", "on", "w", "write", "wall",
    "bogusdoesnotexist",
]

def row(s):
    return s.screen.text().split("\n")[-1].strip()

def run():
    s = start(env=ENV)
    base = row(s)
    for c in CMDS:
        if not s.alive():
            s = start(env=ENV)
        before = s.screen.text()
        s.send(":" + c + "\r", settle=1.6)
        after = s.screen.text()
        said = row(s)
        changed = before != after
        # A command "answered" if it changed the screen or left a message that
        # is not just the ambient status row.
        spoke = "[NORMAL]" not in said and said != ""
        verdict = "spoke " if spoke else ("drew  " if changed else "SILENT")
        print("%-8s :%-22s %s" % (verdict, c, said[:78] if spoke else ""))
        # get back to a clean prompt
        s.send("\x1b", settle=0.2)
        s.send("q", settle=0.4)
        s.send("\x1b", settle=0.2)
    s.close()

    print()
    for c in EXITS:
        s = start(env=ENV)
        s.send(":" + c + "\r", settle=0.3)
        gone = s.wait_exit(4.0)
        print("%-8s :%s" % ("exits" if gone else "STAYS", c))
        if not gone:
            print("         last row: %s" % row(s))
        s.close()

run()
