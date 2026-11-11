# `tools/drive` — driving the real binary through a real pty

## Why this exists

On 2026-08-24 Shannon Code handled **exactly one keystroke** per session and
then deadlocked. Every session since the recovery had been that broken. It
survived a full build, 1500 tests, `clippy`, `cargo doc`, and two whole review
rounds — because the defect lived in `App::run`, which takes a live
`ShannonTerminal`, and *nothing in the workspace can call it*.

That is not an accident of that one bug. The event loop, the terminal setup,
the `$EDITOR` suspend/resume and the repaint paths are all untestable from
`cargo test` by construction: they need a terminal on the other end. So they
were covered by argument and by doc comment, which is how a program can be
completely unusable while its test suite is green.

This is the other end. It opens a pty, runs the built binary on it, answers the
terminal queries a real emulator answers, and parses what comes back into a
grid you can read.

## Use

    cargo build --bin shannon
    python3 tools/drive/smoke.py

Only the standard library is needed. `smoke.py` walks the surfaces a first-run
user touches — typing, `Esc`, a normal-mode edit and undo, sending a turn,
`:help` and its motions, a read-only refusal, `<C-w>` splits, `:keymap`,
`:graph`, `:qa` — and prints a pass/fail line each, with the failing screen.

`term.py` is the harness:

    from term import start
    s = start(env={"EDITOR": "/path/to/fake-editor"})
    s.send('i'); s.send('hello')
    print(s.repaint())          # <C-l>, then the whole grid
    print(s.screen.text())      # the grid as-is, without pressing anything
    s.winsize(70, 20)           # SIGWINCH, the tmux-split case
    s.alive()

## Two traps that cost real time

**Do not `repaint()` before reading a notice.** `<C-l>` is a keypress, and a
notice survives exactly one input cycle (vim's message-line rule), so the
repaint wipes the thing you were about to assert on. `:keymap names` and `i` in
`:graph` both looked like silent failures for this reason; one of them was.
Read `s.screen.text()` instead.

**Send `Esc` and the next key as separate writes.** `send('\x1b\r')` is one
write, the terminal delivers it as an ESC-prefixed sequence, and crossterm
parses it as `Alt+Enter` — a different key with a different binding. Insert a
settle between them.

## What a real emulator answers

`Term._reply` answers `ESC[6n` (cursor position), `ESC[?u` (kitty keyboard
protocol) and `ESC[c` (device attributes). Without those, every crossterm round
trip times out after two seconds and the app reports failures no real terminal
would ever produce — which sent this investigation down a two-hour blind alley.
If a new query starts blocking, add it here rather than treating the timeout as
a finding.
