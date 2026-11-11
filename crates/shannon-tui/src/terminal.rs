//! Terminal lifecycle as RAII.
//!
//! `TerminalGuard` owns the raw-mode + alternate-screen setup. Its
//! `Drop` impl restores the terminal regardless of how the program
//! exits — normal return, error propagation, or panic. The previous
//! procedural shape (best-effort restore via explicit `if let` blocks
//! at the bottom of `run`) was correct as long as nobody added a new
//! `?` or early `return` above the restore. Drop cannot be skipped
//! that way.

use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Push the kitty keyboard-protocol disambiguation flag **iff the terminal
/// supports it**, returning whether we did. Without it the terminal sends a
/// bare `CR` for `Enter`, `Ctrl+Enter`, and `Shift+Enter` alike, so the modifier
/// is invisible to crossterm and the direct-submit key (`Ctrl+Enter`) can never
/// fire. With it, those arrive as distinct `KeyEvent`s. We push only the
/// disambiguation flag (not event-type reporting), so we still get Press events
/// only — matching the keymap's Press-only handling.
fn push_keyboard_enhancement(out: &mut impl Write) -> bool {
    let pushed = if matches!(supports_keyboard_enhancement(), Ok(true)) {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    } else {
        false
    };
    PUSHED_KEYBOARD_ENHANCEMENT.store(pushed, Ordering::Relaxed);
    pushed
}

/// **Did we push the flag?** — not "does the terminal support it", which is the
/// question `supports_keyboard_enhancement` answers and a different one: a
/// supported terminal whose `execute!` failed leaves nothing on the stack, and
/// popping it then unbalances a stack we never wrote to.
///
/// Recorded by **every** code path that pushes — startup and the `$EDITOR`
/// resume — so the pop cannot consult a second opinion. It said "the only code
/// that pushes" while `ResumeGuard::drop` pushed straight past it, which is the
/// shape this project keeps finding: a door that removes a rule only if every
/// caller walks through it (Reviewer B). [`with_terminal_suspended`] used to re-ask the *terminal*,
/// which was two derivations of one fact and also a **blocking query on the
/// event loop's thread** — see [`force_repaint`] for why that class of call is
/// not allowed here.
static PUSHED_KEYBOARD_ENHANCEMENT: AtomicBool = AtomicBool::new(false);

fn pushed_keyboard_enhancement() -> bool {
    PUSHED_KEYBOARD_ENHANCEMENT.load(Ordering::Relaxed)
}

/// **A setting of the terminal's cursor that Shannon takes** — its colour or
/// its shape: what we told the terminal, `0` while it is the terminal's own.
/// Recorded where it is written, like the keyboard flag, so what is given
/// back is exactly what was taken: a colour or shape you set in your shell
/// before starting Shannon is not reset by a Shannon that never changed it.
struct Taken {
    told: AtomicU32,
    /// The escape that gives the terminal its own back.
    own: &'static str,
}

impl Taken {
    const fn new(own: &'static str) -> Self {
        Taken {
            told: AtomicU32::new(0),
            own,
        }
    }

    /// `want` (`0` the terminal's own), written as `escape` spells it only
    /// when it changed — a frame that keeps it writes nothing, at thousands
    /// of frames a second.
    fn paint(
        &self,
        out: &mut impl Write,
        want: u32,
        escape: impl FnOnce(u32) -> String,
    ) -> io::Result<()> {
        if self.told.load(Ordering::Relaxed) == want {
            return Ok(());
        }
        let escape = match want {
            0 => self.own.to_string(),
            want => escape(want),
        };
        out.write_all(escape.as_bytes())?;
        out.flush()?;
        self.told.store(want, Ordering::Relaxed);
        Ok(())
    }

    /// The terminal's own back, if we took it — and forgotten, so the frame
    /// after a resume sets ours again ([`paint_cursor`]).
    fn unpaint(&self, out: &mut impl Write) -> io::Result<()> {
        if self.told.swap(0, Ordering::Relaxed) == 0 {
            return Ok(());
        }
        out.write_all(self.own.as_bytes())?;
        out.flush()
    }
}

/// **The cursor's settings Shannon takes, together**: its colour (OSC 12,
/// `0x01rrggbb` told; OSC 112 the terminal's own) and its shape (DECSCUSR's
/// number told; DECSCUSR 0 the terminal's own — neovim's reset for most
/// terminals, `tui.c`; xterm reads 0 as a blinking block).
struct CursorTaken {
    colour: Taken,
    shape: Taken,
}

/// The cursor as the frame loop last set it.
static CURSOR: CursorTaken = CursorTaken::new();

/// OSC 112: the cursor in the terminal's own colour.
const COLOUR_OWN: &str = "\x1b]112\x07";

/// DECSCUSR 0: the cursor in the terminal's own shape.
const SHAPE_OWN: &str = "\x1b[0 q";

impl CursorTaken {
    const fn new() -> Self {
        CursorTaken {
            colour: Taken::new(COLOUR_OWN),
            shape: Taken::new(SHAPE_OWN),
        }
    }

    /// `cursor`, each setting written only where it changed.
    fn paint(&self, out: &mut impl Write, cursor: Cursor) -> io::Result<()> {
        let colour = cursor.colour.map_or(0, |(r, g, b)| {
            1 << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
        });
        self.colour.paint(out, colour, |rgb| {
            format!("\x1b]12;#{:06x}\x07", rgb & 0xff_ffff)
        })?;
        let shape = cursor.shape.map_or(0, u32::from);
        self.shape.paint(out, shape, |n| format!("\x1b[{n} q"))
    }

    /// **The terminal's cursor back** — colour and shape, whichever we took:
    /// one door for the suspend before `$EDITOR` and the exit, so neither
    /// can give back one and keep the other.
    fn give_back(&self, out: &mut impl Write) -> io::Result<()> {
        let colour = self.colour.unpaint(out);
        self.shape.unpaint(out).and(colour)
    }
}

/// **The terminal's cursor as a frame wants it** (`App::terminal_cursor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    /// The theme's colour (`Theme::cursor`), `None` the terminal's own.
    pub colour: Option<(u8, u8, u8)>,
    /// DECSCUSR's number for the mode keys go to (`tui.guicursor`), `None`
    /// the terminal's own shape.
    pub shape: Option<u8>,
}

/// **The terminal's cursor as the frame wants it**, each frame — written
/// only where it changed.
///
/// The colour: every pane places the terminal's cursor (`view::place`), and
/// its colour was the terminal's config's, chosen for the terminal's ground
/// — under a light theme on a dark terminal, invisible (Steven, 11-02).
/// Neovim does the same for a `'guicursor'` group (options.txt). The shape:
/// a bar in Insert, a block in Normal, as neovim draws them.
///
/// Given back at every suspend and at exit (`CursorTaken::give_back`).
pub fn paint_cursor(out: &mut impl Write, cursor: Cursor) -> io::Result<()> {
    CURSOR.paint(out, cursor)
}

/// Concrete terminal type used throughout the binary. Captured here so
/// the rest of the crate doesn't need to spell out the backend.
pub type ShannonTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Mouse reporting modes we ask the terminal for, and give back.
///
/// **Not `EnableMouseCapture`.** crossterm's version sends `?1000h ?1002h
/// ?1003h ?1015h ?1006h`, and `?1003h` is *any-event tracking*: the terminal
/// reports every pointer MOVEMENT, with no button held, forever. Nothing in
/// this program wants that. Click-to-focus needs press/release (`?1000h`),
/// drag-select needs motion **while a button is down** (`?1002h`), and both
/// need SGR coordinates (`?1006h`) so columns past 223 are addressable.
///
/// The cost of the one we did not want was not theoretical. The event loop
/// takes one event per wake and repaints for each, so on a frame that is
/// expensive for other reasons a moving pointer produces events faster than
/// they drain and the backlog grows for as long as the mouse moves. That is a
/// freeze, and it is indistinguishable from a hang.
///
/// `?1015h` (urxvt extended coordinates) is dropped too: it is a fallback for
/// terminals that lack SGR, every terminal we target has SGR, and asking for
/// both invites a terminal to answer in the format we did not parse.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";

/// [`MOUSE_ON`], undone in the reverse order — the symmetry a terminal-mode
/// guard lives or dies by ([[feedback_suspend_tempfile_hygiene]]).
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";

fn enable_mouse(w: &mut impl Write) -> std::io::Result<()> {
    w.write_all(MOUSE_ON.as_bytes())?;
    w.flush()
}

fn disable_mouse(w: &mut impl Write) -> std::io::Result<()> {
    w.write_all(MOUSE_OFF.as_bytes())?;
    w.flush()
}

pub struct TerminalGuard {
    /// `Option` so `Drop` can take ownership of the inner terminal to
    /// run `LeaveAlternateScreen` against it. After `Drop` runs the
    /// `None` state is unobservable.
    terminal: Option<ShannonTerminal>,
    /// Whether we pushed the keyboard-enhancement flag at `enter` (only when the
    /// terminal supports it), so `Drop` pops exactly what it pushed.
    keyboard_enhanced: bool,
    /// Whether mouse capture is on (the `tui.mouse` config), so `Drop` disables
    /// exactly what it enabled.
    mouse: bool,
}

impl TerminalGuard {
    /// Enter raw mode + alt-screen and construct a ratatui `Terminal`. `mouse`
    /// (the `tui.mouse` config) gates mouse capture — when off, the mouse is
    /// left entirely to the terminal.
    pub fn enter(mouse: bool) -> Result<Self> {
        enable_raw_mode().context("enable raw mode")?;
        let mut stdout = io::stdout();
        // Bracketed paste so the terminal hands a paste over as one
        // `Event::Paste` blob instead of a key storm (no stray submits, no
        // pasted text executed as Normal-mode commands). Focus reports
        // (`?1004`) so news in a pane you are not looking at — the terminal
        // behind another window — is news (`App::looked`); a terminal that
        // sends none leaves you taken as looking.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableFocusChange
        )
        .context("enter alt screen")?;
        // Disambiguate modified keys (so `Ctrl+Enter` is a distinct event, not a
        // bare CR). No-op on terminals that don't support it.
        let keyboard_enhanced = push_keyboard_enhancement(&mut stdout);
        // Mouse capture (click-focus / wheel-scroll / drag-select). Off → the
        // terminal keeps the mouse (native selection, etc.).
        if mouse {
            enable_mouse(&mut stdout).context("enable mouse capture")?;
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("create terminal")?;
        Ok(Self {
            terminal: Some(terminal),
            keyboard_enhanced,
            mouse,
        })
    }

    /// Mutable handle to the wrapped terminal. Panics only if called
    /// after `Drop` has run, which by Rust's ownership rules is
    /// unreachable for safe callers.
    pub fn terminal(&mut self) -> &mut ShannonTerminal {
        self.terminal
            .as_mut()
            .expect("TerminalGuard::terminal called after Drop — unreachable")
    }
}

/// Force a full repaint of the next frame by wiping ratatui's
/// previous-frame cache. Used to heal external screen corruption —
/// text-selection highlights, foreign cursor moves, paste echo,
/// anything painted by the terminal emulator outside our virtual
/// buffer. ratatui's diff renderer only emits cells it thinks
/// changed and cannot recover from out-of-band writes without an
/// explicit clear.
///
/// `reason` flows into the `anyhow` context so a failure tells you
/// *why* we were repainting, not just that we tried.
///
/// **`Terminal::resize` and NOT `Terminal::clear`,** which is the same wipe
/// plus one fatal extra step: `clear` snapshots the cursor with
/// `get_cursor_position`, and crossterm answers that by writing a DSR query and
/// *reading the reply out of the shared input queue*. While an
/// [`EventStream`](crossterm::event::EventStream) is alive its wake thread is
/// parked in `poll_internal(None)` **holding that queue's mutex**, so the read
/// cannot happen: it stalls until the lock times out and then returns
/// `Err("The cursor position could not be read within a normal duration")`,
/// which we propagate out of the event loop and the program exits. Every
/// repaint path was affected — a terminal resize, `<C-l>`, and the resume from
/// `$EDITOR` — so *resizing the window killed the app*.
///
/// `resize` reaches the identical place for our viewport (`Fullscreen`, so
/// `clear_viewport` issues `ClearType::All` and resets the back buffer) and
/// touches no cursor at all. Passing the terminal's *current* size makes it a
/// pure repaint rather than a geometry change; the caller that actually
/// resizes has already clamped the workspace.
///
/// The rule, since this is not the only such call: **nothing on the event
/// loop's thread may use a crossterm API that BLOCKS on the input queue.**
/// That is `cursor::position`, `supports_keyboard_enhancement`, and
/// `event::read` — each waits for a specific reply that only the task owning
/// the stream can deliver. See also [`crate::input`], where the same shared
/// queue deadlocks in the other direction.
///
/// `event::poll(timeout)` is the exception and it is worth naming, because
/// `drain_stale_input` uses it and the two comments used to contradict each
/// other (Reviewer B). It takes the reader lock with `try_lock_for` and returns
/// `Ok(false)` when it cannot get it, so it degrades to a no-op rather than a
/// deadlock — **but a no-op is what it becomes whenever the wake thread is
/// parked**, which is the case on the `stream_rx` arm. The one `$EDITOR`
/// opened from that arm — a distillation's review, as it arrived — is gone
/// since A2.2: every editor now opens from a key (`ToolKeyEffect::Edit`, the
/// key arm), where the drain runs (review stop 8, B 10; traced, the smoke
/// run to confirm no OSC-11 tail after `<CR>` in a review).
pub fn force_repaint(terminal: &mut ShannonTerminal, reason: &'static str) -> Result<()> {
    let area = terminal
        .size()
        .with_context(|| format!("force repaint: {reason}: query size"))?;
    terminal
        .resize(area.into())
        .with_context(|| format!("force repaint: {reason}"))
}

/// Run `f` with the TUI **suspended** — leave the alt-screen, drop raw mode,
/// bracketed paste, focus reports and mouse capture, give the cursor its
/// own colour back and show it — so a spawned child (the
/// `$EDITOR`) gets a normal cooked terminal. `mouse` is whether capture is ON
/// (`App::mouse_captured` — what launch set, not the live `tui.mouse` setting),
/// so the resume restores capture iff it was on (leaving it on would feed the
/// editor our mouse escape sequences as garbage). The TUI is **resumed
/// afterward unconditionally**, even if `f` panics: the resume lives in a `Drop`
/// guard, so a crashing editor can't strand the user in a half-cooked terminal
/// (the criterion-9 invariant "cleanly restored even if the spawn path errors").
/// The next frame is force-repainted since the editor scribbled over our screen.
/// `f` must not touch the terminal (it's borrowed by the guard); it just spawns
/// + waits.
pub fn with_terminal_suspended<T>(
    terminal: &mut ShannonTerminal,
    mouse: bool,
    f: impl FnOnce() -> T,
) -> Result<T> {
    // Drop the keyboard-protocol flag (if we pushed one) so the child editor
    // gets a plain cooked terminal; the resume guard re-pushes it. Read from
    // what startup actually pushed rather than re-asking the terminal, so pop
    // and push can never disagree about what is on the stack.
    let keyboard_enhanced = pushed_keyboard_enhancement();
    // **The resume first, the suspend through it.** A step below that fails
    // returns early, and it returned past a guard not made yet: the
    // keyboard flags popped and the mouse off, or raw mode off as well — the
    // TUI went on in a cooked terminal, keys echoing (review stop 8, B 6).
    // Made first, the guard puts back every step on every way out; each of
    // its steps is harmless on a terminal the matching step never left.
    let resume = ResumeGuard {
        terminal,
        keyboard_enhanced,
        mouse,
    };
    if keyboard_enhanced {
        let _ = execute!(resume.terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    // Likewise drop mouse capture, or the editor receives our mouse escape
    // sequences as stray input; the resume guard re-enables it.
    if mouse {
        let _ = disable_mouse(resume.terminal.backend_mut());
    }
    // The editor gets the terminal's cursor; the first frame after the
    // resume sets ours again.
    let _ = CURSOR.give_back(resume.terminal.backend_mut());
    disable_raw_mode().context("suspend: disable raw mode")?;
    // Focus reports off too: an editor that did not ask for them reads
    // `ESC [ O` as keys — in vim's Normal mode, `O` opens a line.
    execute!(
        resume.terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableFocusChange
    )
    .context("suspend: leave alt screen")?;
    let _ = resume.terminal.show_cursor();
    let out = f();
    drop(resume);
    Ok(out)
}

/// Resumes the TUI on `Drop` (so it runs after `f` returns *or* unwinds). Each
/// step is best-effort — like `TerminalGuard::drop`, restoring the terminal
/// matters more than surfacing a teardown error from a destructor.
struct ResumeGuard<'a> {
    terminal: &'a mut ShannonTerminal,
    /// Re-push the keyboard-enhancement flag on resume iff suspend popped it.
    keyboard_enhanced: bool,
    /// Re-enable mouse capture on resume iff suspend disabled it.
    mouse: bool,
}

impl Drop for ResumeGuard<'_> {
    fn drop(&mut self) {
        let _ = enable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableFocusChange
        );
        if self.keyboard_enhanced {
            // **Through the recorder, like the startup push.** This pushed
            // directly and left the atomic saying `true` even when the
            // `execute!` failed — so the exit pop would pop a flag that was
            // never on the stack, which is the exact failure `only one code
            // path pushes` was written to prevent (Reviewer B). Two pushers,
            // one recorder.
            let _ = push_keyboard_enhancement(self.terminal.backend_mut());
        }
        if self.mouse {
            let _ = enable_mouse(self.terminal.backend_mut());
        }
        drain_stale_input();
        // Force a full repaint over the editor's output. Best-effort like the
        // rest of this guard, but it goes through `force_repaint` so it is the
        // same wipe the rest of the app uses — including the part where it does
        // not query the cursor.
        let _ = force_repaint(self.terminal, "editor resume");
    }
}

/// Swallow input that queued up across an editor suspend/resume. Editors query
/// the terminal on startup (vim sends OSC 11 to detect the background color);
/// a reply that races the handover lands in *our* input stream and gets parsed
/// as keystrokes — the observed `:0e0e/1010/1a1ag` bug was the tail of
/// `ESC ] 11 ; rgb:0e0e/1010/1a1a BEL` typing itself into the command line
/// (vim consumed the head, we got the rest: `:` opened the cmdline, `^G` was
/// the terminator). Anything buffered in the first instants after resume is
/// terminal chatter, not the user, so drain until the line goes quiet (with a
/// hard deadline — a held-down key must not pin us here). Blocking
/// `poll`/`read` is safe: the async `EventStream` shares crossterm's internal
/// queue and isn't polled while we're inside a key handler.
fn drain_stale_input() {
    use std::time::{Duration, Instant};
    // One quiet-window is enough for a queued reply; the deadline caps the
    // total pause well under perception after an editor session.
    let deadline = Instant::now() + Duration::from_millis(50);
    let quiet = Duration::from_millis(15);
    loop {
        let window = quiet.min(deadline.saturating_duration_since(Instant::now()));
        match crossterm::event::poll(window) {
            Ok(true) if Instant::now() < deadline => {
                let _ = crossterm::event::read();
            }
            _ => break, // quiet, deadline hit, or poll error: stop draining
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Restore in the reverse order of setup. Each step is
        // best-effort and we deliberately do NOT log from `Drop`:
        // when unwinding from a panic, the tracing appender's worker
        // may already be torn down; logging from here would risk a
        // double-panic and abort the process. The terminal is more
        // important to restore than a teardown warning is to record
        // — if both fail, the user can run `reset`.
        let _ = disable_raw_mode();
        if let Some(mut terminal) = self.terminal.take() {
            // Pop the keyboard-enhancement flag we pushed (only if we pushed it),
            // so the protocol stack is left as we found it.
            if self.keyboard_enhanced {
                let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
            }
            if self.mouse {
                let _ = disable_mouse(terminal.backend_mut());
            }
            let _ = CURSOR.give_back(terminal.backend_mut());
            let _ = execute!(
                terminal.backend_mut(),
                DisableFocusChange,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
            let _ = terminal.show_cursor();
        }
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::{COLOUR_OWN, Cursor, CursorTaken, SHAPE_OWN};

    fn written(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    const LATTE: Option<(u8, u8, u8)> = Some((0x4c, 0x4f, 0x69));

    fn coloured(colour: Option<(u8, u8, u8)>) -> Cursor {
        Cursor {
            colour,
            shape: None,
        }
    }

    fn shaped(shape: Option<u8>) -> Cursor {
        Cursor {
            colour: None,
            shape,
        }
    }

    /// **The colour is written when it changes, and only then** — a frame
    /// that keeps it writes nothing, at thousands of frames a second; a
    /// change of theme writes the new one; no colour gives the terminal's
    /// back.
    #[test]
    fn a_cursor_colour_is_written_once_per_change() {
        let cursor = CursorTaken::new();
        let set = written(|out| cursor.paint(out, coloured(LATTE)).unwrap());
        assert_eq!(set, "\x1b]12;#4c4f69\x07");
        assert_eq!(
            written(|out| cursor.paint(out, coloured(LATTE)).unwrap()),
            ""
        );
        let mocha = Some((0xcd, 0xd6, 0xf4));
        assert_eq!(
            written(|out| cursor.paint(out, coloured(mocha)).unwrap()),
            "\x1b]12;#cdd6f4\x07"
        );
        assert_eq!(
            written(|out| cursor.paint(out, coloured(None)).unwrap()),
            COLOUR_OWN
        );
        assert_eq!(
            written(|out| cursor.paint(out, coloured(None)).unwrap()),
            ""
        );
    }

    /// **The shape likewise** — DECSCUSR's number when the mode's shape
    /// changes (`i`: a bar), nothing while it holds, DECSCUSR 0 when
    /// `tui.guicursor` is emptied; and apart from the colour, which a
    /// change of shape does not write again.
    #[test]
    fn a_cursor_shape_is_written_once_per_change() {
        let cursor = CursorTaken::new();
        assert_eq!(
            written(|out| cursor.paint(out, shaped(Some(2))).unwrap()),
            "\x1b[2 q"
        );
        assert_eq!(
            written(|out| cursor.paint(out, shaped(Some(2))).unwrap()),
            ""
        );
        assert_eq!(
            written(|out| cursor.paint(out, shaped(Some(6))).unwrap()),
            "\x1b[6 q"
        );
        assert_eq!(
            written(|out| cursor.paint(out, shaped(None)).unwrap()),
            SHAPE_OWN
        );
        let both = Cursor {
            colour: LATTE,
            shape: Some(6),
        };
        assert_eq!(
            written(|out| cursor.paint(out, both).unwrap()),
            "\x1b]12;#4c4f69\x07\x1b[6 q"
        );
        let barred = Cursor {
            shape: Some(2),
            ..both
        };
        assert_eq!(
            written(|out| cursor.paint(out, barred).unwrap()),
            "\x1b[2 q",
            "the colour held"
        );
    }

    /// **Given back only if taken, and forgotten** — a Shannon that never
    /// set the cursor resets nothing at a suspend or an exit; one that set
    /// both gives both back, through the one door; and the frame after a
    /// resume sets them again.
    #[test]
    fn a_cursor_is_given_back_once_and_taken_again() {
        let cursor = CursorTaken::new();
        assert_eq!(
            written(|out| cursor.give_back(out).unwrap()),
            "",
            "never taken"
        );
        let both = Cursor {
            colour: LATTE,
            shape: Some(6),
        };
        cursor.paint(&mut Vec::new(), both).unwrap();
        assert_eq!(
            written(|out| cursor.give_back(out).unwrap()),
            format!("{COLOUR_OWN}{SHAPE_OWN}")
        );
        assert_eq!(written(|out| cursor.give_back(out).unwrap()), "", "once");
        cursor.paint(&mut Vec::new(), shaped(Some(2))).unwrap();
        assert_eq!(
            written(|out| cursor.give_back(out).unwrap()),
            SHAPE_OWN,
            "only what was taken"
        );
        assert_eq!(
            written(|out| cursor.paint(out, both).unwrap()),
            "\x1b]12;#4c4f69\x07\x1b[6 q",
            "the frame after a resume"
        );
    }
}

#[cfg(test)]
mod mouse_mode_tests {
    use super::{MOUSE_OFF, MOUSE_ON};

    /// **We never ask for any-event tracking.**
    ///
    /// `?1003h` makes the terminal report every pointer movement with no button
    /// held. The event loop repaints once per event, so on an expensive frame a
    /// moving mouse outruns the redraw and stacks a backlog that reads as a
    /// hang. crossterm's `EnableMouseCapture` sends it; we send the three modes
    /// we actually use and nothing else.
    #[test]
    fn we_do_not_ask_for_motion_with_no_button_held() {
        assert!(
            !MOUSE_ON.contains("1003"),
            "any-event tracking: {MOUSE_ON:?}"
        );
        assert!(!MOUSE_OFF.contains("1003"));
        // …and we DO ask for the three that click-focus, drag-select and SGR
        // coordinates need, or the feature quietly stops working instead.
        for mode in ["1000", "1002", "1006"] {
            assert!(MOUSE_ON.contains(mode), "{mode} missing from {MOUSE_ON:?}");
        }
    }

    /// **The functions write what their names say.**
    ///
    /// Reviewer C swapped the two bodies — `enable_mouse` writing `MOUSE_OFF`
    /// and vice versa, so the terminal ends up in the wrong mode after every
    /// launch, suspend and resume — and all 951 tests passed. The two tests
    /// here inspected the string CONSTANTS and never the wiring, which is the
    /// wiring for the bug this file was changed to fix. A constant nothing
    /// writes is documentation.
    #[test]
    fn the_enable_and_disable_functions_write_their_own_constants() {
        let mut on = Vec::new();
        super::enable_mouse(&mut on).expect("writes");
        assert_eq!(String::from_utf8(on).unwrap(), MOUSE_ON);

        let mut off = Vec::new();
        super::disable_mouse(&mut off).expect("writes");
        assert_eq!(String::from_utf8(off).unwrap(), MOUSE_OFF);
    }

    /// Every mode turned on is turned off, in reverse order. An asymmetric
    /// terminal-mode guard leaves the terminal wrong after `$EDITOR` or exit.
    #[test]
    fn the_pair_is_symmetric() {
        let on: Vec<&str> = MOUSE_ON.split("\x1b[?").filter(|s| !s.is_empty()).collect();
        let off: Vec<&str> = MOUSE_OFF
            .split("\x1b[?")
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(on.len(), off.len(), "{on:?} vs {off:?}");
        let numbers = |v: &[&str]| -> Vec<String> {
            v.iter()
                .map(|s| s.trim_end_matches(['h', 'l']).to_string())
                .collect()
        };
        let mut want = numbers(&on);
        want.reverse();
        assert_eq!(numbers(&off), want, "undone in reverse order");
        assert!(on.iter().all(|s| s.ends_with('h')));
        assert!(off.iter().all(|s| s.ends_with('l')));
    }
}
