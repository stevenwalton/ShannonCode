//! **Which lines of a file a model has seen**, and the [`Stamp`] a file call
//! leaves beside it — what read-first (S21-Q20, `file-tools.md` §3.6) asks
//! of the calls still in the context. A stamp holds positions and a hash,
//! never the file's text.

use super::text::Place;
use std::path::PathBuf;

/// **A set of lines**: 1-based, held as sorted ranges that neither overlap
/// nor touch, so two sets that hold the same lines are equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lines(Vec<(usize, usize)>);

impl Lines {
    /// Lines `first..=last`; none when `last < first`.
    pub fn span(first: usize, last: usize) -> Lines {
        let mut lines = Lines::default();
        lines.push(first, last);
        lines
    }

    /// **The lines of `ranges`, given in any order** — sorted once and
    /// merged in one sweep ([`push`](Self::push)). Every set is made here or
    /// by [`edited`](Self::edited)'s sweep, which makes its ranges in order:
    /// `add` once a range, copying and re-sorting the whole set each time,
    /// took half a second to join a stamp of 20,000 scattered ranges (review
    /// stop 16, D 6, measured).
    fn of(ranges: impl IntoIterator<Item = (usize, usize)>) -> Lines {
        let mut ranges: Vec<(usize, usize)> = ranges.into_iter().collect();
        ranges.sort_unstable();
        let mut lines = Lines::default();
        for (first, last) in ranges {
            lines.push(first, last);
        }
        lines
    }

    /// Lines `first..=last` added, merged with any they overlap or touch.
    pub fn add(&mut self, first: usize, last: usize) {
        *self = Lines::of(self.0.iter().copied().chain([(first, last)]));
    }

    /// Whether every line of a file `total` lines long is here.
    pub fn covers(&self, total: usize) -> bool {
        self.holds(1, total)
    }

    /// Whether every line `first..=last` is here — none asked, when
    /// `last < first`.
    pub fn holds(&self, first: usize, last: usize) -> bool {
        last < first || (self.0.iter()).any(|&(a, b)| a <= first && b >= last)
    }

    /// Every line of `self` and of `other`.
    pub fn union(self, other: &Lines) -> Lines {
        Lines::of(self.0.into_iter().chain(other.0.iter().copied()))
    }

    /// **These lines after an Edit's `places`** — its replacements in order,
    /// as [`text::edit`](super::text::edit) finds them. The lines before a
    /// place stay, the ones it replaced go, the ones after move by what the
    /// places before them added less what they removed; and the lines each
    /// place wrote are added, since the model wrote them. One sweep, in
    /// order: placed one at a time, each re-sorting every range, a
    /// `replace_all` of 4,000 matches took 15 s (review stop 16, B 3).
    pub fn edited(&self, places: &[Place]) -> Lines {
        let mut out = Lines::default();
        let mut next = 0; // the first range not yet carried whole
        let mut shift = 0isize;
        let mut from = 1; // the first old line not yet carried or replaced
        for place in places {
            self.carry(&mut out, &mut next, (from, place.line - 1), shift);
            let at = (place.line as isize + shift) as usize;
            out.push(at, at + place.added - 1);
            shift += place.added as isize - place.removed as isize;
            from = from.max(place.line + place.removed);
        }
        self.carry(&mut out, &mut next, (from, usize::MAX), shift);
        out
    }

    /// These lines in `lo..=hi`, moved by `shift`, pushed onto `out` — from
    /// the range `next`, left at the first one reaching past `hi`. Only
    /// lines after a place move, and a place removes at least its own line:
    /// a moved line stays at 1 or more.
    fn carry(&self, out: &mut Lines, next: &mut usize, (lo, hi): (usize, usize), shift: isize) {
        let moved = |n: usize| (n as isize + shift) as usize;
        while let Some(&(a, b)) = self.0.get(*next) {
            if a > hi {
                break;
            }
            if b >= lo {
                out.push(moved(a.max(lo)), moved(b.min(hi)));
            }
            if b > hi {
                break;
            }
            *next += 1;
        }
    }

    /// Lines `first..=last` after every range here, merged into the last
    /// where they meet it — the one way a set grows: [`of`](Self::of) sorts
    /// first, and [`edited`](Self::edited) makes its ranges in order.
    fn push(&mut self, first: usize, last: usize) {
        if last < first || last == 0 {
            return;
        }
        let first = first.max(1);
        debug_assert!(
            self.0.last().is_none_or(|&(a, _)| a <= first),
            "pushed in order"
        );
        match self.0.last_mut() {
            Some((_, end)) if first <= *end + 1 => *end = (*end).max(last),
            _ => self.0.push((first, last)),
        }
    }
}

/// **What one file call leaves for read-first**: the file it touched, the
/// lines the model has seen of it, how many there were, and a hash of the
/// file's bytes then — a stamp counts only while the file still hashes the
/// same, so a change by anyone (you, a formatter, a `bash` command) makes
/// the file unread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// The file the call opened, by the name it was judged and run under
    /// (`Job::file`) — as a later call on the file names it.
    pub path: PathBuf,
    pub seen: Lines,
    pub total: usize,
    pub hash: [u8; 32],
    /// **Its lines were shown to the model** — a Read's, which an unchanged
    /// re-read can point back to; an Edit's and a Write's lines are ones it
    /// wrote, and a re-read after them is how it checks what it made.
    pub shown: bool,
}

impl Stamp {
    /// A Read's: the lines it showed.
    pub fn read(path: PathBuf, bytes: &[u8], total: usize, first: usize, last: usize) -> Stamp {
        Stamp {
            path,
            seen: Lines::span(first, last),
            total,
            hash: hash(bytes),
            shown: true,
        }
    }

    /// A Write's: every line — the model wrote them all.
    pub fn written(path: PathBuf, bytes: &[u8], total: usize) -> Stamp {
        Stamp {
            path,
            seen: Lines::span(1, total),
            total,
            hash: hash(bytes),
            shown: false,
        }
    }

    /// **An Edit's**: what had been seen before it (`seen`, the current
    /// stamps' lines — `seen_now`), carried through the edit's places, and
    /// the lines each place wrote ([`Lines::edited`]) — so a model that read
    /// the whole file and edited it has still seen the whole file.
    pub fn edited(
        seen: Lines,
        path: PathBuf,
        places: &[Place],
        bytes: &[u8],
        total: usize,
    ) -> Stamp {
        Stamp {
            path,
            seen: seen.edited(places),
            total,
            hash: hash(bytes),
            shown: false,
        }
    }

    /// Whether the file is still as this stamp saw it.
    pub fn current(&self, bytes: &[u8]) -> bool {
        self.hash == hash(bytes)
    }
}

/// **What the model has seen of a file whose bytes are `bytes`** — the
/// lines of every stamp in `stamps` still current, together; `None` when
/// none is: never read, or changed since by anyone.
pub fn seen_now(stamps: &[Stamp], bytes: &[u8]) -> Option<Lines> {
    let now = hash(bytes);
    let mut current = (stamps.iter()).filter(|stamp| stamp.hash == now).peekable();
    current.peek()?;
    // Every current stamp's ranges, joined once ([`Lines::of`]).
    Some(Lines::of(
        current.flat_map(|stamp| stamp.seen.0.iter().copied()),
    ))
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn place(line: usize, removed: usize, added: usize) -> Place {
        Place {
            line,
            removed,
            added,
        }
    }

    /// **Lines merge where they overlap or touch**, so equal sets are equal.
    #[test]
    fn lines_merge_where_they_meet() {
        let mut lines = Lines::span(10, 12);
        lines.add(1, 3);
        lines.add(4, 5);
        assert_eq!(lines, Lines(vec![(1, 5), (10, 12)]));
        lines.add(6, 9);
        assert_eq!(lines, Lines::span(1, 12));
        assert!(lines.covers(12) && !lines.covers(13));
        assert!(Lines::default().covers(0) && !Lines::default().covers(1));
        assert_eq!(Lines::span(3, 2), Lines::default());
    }

    /// **An edit moves what follows it, drops what it replaced, and adds
    /// what it wrote.**
    #[test]
    fn an_edit_moves_the_lines_after_it() {
        let all = Lines::span(1, 10);
        // Line 4 became three lines: 1–3 stay, 4–6 written, 5–10 now 7–12.
        assert_eq!(all.edited(&[place(4, 1, 3)]), Lines::span(1, 12));
        // Lines 4–6 became one: 4 written, 7–10 now 5–8.
        assert_eq!(all.edited(&[place(4, 3, 1)]), Lines::span(1, 8));
        // Seen only inside what it replaced: gone, but for what it wrote.
        assert_eq!(
            Lines::span(5, 6).edited(&[place(4, 3, 1)]),
            Lines::span(4, 4)
        );
        // Seen only after it: moved, past what it wrote.
        assert_eq!(
            Lines::span(8, 9).edited(&[place(2, 2, 1)]),
            Lines(vec![(2, 2), (7, 8)])
        );
    }

    /// **The rule placed one at a time** — what [`Lines::edited`] did before
    /// it was one sweep (review stop 16, B 3), kept to hold the sweep to it.
    fn edited_one_at_a_time(seen: &Lines, places: &[Place]) -> Lines {
        let shifted = |lines: &Lines, place: Place| {
            let end = place.line + place.removed; // the first line after it
            let moved = |n: usize| n + place.added - place.removed;
            let mut out = Lines::default();
            for &(a, b) in &lines.0 {
                out.add(a, b.min(place.line - 1));
                if b >= end {
                    out.add(moved(a.max(end)), moved(b));
                }
            }
            out
        };
        // From the last place up, so each `line` is still where it was.
        let mut seen = seen.clone();
        for &place in places.iter().rev() {
            seen = shifted(&seen, place);
        }
        let mut shift = 0isize;
        for place in places {
            let at = (place.line as isize + shift) as usize;
            seen.add(at, at + place.added - 1);
            shift += place.added as isize - place.removed as isize;
        }
        seen
    }

    /// **A whole-file read stays whole through the model's own edits**, and
    /// a partial one keeps its part plus what was written.
    #[test]
    fn an_edit_carries_what_was_seen() {
        let read = Stamp::read("/f".into(), b"old", 10, 1, 10);
        let places = [place(2, 1, 3), place(8, 2, 1)];
        let edited = Stamp::edited(read.seen, "/f".into(), &places, b"new", 11);
        assert!(edited.seen.covers(11), "{:?}", edited.seen);
        assert!(!edited.shown, "lines it wrote, not ones it was shown");
        let part = Stamp::read("/f".into(), b"old", 10, 1, 3);
        let edited = Stamp::edited(part.seen, "/f".into(), &places, b"new", 11);
        // 1 seen, 2–4 written (line 2 became three), 3 → 5 seen, and old
        // 8–9 written as one line, now 10 (two lines down from the first).
        assert_eq!(edited.seen, Lines(vec![(1, 5), (10, 10)]));
        let blind = Stamp::edited(Lines::default(), "/f".into(), &[place(5, 1, 2)], b"new", 11);
        assert_eq!(blind.seen, Lines::span(5, 6));
    }

    /// **What has been seen now is every current stamp's lines together** —
    /// a stamp of other bytes counts for nothing; none current is none.
    #[test]
    fn seen_now_joins_the_current_stamps() {
        let early = Stamp::read("/f".into(), b"v1", 30, 1, 10);
        let late = Stamp::read("/f".into(), b"v2", 30, 11, 20);
        let later = Stamp::read("/f".into(), b"v2", 30, 25, 30);
        let stamps = [early, late, later];
        let seen = seen_now(&stamps, b"v2").expect("two current");
        assert_eq!(seen, Lines(vec![(11, 20), (25, 30)]));
        assert!(seen.holds(12, 18) && !seen.holds(18, 26) && seen.holds(5, 4));
        assert_eq!(seen_now(&stamps, b"v3"), None, "changed since all");
        assert_eq!(seen_now(&[], b"v2"), None);
    }

    /// **A stamp counts only while the file hashes as it did.**
    #[test]
    fn a_stamp_is_current_only_for_the_same_bytes() {
        let stamp = Stamp::written("/f".into(), b"abc\n", 1);
        assert!(stamp.current(b"abc\n"));
        assert!(!stamp.current(b"abd\n"));
    }

    proptest! {
        /// **Every set stays sorted, apart and untouching** however lines
        /// are added and shifted, and an edit that adds as many lines as it
        /// removes leaves every line outside it where it was.
        #[test]
        fn lines_stay_canonical(
            spans in proptest::collection::vec((1usize..40, 0usize..8), 0..8),
            line in 1usize..40,
            removed in 1usize..5,
            added in 1usize..5,
        ) {
            let mut lines = Lines::default();
            for (first, len) in spans {
                lines.add(first, first + len);
            }
            let canonical = |l: &Lines| l.0.windows(2).all(|w| w[0].1 + 1 < w[1].0)
                && l.0.iter().all(|&(a, b)| 1 <= a && a <= b);
            prop_assert!(canonical(&lines));
            let edited = lines.edited(&[place(line, removed, added)]);
            prop_assert!(canonical(&edited));
            let same = lines.edited(&[place(line, removed, removed)]);
            let outside = |l: &Lines| -> Vec<usize> {
                l.0.iter()
                    .flat_map(|&(a, b)| a..=b)
                    .filter(|&n| n < line || n >= line + removed)
                    .collect()
            };
            prop_assert_eq!(outside(&same), outside(&lines));
        }

        /// **One sweep places an Edit as one place at a time did** — over
        /// the places a real `replace_all` makes, matches sharing a line
        /// among them, and canonical as every set is.
        #[test]
        fn one_sweep_agrees_with_one_place_at_a_time(
            body in "[ab\n]{0,60}",
            old in "[ab\n]{1,3}",
            new in "[ab\n]{0,3}",
            spans in proptest::collection::vec((1usize..40, 0usize..8), 0..8),
        ) {
            let text = super::super::text::Text::decode(body.as_bytes()).unwrap();
            if let Ok(edit) = super::super::text::edit(&text, &old, &new, true) {
                let mut seen = Lines::default();
                for (first, len) in spans {
                    seen.add(first, first + len);
                }
                let swept = seen.edited(&edit.places);
                prop_assert_eq!(&swept, &edited_one_at_a_time(&seen, &edit.places));
                prop_assert!(swept.0.windows(2).all(|w| w[0].1 + 1 < w[1].0));
            }
        }
    }
}
