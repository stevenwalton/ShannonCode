//! **Every colour the UI paints, named by what it MEANS.**
//!
//! Colours were chosen at ~50 call sites across four files, so "what does red
//! mean here" could only be answered by reading each one — and the answer
//! differed. Red was already documented twice as "one vocabulary" (the decision
//! pane and the notice row say so in as many words) while a third site invented
//! its own, which is how the confirm bar came to fill red for every
//! confirmation including the one whose safe answer is `y`.
//!
//! So the names here are **roles, not hues**. A call site asks for
//! [`DANGER`] or [`OPERAND`]; it never says `Color::Red`. Two consequences:
//!
//! - A rule about colour is stated once. Changing what "danger" looks like is
//!   one edit, not a grep that has to be complete to be correct
//!   (`[[feedback_rules_rot_by_retyping]]`).
//! - Roles that happen to share a hue today keep **separate names**. A focused
//!   border, a tool block and a question are all cyan right now; they are not
//!   the same idea, and a configurable colourscheme is exactly the feature that
//!   will make them differ (`[[feedback_features_split_identities]]`).
//!
//! **ANSI slots, not RGB, everywhere we can.** A named colour resolves through
//! the user's own terminal theme — `MAGENTA` renders as whatever purple their
//! scheme defines — which is what a terminal-native app owes them
//! (`[[feedback_terminal_native]]`). The two truecolor holdouts are quarantined
//! at the bottom of this file and say why.
//!
//! **The shipped theme's values, and nothing else's.** Every colour the UI
//! paints is a theme role now (`ui.*`, Steven 10-14: "If there's a color
//! defined in our program, we should let someone be able to specify how
//! *they* want that drawn"), and `theme::TERMINAL` reads its defaults from
//! here. No render site names one of these: it asks the theme
//! (`Theme::ui`) or paints a role (`paint::Run::role`), so a theme file
//! reaches it — held by `only_the_theme_names_a_colour`, over every source
//! file.

use ratatui::style::Color;

// ---------------------------------------------------------------------------
// The danger scale
//
// One vocabulary across the decision pane, the confirm row and the notice
// line. Green/yellow/red is the scale the permission view already established
// and the confirm row was retrofitted onto; naming it keeps a fourth surface
// from inventing a fifth meaning for red.
// ---------------------------------------------------------------------------

/// Benign: an allowed command, a read-only stage, a passing check.
pub const SAFE: Color = Color::Green;

/// Worth reading before continuing: an `ask` verdict, a warn-tone stage, and
/// the "we did something and are telling you" notice.
pub const CAUTION: Color = Color::Yellow;

/// Stop: a denied command, a refusal, an unrecoverable action, and the
/// `WARNING:` sign on a confirmation.
pub const DANGER: Color = Color::Red;

// ---------------------------------------------------------------------------
// Confirmations
// ---------------------------------------------------------------------------

/// A question, not an alarm — the rung where `y` is the *protective* answer.
/// Deliberately outside the danger scale so it cannot be misread as "something
/// went wrong".
pub const QUESTION: Color = Color::Cyan;

/// **What the action lands on** — the id, the count, the recognised secret.
/// The part checked against the screen before answering.
///
/// Fixed across every severity on purpose: the operand's colour must not shift
/// with the rung, or a reader has two things to decode where there is one fact.
/// That is also why it is none of [`SAFE`]/[`CAUTION`]/[`DANGER`].
pub const OPERAND: Color = Color::Magenta;

// ---------------------------------------------------------------------------
// Window chrome
// ---------------------------------------------------------------------------

/// The focused pane's border and title.
pub const FOCUSED: Color = Color::Cyan;

/// A pane that is not focused but *is* where the prompt is aimed — the middle
/// state, which needs to be visible without competing with focus.
pub const AIMED: Color = Color::LightCyan;

/// An unfocused border, and the placeholder text sitting inside one. `Gray`
/// rather than `DarkGray`: it has to be legible enough to tell you to read it.
pub const CHROME: Color = Color::Gray;

// ---------------------------------------------------------------------------
// Text weights
// ---------------------------------------------------------------------------

/// Ordinary foreground text the user is meant to read.
pub const TEXT: Color = Color::White;

/// Quieter than [`TEXT`]: prose around a marked run, empty-state messages.
pub const MUTED: Color = Color::Gray;

/// Quietest: annotations, the app talking *about* a message rather than saying
/// it, and the elision marker.
pub const FAINT: Color = Color::DarkGray;

/// Rows past the end of a page — vim's `~` in its `NonText` colour (blue in
/// the default scheme). Its own hue because it is its own KIND: not the
/// document, and not the app talking about it either.
pub const NON_TEXT: Color = Color::Blue;

/// Foreground for a filled chip — an inverse-video tag or a selected
/// completion. Pairs with a loud background.
pub const INVERSE_FG: Color = Color::Black;

/// Background for the selected wildmenu candidate.
pub const INVERSE_BG: Color = Color::White;

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

/// A vaulted secret's handle where its value would have been. Shares a hue
/// with [`OPERAND`] and not a name: both say "this stands for something you
/// should look at", but one is a confirmation's target and the other is
/// transcript content, and a theme may well want them apart.
pub const SECRET: Color = Color::Magenta;

/// A tool-use / tool-result block in the transcript.
pub const TOOL: Color = Color::Cyan;

/// The `:graph` tree's connector glyphs.
pub const TREE: Color = Color::Cyan;

/// A session with a turn in flight.
pub const RUNNING: Color = Color::Yellow;

/// A session parked waiting on a decision.
pub const HELD: Color = Color::Cyan;

/// A Claude Code conversation a Shannon session continues — apart from the
/// plain conversation's `TOOL` cyan, and off every alarm's hue (yellow is
/// caution, red danger, green safe). A default to repaint (`ui.graph.continued`).
pub const CONTINUED: Color = Color::LightMagenta;

// ---------------------------------------------------------------------------
// Sequences
// ---------------------------------------------------------------------------

/// Per-branch colours for `:graph` lanes, cycled by lane index.
///
/// Avoids gray, which is the unfocused-chrome colour — a lane painted like a
/// border reads as inactive.
pub const LANES: [Color; 6] = [
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Red,
];

/// Colours for the analyser's data-carrier annotations, cycled per carrier.
/// Cool hues only: the warm end of the spectrum is the danger scale, and a
/// carrier is neutral information.
pub const CARRIERS: [Color; 4] = [Color::Cyan, Color::Magenta, Color::Blue, Color::LightCyan];

// ---------------------------------------------------------------------------
// `:help` markup
//
// The manual is vim-help-shaped, so these mirror vim's own highlight groups:
// a heading, a `*tag*`, a `|reference|`, and `code`.
// ---------------------------------------------------------------------------

/// A section heading in the manual.
pub const HELP_HEADING: Color = Color::Yellow;

/// A `*tag*` — the anchor a `|reference|` jumps to.
pub const HELP_TAG: Color = Color::Green;

/// A `|reference|` — a jump to a tag.
pub const HELP_REF: Color = Color::Cyan;

/// An inline `code` span — and a reference on a listing page (`:rules`'
/// `[permissions]`, `:allow`), the same kind of thing: what you type.
pub const HELP_CODE: Color = Color::Magenta;

/// A section heading on a listing page (`:rules`). Not the manual's
/// yellow: there yellow is `ask`, and a heading must not read as a verdict
/// (Steven, 10-11). Cool and neutral, off the verdict scale.
pub const LISTING_HEADING: Color = Color::Cyan;

// ---------------------------------------------------------------------------
// Truecolor holdouts
//
// **These two do NOT follow the user's terminal theme**, which every other
// colour in this file does. They are 24-bit values chosen against a dark
// background, so on a light scheme they will be wrong — a muted indigo behind
// dark text, and a light gray on white.
//
// Left as they are rather than guessed at: replacing them means picking a new
// appearance, and that is a judgement to make while looking at a terminal
// rather than inferring from a hex triple. Docketed with the colourscheme work
// in `idea_list.md`. Naming them here is the point — they were two anonymous
// literals in the middle of a render function, and nothing said they were the
// exception.
// ---------------------------------------------------------------------------

/// A `:graph` row whose turns belong to another session — rendered italic and
/// faded, a light gray between the annotation colour and ordinary text.
pub const SHARED_ROW: Color = Color::Rgb(176, 176, 176);

/// The background of a row marked with `mf`/`<Space>`. Persists as the cursor
/// moves on, so a multi-select reads at a glance.
pub const MARK_BG: Color = Color::Rgb(58, 58, 92);

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The files allowed to name a colour: the theme module — `theme.rs`,
    /// which turns roles into colours and reads its defaults from here, and
    /// its submodules (`theme/ghostty.rs` reads a palette's colours) — and
    /// this file.
    const NAMERS: &[&str] = &["theme.rs", "palette.rs"];

    /// Is `path` one of [`NAMERS`], or in the theme module's directory?
    fn names_colours(path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let parent = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str());
        NAMERS.contains(&name) || parent == Some("theme")
    }

    /// Every `.rs` file under `src/`, but the test files — read at test time,
    /// so a file added tomorrow is in the gate the day it is added. A list
    /// of files would be a list that cannot check it lists everything.
    fn sources() -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("src reads").flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if path.is_dir() {
                    if name != "tests" {
                        dirs.push(path);
                    }
                } else if name.ends_with(".rs") && name != "tests.rs" {
                    out.push(path);
                }
            }
        }
        out
    }

    /// What in `src` (named `name`) names a colour outside a test module.
    ///
    /// **Skips test MODULES, not every `#[cfg(test)]` item.** The first
    /// attempt truncated the scan at the first `#[cfg(test)]`, which in
    /// `view.rs` was a test *helper fn* — so the whole confirm row, 900 lines
    /// further down, was never looked at and the gate passed for the wrong
    /// reason. Found by mutation. A top-level `mod tests {` closes with `}`
    /// in column 0 and everything inside it is indented, so that is the
    /// terminator — robust against the braces in a test's `format!` strings,
    /// which a depth counter would miscount.
    fn strays(name: &str, src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut skipping = false;
        let mut after_cfg = false;
        for (number, line) in src.lines().enumerate() {
            if skipping {
                skipping = line != "}";
                continue;
            }
            let code = line.trim();
            if code == "#[cfg(test)]" {
                after_cfg = true;
                continue;
            }
            if std::mem::take(&mut after_cfg) && line.starts_with("mod ") {
                skipping = true;
                continue;
            }
            // A comment naming a colour is prose, not a call site.
            if code.starts_with("//") {
                continue;
            }
            if line.contains("Color::") || line.contains("palette::") {
                out.push(format!("{name}:{} {code}", number + 1));
            }
        }
        out
    }

    /// **Only the theme names a colour** (Steven, 10-14: every colour the
    /// program draws is the user's to choose). A colour typed at a render
    /// site — `Color::Red`, or `palette::DANGER`, the shipped value — is one
    /// no theme reaches: it compiles, it renders, and a theme file setting
    /// that role changes nothing there (`[[feedback_rules_rot_by_retyping]]`).
    /// Every site asks `Theme::ui` or paints a role (`paint::Run::role`).
    ///
    /// Test modules are skipped: a test asserting a *rendered* colour must
    /// name one, and that is the point of those tests.
    #[test]
    fn only_the_theme_names_a_colour() {
        let mut stray = Vec::new();
        for path in sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if names_colours(&path) {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("a source reads");
            stray.extend(strays(name, &src));
        }
        assert!(
            stray.is_empty(),
            "these sites choose a colour instead of asking the theme what a role \
             looks like — name a role (`style::Ink`, `ui.*`) and ask `Theme::ui`, \
             so a theme reaches it:\n{}",
            stray.join("\n")
        );
    }

    /// The premise: the scan reads the real sources, and would catch a stray.
    /// A gate searching for a pattern its input can never contain passes for
    /// the wrong reason (`[[feedback_assert_the_subject]]`).
    #[test]
    fn the_scan_reads_real_sources_and_would_catch_a_stray() {
        let names: Vec<String> = sources()
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_string))
            .collect();
        for expected in ["view.rs", "permission_view.rs", "setup.rs", "app.rs"] {
            assert!(names.iter().any(|n| n == expected), "{expected}: {names:?}");
        }
        assert!(
            !names.iter().any(|n| n == "tests.rs"),
            "test files are skipped"
        );
        let planted = "fn f() {\n    let s = Style::default().fg(palette::DANGER);\n}\n";
        assert_eq!(
            strays("x.rs", planted).len(),
            1,
            "a palette value is caught"
        );
        let planted = "fn f() {\n    let s = Style::default().fg(Color::Red);\n}\n";
        assert_eq!(strays("x.rs", planted).len(), 1, "a colour is caught");
        let tested = "#[cfg(test)]\nmod tests {\n    let c = Color::Red;\n}\n";
        assert!(strays("x.rs", tested).is_empty(), "a test module is not");
    }
}
