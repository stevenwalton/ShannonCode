//! **What a word is** — vim's answer (`:h word`, `:h WORD`), asked by every
//! word motion (`w` `b` `e`, `W` `B` `E`), the word text objects (`iw` `aw`,
//! `iW` `aW`), and `<C-w>` in Insert mode and on the command line.
//!
//! A *word* is a run of keyword characters — letters, digits and `_`, vim's
//! default `iskeyword` — or a run of other non-blank characters; a *WORD*
//! is any run of non-blanks. So `w` stops at each `/` of a path and `W`
//! crosses it whole. Every one of those sites spelled "a run of
//! non-whitespace", so `w` was vim's `W`, and there was no `W` (Steven,
//! 10-07: "vim movements on `w` jump over whole file paths as if they are a
//! single word").

/// The kind of a character, as a word motion sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Blank,
    Keyword,
    Other,
}

/// `c`'s class — for a WORD (`big`), every non-blank is one class.
pub fn class(c: char, big: bool) -> Class {
    if c.is_whitespace() {
        Class::Blank
    } else if big || c.is_alphanumeric() || c == '_' {
        Class::Keyword
    } else {
        Class::Other
    }
}

/// A place among a buffer's lines: on a char, or on a line's end (`col ==
/// len`) — the cell vim's word motions step through as white space, words
/// being "separated with white space (spaces, tabs, `<EOL>`)", and an empty
/// line "also considered to be a word" (`:h word`).
pub type Pos = (usize, usize);

/// What one step did — vim's `inc()` / `dec()` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// To the next (previous) char of the line.
    Along,
    /// Onto a line's end, or into the next (previous) line.
    Over,
    /// At the buffer's edge: nothing moved.
    Edge,
}

/// A cursor stepping through `lines` cell by cell, as vim's word motions
/// step (`fwd_word`, `bck_word`, `end_word`, `current_word`).
struct Walk<'a> {
    lines: &'a [Vec<char>],
    at: Pos,
    big: bool,
}

impl Walk<'_> {
    /// The class under the cursor; a line's end is white space.
    fn class(&self) -> Class {
        let (row, col) = self.at;
        (self.lines[row].get(col)).map_or(Class::Blank, |&c| class(c, self.big))
    }

    /// On an empty line — a word of its own.
    fn on_empty_line(&self) -> bool {
        self.lines[self.at.0].is_empty()
    }

    fn forward(&mut self) -> Step {
        let (row, col) = self.at;
        let len = self.lines[row].len();
        if col < len {
            self.at.1 += 1;
            return if col + 1 == len {
                Step::Over
            } else {
                Step::Along
            };
        }
        if row + 1 < self.lines.len() {
            self.at = (row + 1, 0);
            return Step::Over;
        }
        Step::Edge
    }

    fn back(&mut self) -> Step {
        let (row, col) = self.at;
        if col > 0 {
            self.at.1 = col - 1;
            return Step::Along;
        }
        if row > 0 {
            self.at = (row - 1, self.lines[row - 1].len());
            return Step::Over;
        }
        Step::Edge
    }

    /// Past every cell of `run`; `true` when the buffer's edge stopped it.
    fn skip(&mut self, run: Class, forward: bool) -> bool {
        while self.class() == run {
            let step = if forward { self.forward() } else { self.back() };
            if step == Step::Edge {
                return true;
            }
        }
        false
    }

    /// Back to the start of the word, or of the white space, under the
    /// cursor — within its line (vim's `back_in_line`).
    fn back_in_line(&mut self) {
        let run = self.class();
        while self.at.1 > 0 {
            self.at.1 -= 1;
            if self.class() != run {
                self.at.1 += 1;
                break;
            }
        }
    }
}

/// **`w`, `count` times** — vim's `fwd_word`: past the rest of this word,
/// then past white space to the next word's start, an empty line being one.
/// `eol`, for an operator: its last word stops at the end of its line, not at
/// the next line's first word (`:h w`'s second special case). `false`: it
/// started on the buffer's last char, and moved onto its end.
pub fn next_start(lines: &[Vec<char>], at: Pos, count: usize, big: bool, eol: bool) -> (Pos, bool) {
    let mut w = Walk { lines, at, big };
    for left in (0..count.max(1)).rev() {
        let stop_at_eol = eol && left == 0;
        let run = w.class();
        let last_line = w.at.0 + 1 == lines.len();
        let step = w.forward();
        if step == Step::Edge || (step == Step::Over && last_line) {
            return (w.at, false);
        }
        if step == Step::Over && stop_at_eol {
            return (w.at, true);
        }
        if run != Class::Blank {
            while w.class() == run {
                let step = w.forward();
                if step == Step::Edge || (step == Step::Over && stop_at_eol) {
                    return (w.at, true);
                }
            }
        }
        while w.class() == Class::Blank {
            if w.at.1 == 0 && w.on_empty_line() {
                break;
            }
            let step = w.forward();
            if step == Step::Edge || (step == Step::Over && stop_at_eol) {
                return (w.at, true);
            }
        }
    }
    (w.at, true)
}

/// **`b`, `count` times** — vim's `bck_word`: back past white space (an
/// empty line stops it, being a word), then to the start of the word there.
/// `false`: a count began at the buffer's start, where it stays; an
/// operator over it does nothing. Reaching the start part-way is no failure.
pub fn prev_start(lines: &[Vec<char>], at: Pos, count: usize, big: bool) -> (Pos, bool) {
    let mut w = Walk { lines, at, big };
    'counts: for _ in 0..count.max(1) {
        if w.back() == Step::Edge {
            return (w.at, false);
        }
        while w.class() == Class::Blank {
            if w.at.1 == 0 && w.on_empty_line() {
                continue 'counts;
            }
            if w.back() == Step::Edge {
                return (w.at, true);
            }
        }
        if w.skip(w.class(), false) {
            return (w.at, true);
        }
        w.forward(); // overshot, onto the word's first char
    }
    (w.at, true)
}

/// **`ge`, `count` times** — vim's `bckend_word`: back before the start of
/// the word under the cursor, then back past white space onto the previous
/// word's last char, an empty line stopping it (being a word). `false`: a
/// count began at the buffer's start, where it stays, and an operator over
/// it does nothing (as [`prev_start`]); reaching the start part-way is no
/// failure.
pub fn prev_end(lines: &[Vec<char>], at: Pos, count: usize, big: bool) -> (Pos, bool) {
    let mut w = Walk { lines, at, big };
    for _ in 0..count.max(1) {
        let run = w.class();
        if w.back() == Step::Edge {
            return (w.at, false);
        }
        if run != Class::Blank {
            while w.class() == run {
                if w.back() == Step::Edge {
                    return (w.at, true);
                }
            }
        }
        while w.class() == Class::Blank {
            if w.at.1 == 0 && w.on_empty_line() {
                break;
            }
            if w.back() == Step::Edge {
                return (w.at, true);
            }
        }
    }
    (w.at, true)
}

/// **`e`, `count` times** — vim's `end_word`: to the end of this word, or
/// past white space to the next word's end (an empty line does not stop it,
/// `:h e`). `stop`, for `cw`: a cursor already on its word's last char stays
/// there for the first count (`:h cw`). `false`: the buffer ended first, at
/// the cell it reached.
pub fn next_end(lines: &[Vec<char>], at: Pos, count: usize, big: bool, stop: bool) -> (Pos, bool) {
    let mut w = Walk { lines, at, big };
    let mut stop = stop;
    for _ in 0..count.max(1) {
        if !end_once(&mut w, stop, false) {
            return (w.at, false);
        }
        stop = false;
    }
    (w.at, true)
}

/// One step of `end_word`; `empty` stops at an empty line (for `aw`).
/// `false` when the buffer ended first.
fn end_once(w: &mut Walk, stop: bool, empty: bool) -> bool {
    let run = w.class();
    if w.forward() == Step::Edge {
        return false;
    }
    if w.class() == run && run != Class::Blank {
        if w.skip(run, true) {
            return false;
        }
    } else if !stop || run == Class::Blank {
        while w.class() == Class::Blank {
            if empty && w.at.1 == 0 && w.on_empty_line() {
                return true;
            }
            if w.forward() == Step::Edge {
                return false;
            }
        }
        if w.skip(w.class(), true) {
            return false;
        }
    }
    w.back(); // overshot, onto the word's last char
    true
}

/// **`iw` / `aw` (`iW` / `aW`, `big`), `count` of them, from `at`** — vim's
/// `current_word` outside Visual: the word, or the white space, under the
/// cursor; `aw` takes the white space after it, or from white space the word
/// after, or — with none after — the white space before (not an indent).
/// Across lines, as vim's: `aw` on a line's trailing blanks takes the next
/// line's first word. `(start, end)`, `end` exclusive; `None` when the
/// buffer ends first.
pub fn object(
    lines: &[Vec<char>],
    at: Pos,
    count: usize,
    around: bool,
    big: bool,
) -> Option<(Pos, Pos)> {
    let mut w = Walk { lines, at, big };
    w.back_in_line();
    let mut start = w.at;
    let mut take_white_before = false;
    if (w.class() == Class::Blank) == around {
        if !end_once(&mut w, true, true) {
            return None;
        }
    } else {
        w.at = next_start(lines, w.at, 1, big, true).0;
        back_one(&mut w);
        take_white_before = around;
    }
    let mut inclusive = true;
    for left in (1..count.max(1)).rev() {
        inclusive = true;
        if !forward_past_end(&mut w) {
            return None;
        }
        if around != (w.class() == Class::Blank) {
            let (to, ok) = next_start(lines, w.at, 1, big, true);
            w.at = to;
            if !ok && left > 1 {
                return None;
            }
            if w.at.1 == 0 {
                inclusive = false;
            } else {
                w.at.1 -= 1;
            }
        } else if !end_once(&mut w, true, true) {
            return None;
        }
    }
    let (row, col) = w.at;
    if take_white_before && (w.class() != Class::Blank || (col == 0 && !inclusive)) {
        // Nothing after to take: the white space before the word, if it is
        // not the line's indent.
        let mut before = Walk {
            lines,
            at: start,
            big,
        };
        if before.at.1 > 0 {
            before.at.1 -= 1;
            before.back_in_line();
            if before.class() == Class::Blank && before.at.1 > 0 {
                start = before.at;
            }
        }
    }
    // From an empty last line the walk backs into the line before, behind
    // where it began; vim orders the two ends, so `yiw` there takes that
    // line's last char and the break (measured, nvim 0.12.6).
    let (first, last) = match (row, col) < start {
        true => ((row, col), start),
        false => (start, (row, col)),
    };
    let end = match inclusive {
        true => (last.0, last.1 + 1),
        false => last,
    };
    Some((first, end))
}

/// Onto the cell before, as vim's `current_word` backs off a `w`: one left,
/// or from a line's first column to the end of the line before and off it,
/// onto its last char (vim's `decl`); at the buffer's start, nowhere.
fn back_one(w: &mut Walk) {
    if w.at.1 > 0 || w.back() == Step::Over {
        w.at.1 = w.at.1.saturating_sub(1);
    }
}

/// One cell forward, past a line's end onto the next line (vim's `incl`);
/// `false` at the buffer's end.
fn forward_past_end(w: &mut Walk) -> bool {
    match w.forward() {
        Step::Edge => false,
        Step::Over if w.at.1 > 0 => w.forward() != Step::Edge,
        _ => true,
    }
}

/// Where `<C-w>` cuts back to from `at` in `line`: over the blanks before
/// it, then over one run of a class (`:h i_CTRL-W`, `:h c_CTRL-W`).
pub fn start_before(line: &[char], at: usize) -> usize {
    let mut n = at.min(line.len());
    while n > 0 && class(line[n - 1], false) == Class::Blank {
        n -= 1;
    }
    if n > 0 {
        let run = class(line[n - 1], false);
        while n > 0 && class(line[n - 1], false) == run {
            n -= 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A path is many words and one WORD**, as in vim.
    #[test]
    fn a_path_is_many_words_and_one_word() {
        let path: Vec<char> = "/Users/me/a_b.rs".chars().collect();
        let runs = |big: bool| {
            let mut n = 1;
            for w in path.windows(2) {
                n += usize::from(class(w[0], big) != class(w[1], big));
            }
            n
        };
        assert_eq!(runs(false), 8, "/ Users / me / a_b . rs: 8, `_` a keyword");
        assert_eq!(runs(true), 1);
        assert_eq!(class('é', false), Class::Keyword, "a letter is a letter");
        assert_eq!(class('\t', true), Class::Blank);
    }

    use proptest::prelude::*;

    /// Short lines of words, punctuation and blanks, empty lines among them.
    fn arb_lines() -> impl Strategy<Value = Vec<Vec<char>>> {
        let cell = prop::sample::select(vec!['a', 'b', '.', ' ', '\t']);
        prop::collection::vec(prop::collection::vec(cell, 0..6), 1..5)
    }

    proptest! {
        /// **Every walk stays inside the buffer, and every object is a
        /// span** — any lines, any place, any count and flags: a landing on a
        /// char or a line's end, and an object whose end is not before its
        /// start (empty is vim's too: `2iw` over two empty lines).
        #[test]
        fn every_walk_stays_inside_the_buffer(
            lines in arb_lines(),
            row in 0usize..5,
            col in 0usize..7,
            count in 1usize..4,
            big: bool,
            flag: bool,
        ) {
            let row = row % lines.len();
            let at = (row, col.min(lines[row].len()));
            let inside = |(r, c): Pos| r < lines.len() && c <= lines[r].len();
            prop_assert!(inside(next_start(&lines, at, count, big, flag).0));
            prop_assert!(inside(prev_start(&lines, at, count, big).0));
            prop_assert!(inside(next_end(&lines, at, count, big, flag).0));
            if let Some((start, end)) = object(&lines, at, count, flag, big) {
                prop_assert!(inside(start) && start <= end, "{at:?}: {start:?}..{end:?}");
                prop_assert!(end.0 < lines.len() && end.1 <= lines[end.0].len() + 1);
            }
        }
    }

    /// **`<C-w>` cuts one run of a class**, after the blanks before it.
    #[test]
    fn ctrl_w_cuts_one_run() {
        let at = |s: &str| {
            let line: Vec<char> = s.chars().collect();
            start_before(&line, line.len())
        };
        assert_eq!(at("cd /usr/lib"), 8, "`lib`, not the path");
        assert_eq!(at("cd /usr/"), 7, "the `/`");
        assert_eq!(at("foo   "), 0, "blanks, then `foo`");
        assert_eq!(at(""), 0);
    }
}
