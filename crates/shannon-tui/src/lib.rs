//! Shannon Code TUI — library surface.
//!
//! The crate is a library *and* a binary. `main.rs` is the thin entry
//! point (logging, backend resolution, tokio runtime, terminal guard);
//! everything else lives here as a public module so it can be reached
//! from outside the binary: benchmarks (`benches/`), integration tests,
//! and — the design.md §0 ambition — a future non-TUI frontend built on
//! the same modules rather than reaching into a binary's guts.
//!
//! Exposing these as `pub mod` is the flexibility bet the rest of the
//! project makes structurally: the binary is one consumer of this
//! library, not the place the code lives. Items that should stay
//! crate-internal remain `pub(crate)`; only the genuinely-public surface
//! (e.g. `Output`, `App::new`/`run`, `TerminalGuard`) crosses into the
//! binary.

/// **Every variant of a fieldless enum, complete by the compiler** — the
/// slice of the variants named, and beside it an exhaustive `match` over
/// exactly those names: one left out is a non-exhaustive match (E0004), one
/// named twice an unreachable pattern, denied. The list IS the match, so it
/// cannot omit what the type has.
///
/// What it replaced, twice (`Command::ALL`, `bindings::CONTEXTS`): a `next()`
/// chain said to make its list complete "by the compiler" — but its
/// exhaustive match gave each variant a successor, never a predecessor, so a
/// variant nothing pointed to was in neither list, and every test that
/// walked the list was green (review stop 12, T6 10).
///
/// **With one variant that holds another such enum** — `Type: A, B;
/// Nested(Inner)` — the match covers `Nested(_)` too, and the list is the
/// units, then `Nested` of each of `Inner::ALL` (itself made by this macro),
/// as a `Vec`: `PaneKind` and its pages, where a `next_kind` chain let a
/// variant wired in as a dead end truncate the walk.
macro_rules! every_variant {
    ($ty:ident: $($variant:ident),+ $(,)?) => {{
        #[deny(unreachable_patterns)]
        #[allow(dead_code)]
        fn every(of: $ty) {
            match of {
                $($ty::$variant => {})+
            }
        }
        &[$($ty::$variant),+]
    }};
    ($ty:ident: $($variant:ident),+ ; $nested:ident($inner:ident)) => {{
        #[deny(unreachable_patterns)]
        #[allow(dead_code)]
        fn every(of: $ty) {
            match of {
                $($ty::$variant => {})+
                $ty::$nested(_) => {}
            }
        }
        let mut all = vec![$($ty::$variant),+];
        all.extend($inner::ALL.iter().copied().map($ty::$nested));
        all
    }};
}

pub mod app;
pub mod bindings;
pub mod claude_sessions;
pub mod clipboard;
pub mod cmdline;
pub mod command;
pub(crate) mod decision_page;
pub mod editor;
pub(crate) mod effort_panel;
pub(crate) mod exit_signal;
pub(crate) mod facts;
pub mod flags;
pub mod fold;
pub mod graph_view;
pub mod guicursor;
pub mod help;
pub(crate) mod help_view;
pub mod highlight;
pub mod input;
pub mod keymap;
pub mod keys;
pub(crate) mod layout;
pub mod logging;
pub mod markdown;
pub(crate) mod model_picker;
pub(crate) mod options_chapter;
pub mod output;
pub(crate) mod page;
pub mod paint;
pub mod palette;
pub mod permission_view;
pub(crate) mod private_file;
pub mod prompt;
pub(crate) mod ring;
pub(crate) mod rows;
pub(crate) mod scrub;
pub(crate) mod search;
pub(crate) mod setup;
pub(crate) mod shelf;
pub mod shortcuts;
pub(crate) mod status_line;
pub mod style;
pub mod terminal;
pub mod text_view;
pub mod theme;
pub(crate) mod theme_picker;
pub(crate) mod tool_record;
pub mod trust;
pub mod tutor;
pub mod view;
pub mod viewport;
pub mod vim_keymap;
pub mod wm;
pub mod word;
pub(crate) mod workspace;
pub mod wrap;
