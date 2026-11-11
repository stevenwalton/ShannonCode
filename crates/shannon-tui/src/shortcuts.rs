//! Above-the-keymap shortcuts: the keys resolved *before* a pane's modal
//! keymap sees them.
//!
//! `classify(key)` turns a `KeyEvent` into a `Shortcut`. Only the keys that
//! must work identically regardless of pane or mode live here:
//! - **Truly global, un-shadowable** keys — `Ctrl-C` (interrupt), `Ctrl-L`
//!   (redraw) — checked first so no mode or rebinding can swallow them.
//! - **Pane-agnostic Page scroll** (`PgUp`/`PgDn`) — scrolls the focused
//!   pane, mode-independent.
//!
//! Everything else — including focus switching, which used to be a single
//! mode-aware `Ctrl-W` here — is now a keymap atom: focus is `<C-w>w`, part
//! of the `<C-w>` window grammar (see `bindings.rs`). So `classify` is no
//! longer mode-aware; it's a short, fixed set of genuinely-global keys and
//! takes no mode.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::keymap::{ScrollCmd, ScrollMagnitude::Page};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// `Ctrl-C` — **vim's interrupt**, and it is the handler's job to say what
    /// there is to interrupt: a turn in flight is stopped and *kept*, and with
    /// nothing running the only thing left to stop is the program.
    ///
    /// Deliberately classified without consulting app state. The classifier
    /// answers "which key", never "what is happening" — a stateless `classify`
    /// is what keeps this set genuinely un-shadowable.
    ///
    /// Until 2026-08-07 this was `Quit` unconditionally, so there was **no way
    /// to stop a running turn at all**: a prompt you regretted billed to
    /// completion unless you killed the program. The first fix kept a quit for
    /// the idle case, justified as the escape hatch a wedged TUI needs — which
    /// does not survive inspection, since this key is processed by the same
    /// event loop as `:qa`, so if the loop runs both work and if it does not
    /// neither arrives. It no longer quits at all (Steven's ruling).
    Interrupt,
    ForceRedraw,
    /// Scroll the focused pane (PgUp / PgDn — mode-independent).
    Scroll(ScrollCmd),
    /// No global shortcut matched; forward the key to the focused pane's
    /// keymap.
    None,
    /// **Not a press** — a key RELEASE, which terminals speaking the kitty
    /// keyboard protocol report alongside the press.
    ///
    /// Classified here rather than filtered at each handler because five of
    /// them read `key.code` directly (the graph, the decision pane, the text
    /// pages, the confirm line and `:`) and would each act twice. The keymap
    /// filtered releases and said why; the other five did not, and the door
    /// that fixed that had no test because it lived inside `handle_event`,
    /// which needs a real terminal. It lives in the classifier now: one place,
    /// consulted by the one match, and a pure function a test can ask
    /// (Reviewer D).
    Ignore,
}

pub fn classify(key: &KeyEvent) -> Shortcut {
    // Before everything, including the un-shadowable keys: a `Ctrl-C` release
    // must not interrupt a second time.
    if key.kind == KeyEventKind::Release {
        return Shortcut::Ignore;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Shortcut::Interrupt,
        KeyCode::Char('l') if ctrl => Shortcut::ForceRedraw,
        KeyCode::PageUp => Shortcut::Scroll(ScrollCmd::Up(Page)),
        KeyCode::PageDown => Shortcut::Scroll(ScrollCmd::Down(Page)),
        _ => Shortcut::None,
    }
}

/// **A key as vim reads it**, whatever the terminal's encoding: `<C-[>` is
/// `<Esc>` (`:h i_CTRL-[`). In a terminal's legacy encoding the two are one
/// byte (Ctrl clears bits 5 and 6, so `[` 0x5B is ESC 0x1B), which is why it
/// worked wherever that encoding reached us — through tmux (Steven, 11-03).
/// The kitty protocol Shannon asks for (`terminal.rs`,
/// `DISAMBIGUATE_ESCAPE_CODES`) reports it as its own key, Ctrl + `[`, which
/// nothing binds: in a terminal speaking it directly, the key many vim users
/// leave Insert mode with did nothing.
pub fn as_vim_reads(key: KeyEvent) -> KeyEvent {
    match key.code {
        KeyCode::Char('[') if key.modifiers == KeyModifiers::CONTROL => KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            ..key
        },
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// **A release is not a press.** Terminals speaking the kitty keyboard
    /// protocol report both, and five handlers downstream read `key.code`
    /// directly — so an unfiltered release makes every graph, decision and
    /// confirm key fire twice.
    ///
    /// Checked for the un-shadowable keys too: a `Ctrl-C` release interrupting
    /// a second time is the worst version of this.
    #[test]
    fn a_key_release_is_ignored() {
        for (code, mods) in [
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
            (KeyCode::Char('l'), KeyModifiers::CONTROL),
            (KeyCode::PageDown, KeyModifiers::NONE),
        ] {
            let mut k = key(code, mods);
            assert_ne!(
                classify(&k),
                Shortcut::Ignore,
                "premise: {code:?} means something as a PRESS"
            );
            k.kind = KeyEventKind::Release;
            assert_eq!(
                classify(&k),
                Shortcut::Ignore,
                "{code:?} released must not act a second time"
            );
        }
    }

    #[test]
    fn ctrl_c_is_the_interrupt() {
        assert_eq!(
            classify(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Shortcut::Interrupt,
        );
    }

    #[test]
    fn ctrl_l_is_force_redraw() {
        assert_eq!(
            classify(&key(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Shortcut::ForceRedraw,
        );
    }

    #[test]
    fn ctrl_w_falls_through_to_the_keymap() {
        // Focus is `<C-w>w` now (the window grammar), so `<C-w>` is no longer a
        // global shortcut — it flows to the pane keymap as the window prefix.
        assert_eq!(
            classify(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Shortcut::None,
        );
    }

    #[test]
    fn page_keys_scroll() {
        assert_eq!(
            classify(&key(KeyCode::PageUp, KeyModifiers::NONE)),
            Shortcut::Scroll(ScrollCmd::Up(Page)),
        );
        assert_eq!(
            classify(&key(KeyCode::PageDown, KeyModifiers::NONE)),
            Shortcut::Scroll(ScrollCmd::Down(Page)),
        );
    }

    #[test]
    fn plain_keys_fall_through() {
        assert_eq!(
            classify(&key(KeyCode::Char('j'), KeyModifiers::NONE)),
            Shortcut::None,
        );
    }
}
