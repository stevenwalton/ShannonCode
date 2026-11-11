//! Display-cell-aware soft wrapping.
//!
//! The single wrapping primitive shared by the output pane, the prompt
//! editor and every text page. It wraps by **terminal display width** (via `unicode-width`),
//! not codepoint count: a CJK ideograph or wide emoji occupies two cells,
//! a combining mark zero. Counting chars instead — as M1 did — overflows
//! lines and (in the editable prompt) misplaces the cursor for any
//! non-Latin text. Selection and mouse hit-testing, landing in later M2
//! phases, both invert this geometry, so getting it right here is
//! foundational, not cosmetic.
//!
//! Two wrap strategies share the cell-measurement core:
//! - [`wrap_cells`] breaks at a hard cell boundary (a long word splits
//!   mid-word). The **prompt editor** uses it because the cursor maps a
//!   logical char index to a visual position by walking rows by `char_len`,
//!   which only stays exact when every source char lands in exactly one row
//!   *and renders*.
//! - [`wrap_cells_word`] breaks at the last space that fits, dropping that
//!   break space (standard greedy word wrap), and hard-breaks a word longer
//!   than the pane. The **output pane** uses it for readability — it only
//!   needs the row *text*, not a cursor-faithful char mapping — and so does
//!   every text page (`PageLine::wrap`), whose `TextView` counts rows by it
//!   and draws them by it.

use unicode_width::UnicodeWidthChar;

/// One visual row produced by wrapping a logical line. `text` is the
/// row's characters; `char_len` is how many source chars it consumed, so
/// a caller mapping a logical char index to a visual position can walk
/// rows by char count (the prompt cursor needs this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapRow {
    pub text: String,
    pub char_len: usize,
}

/// Display width of `ch` in terminal cells. Controls/None → 0 (output is
/// sanitized of controls before wrapping; this only has to be sane for
/// printable text and zero-width combining marks).
pub(crate) fn char_cells(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// Total display width of a char slice, in cells.
pub fn cells_width(chars: &[char]) -> usize {
    chars.iter().copied().map(char_cells).sum()
}

/// Total display width of a string slice, in cells — for positioning a cursor
/// past text that may contain wide (CJK/emoji) or zero-width chars.
pub fn str_cells(s: &str) -> usize {
    s.chars().map(char_cells).sum()
}

/// **`text` in at most `cells` terminal cells**, ending in `…` where it was
/// cut — by cell, not by char or byte: a command can hold wide characters,
/// and a byte slice can split one. The one cut a one-line label takes: the
/// status line's running job, the `:memories` screen's rows (which kept a
/// retyped copy measuring by a second ruler, Reviewer C, round 28).
pub(crate) fn clip(text: &str, cells: usize) -> String {
    match (str_cells(text) <= cells, cells) {
        (true, _) => text.to_string(),
        (false, 0) => String::new(),
        (false, _) => format!("{}…", leading(text, cells - 1)),
    }
}

/// **`path` in at most `cells` cells, cut in the middle** — its start and
/// its end kept, which tell one place from another; the end, the more
/// telling, the larger share. Under five cells, [`clip`]'s cut: there is no
/// middle to keep.
pub(crate) fn clip_middle(path: &str, cells: usize) -> String {
    if str_cells(path) <= cells || cells < 5 {
        return clip(path, cells);
    }
    let keep = cells - 1;
    let tail = trailing(path, keep - keep / 3);
    let head = leading(path, keep - str_cells(tail));
    format!("{head}…{tail}")
}

/// The longest leading part of `text` within `cells` cells.
pub(crate) fn leading(text: &str, cells: usize) -> &str {
    let mut used = 0;
    let end = text
        .char_indices()
        .find(|&(_, c)| {
            used += char_cells(c);
            used > cells
        })
        .map_or(text.len(), |(at, _)| at);
    &text[..end]
}

/// The longest trailing part of `text` within `cells` cells.
fn trailing(text: &str, cells: usize) -> &str {
    let mut used = 0;
    let start = text
        .char_indices()
        .rev()
        .find(|&(_, c)| {
            used += char_cells(c);
            used > cells
        })
        .map_or(0, |(at, c)| at + c.len_utf8());
    &text[start..]
}

/// Wrap `chars` into visual rows no wider than `width` display cells. A
/// character whose width would overflow the current row starts a new row;
/// a single character wider than `width` occupies its own row (an
/// unavoidable one-cell overflow on a 1-wide pane). An empty input — or
/// `width == 0` — yields a single empty row so it renders as a blank line
/// rather than vanishing.
pub fn wrap_cells(chars: &[char], width: usize) -> Vec<WrapRow> {
    if width == 0 || chars.is_empty() {
        return vec![WrapRow {
            text: String::new(),
            char_len: 0,
        }];
    }

    let mut rows = Vec::new();
    let mut text = String::new();
    let mut cells = 0usize;
    let mut char_len = 0usize;
    for &ch in chars {
        let w = char_cells(ch);
        // Break before a char that would overflow — but never break into
        // an empty row (a too-wide char then gets its own overflowing
        // row, which is the least-bad option on a narrow pane).
        if cells + w > width && char_len > 0 {
            rows.push(WrapRow {
                text: std::mem::take(&mut text),
                char_len,
            });
            cells = 0;
            char_len = 0;
        }
        text.push(ch);
        cells += w;
        char_len += 1;
    }
    rows.push(WrapRow { text, char_len });
    rows
}

/// Where char `col` of a line sits once that line is wrapped to `width` cells:
/// `(row_within_line, cell_within_row)`. `starts` is the line's per-row start
/// offsets — the prefix sums of each [`WrapRow::char_len`] from [`wrap_cells`] —
/// which both callers already build, so this does no re-wrapping.
///
/// It implements the one **phantom-row bump** rule: a cursor sitting exactly at
/// the end of a row that fills the full `width` reports as column 0 of the
/// *next* row, not `width` cells into the current one — so the caret renders at
/// the start of the continuation line, matching where typing the next char would
/// land it. This subtle rule was previously inlined *identically* in both the
/// forward (`Buffer::visible_window`) and reverse (`Buffer::cursor_from_visual`)
/// wrap maps; centralizing it here is what keeps the two from silently drifting.
///
/// `starts` is non-empty (`wrap_cells` always yields ≥1 row) and `col` is a
/// valid char index into `line` (`col <= line.len()`), so the indexing is safe.
///
/// **Never on a char that draws in no cells.** The cells before `col` fill
/// the row with `col` still in it only when the char at `col` is zero-width
/// — a combining mark (macOS writes `é` as `e` + U+0301) — or, word-wrapped,
/// a dropped break space (`cursor_in_wrapped_word` caps to the visible end).
/// The space bumps, as the caret rests where typing would land; the mark is
/// drawn with its base, so its cursor is the row's last cell. It bumped too,
/// and drew the cursor on the next row's first char (review stop 12, T6 9).
pub fn cursor_in_wrapped(
    line: &[char],
    starts: &[usize],
    width: usize,
    col: usize,
) -> (usize, usize) {
    // The last row whose start is at or before `col` is the row `col` is in.
    let r = starts.partition_point(|&s| s <= col).saturating_sub(1);
    let cell = cells_width(&line[starts[r]..col]);
    let drawn_with_its_base = line.get(col).is_some_and(|&c| char_cells(c) == 0);
    match cell >= width {
        true if drawn_with_its_base => (r, width.saturating_sub(1)),
        true => (r + 1, 0),
        false => (r, cell),
    }
}

/// Forward cursor map for **word-wrapped** rows: where logical char `col` of
/// `line` sits once wrapped to `width` by [`wrap_cells_word`], as
/// `(row_within_line, cell_within_row)`.
///
/// Word wrap drops break spaces — a source char consumed (counted in
/// [`WrapRow::char_len`]) but not rendered (absent from [`WrapRow::text`]). So a
/// `col` landing on that dropped run has no cell of its own; by convention it
/// resolves to the **end of its row's visible text** (the caret rests where the
/// space would have rendered). Within a row's visible run the map is exact. It
/// delegates to [`cursor_in_wrapped`] after capping `col` to the row's visible
/// end, so the one phantom-row-bump kernel governs both wrap modes and they
/// can't drift. `rows` is the line's [`wrap_cells_word`] output (non-empty);
/// `col <= line.len()`.
pub fn cursor_in_wrapped_word(
    line: &[char],
    rows: &[WrapRow],
    width: usize,
    col: usize,
) -> (usize, usize) {
    let starts = row_starts(rows);
    let r = starts.partition_point(|&s| s <= col).saturating_sub(1);
    // Cap to the row's visible end so a col in the dropped break-space run at the
    // row's tail measures to the visible end, not into the (unrendered) hole.
    let visible_end = starts[r] + rows[r].text.chars().count();
    cursor_in_wrapped(line, &starts, width, col.min(visible_end))
}

/// Inverse cursor map for **word-wrapped** rows: a visual position — `vrow` a row
/// index into `rows`, `vcol` a display-cell column — back to a logical char `col`
/// in the line. Walks `rows[vrow]`'s visible chars by display cells until `vcol`
/// is reached (a `vcol` past the visible width, or inside a wide char, clamps to
/// the last char boundary at or before it), then adds the row's logical start.
/// The inverse of [`cursor_in_wrapped_word`]; their round-trip is proptest-pinned.
pub fn col_from_visual_word(rows: &[WrapRow], vrow: usize, vcol: usize) -> usize {
    let start: usize = rows.iter().take(vrow).map(|r| r.char_len).sum();
    let Some(row) = rows.get(vrow) else {
        return start;
    };
    let mut cells = 0usize;
    let mut off = 0usize;
    for ch in row.text.chars() {
        let w = char_cells(ch);
        if cells + w > vcol {
            break;
        }
        cells += w;
        off += 1;
    }
    start + off
}

/// Per-row logical start offsets — the prefix sums of each [`WrapRow::char_len`].
fn row_starts(rows: &[WrapRow]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(rows.len());
    let mut acc = 0usize;
    for row in rows {
        starts.push(acc);
        acc += row.char_len;
    }
    starts
}

/// Word-aware wrap: like [`wrap_cells`], but a row that would overflow breaks
/// at the **last space that fits** instead of mid-word, and that break space
/// is dropped from the row's text (consumed, not rendered). A word longer than
/// `width` still hard-breaks at the cell boundary (the least-bad option). A
/// row's `char_len` counts every source char it consumed — including a dropped
/// break space — so the rows still partition the input exactly; `text` holds
/// only the visible chars. The transcript's wrap, and every text page's
/// (`PageLine::wrap`: one ruler for counting and drawing).
pub fn wrap_cells_word(chars: &[char], width: usize) -> Vec<WrapRow> {
    if width == 0 || chars.is_empty() {
        return vec![WrapRow {
            text: String::new(),
            char_len: 0,
        }];
    }

    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        // The greedy hard-cell fit: the longest prefix `[start, fit)` within
        // `width` (at least one char, even if it overflows a 1-cell pane).
        let mut cells = 0usize;
        let mut fit = start;
        while fit < chars.len() {
            let w = char_cells(chars[fit]);
            if cells + w > width && fit > start {
                break;
            }
            cells += w;
            fit += 1;
        }

        // Everything left fits on this row.
        if fit >= chars.len() {
            rows.push(WrapRow {
                text: chars[start..].iter().collect(),
                char_len: chars.len() - start,
            });
            break;
        }

        // More chars remain, so we must break before `fit`. Prefer the
        // rightmost space in `(start, fit]` — break there and drop the space.
        // No space in the row → hard-break at the cell boundary `fit`.
        let mut brk = fit;
        let mut k = fit;
        while k > start {
            if chars[k] == ' ' {
                brk = k;
                break;
            }
            k -= 1;
        }
        // Consume the run of break spaces (so the next row starts on a word).
        let mut next = brk;
        while next < chars.len() && chars[next] == ' ' {
            next += 1;
        }
        rows.push(WrapRow {
            text: chars[start..brk].iter().collect(),
            char_len: next - start,
        });
        start = next;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn texts(rows: &[WrapRow]) -> Vec<&str> {
        rows.iter().map(|r| r.text.as_str()).collect()
    }

    /// `clip` measures CELLS: a wide character is two, and the `…` it adds
    /// counts against the budget, so the result never exceeds `cells` — at
    /// no width at all, nothing.
    #[test]
    fn clip_cuts_by_cells_and_marks_the_cut() {
        assert_eq!(clip("grep -r x .", 40), "grep -r x .", "fits: untouched");
        assert_eq!(clip("abcdefgh", 5), "abcd…");
        let wide = clip("日本語のコマンド", 7);
        assert_eq!(wide, "日本語…", "three wide chars (6 cells) plus the mark");
        assert!(str_cells(&wide) <= 7);
        assert_eq!(clip("abc", 0), "");
        assert_eq!(clip("abc", 1), "…");
    }

    /// **`clip_middle` keeps both ends**, the end's share the larger, within
    /// the cells at every width — under five, `clip`'s cut, and at none,
    /// nothing (Reviewer B, round 28: the five-cell guard's deletion underflowed
    /// `width - 1` and passed every test).
    #[test]
    fn clip_middle_keeps_both_ends_within_the_cells() {
        let path = "~/.claude/projects/-Users-me-proj/memory";
        assert_eq!(clip_middle(path, 80), path, "fits: untouched");
        let cut = clip_middle(path, 20);
        assert_eq!(cut, "~/.cla…e-proj/memory", "a third to the start");
        assert_eq!(str_cells(&cut), 20);
        for width in 0..=path.len() {
            let cut = clip_middle(path, width);
            assert!(str_cells(&cut) <= width, "{width}: {cut}");
        }
        assert_eq!(clip_middle(path, 4), "~/.…");
        let wide = clip_middle("日本語/の/コマンド", 9);
        assert!(str_cells(&wide) <= 9, "{wide}");
        assert!(wide.ends_with("ンド"), "{wide}");
    }

    #[test]
    fn ascii_wraps_by_count_unchanged() {
        let rows = wrap_cells(&chars("abcdefghij"), 4);
        assert_eq!(texts(&rows), vec!["abcd", "efgh", "ij"]);
        assert_eq!(
            rows.iter().map(|r| r.char_len).collect::<Vec<_>>(),
            vec![4, 4, 2]
        );
    }

    #[test]
    fn empty_and_zero_width_yield_one_blank_row() {
        assert_eq!(texts(&wrap_cells(&[], 10)), vec![""]);
        assert_eq!(texts(&wrap_cells(&chars("abc"), 0)), vec![""]);
    }

    #[test]
    fn wide_chars_wrap_by_cells_not_count() {
        // Each CJK char is 2 cells; width 4 fits exactly two per row.
        let rows = wrap_cells(&chars("中文字漢"), 4);
        assert_eq!(texts(&rows), vec!["中文", "字漢"]);
        // char_len is still 2 (two chars), though it's 4 cells.
        assert!(rows.iter().all(|r| r.char_len == 2));
    }

    #[test]
    fn wide_char_does_not_split_across_a_boundary() {
        // Width 3 can't fit two 2-cell chars; each wide char takes a row.
        let rows = wrap_cells(&chars("中文"), 3);
        assert_eq!(texts(&rows), vec!["中", "文"]);
    }

    #[test]
    fn too_wide_char_gets_its_own_row() {
        // A 2-cell char on a 1-cell pane overflows but never vanishes.
        let rows = wrap_cells(&chars("中a"), 1);
        assert_eq!(texts(&rows), vec!["中", "a"]);
    }

    #[test]
    fn cells_width_counts_display_columns() {
        assert_eq!(cells_width(&chars("ab")), 2);
        assert_eq!(cells_width(&chars("中")), 2);
        assert_eq!(cells_width(&chars("a中")), 3);
    }

    // --- cursor geometry ---------------------------------------------------

    #[test]
    fn cursor_in_wrapped_locates_within_rows() {
        // "abcdefghij" @4 → "abcd"/"efgh"/"ij", starts [0,4,8].
        let line = chars("abcdefghij");
        let starts = [0usize, 4, 8];
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 0), (0, 0)); // start
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 2), (0, 2)); // mid row 0
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 4), (1, 0)); // start of row 1
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 10), (2, 2)); // end; short last row, no bump
    }

    #[test]
    fn cursor_in_wrapped_bumps_at_a_full_row_end() {
        // "abcdefgh" @4 → "abcd"/"efgh", starts [0,4]. A cursor at col 8 sits at
        // the end of a row that exactly fills the width → phantom-bump to (2, 0).
        let line = chars("abcdefgh");
        let starts = [0usize, 4];
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 8), (2, 0));
    }

    #[test]
    fn cursor_in_wrapped_counts_wide_cells() {
        // "中文字漢" @4 → "中文"/"字漢", starts [0,2] (2 chars / 4 cells each).
        let line = chars("中文字漢");
        let starts = [0usize, 2];
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 1), (0, 2)); // after one 2-cell char
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 2), (1, 0)); // start of row 1
        assert_eq!(cursor_in_wrapped(&line, &starts, 4, 4), (2, 0)); // end; full last row → bump
    }

    // --- word-aware wrap ---------------------------------------------------

    #[test]
    fn word_wrap_breaks_at_spaces_and_drops_them() {
        // "foo bar baz" at width 7: "foo bar" fits (7 cells), then "baz".
        let rows = wrap_cells_word(&chars("foo bar baz"), 7);
        assert_eq!(texts(&rows), vec!["foo bar", "baz"]);
        // "the quick brown fox" at width 10.
        let rows = wrap_cells_word(&chars("the quick brown fox"), 10);
        assert_eq!(texts(&rows), vec!["the quick", "brown fox"]);
    }

    #[test]
    fn word_wrap_hard_breaks_a_too_long_word() {
        // No space to break on → falls back to the cell boundary.
        let rows = wrap_cells_word(&chars("abcdefghij"), 4);
        assert_eq!(texts(&rows), vec!["abcd", "efgh", "ij"]);
        // A long word after a short one: the short word's row stays short.
        let rows = wrap_cells_word(&chars("hi abcdefgh"), 4);
        assert_eq!(texts(&rows), vec!["hi", "abcd", "efgh"]);
    }

    #[test]
    fn word_wrap_partitions_the_input_exactly() {
        // Every source char is consumed by exactly one row (char_len sums to
        // the input length) even though dropped break spaces aren't rendered.
        for (s, w) in [
            ("foo bar baz", 7),
            ("the quick brown fox", 10),
            ("a b c d e f", 3),
            ("supercalifragilistic", 5),
            ("   leading and  double  spaces   ", 6),
            ("中文 字漢 test", 5),
        ] {
            let cs = chars(s);
            let rows = wrap_cells_word(&cs, w);
            let total: usize = rows.iter().map(|r| r.char_len).sum();
            assert_eq!(total, cs.len(), "char_len must partition {s:?}@{w}");
            // No rendered row exceeds the width.
            for r in &rows {
                assert!(
                    cells_width(&chars(&r.text)) <= w,
                    "row {:?} over {w}",
                    r.text
                );
            }
        }
    }

    #[test]
    fn word_wrap_wide_chars() {
        // Wide chars with a space break: "中文 字" at width 5 → "中文" then "字".
        let rows = wrap_cells_word(&chars("中文 字"), 5);
        assert_eq!(texts(&rows), vec!["中文", "字"]);
    }

    #[test]
    fn word_wrap_empty_and_zero_width() {
        assert_eq!(texts(&wrap_cells_word(&[], 10)), vec![""]);
        assert_eq!(texts(&wrap_cells_word(&chars("abc"), 0)), vec![""]);
    }

    #[test]
    fn word_wrap_cursor_maps_concrete_cases() {
        // "ab cd" @ width 2 → rows ["ab","cd"], the space at col 2 dropped.
        let cs = chars("ab cd");
        let rows = wrap_cells_word(&cs, 2);
        assert_eq!(texts(&rows), vec!["ab", "cd"]);
        // col 0,1 land in row 0; the dropped space (col 2) and 'c' (col 3) both
        // resolve to the start of row 1 (the row-0 visible width fills `width`,
        // so the phantom bump carries the caret to the next row's start).
        assert_eq!(cursor_in_wrapped_word(&cs, &rows, 2, 0), (0, 0));
        assert_eq!(cursor_in_wrapped_word(&cs, &rows, 2, 1), (0, 1));
        assert_eq!(cursor_in_wrapped_word(&cs, &rows, 2, 2), (1, 0));
        assert_eq!(cursor_in_wrapped_word(&cs, &rows, 2, 3), (1, 0));
        assert_eq!(cursor_in_wrapped_word(&cs, &rows, 2, 4), (1, 1));
        // Inverse: a click past row 0's text clamps to its end (col 2, the hole).
        assert_eq!(col_from_visual_word(&rows, 0, 9), 2);
        assert_eq!(col_from_visual_word(&rows, 1, 0), 3);
    }

    // The word-wrap cursor maps are what let the chat pane navigate the *same*
    // word-wrapped rows the output renders (P3). Dropped break spaces make the
    // map non-injective on the logical side, so the contract we pin is
    // *consistency*: a forward position, inverted and forwarded again, is stable.
    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// `forward ∘ inverse ∘ forward == forward` — the maps agree despite
            /// dropped break spaces (the canonical-position idempotence).
            #[test]
            fn word_cursor_roundtrip_is_stable(s in "[a-z .]{0,40}", width in 1usize..12) {
                let cs = chars(&s);
                let rows = wrap_cells_word(&cs, width);
                for col in 0..=cs.len() {
                    let (vr, vc) = cursor_in_wrapped_word(&cs, &rows, width, col);
                    // forward may bump to a phantom row == rows.len() (a row that
                    // exactly fills width); callers pad for it — skip here.
                    if vr < rows.len() {
                        let back = col_from_visual_word(&rows, vr, vc);
                        prop_assert_eq!(
                            cursor_in_wrapped_word(&cs, &rows, width, back),
                            (vr, vc),
                        );
                    }
                }
            }

            /// Forward never points past the rows or beyond `width`.
            #[test]
            fn word_forward_in_bounds(s in "[a-z .]{0,40}", width in 1usize..12) {
                let cs = chars(&s);
                let rows = wrap_cells_word(&cs, width);
                for col in 0..=cs.len() {
                    let (r, cell) = cursor_in_wrapped_word(&cs, &rows, width, col);
                    prop_assert!(r <= rows.len());
                    prop_assert!(cell <= width);
                }
            }

            /// Inverse never points past the line, for any visual coordinate.
            #[test]
            fn word_inverse_in_bounds(s in "[a-z .]{0,40}", width in 1usize..12) {
                let cs = chars(&s);
                let rows = wrap_cells_word(&cs, width);
                for vr in 0..rows.len() {
                    for vc in 0..=width + 1 {
                        prop_assert!(col_from_visual_word(&rows, vr, vc) <= cs.len());
                    }
                }
            }

            /// **Every char's cursor is drawn in the row that holds it, inside
            /// the width** — over text with a wide char and a combining mark
            /// (U+0301; macOS keeps `é` as `e` + it in file names). The bump
            /// fired mid-row on a zero-width char and drew the cursor on the
            /// NEXT row's first char (review stop 12, T6 9); the properties
            /// above drew only from `[a-z .]`.
            #[test]
            fn every_cursor_is_drawn_in_its_own_row(
                cs in prop::collection::vec(
                    prop_oneof![
                        6 => proptest::char::range('a', 'e'),
                        1 => Just(' '),
                        1 => Just('世'),
                        1 => Just('\u{301}'),
                    ],
                    0..40,
                ),
                width in 1usize..8,
            ) {
                let rows = wrap_cells(&cs, width);
                let starts = row_starts(&rows);
                for col in 0..cs.len() {
                    let (r, cell) = cursor_in_wrapped(&cs, &starts, width, col);
                    let holds = starts.partition_point(|&s| s <= col) - 1;
                    prop_assert_eq!(r, holds, "col {} of {:?} at width {}", col, cs, width);
                    prop_assert!(cell < width, "col {} drawn at cell {}", col, cell);
                }
            }
        }
    }

    /// **The cursor on a combining mark stays on its base's cell** — the
    /// case found (review stop 12, T6 9): `ab` + U+0301 + `c` at width 2
    /// wraps to `ab́` / `c`; the accent, column 2, drew on the `c`.
    #[test]
    fn a_combining_mark_at_a_full_rows_end_stays_in_its_row() {
        let cs = chars("ab\u{301}c");
        let rows = wrap_cells(&cs, 2);
        assert_eq!(rows.len(), 2, "premise: `ab́` / `c`");
        let starts = row_starts(&rows);
        assert_eq!(cursor_in_wrapped(&cs, &starts, 2, 2), (0, 1), "on the b");
        assert_eq!(cursor_in_wrapped(&cs, &starts, 2, 3), (1, 0), "the c");
        assert_eq!(
            cursor_in_wrapped(&cs, &starts, 2, 4),
            (1, 1),
            "the line's end"
        );
    }
}
