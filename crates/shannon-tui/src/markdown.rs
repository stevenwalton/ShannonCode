//! Light markdown styling for the chat transcript.
//!
//! Claude's responses are markdown. This produces a **per-character style map**
//! for a block's logical lines — headings, fenced code blocks, inline `code`,
//! and `**bold**` / `*italic*` — which the output cache slices per wrapped
//! visual row (computed once at commit, then cached). It is deliberately
//! *light* (no syntax hiding: the `#`/```` ``` ````/`` ` ``/`**` markers stay,
//! just styled), and line-oriented with
//! a single piece of cross-line state: whether we're inside a ```` ``` ````
//! fence. Per-language syntax highlighting *inside* code fences is layered on in
//! the syntect phase; this owns the markdown structure around it.
//!
//! Roles, not colours (`style::Ink`): the theme paints them
//! (`theme::Theme`). They were One-Dark-ish 24-bit placeholders the terminal's
//! own palette never reached (Steven, 10-06).

use crate::highlight::CodeHighlighter;
use crate::style::{CellStyle, Ink};

/// Heading lines (`#`…).
const HEADING: CellStyle = CellStyle::ink(Ink::Heading);
/// Fenced + inline code, where no syntax highlights it.
const CODE: CellStyle = CellStyle::ink(Ink::Raw);
/// [`CODE`], for a test in another module that needs to name the flat style.
#[cfg(test)]
pub const CODE_STYLE: CellStyle = CODE;
/// The ```` ``` ```` fence delimiter lines themselves (structure, not
/// content). A tool record's fences too (`tool_record`), so a code block
/// looks like one whoever wrote it.
pub(crate) const FENCE: CellStyle = CellStyle::ink(Ink::RawFence);

/// The fenced-code state carried across lines: outside any fence, inside a fence
/// whose language syntect highlights (the `CodeHighlighter` is stateful across
/// the block's lines), or inside a fence with no/unknown language (flat code).
enum Fence {
    None,
    // Boxed: a `CodeHighlighter` (a syntect `HighlightLines`) is large next to
    // the unit variants, and a fence opens rarely.
    Highlighted(Box<CodeHighlighter>),
    Plain,
}

/// Per-character styles for each logical line of a block, parallel to the input
/// (each inner `Vec` has one [`CellStyle`] per char of its line). The only
/// cross-line state is the fenced-code `Fence`; everything else is intra-line.
pub fn style_lines(lines: &[String]) -> Vec<Vec<CellStyle>> {
    style_lines_with(lines, Highlight::Syntax)
}

#[cfg(test)]
thread_local! {
    /// **How many times this thread styled a block** — for the gates whose
    /// subject is a cost (`style_lines_with` runs syntect): per thread, since
    /// each test runs on its own and a shared count would be another test's.
    pub(crate) static STYLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether a fenced code block gets real syntax highlighting, or flat `CODE`.
///
/// The distinction exists for **the block that is still streaming**. Every
/// arriving chunk invalidates its cache, so the next frame re-styles the whole
/// reply so far — and with syntect in the loop that is a ~45× multiplier on an
/// already-quadratic pass (measured: 36.8 ms per chunk at 32k of open code,
/// against 0.47 ms without). Nobody can read a colour scheme settling on text
/// that is still arriving, so the cost buys nothing until the turn seals, and
/// `Output::end_turn` invalidates on seal, which is when the real pass runs.
///
/// `highlight.rs` has always said the styles are "computed once at commit,
/// sliced per row — never per frame". This is what makes that true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    /// Drive syntect through the fence — for sealed content.
    Syntax,
    /// Fence bodies get flat `CODE`. Headings and inline spans still style:
    /// those are a character scan, not a parser, and dropping them would make
    /// text visibly change appearance when the turn ends.
    Flat,
}

pub fn style_lines_with(lines: &[String], highlight: Highlight) -> Vec<Vec<CellStyle>> {
    #[cfg(test)]
    STYLED.with(|n| n.set(n.get() + 1));
    let mut fence = Fence::None;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            // A delimiter toggles the fence: open it (parsing the info string as
            // a language for syntect), or close it. The ``` line is itself dimmed.
            fence = match fence {
                Fence::None if highlight == Highlight::Syntax => {
                    match CodeHighlighter::for_lang(trimmed.trim_matches('`')) {
                        Some(h) => Fence::Highlighted(Box::new(h)),
                        None => Fence::Plain,
                    }
                }
                // Streaming: open the fence, but do not build a highlighter for
                // it. The body renders as flat code and re-renders in colour
                // the moment the turn seals.
                Fence::None => Fence::Plain,
                _ => Fence::None,
            };
            out.push(vec![FENCE; chars.len()]);
            continue;
        }
        out.push(match &mut fence {
            Fence::Highlighted(h) => {
                let styles = h.line(line);
                // Defensive: fall back to flat code if the highlighter returned
                // a different char count than the line has.
                if styles.len() == chars.len() {
                    styles
                } else {
                    vec![CODE; chars.len()]
                }
            }
            Fence::Plain => vec![CODE; chars.len()],
            Fence::None if is_heading(trimmed) => vec![HEADING; chars.len()],
            Fence::None => style_inline(&chars),
        });
    }
    out
}

/// A CommonMark ATX heading: 1–6 leading `#` followed by a space (or
/// end-of-line). This excludes `#!shebang`, `#include`, `#define`, `#1` — a `#`
/// without a trailing space is *not* a heading, so those don't get mis-styled.
fn is_heading(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && trimmed[hashes..].chars().next().is_none_or(|c| c == ' ')
}

/// Style inline `` `code` `` spans (including the backticks) within a non-code
/// line, then its emphasis ([`emphasise`]); everything else is plain.
/// Greedy: each backtick pairs with the next. An unmatched backtick is left
/// plain.
fn style_inline(chars: &[char]) -> Vec<CellStyle> {
    let mut out = vec![CellStyle::PLAIN; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`'
            && let Some(rel) = chars[i + 1..].iter().position(|&c| c == '`')
        {
            let close = i + 1 + rel;
            for s in &mut out[i..=close] {
                *s = CODE;
            }
            i = close + 1;
            continue;
        }
        i += 1;
    }
    emphasise(chars, &mut out);
    out
}

/// An emphasis marker that paired: there, but quieter than what it marks.
const MARKER: CellStyle = CellStyle::PLAIN.dim();

/// **`*` and `_` emphasis** on one line: a run of one is italic, two bold,
/// three both — the text's own emphasis, over whatever role it wears (a
/// code span in bold is bold code). The markers stay, [`MARKER`]: what is
/// drawn is what was written, so a cursor, a search, a yank and a mark see
/// the same characters (vim's `conceallevel=0`).
///
/// CommonMark's flanking rules decide what can open and close, so `* item`,
/// `2 * 3`, `snake_case_name` and `\*` stay plain; a code span (styled
/// already, by the caller) is never searched. A closing run pairs with the
/// nearest open run of its marker and length — one stack each — and openers
/// of another kind inside the pair are dropped, as CommonMark's are: each
/// run is pushed and popped once, so a long line of markers that never
/// close costs its length, not its square.
fn emphasise(chars: &[char], out: &mut [CellStyle]) {
    // Open runs by marker (`*`, `_`) and length (1–3): where each starts.
    let mut open: [[Vec<usize>; 3]; 2] = Default::default();
    // +1 where a bold (italic) span's text starts, -1 where it ends: summed
    // below, so nesting costs nothing more.
    let mut bold = vec![0i32; chars.len() + 1];
    let mut italic = vec![0i32; chars.len() + 1];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !out[i].is_plain() {
            i += 1;
            continue;
        }
        if c == '\\' && chars.get(i + 1).is_some_and(char::is_ascii_punctuation) {
            i += 2;
            continue;
        }
        if c != '*' && c != '_' {
            i += 1;
            continue;
        }
        let end = i + chars[i..].iter().take_while(|&&d| d == c).count();
        let run = end - i;
        let before = i.checked_sub(1).map(|p| chars[p]);
        let (opens, closes) = flanking(c, before, chars.get(end).copied());
        if (1..=3).contains(&run) {
            let stack = &mut open[usize::from(c == '_')][run - 1];
            match stack.pop() {
                Some(from) if closes => {
                    out[from..from + run].fill(MARKER);
                    out[i..end].fill(MARKER);
                    if run >= 2 {
                        bold[from + run] += 1;
                        bold[i] -= 1;
                    }
                    if run != 2 {
                        italic[from + run] += 1;
                        italic[i] -= 1;
                    }
                    for stack in open.iter_mut().flatten() {
                        while stack.last().is_some_and(|&at| at > from) {
                            stack.pop();
                        }
                    }
                }
                waiting => {
                    stack.extend(waiting);
                    if opens {
                        stack.push(i);
                    }
                }
            }
        }
        i = end;
    }
    let (mut in_bold, mut in_italic) = (0, 0);
    for (at, cell) in out.iter_mut().enumerate() {
        in_bold += bold[at];
        in_italic += italic[at];
        if *cell != MARKER {
            cell.bold |= in_bold > 0;
            cell.italic |= in_italic > 0;
        }
    }
}

/// Whether a run of `marker` between `before` and `after` (`None` at the
/// line's ends) can open and can close emphasis — CommonMark's left- and
/// right-flanking, and `_`'s refusal inside a word.
fn flanking(marker: char, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
    let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
    let left = !space(after) && (!punct(after) || space(before) || punct(before));
    let right = !space(before) && (!punct(before) || space(after) || punct(after));
    if marker == '_' {
        (
            left && (!right || punct(before)),
            right && (!left || punct(after)),
        )
    } else {
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<CellStyle> {
        style_lines(&[line.to_string()]).pop().unwrap()
    }

    #[test]
    fn heading_line_is_all_heading() {
        let s = kinds("## Title");
        assert!(s.iter().all(|c| *c == HEADING));
        assert_eq!(s.len(), "## Title".chars().count());
    }

    #[test]
    fn hash_without_a_space_is_not_a_heading() {
        // CommonMark needs `#` + space; a shebang / preprocessor line is plain
        // (this was the `#!/usr/bin/env bash` rendered-cyan bug).
        for line in [
            "#!/usr/bin/env bash",
            "#include <stdio.h>",
            "#define X 1",
            "#1",
        ] {
            assert!(
                kinds(line).iter().all(|c| c.is_plain()),
                "{line:?} must not be a heading"
            );
        }
        // But real headings (and `#` alone) still are.
        assert!(kinds("# Title").iter().all(|c| *c == HEADING));
        assert!(kinds("#").iter().all(|c| *c == HEADING));
    }

    #[test]
    fn inline_code_spans_including_backticks() {
        // "use `cargo` now" → only the backtick-delimited run is CODE.
        let line = "use `cargo` now";
        let s = kinds(line);
        let chars: Vec<char> = line.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            let in_code = (4..=10).contains(&i); // `cargo` incl. both backticks
            let expect = if in_code { CODE } else { CellStyle::PLAIN };
            assert_eq!(s[i], expect, "char {i} ({c:?})");
        }
    }

    #[test]
    fn plain_fence_state_carries_across_lines() {
        // A fence with no language → flat `CODE`; the state carries across lines.
        let lines = [
            "before".to_string(),
            "```".to_string(),
            "anything goes".to_string(),
            "```".to_string(),
            "after".to_string(),
        ];
        let s = style_lines(&lines);
        assert!(s[0].iter().all(|c| c.is_plain())); // before the fence
        assert!(s[1].iter().all(|c| *c == FENCE)); // opening ```
        assert!(s[2].iter().all(|c| *c == CODE)); // code inside
        assert!(s[3].iter().all(|c| *c == FENCE)); // closing ```
        assert!(s[4].iter().all(|c| c.is_plain())); // after the fence
    }

    /// A line's styles as one letter a cell: `m` a marker, `b` bold, `i`
    /// italic, `B` both, `c` code, `C` bold code, `.` plain.
    fn shape(line: &str) -> String {
        kinds(line)
            .iter()
            .map(|s| match (*s, s.ink.is_some(), s.bold, s.italic) {
                (MARKER, ..) => 'm',
                (_, true, true, _) => 'C',
                (_, true, false, _) => 'c',
                (_, false, true, true) => 'B',
                (_, false, true, false) => 'b',
                (_, false, false, true) => 'i',
                _ => '.',
            })
            .collect()
    }

    /// **`**bold**`, `*italic*`, `_italic_` and `***both***` wear their
    /// emphasis, their markers dim** — the shapes of the reply Steven saw
    /// drawn raw (11-01), a list item's lead and a line that is a heading
    /// in all but name, among them.
    #[test]
    fn emphasis_wears_its_emphasis_and_its_markers_dim() {
        for (line, want) in [
            ("**firm** and *soft*", "mmbbbbmm.....miiiim"),
            ("_soft_", "miiiim"),
            ("***both***", "mmmBBBBmmm"),
            ("- **1:** Bonds", "..mmbbmm......"),
            ("**What I'd do next**", "mmbbbbbbbbbbbbbbbbmm"),
            ("un*frig*ly", "..miiiim.."),
        ] {
            assert_eq!(shape(line), want, "{line:?}");
        }
        assert!(
            MARKER.dim && MARKER.ink.is_none(),
            "a marker is the text, dim"
        );
    }

    /// **What is not emphasis stays as written** — CommonMark's flanking:
    /// a bullet, arithmetic, a name with underscores, an escape, a run with
    /// no partner or no matching length, a run longer than three, and
    /// whatever is inside a code span.
    #[test]
    fn what_is_not_emphasis_stays_plain() {
        for line in [
            "* item",
            "2 * 3 * 4",
            "snake_case_name and more_of_it",
            "a ** b",
            "**open",
            r"\*not\* this",
            "**a*",
            "****four****",
            "**open **again",
        ] {
            assert!(
                kinds(line).iter().all(CellStyle::is_plain),
                "{line:?}: {}",
                shape(line)
            );
        }
        assert_eq!(
            shape("`a**b**c`"),
            "ccccccccc",
            "a code span is never searched"
        );
    }

    /// **Emphasis nests, and holds code** — italic inside bold, bold inside
    /// italic, a code span in bold is bold code; a run that can neither open
    /// nor close leaves the one waiting before it (`**a ** b**` is bold
    /// throughout); and a run opened inside a pair cannot close past it
    /// (CommonMark: `**a *b** c*` is bold `a *b` and a plain `c*`).
    #[test]
    fn emphasis_nests_and_holds_code() {
        for (line, want) in [
            ("**bold *it* more**", "mmbbbbbmBBmbbbbbmm"),
            ("*a **b** c*", "miimmBmmiim"),
            ("**use `x`**", "mmbbbbCCCmm"),
            ("**a ** b**", "mmbbbbbbmm"),
            ("**a *b** c*", "mmbbbbmm..."),
        ] {
            assert_eq!(shape(line), want, "{line:?}");
        }
    }

    /// **A line of markers costs its length** — runs that open and never
    /// close, then pairs of another marker after them: each pair drops only
    /// the openers inside it, from the top of each stack, never scanning the
    /// rest. Measured 11-01 (debug): under 40 ms whole; with a `retain`
    /// there, the line's square, about 1.5 s.
    #[test]
    fn a_line_of_markers_costs_its_length() {
        let line: Vec<char> = "_a "
            .repeat(20_000)
            .chars()
            .chain("*b* ".repeat(20_000).chars())
            .collect();
        let start = std::time::Instant::now();
        let styles = style_inline(&line);
        let took = start.elapsed();
        assert_eq!(styles.len(), line.len());
        assert!(styles.iter().any(|s| s.italic), "premise: the pairs paired");
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    }

    proptest::proptest! {
        /// Whatever a model writes, one style a char, and no panic; a
        /// marker is only ever a marker character.
        #[test]
        fn emphasis_never_panics(line in "[*_`\\\\ a.]{0,64}") {
            let chars: Vec<char> = line.chars().collect();
            let styles = style_inline(&chars);
            proptest::prop_assert_eq!(styles.len(), chars.len());
            for (c, s) in chars.iter().zip(&styles) {
                proptest::prop_assert!(*s != MARKER || matches!(c, '*' | '_'), "{line:?}");
            }
        }
    }

    #[test]
    fn fenced_code_with_a_language_is_syntax_highlighted() {
        let lines = [
            "```rust".to_string(),
            "let x = 1;".to_string(),
            "```".to_string(),
        ];
        let s = style_lines(&lines);
        assert!(s[0].iter().all(|c| *c == FENCE)); // ```rust delimiter
        // The code line is syntect-highlighted: one style per char, not flat.
        assert_eq!(s[1].len(), "let x = 1;".chars().count());
        assert!(s[1].iter().any(|c| !c.is_plain() && *c != CODE));
    }
}

#[cfg(test)]
mod streaming_highlight_tests {
    use super::*;

    /// A `rust` fence whose body syntect can genuinely colour — `fn` and the
    /// identifier after it must not come out the same.
    fn fence() -> Vec<String> {
        ["```rust", "fn main() { let x = 1; }", "```"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// **Syntax highlighting happens once, at seal — not on every frame of a
    /// stream.**
    ///
    /// Every arriving chunk invalidates the open block's cache, so the next
    /// frame re-styles the whole reply so far. Driving syntect through that
    /// pass cost 1.7 s of CPU for a 4k-char reply and 25 s for a 16k one
    /// (measured). Flat while it streams; coloured the moment it seals.
    #[test]
    fn a_streaming_fence_is_flat_and_a_sealed_one_is_coloured() {
        let body = 1; // the code line, between the two fence delimiters

        let streaming = style_lines_with(&fence(), Highlight::Flat);
        assert!(
            streaming[body].iter().all(|c| *c == CODE),
            "a fence still arriving must not pay for syntect: {:?}",
            streaming[body]
        );

        let sealed = style_lines_with(&fence(), Highlight::Syntax);
        assert!(
            sealed[body].iter().any(|c| *c != CODE),
            "…and once sealed it must actually be highlighted: {:?}",
            sealed[body]
        );
        assert_eq!(
            sealed[body].len(),
            streaming[body].len(),
            "same cells either way — only their colour changes, so nothing reflows"
        );
    }

    /// The cheap styling keeps working while streaming. Headings and inline
    /// code are a character scan, not a parser; dropping them would make text
    /// visibly change appearance the instant a turn ended.
    #[test]
    fn streaming_still_styles_what_is_cheap() {
        let lines = vec!["## Title".to_string(), "a `span` here".to_string()];
        let flat = style_lines_with(&lines, Highlight::Flat);
        assert!(flat[0].iter().all(|c| *c == HEADING), "{:?}", flat[0]);
        assert!(
            flat[1].contains(&CODE),
            "inline code still styles: {:?}",
            flat[1]
        );
        assert_eq!(
            flat,
            style_lines_with(&lines, Highlight::Syntax),
            "outside a fence the two modes agree exactly"
        );
    }

    /// `style_lines` — what every non-streaming caller uses — still means
    /// "highlight it". The flat mode is opt-in, so a caller that has not
    /// thought about it gets the full pass.
    #[test]
    fn the_plain_entry_point_still_highlights() {
        assert_eq!(
            style_lines(&fence()),
            style_lines_with(&fence(), Highlight::Syntax)
        );
    }
}
