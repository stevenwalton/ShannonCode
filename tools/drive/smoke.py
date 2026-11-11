import base64, re, sys, os, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from term import start, plant_conversation, _HOME

# A stand-in $EDITOR: it copies the file it is handed somewhere we can read and
# exits at once. Without this, `:keymap` and `:config edit` open the real vim on
# the pty and every later keystroke goes to *it*.
_TMP = tempfile.mkdtemp(prefix="shannon-drive-")
_SEEN = os.path.join(_TMP, "last-view.txt")
_ED = os.path.join(_TMP, "editor.sh")
with open(_ED, "w") as f:
    f.write('#!/bin/sh\ncp "$1" %s\nexit 0\n' % _SEEN)
os.chmod(_ED, 0o755)
ED = {"EDITOR": _ED, "VISUAL": _ED}

def view():
    try:
        return open(_SEEN).read()
    except OSError:
        return ""

results = []
def check(name, cond, screen=None):
    results.append((name, bool(cond), screen))

def panes(session):
    """How many pane boxes are drawn. A vertical split puts two on one ROW, so
    count border corners, not lines — the first version counted lines and a
    vsplit read as no split at all."""
    return session.repaint().count("┌")

def wait_for(session, needle, seconds=25.0):
    """Poll until `needle` is on screen. The stub streams 4 chars per 60 ms, so
    its reply takes ~15 s — a fixed settle asserted against a screen that had
    no reply on it yet, and passed anyway (Reviewer C)."""
    import time
    end = time.time() + seconds
    while time.time() < end:
        if needle in session.screen.text():
            return True
        session.read(0.4)
    return False

# --- 1. typing, Esc, normal-mode edit ------------------------------------
s = start(env=ED)
s.send('i'); s.send('hello world')
scr = s.repaint()
check("insert: text reaches the prompt", "hello world" in scr, scr)
check("insert: status says INSERT", "[INSERT]" in scr, scr)
s.send('\x1b')
scr = s.repaint()
check("Esc: status says NORMAL", "[NORMAL]" in scr, scr)
s.send('0'); s.send('dw')
scr = s.repaint()
check("normal: dw deletes a word", "world" in scr and "hello world" not in scr, scr)
s.send('u')
scr = s.repaint()
check("normal: u undoes it", "hello world" in scr, scr)

# --- 2. send a turn (stub backend) ---------------------------------------
s.send('\r', settle=1.0)
# "hello world" is the echoed user turn, so matching it proved only that the
# send happened. The stub's own words are the reply.
arrived = wait_for(s, "canned stub")
scr = s.repaint()
check("send: a reply lands in the transcript", arrived, scr)
body = scr.split("┌ prompt")[-1]
check("send: the prompt empties", "hello world" not in body, scr)

# --- 2b. marks: m{a-z} and '{a-z} in the transcript ------------------------
# The key -> app wiring no cargo test reaches (`App::run`): setting one says
# its turn, going to one not set says so, going to one set says nothing. Each
# notice is read before any repaint (<C-l> clears it).
s.send('\x17'); s.send('k', settle=0.5)
row = s.screen.text().split("\n")[-1]
check("marks: <C-w>k reaches the transcript", "output" in row, row)
# On your words' line, found by search (a context line above them would move
# a counted `j`).
s.send('gg'); s.send('/hello world\r', settle=0.3)
s.send('ma', settle=0.3)
said = s.screen.text()
check("marks: m sets one and says its turn", "mark `a` — turn 1" in said, said)
s.send("'b", settle=0.3)
said = s.screen.text()
check("marks: ' to one not set says so", "mark `b` is not set" in said, said)
s.send('G'); s.send("'a", settle=0.3)
said = s.screen.text()
check("marks: ' to one set goes, unrefused", "is not set" not in said and s.alive(), said)
# The agent's are apart: `a is its, and the stub sets none.
s.send("`a", settle=0.3)
said = s.screen.text()
check("marks: ` is the agent's, apart from yours", "the agent has set no mark `a`" in said, said)
# And where `'a` went, copied and pasted into the prompt: "unrefused" alone
# passed on a `'a` that did nothing, `G`'s keypress having cleared the notice
# and the transcript too short for the screen to differ (review stop 14, C 8).
# A `'a` that moved nothing copies the last line instead.
s.send('yy', settle=0.3)
s.send('\x17'); s.send('j', settle=0.5)
s.send('\x1b'); s.send('p', settle=0.5)
scr = s.repaint()
body = scr.split("┌ prompt")[-1]
check("marks: ' landed on the marked line (yanked, pasted)", "> hello world" in body, scr)
s.send('u', settle=0.3)
s.send(':marks\r', settle=0.8)
scr = s.repaint()
check("marks: :marks lists yours, a table", "mark  turn" in scr and "'a    1" in scr, scr)
s.send(':q\r', settle=0.5)
s.close()

# --- 3. :help ------------------------------------------------------------
s = start(env=ED)
s.send(':help\r', settle=1.2)
scr = s.repaint()
# The manual's own top tag. These checks used to look for "NAME", which the
# manual stopped showing on its first screen at some point — so "q does NOT
# close it" FAILED on a manual that was correctly still open, and ":q closes it"
# asserted `"NAME" not in scr` and so PASSED whatever `:q` did (2026-09-24).
HELP_MARK = "*help*"
check("help: the manual opens", HELP_MARK in scr, scr)
before = scr
s.send('G')
scr = s.repaint()
# Away from the top, where the mark is — any change of screen passed
# (review stop 12, T7).
check("help: G moves to the end", scr != before and HELP_MARK not in scr, scr)
s.send('gg')
scr2 = s.repaint()
check("help: gg comes back", scr2 != scr and HELP_MARK in scr2, scr2)
# **Do not repaint before reading a notice** (the README's own rule, which
# this check used to break): <C-l> is a keypress and clears it. And match the
# words unique to the refusal — "read-only" was ALSO this pane's status row
# (until 10-30, S14-Q1), so the old condition was true before `i` was ever
# pressed (Reviewer C), and any format of yours carrying $readonly still is.
s.send('Z')
quiet = "type in a prompt pane" not in s.screen.text()
s.send('i')
check("help: `i` is refused out loud",
      "type in a prompt pane" in s.screen.text() and quiet, s.screen.text())
# A viewer is a WINDOW: no bare key closes it, because vim binds none (its
# help ftplugin maps nothing, and netrw keeps `q` as a prefix). Both halves are
# driven — "q stopped closing" is only half a feature, and a pane that does not
# close on `:q` either is a trap.
s.send('gg')  # back to the top, where the mark lives
s.send('q')
scr = s.repaint()
check("help: q does NOT close it", HELP_MARK in scr, scr)
s.send(':q\r', settle=0.8)
scr = s.repaint()
# (`"prompt" in scr` beside it was always true: a row says it.)
check("help: :q closes it", HELP_MARK not in scr, scr)

# --- 3b. a paste into the `:` line ---------------------------------------
# The event loop's paste arm is the one piece no cargo test reaches
# (`App::paste` is tested; the routing into it is not). Steven typed a regex
# rule by hand because the `:` line took no paste (2026-10-03). Read the
# notice before any repaint: <C-l> would clear it.
s.send(':')
s.send('\x1b[200~deny re:^rm(\\s|$)\x1b[201~')
s.send('\r', settle=0.8)
said = s.screen.text()
check("paste: a pasted rule reaches the `:` line",
      "re:^rm(\\s|$)" in said and "for this session" in said, said)

# --- 4. windows ----------------------------------------------------------
before = panes(s)
s.send('\x17'); s.send('s', settle=0.8)
check("wm: <C-w>s splits", panes(s) == before + 1, s.repaint())
s.send('\x17'); s.send('v', settle=0.8)
check("wm: <C-w>v vsplits", panes(s) == before + 2, s.repaint())

def focus(session):
    import re
    m = re.search(r'\[\w+\]\s+(\S+)', session.screen.text().split("\n")[-1])
    return m.group(1) if m else "?"

was = focus(s)
seen = {was}
for _ in range(3):
    s.send('\x17'); s.send('w', settle=0.5)
    seen.add(focus(s))
check("wm: <C-w>w moves focus", len(seen) > 1, s.repaint())

# **Closes, or says why.** Not a bare pane count: where the cycle above leaves
# focus decides whether this is a close or a refusal ("this is the only prompt
# — closing it would leave nowhere to type"), and BOTH are correct. What must
# never happen is a silent no-op.
split = panes(s)
s.send('\x17'); s.send('q', settle=0.8)
said = s.screen.text().split("\n")[-1].strip()
refused = "[NORMAL]" not in said and said != ""
check("wm: <C-w>q closes a pane or says why",
      panes(s) == split - 1 or refused, s.repaint())
s.send('\x17'); s.send('o', settle=0.8)

# --- 5. :keymap ----------------------------------------------------------
import os
try: os.remove(_SEEN)
except OSError: pass
s.send(':keymap\r', settle=2.5)
v = view()
check("keymap: it lists bindings", "normal" in v.lower() and "insert" in v.lower(), v[:1500])
# **A fresh frame, not the one from before.** `Screen` blanks only on an ED and
# a dead pty reads nothing, so "prompt" -- on screen since before `:keymap` --
# passed an app that died or hung in `$EDITOR` (review stop 12, T7 6). Forget
# the grid, then ask for a frame, and for a key's answer: `i` is Insert in a
# prompt and refused out loud elsewhere -- where `<C-w>o` left focus is the
# cycle's business -- and either is the keymap answering. The notice is read
# before any repaint (<C-l> clears it).
s.screen.resize(s.cols, s.rows)
scr = s.repaint()
check("keymap: the TUI comes back after $EDITOR", s.alive() and "prompt" in scr, scr)
s.send('i')
said = s.screen.text()
check("keymap: keys reach it after $EDITOR",
      "[INSERT]" in said or "type in a prompt pane" in said, said)
s.send('\x1b')

# --- 6. :graph -----------------------------------------------------------
# One Claude Code conversation for this directory, in the run's own Claude
# Code home: the first `:graph` asks before reading it, and the question's
# keys are sticky — the event loop's routing, which no cargo test reaches.
plant_conversation("cc-smoke", "a planted prompt", "a planted reply")
s.send(':graph\r', settle=1.2)
scr = s.repaint()
# Its border's title: "graph" anywhere matched the splash screen's own
# `:graph<Enter>` line (review stop 12, T7).
check("graph: it opens", "┌ graph " in scr, scr)
check("graph: it asks before reading Claude Code's conversations",
      "read Claude Code's conversations" in scr and "a always" in scr, scr)
s.send('n', settle=0.8)
scr = s.repaint()
check("graph: n reads none, and the heading says how to ask",
      "not read — l asks" in scr and "cc-smoke" not in scr, scr)
s.send(':q\r', settle=0.8)

# --- 6a. :memories / :skills <Enter> --------------------------------------
# <Enter> on a shelf answered the event loop "quit": Shannon ended after every
# edit, and on a line naming no file (Steven, 10-06). No cargo test reaches
# the loop. A hermetic home: this repo's AGENTS.md is the one file, and no
# skill is found.
SHELF = "memories and skills"
s.send(':skills\r', settle=1.0)
scr = s.repaint()
check("shelf: :skills opens the screen", SHELF in scr and "[Skills" in scr, scr)
s.send('\r', settle=0.8)
scr = s.repaint()
check("shelf: Enter on an empty list says why, and the app stays",
      s.alive() and "nothing to edit" in scr, scr)
# Tab turns FORWARD — skills, then MCP — and BackTab back, to the memories
# (three lists since the MCP one: a lone Tab from skills landed on MCP, whose
# Enter says a server is Claude Code's, review 29).
s.send('\t', settle=0.5)
scr = s.repaint()
check("shelf: Tab turns to the MCP list", "[MCP" in scr, scr)
s.send('\x1b[Z\x1b[Z', settle=0.5)
scr = s.repaint()
check("shelf: BackTab turns back, to the memories", "[Memories" in scr, scr)
try: os.remove(_SEEN)
except OSError: pass
s.send('\r', settle=2.0)
scr = s.repaint()
check("shelf: Enter edits the file under the cursor", "Shannon Code" in view(), view()[:300])
# A fresh frame and a key's answer, as after `:keymap`'s editor: SHELF was on
# screen before `$EDITOR`, so it passed an app hung there (review stop 12,
# T7 13). The memories are the list in front; Tab turns to the skills.
s.screen.resize(s.cols, s.rows)
scr = s.repaint()
check("shelf: the app is back, the screen still open, after $EDITOR",
      s.alive() and SHELF in scr, scr)
s.send('\t', settle=0.5)
scr = s.repaint()
check("shelf: keys reach it after $EDITOR", "[Skills" in scr, scr)
s.send('\x1b', settle=0.6)
scr = s.repaint()
check("shelf: Esc stays (vim's way back to Normal)", s.alive() and SHELF in scr, scr)
s.send('q', settle=0.6)
scr = s.repaint()
check("shelf: q leaves it", s.alive() and SHELF not in scr, scr)

# --- 6b. resize, down and back up ----------------------------------------
# The drive sheet claimed a machine checked this. Nothing did — and it is the
# surface two of this round's fixes are about (the prompt-height ratchet and
# the cursor-querying repaint). Reviewer C caught the false claim.
def prompt_rows(session):
    lines = session.repaint().split("\n")
    top = next((i for i, l in enumerate(lines) if l.startswith("┌ prompt")), None)
    if top is None:
        return None
    for j in range(top + 1, len(lines)):
        if lines[j].startswith("└"):
            return j - top - 1
    return None

s.send('\x1b', settle=0.3)
tall = prompt_rows(s)
s.winsize(100, 10); s.read(0.8)
squeezed = prompt_rows(s)
s.winsize(100, 30); s.read(0.8)
restored = prompt_rows(s)
check("resize: a squeeze shrinks the prompt", squeezed is not None and tall is not None and squeezed < tall)
# Measured both times: `None == None` passed (review stop 12, T7).
check("resize: growing back restores it", restored is not None and restored == tall)
# A key's answer, not a frame from before: `[NORMAL]` was drawn by the last
# repaint, so it passed an app that stopped reading (review stop 12, T7 13).
# The grid is blanked, so whatever is on it after `i` was drawn after `i`.
# And it is ratatui's DIFF that answers: `[NORMAL]` -> `[INSERT]` keeps the
# brackets' cells, so only the word is written — `[INSERT]` never matched a
# live app (10-27, Steven's run: the row read `  INSERT`).
# What is pending is drained first, and what arrives with NO key is read
# onto the blank grid too, so "drawn after `i`" is measured, not argued: a
# blank read straight after `resize` could not fail (review stop 13, A 2).
# The answer is waited for, not given a fixed half second.
answered = lambda text: "INSERT" in text or "type in a prompt pane" in text
s.read(0.3)
s.screen.resize(s.cols, s.rows)
s.read(0.3)
blank = s.screen.text()
s.send('i', settle=0.1)
seen = s.wait_for(answered)
said = s.screen.text()
check("resize: the app is still answering keys",
      s.alive() and not answered(blank) and seen, said)
s.send('\x1b')

# --- 7. quit -------------------------------------------------------------
s.send(':qa\r', settle=0.3)
check("quit: :qa exits", s.wait_exit())
s.close()

# --- 7b. a start in your home --------------------------------------------
# Started at ~, all of it is inside the project — a read or a write there
# needs no question — and launch says so (review stop 12, Q8). The rule is
# `start_note`'s unit test; this is its wiring in `main`, which no test
# reaches. Read in `:messages`: a launch can have several notes, and the
# row shows one, cut to its width.
HOME_AT = tempfile.mkdtemp(prefix="shannon-drive-start-")
s = start(cwd=HOME_AT, env={"HOME": HOME_AT})
s.send(':messages\r', settle=0.3)
check("start: launching in ~ says all of it is inside",
      s.wait_for("started in your home directory"), s.screen.text())
s.send(':qa\r', settle=0.3)
check("start: and quits", s.wait_exit())
s.close()

# --- 8. setup ------------------------------------------------------------
# `shannon setup` rather than a first launch: whether a launch is a first one
# depends on keys on the machine running this (a key in the environment, a
# key setup saved), and `first_launch` itself is a unit test.
# Nothing is CHOSEN here — a choice would write the hermetic home's config.
CHOICE = "How should Shannon reach a model?"
s = start(env=ED, args=["setup"])
scr = s.repaint()
check("setup: `shannon setup` opens the choice", CHOICE in scr, scr)
check("setup: the API is fully featured, Claude Code limited",
      "Fully featured." in scr and "Limited." in scr, scr)
# The API, not Claude Code: choosing Claude Code runs `claude auth status`,
# and a smoke run must not reach outside Shannon.
s.send('\r', settle=0.3)
scr = s.repaint()
check("setup: Enter goes to what a choice needs",
      "Paste your API key" in scr and CHOICE not in scr, scr)
s.send('\x1b', settle=0.3)
s.send('\x1b', settle=0.5)
scr = s.repaint()
check("setup: Esc leaves it for the home screen", CHOICE not in scr and "┌ prompt" in scr, scr)
s.send(':setup\r', settle=0.8)
scr = s.repaint()
check("setup: `:setup` opens it again", CHOICE in scr, scr)
s.send('\x1b', settle=0.5)
s.send(':qa\r', settle=0.3)
check("setup: the app quits after it", s.wait_exit())
s.close()

# --- 9. --resume ---------------------------------------------------------
# `shannon --resume` runs what `:resume` does, before anything opens over it
# — wiring in `main`, which no cargo test reaches. A name matching nothing is
# refused on screen; bare, `:graph` opens to choose from, and its question
# (the conversation planted in step 6) says it did.
s = start(env=ED, args=["--resume", "nosuch-session"])
# The screen as launch drew it, NOT `repaint()`: its `<C-l>` is a key, and a
# key ends the notice (vim's rule) — the first run read the row after it.
scr = s.screen.text()
check("resume: a name matching nothing is refused on screen",
      "nosuch-session" in scr and "no session" in scr, scr)
s.send(':qa\r', settle=0.3)
check("resume: the app quits after it", s.wait_exit())
s.close()
s = start(env=ED, args=["--resume"])
scr = s.repaint()
check("resume: bare, :graph opens to choose from",
      "read Claude Code's conversations" in scr, scr)
s.send('n', settle=0.8)
scr = s.repaint()
check("resume: the graph is there under the question", "not read — l asks" in scr, scr)
s.send(':qa\r', settle=0.3)
check("resume: and quits", s.wait_exit())
s.close()

# --- 10. a decision, in no window, and `:decide` --------------------------
# Decisions per session (decision-panes-and-alerts.md §3.5–3.6, A1.4–A1.5).
# The pane opens under its conversation without taking the cursor; `:only`
# from the prompt is refused over it, naming whose (Steven, 10-26: no close
# answers a request); `:only!` closes the conversation, and the call WAITS,
# hidden, said at the row's right; `:decide` shows it again and stands you
# in its pane; `:hide` there keeps it, hidden, and `:decide` brings it back;
# `:q!` there keeps it too (Steven, 10-26: "close as unanswered"); `:q`
# there denies. A `:` line typed in a decision pane, and the row drawn
# after a `:only`, are the event loop's — no cargo test reaches `App::run`.
# Nothing runs: `rm` asks, and every answer here is a deny.
# A refusal is read off the screen as drawn, never after `repaint()`: its
# `<C-l>` is a key, and a key starts a new moment, clearing the row as
# vim's CTRL-L clears its message line (the `:only` check did, and failed).
s = start(env=ED)
s.send(':tool rm -rf /tmp/shannon-smoke-absent\r', settle=1.0)
scr = s.repaint()
check("decide: a typed :tool asks in a pane", "run this?" in scr, scr)
check("decide: the pane does not take the cursor", focus(s) == "prompt", scr)
s.send(':only\r', settle=0.8)
said = s.screen.text()
check("decide: :only from the prompt is refused over it, naming whose",
      "run this?" in said and "request waits in its pane" in said, said)
s.send(':only!\r', settle=0.8)
scr = s.repaint()
check("decide: :only! leaves it waiting, hidden, said at the row's right",
      "run this?" not in scr and "decision hidden" in scr and ":decide" in scr, scr)
s.send(':decide\r', settle=0.8)
scr = s.repaint()
check("decide: :decide shows it and stands you in it",
      "run this?" in scr and focus(s) == "decision", scr)
s.send(':hide\r', settle=0.8)
scr = s.repaint()
check("decide: :hide keeps it, hidden, said at the row's right",
      "run this?" not in scr and "decision hidden" in scr, scr)
s.send(':decide\r', settle=0.8)
scr = s.repaint()
check("decide: and :decide brings it back",
      "run this?" in scr and focus(s) == "decision", scr)
s.send(':q!\r', settle=0.8)
scr = s.repaint()
check("decide: :q! there closes it unanswered, hidden, too",
      "run this?" not in scr and "decision hidden" in scr, scr)
s.send(':decide\r', settle=0.8)
scr = s.repaint()
check("decide: and :decide brings it back again",
      "run this?" in scr and focus(s) == "decision", scr)
s.send(':q\r', settle=0.8)
said = s.screen.text()
check("decide: :q in the pane denies, and says so", "DENIED" in said, said)
s.send(':qa\r', settle=0.3)
check("decide: and quits", s.wait_exit())
s.close()

# --- 11. a summary's review, in a pane --------------------------------------
# Reviews are decisions (decision-panes-and-alerts.md §4, A2.2): `:compact!`
# asks for one, and its draft waits in a pane beside the session — no
# $EDITOR, no cursor taken — `:decide` goes to it, and `d` drops it, said.
# The event loop's part, the draft arriving while you stand in the prompt,
# is what no cargo test reaches (`App::run`). The stub drafts the summary.
s = start(env=ED)
s.send('i'); s.send('hello'); s.send('\x1b'); s.send('\r', settle=1.0)
# The reply's LAST words: "canned stub" is in its first sentence, and the
# stub streams for ~15 s — `:compact!` sent then was refused as busy (a turn
# in flight), its refusal gone by the time the screen was read, and every
# check below failed on a review that was never asked for (Steven's run,
# 10-22, 56/61).
check("review: a turn to compact",
      wait_for(s, "where you put it", seconds=40.0), s.repaint())
# The stand-in editor leaves a copy of what it was handed: none must exist
# after the draft arrives (review stop 8, C 9 — a regression opening it on
# arrival came back "closed unsaved", same title, and passed).
try: os.remove(_SEEN)
except OSError: pass
s.send(':compact!\r', settle=0.5)
up = wait_for(s, "review this summary?", seconds=40.0)
scr = s.repaint()
check("review: the draft waits in a pane, the cursor kept",
      up and focus(s) == "prompt", scr)
check("review: no $EDITOR on arrival",
      up and not os.path.exists(_SEEN) and "closed unsaved" not in scr, scr)
s.send(':decide\r', settle=0.8)
scr = s.repaint()
check("review: :decide stands you in it", focus(s) == "decision", scr)
# `<CR>` opens it from the key arm: the editor quits unsaved, and the review
# stays, said — with no terminal reply left on the line (OSC 11's `rgb:`,
# which the drain swallows there; review stop 8, B 10).
s.send('\r', settle=1.0)
scr = s.repaint()
check("review: <CR> opens $EDITOR, unsaved keeps it",
      os.path.exists(_SEEN) and "closed unsaved" in scr and "rgb:" not in scr, scr)
s.send('d', settle=0.6)
said = s.screen.text()
check("review: d drops it, and says so", "dropped the summary draft" in said, said)
s.send(':qa\r', settle=0.3)
check("review: and quits", s.wait_exit())
s.close()

# --- 12. news off screen: the bell and a notification -----------------------
# The shipped `unshown_session` (decision-panes-and-alerts.md §5.3, B2.3): a
# reply that ends where no pane draws it rings the bell and sends an `OSC 9`,
# written by the event loop after a frame — the one piece no cargo test
# reaches (`App::take_rings` is tested; the write is `event_loop`'s). A turn is
# sent, then `:help` is lent over its transcript, so the reply ends unseen.
# (`:new` would not do: a session with no committed turn is the one `:new`
# keeps.) Only bytes after `:help` are read, and a bell that is merely an
# OSC's terminator does not count as the bell.
import re, time
s = start(env=ED)
s.send('i'); s.send('hello'); s.send('\x1b'); s.send('\r', settle=1.0)
s.send(':help\r', settle=1.0)
since = len(s.log)
def rung(session):
    out = b"".join(session.log[since:])
    notified = re.search(rb'\x1b\]9;Shannon: news in b1[^\x07\x1b]*\x07', out) is not None
    bare = re.sub(rb'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)', b'', out)
    return notified, b'\x07' in bare
end = time.time() + 45
while time.time() < end and not rung(s)[0]:
    s.read(0.5)
notified, belled = rung(s)
scr = s.repaint()
check("ring: a reply ended unseen sends a notification", notified, scr)
check("ring: and rings the bell", belled, scr)
check("ring: the row says it", "news in session 1" in scr, scr)
s.send(':q\r', settle=0.8)
scr = s.repaint()
check("ring: seen, the row says it no more", "news in session 1" not in scr, scr)
s.send(':qa\r', settle=0.3)
check("ring: and quits", s.wait_exit())
s.close()

# --- 13. news while you look away: the terminal's focus -----------------------
# Steven, 10-24: "the window not being in focus is not considered seen". Launch
# asks the terminal for focus reports (`?1004h`, `terminal.rs`); a reply that
# ends in the very pane it was sent from, while the terminal reports it is not
# in front (`ESC [ O`), rings; in front again (`ESC [ I`), it is seen. The
# report's wiring (`App::looked` in `handle_event`) is what no cargo test
# reaches. Words-free: the row's news reads "session 1" either way.
s = start(env=ED)
check("focus: launch asks for focus reports", b"\x1b[?1004h" in b"".join(s.log))
s.send('i'); s.send('hello'); s.send('\x1b'); s.send('\r', settle=1.0)
s.send(b'\x1b[O', settle=0.3)
since = len(s.log)
end = time.time() + 45
while time.time() < end and not rung(s)[0]:
    s.read(0.5)
notified, belled = rung(s)
scr = s.screen.text()
check("focus: a reply ended while you looked away notifies", notified, scr)
check("focus: and rings the bell", belled, scr)
s.send(b'\x1b[I', settle=0.8)
scr = s.repaint()
row = scr.splitlines()[-1] if scr else ""
check("focus: back in front, seen", "session 1" not in row, scr)
s.send(':qa\r', settle=0.3)
check("focus: and quits", s.wait_exit())
# And gives the reports back off (`TerminalGuard::drop`): left on, the shell
# after it reads every focus change as `ESC [ I` / `ESC [ O` typed at it. The
# teardown's bytes are read off the pty first — `wait_exit` only waits.
s.read(0.5)
check("focus: quitting turns focus reports off", b"\x1b[?1004l" in b"".join(s.log))
s.close()

# --- 14. the tree as a buffer ------------------------------------------------
# `buffers.md` §10.4 V4 identity (c): the first tree takes a number from the
# one counter, `:ls!` lists it as a page is (`u`, `h` once no pane shows it),
# `:b N` opens it in the pane you are in, and a session verb given its number
# says what it numbers. The `:` lines reach `App::run`'s loop, which no cargo
# test does. Its number is read off the row, not assumed. The conversation
# planted in step 6 asks first; `n` reads none.
s = start(env=ED)
s.send(':graph\r', settle=1.2)
s.send('n', settle=0.8)
s.send(':q\r', settle=0.8)
s.send(':ls!\r', settle=1.0)
scr = s.repaint()
tree = re.search(r'b(\d+)\s+uh\s.*graph\s+the session tree', scr)
check("tree: :ls! lists the session tree, unlisted and hidden", tree is not None, scr)
n = tree.group(1) if tree else "2"
s.send(':q\r', settle=0.8)
s.send(':b%s\r' % n, settle=1.2)
scr = s.repaint()
check("tree: :b N opens it in this pane", "not read — l asks" in scr, scr)
s.send(':q\r', settle=0.8)
s.send(':clone b%s\r' % n, settle=0.8)
said = s.screen.text()
check("tree: a session verb says what its number is",
      ("b%s is the session tree, not a session" % n) in said, said)
s.send(':qa\r', settle=0.3)
check("tree: and quits", s.wait_exit())
s.close()

# --- 15. the window's alternate: CTRL-^ and :b# ------------------------------
# `buffers.md` §10.4 V4b: a terminal sends CTRL-^ as the byte 0x1E, which the
# key reader takes as CTRL with `6` — the binding's reading of it, and no cargo
# test can check it (each builds its key event itself). Nothing to go back to is
# said (read before any repaint, which clears it); `:help` over the transcript,
# CTRL-^ back to the transcript, again to the manual; `:b#` the same verb.
s = start(env=ED)
s.send('\x17k', settle=0.5)  # CTRL-W k: up into the transcript
s.send('\x1e', settle=0.8)
said = s.screen.text()
check("alternate: CTRL-^ with nothing to go back to says so",
      "no buffer to go back to in this window" in said, said)
s.send(':help\r', settle=1.2)
scr = s.repaint()
check("alternate: premise, the manual over the transcript", HELP_MARK in scr, scr)
s.send('\x1e', settle=0.8)
scr = s.repaint()
check("alternate: CTRL-^ goes back to the transcript", HELP_MARK not in scr, scr)
s.send('\x1e', settle=0.8)
scr = s.repaint()
check("alternate: and again to the manual", HELP_MARK in scr, scr)
s.send(':b#\r', settle=0.8)
scr = s.repaint()
check("alternate: :b# is the same verb", HELP_MARK not in scr, scr)
s.send(':qa\r', settle=0.3)
check("alternate: and quits", s.wait_exit())
s.close()

# --- 16. every view a buffer: one `:ls`, <CR> on its row -----------------------
# `buffers.md` §10.4 V5.1–V5.2: `:ls!` and `:ls` are ONE buffer, titled `ls!`
# while it names the unlisted; <CR> on a session's row shows it in this pane,
# and CTRL-^ brings `:ls` back. <CR> in a listing is claimed in `App::run`'s
# routing (`viewer_claims_key`), which no cargo test drives. `G` is the last
# line: the one session's row, with nothing left running.
s = start(env=ED)
s.send('\x17k', settle=0.5)  # CTRL-W k: up into the transcript
s.send(':ls!\r', settle=1.0)
scr = s.repaint()
check("listing: :ls! is titled ls!", " ls! " in scr, scr)
s.send(':ls\r', settle=0.8)
scr = s.repaint()
# After `:ls`, no title or row says ls! (a row names the view as its title
# does: `under ls!` while `:ls!` is up, `under ls` after — review stop 10, F 8b).
check("listing: :ls clears it, no title says ls!",
      " ls! " not in scr and "Sessions open this run." in scr, scr)
s.send('G', settle=0.5)
s.send('\r', settle=1.0)
scr = s.repaint()
check("listing: <CR> on a session's row shows it here",
      "Sessions open this run" not in scr, scr)
s.send('\x1e', settle=0.8)
scr = s.repaint()
check("listing: CTRL-^ brings :ls back", "Sessions open this run" in scr, scr)
s.send(':qa\r', settle=0.3)
check("listing: and quits", s.wait_exit())
s.close()

# --- 17. tab pages: :tabnew, gt / gT, :tabclose -------------------------------
# `tabs.md` §6b (T2): `gt` is a `g` chord completing into a window command
# (`MotionChord::Window`) that `App::run`'s window arm applies — the real
# loop, which no cargo test drives. The first tab shows the manual, so the two
# can be told apart on screen; the tab line is the screen's first row while
# there are two, and goes with the second. A tab that is not there is said
# (read before a repaint, which clears it).
s = start(env=ED)
s.send(':help\r', settle=1.2)
s.send(':tabnew\r', settle=1.0)
scr = s.repaint()
top = scr.splitlines()[0] if scr else ""
check("tabs: :tabnew draws the tab line", " 1 " in top and " 2 " in top, scr)
check("tabs: the new tab is a fresh one", HELP_MARK not in scr, scr)
s.send('\x1b', settle=0.3)
s.send('gt', settle=0.8)
scr = s.repaint()
check("tabs: gt goes round to the first", HELP_MARK in scr, scr)
s.send('gT', settle=0.8)
scr = s.repaint()
check("tabs: gT back to the second", HELP_MARK not in scr, scr)
s.send('9gt', settle=0.8)
said = s.screen.text()
check("tabs: a tab that is not there is said", "there is no tab 9" in said, said)
s.send(':tabclose\r', settle=0.8)
scr = s.repaint()
top = scr.splitlines()[0] if scr else ""
check("tabs: :tabclose shows the first again, the line gone",
      HELP_MARK in scr and " 2 " not in top, scr)
s.send(':qa\r', settle=0.3)
check("tabs: and quits", s.wait_exit())
s.close()

# --- 18. T4: :tabrename, :tab help, :tabmove, <C-w>T --------------------------
# `tabs.md` §6b (T4): a name the tab line shows; `:tab help` opens the manual
# in a tab of its own (the one door a window is born by, `App::new_window`);
# `:tabmove 0` re-orders the line, the name moving with its tab; `<C-w>T`
# through the real loop's window arm — refused, said, on a tab's only
# window, and moving a split's window into a third tab. `<C-w>` is sent from
# Normal mode: in the prompt's Insert it deletes a word, as vim's does.
s = start(env=ED)
s.send(':tabrename alpha\r', settle=0.8)
s.send(':tab help\r', settle=1.2)
scr = s.repaint()
top = scr.splitlines()[0] if scr else ""
check("T4: :tab help opens the manual in a second tab",
      " 2 " in top and HELP_MARK in scr, scr)
check("T4: the first tab wears its name", " 1 alpha " in top, scr)
s.send('\x1b', settle=0.3)
s.send(':tabmove 0\r', settle=0.8)
scr = s.repaint()
top = scr.splitlines()[0] if scr else ""
# No trailing space in the needle: the screen's row is trimmed at its end,
# and alpha's label is the last on it.
check("T4: :tabmove 0 puts the manual's tab first, the name with its tab",
      top.startswith(" 1 help") and " 2 alpha" in top, scr)
s.send('\x1b', settle=0.3)
s.send('\x17T', settle=0.8)
said = s.screen.text()
check("T4: <C-w>T on a tab's only window is said",
      "the tab's only window" in said, said)
# Into alpha's transcript (its focus came back to the prompt), split it,
# and move the split's window out.
s.send('\x1b', settle=0.3)
s.send(':tabnext 2\r', settle=0.8)
s.send('\x1b', settle=0.3)
s.send('\x17k', settle=0.5)
s.send(':vsplit\r', settle=0.8)
s.send('\x17T', settle=1.0)
scr = s.repaint()
top = scr.splitlines()[0] if scr else ""
check("T4: <C-w>T moves the window to a third tab", " 3 " in top, scr)
# `:tab sp` typed in the prompt bar, the usual place for `:`: it copies the
# window the prompt is aimed at into a fourth tab. It panicked (r13-F C1).
s.send('\x1b', settle=0.3)
s.send('\x17j', settle=0.5)
s.send(':tab sp\r', settle=1.0)
scr = s.repaint()
top = scr.splitlines()[0] if scr else ""
check("T4: :tab sp from the prompt opens a fourth tab", " 4 " in top, scr)
# `:tabs`, the T4 listing: a page in the pane you are in, a heading per tab.
s.send('\x1b', settle=0.3)
s.send(':tabs\r', settle=1.0)
scr = s.repaint()
check("T4: :tabs lists every tab page",
      "Tab page 1" in scr and "Tab page 4" in scr, scr)
s.send(':qa\r', settle=0.3)
check("T4: and quits", s.wait_exit())
s.close()

# --- 19. launch guards the key file it was given --------------------------------
# `main::app_config` hands the gate every key file (`keys::protected_key_files`)
# in one call no cargo test reaches: deleting it passed every test (review stop
# 12, T6 2). A key file named as launch is told of one, empty as /dev/null is,
# so the backend is still the stub. Under a session allow of its directory, its
# neighbour reads unasked (the premise) and the key asks -- the guard is the
# only thing that can make it. The second report replaces the first in its
# pane (measured in the app's harness).
KEYDIR = os.path.join(_TMP, "keydir")
os.makedirs(KEYDIR)
KEY = os.path.join(KEYDIR, "smoke-key")
open(KEY, "w").close()
s = start(env={**ED, "SHANNON_API_KEY_FILE": KEY})
s.send(':allow :read %s\r' % KEYDIR, settle=0.8)
s.send(':analyze cat %s\r' % os.path.join(KEYDIR, "neighbour"), settle=1.0)
scr = s.repaint()
check("key: premise -- its directory is allowed", "ALLOW runs without asking" in scr, scr)
s.send(':analyze cat %s\r' % KEY, settle=1.0)
scr = s.repaint()
check("key: launch guards the key file it was given",
      "ASK   you'll be prompted first" in scr, scr)
s.send(':qa\r', settle=0.3)
check("key: and quits", s.wait_exit())
s.close()

# --- 20. the cursor: the theme's colour, the mode's shape -----------------------
# Steven, 11-02 (image 42): a light theme on a dark terminal left the prompt's
# cursor in the terminal's own faint yellow. A theme with a ground colours it
# (OSC 12) and the mode shapes it (DECSCUSR, `tui.guicursor`: a block in
# Normal, a bar in Insert — S21-Q22), both written by the event loop after a
# frame (`terminal::paint_cursor`, the piece no cargo test reaches) and given
# back (OSC 112, DECSCUSR 0) for `$EDITOR` and at exit. Launch wears
# `terminal`, which leaves the terminal's colour alone.
SET, OWN = b"\x1b]12;#4c4f69\x07", b"\x1b]112\x07"
BLOCK, BAR, SHAPE_OWN = b"\x1b[2 q", b"\x1b[6 q", b"\x1b[0 q"
s = start(env=ED)
log = lambda since=0: b"".join(s.log[since:])
shaped = lambda out: max(out.rfind(BLOCK), out.rfind(BAR))
scr = s.repaint()
check("cursor: the shipped theme leaves the terminal's own",
      "prompt" in scr and b"\x1b]12;" not in log(), scr)
check("cursor: Normal is a block", BLOCK in log(), repr(log()[-300:]))
since = len(s.log)
s.send('i', settle=0.5)
check("cursor: Insert is a bar", BAR in log(since), repr(log(since)[-300:]))
s.send('\x1b', settle=0.5)
check("cursor: Esc is a block again",
      log(since).rfind(BLOCK) > log(since).rfind(BAR), repr(log(since)[-300:]))
s.send(':set tui.theme=catppuccin-latte\r', settle=1.0)
check("cursor: a theme with a ground colours it in its text",
      SET in log(), s.repaint())
since = len(s.log)
# `:keymap`, not `:config edit`: a view in `$EDITOR` that reloads nothing, so
# the theme worn after it is the one worn before.
s.send(':keymap\r', settle=2.5)
s.send('i', settle=0.5)
out = log(since)
given, taken = out.find(OWN), out.rfind(SET)
check("cursor: given back for $EDITOR, coloured again after",
      0 <= given < taken, repr(out[-400:]))
check("cursor: its shape given back for $EDITOR, and taken again after",
      0 <= out.find(SHAPE_OWN) < shaped(out), repr(out[-400:]))
# Esc on its own write: an Esc read in one chunk with the `:` after it is
# Alt-`:` to crossterm, and the `:qa` never runs.
s.send('\x1b', settle=0.3)
s.send(':qa\r', settle=0.3)
check("cursor: and quits", s.wait_exit())
s.read(0.5)
check("cursor: quitting gives the terminal's colour and shape back",
      log().rfind(OWN) > log().rfind(SET) and log().rfind(SHAPE_OWN) > shaped(log()))
s.close()

# --- 21. one cursor, the terminal's, in every pane ------------------------------
# S21-Q23: a page painted its own cursor, a reversed cell, while the prompt
# and a transcript placed the terminal's. Every pane places the terminal's
# now, through one door (`view::place`, the piece no cargo test drives end to
# end); an open `:` line takes it. The screen's cursor is where the last
# frame's placement left it.
s = start(env=ED)
s.send(':help\r', settle=1.0)
scr = s.repaint()
at = (s.screen.cy, s.screen.cx)
row = "".join(s.screen.grid[at[0]]) if 0 <= at[0] < s.screen.rows else ""
check("one cursor: a page's is the terminal's, on its first char",
      row[at[1]:at[1] + len(HELP_MARK)] == HELP_MARK, repr((at, row[:40])))
s.send(':ab', settle=0.5)
check("one cursor: the `:` line takes it",
      (s.screen.cy, s.screen.cx) == (s.screen.rows - 1, 3), (s.screen.cy, s.screen.cx))
s.send('\x1b', settle=0.3)
s.send(':qa\r', settle=0.3)
check("one cursor: and quits", s.wait_exit())
s.close()

# --- 22. the stub asks for the calls a prompt writes out (F2d) ------------------
# A fenced block whose info string is `tool <name>` is a call the offline stub
# asks for (`backend/stub.rs`), so the gate, the pane, the file seam and the
# round back to the model are driven with nothing spent. A Read inside the
# start runs unasked and the stub says what it was handed (one line: the stub
# streams 4 chars per 60 ms); a delete asks, and `d` reaches it as an error.
# The pane takes no cursor (step 3), so `:decide` goes to it before `d`: sent
# from the prompt, `d` is an operator there and answers nothing.
s = start(env=ED)
s.send('i')
s.send('```tool Read\r{"file_path": "Cargo.toml", "limit": 1}\r```', settle=0.5)
s.send('\x1b', settle=0.3)
s.send('\r', settle=1.0)
read = wait_for(s, "workspace root", seconds=40.0)
scr = s.repaint()
check("stub call: a Read inside runs, and the stub says what it was handed",
      read and "answered" in scr, scr)
s.send('i')
s.send('```tool bash\rrm /tmp/shannon-smoke-never-made\r```', settle=0.5)
s.send('\x1b', settle=0.3)
s.send('\r', settle=1.0)
asked = wait_for(s, "run this?", seconds=40.0)
scr = s.repaint()
check("stub call: a delete asks", asked, scr)
s.send(':decide\r', settle=0.8)
s.send('d', settle=0.5)
denied = wait_for(s, "as an error", seconds=40.0)
scr = s.repaint()
check("stub call: d reaches the stub as an error", denied, scr)
s.send('\x1b', settle=0.3)
s.send(':qa\r', settle=0.3)
check("stub call: and quits", s.wait_exit())
s.close()

# --- 23. `shannon tutor`: the lessons, yours to change, never sent ------------
# It opens with you in it (`App::open_tutor`): `A` appends to its first line,
# as vimtutor's copy takes an edit; `<CR>` there sends nothing and says so.
# What it launches with is held here too, since `main` is where it is chosen
# (review stop 16, C 1, C 2): a command runs in the tutor's own directory;
# `:reload` reads no file of yours; a practice `:secret` and `:trust .` leave
# nothing in your state.
s = start(["tutor"])
scr = s.repaint()
check("tutor: opens on its lessons", "Welcome to Shannon Tutor" in scr, scr)
s.send('A smoke was here', settle=0.3)
s.send('\x1b', settle=0.5)
rows = s.screen.text().splitlines()
banner = next((r for r in rows if "=====" in r), "")
check("tutor: A edits the page, on its own first line", "smoke was here" in banner, rows)
s.send('\r', settle=0.5)
said = s.screen.text()
check("tutor: <CR> there is refused, said", "the tutor is never sent" in said, said)
s.send(':reload\r', settle=0.5)
said = s.screen.text()
check("tutor: :reload reads no file of yours", "runs on Shannon's defaults" in said, said)
# The tutor is `b1`, listed alone: its chat is not called yet (Steven, 11-09:
# `:ls` then `:b1` opened an empty chat). `main` chooses what the launch
# opens on (`Opens::Tutor`), which no cargo test reaches.
s.send(':ls\r', settle=0.8)
scr = s.repaint()
check("tutor: :ls lists it as b1, and no chat yet",
      re.search(r'\bb1\s.*the tutor', scr) is not None and re.search(r'\bb2\s', scr) is None,
      scr)
s.send(':b1\r', settle=0.8)
scr = s.repaint()
check("tutor: :b1 is the tutor", "smoke was here" in scr, scr)
# A chat where it was (full width: a halved pane could wrap the path through
# the name looked for), and a command run from it: where it runs.
s.send(':new\r', settle=0.5)
s.send('\x17j', settle=0.5)
s.send('i')
s.send('```tool bash\rpwd\r```', settle=0.5)
s.send('\x1b', settle=0.3)
s.send('\r', settle=1.0)
ran = wait_for(s, "shannon-tutor-", seconds=40.0)
check("tutor: a command runs in the tutor's own directory", ran, s.repaint())
s.send(':secret smoke-practice-value\r', settle=0.5)
s.send(':trust .\r', settle=0.5)
s.send(':qa!\r', settle=0.3)
check("tutor: and quits", s.wait_exit())
s.close()
state = os.path.join(_HOME, "state", "shannon")
kept = os.listdir(os.path.join(state, "secrets")) if os.path.isdir(os.path.join(state, "secrets")) else []
check("tutor: a practice :secret is not kept", kept == [], kept)
trusted_file = os.path.join(state, "trusted_dirs")
trusted = open(trusted_file).read() if os.path.exists(trusted_file) else ""
check("tutor: its :trust . is not written to yours", "shannon-tutor-" not in trusted, trusted)
# Memory and skills read from your config directory made a
# `projects/<tutor>/memory` there at every launch (review stop 16, D 1).
projects = os.path.join(_HOME, "config", "shannon", "projects")
made = [p for p in os.listdir(projects) if "shannon-tutor-" in p] if os.path.isdir(projects) else []
check("tutor: makes nothing in your config directory", made == [], made)

# --- 24. The clipboard: the escape a yank really writes ------------------------
# The suite stops at `OscSink` (`TerminalClipboard` refuses to run under test:
# a test's escape reached the developer's terminal, d493060), so only this sees
# the bytes a terminal obeys. Steven, 11-08: "I thought yank puts things into
# `pbpaste`"; Ghostty took a hand-typed OSC 52, so the question was this end.
def osc52(raw):
    """The text of the OSC 52 in `raw`, None if it holds none."""
    m = re.search(rb'\x1b\]52;c;([A-Za-z0-9+/=]*)\x07', raw)
    return base64.b64decode(m.group(1)).decode() if m else None

s = start(["tutor"])
copied = osc52(s.send('yy', settle=0.6))
# The lesson's first line, read from the lesson: its width is the tutor's.
first_line = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "crates", "shannon-tui", "src", "tutor.txt")).readline()
check("clipboard: yy writes its line as OSC 52", copied == first_line, copied)
# A drag over a word on screen (found there, so no layout is assumed): SGR
# mouse reports, 1-based. Selecting copies nothing until `y`
# (`tui.copy_on_select` is off by default); then `y` copies what it selected.
s.repaint()
rows = s.screen.text().splitlines()
y = next((i for i, r in enumerate(rows) if "Welcome" in r), None)
check("clipboard: premise, the word to drag over is on screen", y is not None, rows)
if y is not None:
    x = rows[y].index("Welcome")
    s.send('\x1b[<0;%d;%dM' % (x + 1, y + 1), settle=0.2)
    s.send('\x1b[<32;%d;%dM' % (x + 7, y + 1), settle=0.2)
    released = s.send('\x1b[<0;%d;%dm' % (x + 7, y + 1), settle=0.3)
    check("clipboard: letting go of a drag sends nothing", osc52(released) is None, repr(released[-120:]))
    copied = osc52(s.send('y', settle=0.6))
    check("clipboard: y after a drag writes what it selected", copied == "Welcome", copied)
s.send(':qa!\r', settle=0.3)
check("clipboard: and quits", s.wait_exit())
s.close()

# --- 25. A link followed: the bytes a terminal really sends ---------------------
# `<C-]>` is 0x1D, which crossterm reads as CTRL-5, and `<C-t>` is 0x14. The
# tests hold the table; only this holds what arrives.
s = start(["tutor"])
# A search is a Rust regex (search.rs), where a bare `|` is alternation:
# `/|motions|` matched the empty string where the cursor stood and went nowhere.
s.send('/\\|motions\\|\r', settle=0.4)
s.send('\x1d', settle=0.5)
s.repaint()
opened = "help — motions" in s.screen.text()
check("links: <C-]> in the tutor opens the manual at the link", opened, s.screen.text())
s.send('\x14', settle=0.5)
s.repaint()
# Only after an open does coming back say anything; the title is the window's
# own, where "Tutor" in the prose may be scrolled away at the link's line.
back = "┌ tutor " in s.screen.text() and "help — motions" not in s.screen.text()
check("links: <C-t> gives the tutor back", opened and back, s.screen.text())
s.send(':qa!\r', settle=0.3)
check("links: and quits", s.wait_exit())
s.close()

bad = [r for r in results if not r[1]]
for name, ok, _ in results:
    print(("PASS  " if ok else "FAIL  ") + name)
print("\n%d/%d" % (len(results) - len(bad), len(results)))
for name, ok, scr in bad:
    print("\n===== %s =====" % name)
    print(scr if scr else "(no screen)")

# **A real exit code.** AGENTS.md calls this the fifth bar command, and a gate
# that always returns success cannot be scripted, chained or trusted — its only
# signal was a human reading the output (Reviewer C).
sys.exit(1 if bad else 0)
