//! **The cursor's shape in each mode** — `tui.guicursor`, neovim's
//! `'guicursor'` read as neovim reads it. A change of mode shows on the
//! status row and, at once, where you type: a bar in Insert, a block in
//! Normal, an underline while an operator waits for its motion (Steven,
//! 11-02: two signals for one change of state).
//!
//! Neovim's source decided each rule here, not memory: the mode a shape is
//! chosen by is `cursor_get_mode_idx` (`cursor_shape.c`) — the `:` line by
//! where its cursor is, then an operator waiting, then the keymap's mode;
//! the value's reading is `parse_shape_opt` (every mode a steady block, then
//! each part overwrites only what it names; an error anywhere refuses the
//! value); the escape is `tui_set_mode` (`tui.c`: DECSCUSR, steady unless
//! `blinkon` and `blinkoff` are both above 0). A highlight group in a part
//! is read and unused: the cursor's colour is the theme's `ui.cursor`.

use crate::keymap::Mode;

/// **The modes a shape is chosen by** — `'guicursor'`'s mode list, the ones
/// Shannon enters. `ve`, `sm`, `cr` and `t` read too, and name modes it
/// never enters: an inclusive selection only, no showmatch, no overstrike
/// on the `:` line, no terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    /// `n`
    Normal,
    /// `v` — every Visual (`v`, `V`, `CTRL-V`).
    Visual,
    /// `o` — an operator waiting for its motion (`d`, `c`, `y`).
    Operator,
    /// `i`
    Insert,
    /// `r` — Replace, and `r` waiting for its character (neovim pretends
    /// Replace there: `nvim_get_mode()` says `R`).
    Replace,
    /// `c` — the `:` line, its cursor at the end.
    Cmdline,
    /// `ci` — the `:` line, its cursor inside it.
    CmdlineInsert,
}

impl CursorMode {
    /// Every one — the compiler refuses a list that leaves one out.
    pub(crate) const ALL: &'static [CursorMode] = every_variant!(
        CursorMode: Normal,
        Visual,
        Operator,
        Insert,
        Replace,
        Cmdline,
        CmdlineInsert,
    );

    /// How many there are: the length of a [`Shapes`] table, which a mode
    /// indexes by its discriminant.
    const COUNT: usize = Self::ALL.len();

    /// A mode-list name as `'guicursor'` writes it (case ignored, as neovim
    /// does): the modes it covers — none for a mode Shannon never enters —
    /// or `None` for a name that is no mode.
    fn named(name: &str) -> Option<&'static [CursorMode]> {
        use CursorMode::*;
        Some(match name.to_ascii_lowercase().as_str() {
            "a" => Self::ALL,
            "n" => &[Normal],
            "v" => &[Visual],
            "o" => &[Operator],
            "i" => &[Insert],
            "r" => &[Replace],
            "c" => &[Cmdline],
            "ci" => &[CmdlineInsert],
            "ve" | "sm" | "cr" | "t" => &[],
            _ => return None,
        })
    }
}

impl From<Mode> for CursorMode {
    /// A keymap's own mode, nothing waiting.
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Normal => CursorMode::Normal,
            Mode::Insert => CursorMode::Insert,
            Mode::Replace => CursorMode::Replace,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => CursorMode::Visual,
        }
    }
}

/// The three shapes a terminal draws (a percentage is a GUI's to honour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Block,
    Bar,
    Underline,
}

/// One mode's cursor: its shape and its blink times, as a part wrote them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Look {
    shape: Shape,
    blinkon: u32,
    blinkoff: u32,
}

impl Look {
    /// What every mode is before a part names it: a steady block.
    const RESET: Look = Look {
        shape: Shape::Block,
        blinkon: 0,
        blinkoff: 0,
    };

    /// DECSCUSR's number for it: 1, 3, 5 blinking, one more steady.
    fn decscusr(self) -> u8 {
        let blinking = match self.shape {
            Shape::Block => 1,
            Shape::Underline => 3,
            Shape::Bar => 5,
        };
        blinking + u8::from(self.blinkon == 0 || self.blinkoff == 0)
    }
}

/// **`tui.guicursor`, read**: each mode's cursor, or `None` — an empty
/// value — for the terminal's own shape, untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shapes(Option<[Look; CursorMode::COUNT]>);

impl Default for Shapes {
    /// The shipped `tui.guicursor`, read — core's one string, not a copy.
    fn default() -> Self {
        Shapes::read(&shannon_core::config::TuiConfig::default().guicursor)
            .expect("the shipped guicursor reads (the_shipped_guicursor_reads_as_neovims)")
    }
}

impl Shapes {
    /// **Read a `'guicursor'` value** — `mode-list:argument-list` parts,
    /// comma-separated — or say why not. As neovim: every mode starts a
    /// steady block, and a part overwrites only what it names, for each
    /// mode it lists (`a` every mode).
    pub(crate) fn read(spec: &str) -> Result<Shapes, String> {
        if spec.is_empty() {
            return Ok(Shapes(None));
        }
        let mut looks = [Look::RESET; CursorMode::COUNT];
        // A part ends at a comma; one after the last part is no part.
        for part in spec.strip_suffix(',').unwrap_or(spec).split(',') {
            let Some((modes, args)) = part.split_once(':') else {
                return Err(format!(
                    "tui.guicursor: `{part}` has no `:` (mode-list:argument-list)"
                ));
            };
            let mut named: Vec<CursorMode> = Vec::new();
            for name in modes.split('-') {
                let Some(modes) = CursorMode::named(name) else {
                    return Err(format!(
                        "tui.guicursor: `{name}` in `{part}` is no mode \
                         (n v o i r c ci cr ve sm t a)"
                    ));
                };
                named.extend_from_slice(modes);
            }
            for arg in args.split('-').filter(|arg| !arg.is_empty()) {
                let arg = Arg::read(arg, part)?;
                for &mode in &named {
                    arg.apply(&mut looks[mode as usize]);
                }
            }
        }
        Ok(Shapes(Some(looks)))
    }

    /// **The DECSCUSR number for `mode`**, or `None` to leave the
    /// terminal's own shape (`tui.guicursor` empty).
    pub(crate) fn code(&self, mode: CursorMode) -> Option<u8> {
        self.0.map(|looks| looks[mode as usize].decscusr())
    }
}

/// **One argument of a part**: what it does to each mode the part lists.
#[derive(Debug, Clone, Copy)]
enum Arg {
    Shape(Shape),
    BlinkOn(u32),
    BlinkOff(u32),
    /// `blinkwait` (a terminal blinks on its own clock) and a highlight
    /// group (the colour is the theme's `ui.cursor`): read, and unused.
    Unused,
}

impl Arg {
    /// Read `arg`, from `part`: neovim's keywords, case ignored — a number
    /// where one is due, a bar or underline above 0% — and any other word a
    /// highlight group.
    fn read(arg: &str, part: &str) -> Result<Arg, String> {
        let lower = arg.to_ascii_lowercase();
        let number = |keyword: &str| -> Result<u32, String> {
            let digits = &lower[keyword.len()..];
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!(
                    "tui.guicursor: `{arg}` in `{part}` needs a number after `{keyword}`"
                ));
            }
            // Only zero against not zero matters past here: saturate.
            Ok(digits.parse().unwrap_or(u32::MAX))
        };
        let bar = |keyword: &str, shape: Shape| -> Result<Arg, String> {
            match number(keyword)? {
                0 => Err(format!(
                    "tui.guicursor: `{arg}` in `{part}` — a cursor 0% wide is none"
                )),
                _ => Ok(Arg::Shape(shape)),
            }
        };
        if lower == "block" {
            Ok(Arg::Shape(Shape::Block))
        } else if lower.starts_with("ver") {
            bar("ver", Shape::Bar)
        } else if lower.starts_with("hor") {
            bar("hor", Shape::Underline)
        } else if lower.starts_with("blinkwait") {
            number("blinkwait").map(|_| Arg::Unused)
        } else if lower.starts_with("blinkoff") {
            number("blinkoff").map(Arg::BlinkOff)
        } else if lower.starts_with("blinkon") {
            number("blinkon").map(Arg::BlinkOn)
        } else {
            Ok(Arg::Unused)
        }
    }

    /// Overwrite what this argument names, and only that.
    fn apply(self, look: &mut Look) {
        match self {
            Arg::Shape(shape) => look.shape = shape,
            Arg::BlinkOn(ms) => look.blinkon = ms,
            Arg::BlinkOff(ms) => look.blinkoff = ms,
            Arg::Unused => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorMode, Shapes};
    use CursorMode::*;

    /// Each mode's DECSCUSR number, in [`CursorMode::ALL`] order.
    fn codes(spec: &str) -> Vec<Option<u8>> {
        let shapes = Shapes::read(spec).unwrap();
        CursorMode::ALL.iter().map(|&m| shapes.code(m)).collect()
    }

    /// **Neovim's default, read**: a steady block in Normal, Visual and on
    /// the `:` line at its end; a steady bar in Insert and inside the `:`
    /// line; a steady underline after an operator and in Replace. The
    /// shipped value, and neovim's whole default (its Terminal part and
    /// `TermCursor` group read and unused), read the same.
    #[test]
    fn the_shipped_guicursor_reads_as_neovims() {
        let want = [2, 2, 4, 6, 4, 2, 6].map(Some).to_vec();
        assert_eq!(
            CursorMode::ALL,
            [
                Normal,
                Visual,
                Operator,
                Insert,
                Replace,
                Cmdline,
                CmdlineInsert
            ]
        );
        let shipped = shannon_core::config::TuiConfig::default().guicursor;
        assert_eq!(codes(&shipped), want);
        assert_eq!(Shapes::default(), Shapes::read(&shipped).unwrap());
        let neovims = "n-v-c-sm:block,i-ci-ve:ver25,r-cr-o:hor20,\
                       t:block-blinkon500-blinkoff500-TermCursor";
        assert_eq!(codes(neovims), want);
    }

    /// **Empty leaves the terminal's own shape** (`:set guicursor=`), in
    /// every mode.
    #[test]
    fn an_empty_guicursor_leaves_the_terminals_shape() {
        assert_eq!(codes(""), vec![None; CursorMode::COUNT]);
    }

    /// **A part overwrites only what it names**: a mode no part names is a
    /// steady block; a later part's blink keeps an earlier part's shape;
    /// `a` reaches every mode without resetting one; case is ignored.
    #[test]
    fn a_part_overwrites_only_what_it_names() {
        assert_eq!(
            codes("i:ver25"),
            [2, 2, 2, 6, 2, 2, 2].map(Some).to_vec(),
            "the rest are blocks"
        );
        assert_eq!(
            codes("a:hor10,i:ver5"),
            [4, 4, 4, 6, 4, 4, 4].map(Some).to_vec(),
            "`a`, then one mode"
        );
        assert_eq!(
            codes("N-I:VER25,i:blinkon5-blinkoff5"),
            [6, 2, 2, 5, 2, 2, 2].map(Some).to_vec(),
            "a blink over a shape, case ignored"
        );
        assert_eq!(
            codes("a:blinkon5-blinkoff5,i:ver25"),
            [1, 1, 1, 5, 1, 1, 1].map(Some).to_vec(),
            "a shape over a blink"
        );
        assert_eq!(
            codes("n:block-Cursor/lCursor-blinkwait9,"),
            vec![Some(2); CursorMode::COUNT],
            "a group and blinkwait unused; a trailing comma"
        );
    }

    /// **Blinking needs both numbers above 0** (`tui.c`): one alone, or
    /// either 0, is steady.
    #[test]
    fn a_cursor_blinks_only_on_both_numbers() {
        assert_eq!(codes("a:blinkon500")[0], Some(2));
        assert_eq!(codes("a:blinkon500-blinkoff0")[0], Some(2));
        assert_eq!(codes("a:blinkon500-blinkoff500")[0], Some(1));
        assert_eq!(codes("a:ver1-blinkon1-blinkoff1")[0], Some(5));
        assert_eq!(codes("a:hor1-blinkon1-blinkoff1")[0], Some(3));
    }

    /// **What neovim refuses is refused, and says why**: no colon, a name
    /// that is no mode, an empty mode, a keyword without its number, a bar
    /// 0% wide.
    #[test]
    fn what_neovim_refuses_is_refused_and_says_why() {
        for (spec, says) in [
            ("n", "no `:`"),
            ("n:block,,i:ver25", "no `:`"),
            ("x:block", "`x` in `x:block` is no mode"),
            ("nv:block", "`nv`"),
            (":block", "`` in `:block` is no mode"),
            ("n:ver", "needs a number after `ver`"),
            ("n:hor2x", "needs a number after `hor`"),
            ("n:blinkon", "needs a number after `blinkon`"),
            ("n:ver0", "0% wide"),
            ("n:hor0", "0% wide"),
        ] {
            let why = Shapes::read(spec).expect_err(spec);
            assert!(why.contains(says), "{spec}: {why}");
        }
        assert!(
            Shapes::read("a:blinkon99999999999-blinkoff1").is_ok(),
            "a long number saturates"
        );
    }
}
