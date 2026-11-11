//! **The prompt's Normal mode against vim's** — one table of cases measured
//! in Neovim 0.12.6 headless (`nvim_feedkeys` as typed, Vim's `cw` special
//! case on: `'cpoptions'` `_`, Vim's `z`), each typed through the keys a user
//! presses. The manual promises "canonical vim — a faithful subset"
//! (`help.rs`); review stop 12 (T6 5) measured these where the code and vim
//! disagreed, and the tests asserted the code, or only the one case where
//! the two agree.

use super::*;
use crate::keymap::Mode;

/// One measured case: the lines, the cursor, the keys; then the lines, the
/// cursor and the register (`None`: never filled) vim left — in Normal mode,
/// every one.
type Case = (
    &'static [&'static str],
    (usize, usize),
    &'static str,
    &'static [&'static str],
    (usize, usize),
    Option<&'static str>,
);

/// Run each case from a fresh prompt in Normal mode, and say every one that
/// disagrees with vim, not only the first.
fn agrees_with_vim(cases: &[Case]) {
    let mut wrong = Vec::new();
    for &(lines, from, keys, want_lines, want_cursor, want_register) in cases {
        let mut app = test_app();
        app.set_focus(PaneKind::Prompt);
        app.prompt_mut().replace_with(&lines.join("\n"));
        type_keys(&mut app, "<Esc>");
        app.prompt_mut().place_cursor(from.0, from.1);
        type_keys(&mut app, keys);
        let got = (
            app.prompt().lines().collect::<Vec<_>>(),
            app.prompt().cursor(),
            app.prompt().register_text(),
            app.focused_mode(),
        );
        let want = (
            want_lines.iter().map(|l| l.to_string()).collect::<Vec<_>>(),
            want_cursor,
            want_register.map(str::to_string),
            Mode::Normal,
        );
        if got != want {
            wrong.push(format!(
                "{keys:?} on {lines:?} at {from:?}: {got:?}, vim {want:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **`a"` takes the white space after the quote, else the white space
/// before** — not both, not neither (`:h aquote`).
#[test]
fn a_quote_takes_trailing_white_space_else_leading() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["say \"hi\" now"], (0, 5), "da\"", &["say now"], (0, 4), Some("\"hi\" ")),
        (&["x \"hi\""], (0, 3), "da\"", &["x"], (0, 0), Some(" \"hi\"")),
        (&["\"hi\" there"], (0, 1), "da\"", &["there"], (0, 0), Some("\"hi\" ")),
        (&["a\"hi\"b"], (0, 2), "da\"", &["ab"], (0, 1), Some("\"hi\"")),
    ]);
}

/// **`cw` is `ce` only from inside a word** — and from a word's last char it
/// changes that char alone (the word's end is where it is); on blanks it is
/// `dw`, the blanks up to the next word (`:h cw`, `'cpoptions'` without `w`).
#[test]
fn change_word_changes_what_vim_changes() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["foo bar"], (0, 2), "cwX<Esc>", &["foX bar"], (0, 2), Some("o")),
        (&["a b"], (0, 0), "cwX<Esc>", &["X b"], (0, 0), Some("a")),
        (&["foo   bar"], (0, 3), "cwX<Esc>", &["fooXbar"], (0, 3), Some("   ")),
        (&["foo   bar"], (0, 5), "cwX<Esc>", &["foo  Xbar"], (0, 5), Some(" ")),
        (&["a/b c"], (0, 2), "cWX<Esc>", &["a/X c"], (0, 2), Some("b")),
        (&["foo bar"], (0, 0), "cwX<Esc>", &["X bar"], (0, 0), Some("foo")),
        (&["foo bar"], (0, 4), "cwX<Esc>", &["foo X"], (0, 4), Some("bar")),
        (&["foo bar baz"], (0, 2), "2cwX<Esc>", &["foX baz"], (0, 2), Some("o bar")),
        (&["a.. b"], (0, 2), "cwX<Esc>", &["a.X b"], (0, 2), Some(".")),
    ]);
}

/// **`aw` from blanks takes them and the word after** — the white space is
/// the leading kind then, so the word's trailing blank stays (`:h aw`).
#[test]
fn a_word_from_blanks_takes_the_word_after() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["foo   bar baz"], (0, 4), "daw", &["foo baz"], (0, 3), Some("   bar")),
        (&["foo   bar"], (0, 3), "daw", &["foo"], (0, 2), Some("   bar")),
        (&["   foo bar"], (0, 1), "daw", &[" bar"], (0, 0), Some("   foo")),
        (&["foo bar"], (0, 5), "daw", &["foo"], (0, 2), Some(" bar")),
        (&["foo bar"], (0, 1), "daw", &["bar"], (0, 0), Some("foo ")),
    ]);
}

/// **`b` and `w` cross lines as vim's do** — an indent is no word, a line of
/// blanks is passed over, and an empty line is a word (`:h word`).
#[test]
fn word_motions_cross_lines_as_vims_do() {
    const INDENTED: &[&str] = &["abc def", "  foo"];
    const BLANKS: &[&str] = &["abc", "   ", "foo"];
    const EMPTY: &[&str] = &["abc", "", "foo"];
    #[rustfmt::skip]
    agrees_with_vim(&[
        (INDENTED, (1, 2), "b", INDENTED, (0, 4), None),
        (BLANKS, (2, 0), "b", BLANKS, (0, 0), None),
        (BLANKS, (0, 0), "w", BLANKS, (2, 0), None),
        (EMPTY, (0, 0), "w", EMPTY, (1, 0), None),
        (EMPTY, (2, 0), "b", EMPTY, (1, 0), None),
        (&["abc", "  foo"], (0, 1), "w", &["abc", "  foo"], (1, 2), None),
        (&["a/b", "  c/d"], (0, 0), "W", &["a/b", "  c/d"], (1, 2), None),
        (&["foo", "", "bar"], (0, 2), "e", &["foo", "", "bar"], (2, 2), None),
        (&["foo bar"], (0, 4), "w", &["foo bar"], (0, 6), None), // runs out: its last char
        (&["foo bar"], (0, 6), "w", &["foo bar"], (0, 6), None),
        (&["foo "], (0, 2), "e", &["foo "], (0, 3), None),
        (&["foo bar"], (0, 5), "3b", &["foo bar"], (0, 0), None),
    ]);
}

/// **An operator over a word motion spans what vim's does, across lines** —
/// `w`'s last word stops at its line's end (`:h w`), `e` and `b` and a
/// count's earlier words cross; an exclusive end in column 0 goes back to
/// the line before or, from an indent, takes the lines (`:h exclusive`); a
/// `d` with only blanks around takes them whole (`:h d-special`). Every
/// operator over a motion was clamped to the cursor's line.
#[test]
fn word_operators_span_lines_as_vims_do() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["foo bar", "baz"], (0, 4), "dw", &["foo ", "baz"], (0, 3), Some("bar")),
        (&["foo bar", "  baz"], (0, 4), "dw", &["foo ", "  baz"], (0, 3), Some("bar")),
        (&["foo bar", "baz qux"], (0, 4), "d2w", &["foo qux"], (0, 4), Some("bar\nbaz ")),
        (&["foo", "bar"], (0, 0), "d2w", &[""], (0, 0), Some("foo\nbar\n")),
        (&["foo", "bar"], (0, 0), "y2w", &["foo", "bar"], (0, 0), Some("foo\nbar")),
        (&["a", "", "b"], (1, 0), "dw", &["a", "b"], (1, 0), Some("\n")),
        // From an empty line, `w`'s first step is onto the next: it stops
        // there, short of that line's indent.
        (&["a", "", "  b"], (1, 0), "dw", &["a", "  b"], (1, 2), Some("\n")),
        (&["a", "", "  b"], (1, 0), "cwX<Esc>", &["a", "X", "  b"], (1, 0), Some("\n")),
        (&["a", "", "  b"], (1, 0), "yw", &["a", "", "  b"], (1, 0), Some("\n")),
        (&["foo", "  bar"], (0, 2), "dw", &["fo", "  bar"], (0, 1), Some("o")),
        (&["foo"], (0, 2), "dw", &["fo"], (0, 1), Some("o")),
        (&["foo   ", "bar"], (0, 4), "dw", &["foo ", "bar"], (0, 3), Some("  ")),
        (&["foo bar"], (0, 0), "d3w", &[""], (0, 0), Some("foo bar")),
        (&["a", "", "b"], (1, 0), "cwX<Esc>", &["a", "X", "b"], (1, 0), Some("\n")),
        (&["foo bar", "baz qux"], (0, 4), "c2wX<Esc>", &["foo X qux"], (0, 4), Some("bar\nbaz")),
        (&["foo bar", "baz"], (0, 6), "de", &["foo ba"], (0, 5), Some("r\nbaz")),
        (&["foo  ", "bar"], (0, 3), "de", &["foo"], (0, 2), Some("  \nbar")),
        (&["foo "], (0, 3), "de", &["foo"], (0, 2), Some(" ")),
        (&["foo", "  bar"], (1, 2), "db", &["bar"], (0, 0), Some("foo\n  ")),
        (&["foo bar"], (0, 5), "d3b", &["ar"], (0, 0), Some("foo b")),
        (&["foo"], (0, 0), "db", &["foo"], (0, 0), None),
        (&["a/b c/d", "x"], (1, 0), "dB", &["a/b ", "x"], (0, 3), Some("c/d")),
        (&["foo bar", "baz"], (1, 0), "yb", &["foo bar", "baz"], (0, 4), Some("bar")),
    ]);
}

/// **`aw` reaches the next line as vim's does** — from a line's trailing
/// blanks, the next line's first word. One place the code parts from vim on
/// purpose: on blanks that end the buffer, `daw` fails in both, and vim's
/// cursor is left at the line's end; here a failed object moves nothing.
#[test]
fn a_word_object_reaches_the_next_line_as_vims_does() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["foo   ", "bar"], (0, 4), "daw", &["foo"], (0, 2), Some("   \nbar")),
        (&["foo bar", "baz"], (0, 5), "daw", &["foo", "baz"], (0, 2), Some(" bar")),
        (&["foo   "], (0, 4), "daw", &["foo   "], (0, 4), None), // vim: (0, 5)
        (&["foo", "", "bar"], (1, 0), "daw", &["foo"], (0, 0), Some("\nbar\n")),
        (&["foo", ""], (1, 0), "daw", &["foo", ""], (1, 0), None),
        (&["foo", "", "bar"], (1, 0), "diw", &["foo", "", "bar"], (1, 0), None),
        // From an empty LAST line vim's `iw` backs into the line before.
        (&["foo", ""], (1, 0), "diw", &["fo"], (0, 1), Some("o\n")),
        (&["foo", ""], (1, 0), "yiw", &["foo", ""], (0, 2), Some("o\n")),
        (&["", ""], (1, 0), "diw", &[""], (0, 0), Some("\n\n")),
    ]);
}

/// **`db` at a line's start deletes back into the line before** — to its
/// end, not the line break (`:h exclusive`); linewise when it starts on an
/// empty line (`:h exclusive-linewise`).
#[test]
fn delete_back_from_a_lines_start_reaches_the_line_before() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["foo bar", "baz"], (1, 0), "db", &["foo ", "baz"], (0, 3), Some("bar")),
        (&["foo", "", "baz"], (2, 0), "db", &["foo", "baz"], (1, 0), Some("\n")),
    ]);
}

/// **A linewise delete lands on the next line's first non-blank** — under
/// Vim's default `'startofline'`, measured with it set (Neovim's default,
/// off, keeps the column for `dd`; vim is the reference).
#[test]
fn a_linewise_delete_lands_on_the_first_non_blank() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["x", "  b"], (0, 0), "dd", &["  b"], (0, 2), Some("x\n")),
        (&["xyz", "abcd"], (0, 2), "dd", &["abcd"], (0, 0), Some("xyz\n")),
        (&["x", "y", "   z"], (0, 0), "dj", &["   z"], (0, 3), Some("x\ny\n")),
        (&["x", "   "], (0, 0), "dd", &["   "], (0, 2), Some("x\n")),
        (&["a", "", "  b"], (0, 0), "dap", &["  b"], (0, 2), Some("a\n\n")),
        (&["  a", "b"], (1, 0), "dd", &["  a"], (0, 2), Some("b\n")),
    ]);
}

/// **A charwise paste over lines leaves the cursor on its first char** —
/// `p` and `P` alike (`:h p`).
#[test]
fn a_charwise_paste_over_lines_leaves_the_cursor_at_its_start() {
    const TWO: &[&str] = &["foo", "bar"];
    #[rustfmt::skip]
    agrees_with_vim(&[
        (TWO, (0, 1), "vjyP", &["foo", "baoo", "bar"], (0, 1), Some("oo\nba")),
        (TWO, (0, 1), "vjyp", &["fooo", "bao", "bar"], (0, 2), Some("oo\nba")),
        (TWO, (0, 1), "vjy$p", &["foooo", "ba", "bar"], (0, 3), Some("oo\nba")),
    ]);
}

/// **The column comes back** — vim's sticky column (`:h curswant`): `j`
/// `k` through a shorter line land on its end and return to the column on a
/// longer one; `$` aims at every line's end; the column is a SCREEN column,
/// so a wide char holds two; nearly every other action resets it — an edit
/// that leaves the cursor where it is too (`x`, `r`, `u`, `yy`, `v<Esc>`) —
/// while a motion that fails, and Visual's `j` `k`, keep it. Measured in
/// Neovim 0.12 (review stop 12, Q5: "Match vim"). `G` and `gg` against vim's
/// `nostartofline`, as Shannon's keep the column; under vim's default
/// `startofline` they go to the first non-blank. A tab is not here: Shannon
/// draws it in no cells, vim in eight.
#[test]
fn the_column_comes_back_as_vims_does() {
    const L: &[&str] = &["abcdefghij", "abc", "abcdefghij"];
    const END: &[&str] = &["abcdef", "ab", "abcdefghij"];
    const EMPTY: &[&str] = &["abcdef", "", "abcdef"];
    const WORDS: &[&str] = &["abc def ghi", "x", "abc def ghi"];
    const WIDE: &[&str] = &["日本語x", "ab", "日本語x"];
    const UNDER: &[&str] = &["abcdefg", "日本語", "abcdefg"];
    const ONE: &[&str] = &["abcdefghij", "a", "abcdefghij"];
    const MID: &[&str] = &["abcdefghij", "abcd", "abcdefghij"];
    #[rustfmt::skip]
    agrees_with_vim(&[
        (L, (0, 7), "j", L, (1, 2), None),
        (L, (0, 7), "jj", L, (2, 7), None),
        (L, (0, 7), "jjk", L, (1, 2), None),
        (L, (0, 7), "jjkk", L, (0, 7), None),
        (L, (0, 7), "2j", L, (2, 7), None),
        (L, (2, 7), "kk", L, (0, 7), None),
        (L, (0, 7), "jhj", L, (2, 1), None),
        (L, (0, 7), "jlj", L, (2, 7), None),
        (L, (0, 7), "j0j", L, (2, 0), None),
        (L, (0, 7), "jxj", &["abcdefghij", "ab", "abcdefghij"], (2, 1), Some("c")),
        (L, (0, 7), "jix<Esc>j", &["abcdefghij", "abxc", "abcdefghij"], (2, 2), None),
        (L, (0, 7), "ja<Esc>j", L, (2, 2), None),
        (L, (0, 7), "vjj<Esc>", L, (2, 7), None),
        (L, (0, 7), "i<Down><Down><Esc>", L, (2, 6), None),
        (L, (0, 7), "G", L, (2, 7), None),
        (L, (0, 7), "jG", L, (2, 7), None),
        (&["abcdefghij", "abc"], (0, 7), "jgg", &["abcdefghij", "abc"], (0, 7), None),
        (END, (0, 0), "$jj", END, (2, 9), None),
        (END, (0, 0), "$jjk", END, (1, 1), None),
        (END, (0, 0), "$jhj", END, (2, 0), None),
        (&["abcdef", "ab", "abcdefgh"], (0, 5), "A<Down><Down><Esc>", &["abcdef", "ab", "abcdefgh"], (2, 5), None),
        (EMPTY, (0, 4), "j", EMPTY, (1, 0), None),
        (EMPTY, (0, 4), "jj", EMPTY, (2, 4), None),
        (WORDS, (0, 0), "wwjj", WORDS, (2, 8), None),
        (WORDS, (0, 0), "ejj", WORDS, (2, 2), None),
        (WIDE, (0, 3), "jj", WIDE, (2, 3), None),
        (UNDER, (0, 3), "j", UNDER, (1, 1), None),
        (UNDER, (0, 3), "jj", UNDER, (2, 3), None),
        (UNDER, (0, 4), "j", UNDER, (1, 2), None),
        (ONE, (0, 7), "jhj", ONE, (2, 7), None),
        (ONE, (0, 7), "jlj", ONE, (2, 7), None),
        (MID, (0, 7), "kjj", MID, (2, 7), None),
        (MID, (2, 7), "jkk", MID, (0, 7), None),
        (MID, (0, 7), "jvj<Esc>", MID, (2, 7), None),
        (MID, (0, 7), "jv<Esc>j", MID, (2, 3), None),
        (MID, (0, 7), "jV<Esc>j", MID, (2, 3), None),
        (MID, (0, 7), "jyyj", MID, (2, 3), Some("abcd\n")),
        (MID, (0, 7), "jhxj", &["abcdefghij", "abd", "abcdefghij"], (2, 2), Some("c")),
        (MID, (0, 7), "jrzj", &["abcdefghij", "abcz", "abcdefghij"], (2, 3), None),
        (MID, (0, 7), "jxuj", MID, (2, 3), Some("d")),
    ]);
}

/// **A `c` with nothing to change stays in Normal** — so the next key is a
/// command, not text: after a failed `ci(`, vim's `X` deletes the char
/// before the cursor.
#[test]
fn a_change_with_nothing_to_change_stays_in_normal() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["abc"], (0, 1), "ci(", &["abc"], (0, 1), None),
        (&["abc"], (0, 1), "ci\"", &["abc"], (0, 1), None),
        (&["abc"], (0, 0), "cj", &["abc"], (0, 0), None),
        (&["foo"], (0, 0), "cbx<Esc>", &["oo"], (0, 0), Some("f")),
        (&["abc"], (0, 1), "ci(X<Esc>", &["bc"], (0, 0), Some("a")),
        (&["a(b)c"], (0, 2), "ci(X<Esc>", &["a(X)c"], (0, 2), Some("b")),
    ]);
}

/// **An empty range is two things to the register** — a yank of one
/// empties it, a cut of one leaves it (nothing was cut): `yi"` over `""`,
/// `yl` on an empty line, `y0` at column 0 against their `d`s. Either moves
/// to the range's start. A failed object (`yi(` with no pair) is neither,
/// and leaves both. The code kept the register on every empty range, and
/// its one test fed `yi(` with no pair, which never reached the rule
/// (review stop 12, T6 6).
#[test]
fn an_empty_yank_empties_the_register_an_empty_cut_does_not() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["kept x\"\"y"], (0, 0), "yiwwlyi\"", &["kept x\"\"y"], (0, 7), Some("")),
        (&["kept x\"\"y"], (0, 0), "yiwwldi\"", &["kept x\"\"y"], (0, 7), Some("kept")),
        (&["kept x()y"], (0, 0), "yiwwlyi(", &["kept x()y"], (0, 7), Some("")),
        (&["kept x()y"], (0, 0), "yiwwldi(", &["kept x()y"], (0, 7), Some("kept")),
        (&["kept"], (0, 0), "yiwyi(", &["kept"], (0, 0), Some("kept")),
        (&["kept", ""], (0, 0), "yiwjyl", &["kept", ""], (1, 0), Some("")),
        (&["kept", ""], (0, 0), "yiwjdl", &["kept", ""], (1, 0), Some("kept")),
        (&["kept"], (0, 0), "yiwy0", &["kept"], (0, 0), Some("")),
        (&["kept"], (0, 0), "yiwd0", &["kept"], (0, 0), Some("kept")),
        (&["kept x\"\"y"], (0, 0), "yiwwlyi\"p", &["kept x\"\"y"], (0, 7), Some("")),
    ]);
}

// The motions below were measured 11-03 in Neovim 0.12.6 headless with
// `nvim_feedkeys(keys, "xtn")` — NOT remapped: `--clean` loads matchit,
// whose `%` mapping made `d30%` take two lines where vim's own `%` takes
// three (`:h N%`). Steven, 11-03: "We're still missing quite a number of the
// more common vim movements (e.g. `f`)".

/// **`f` `F` `t` `T` `;` `,` find along the line** (`:h f`) — the `count`-th
/// char, `t` one short; `;` again and `,` back, a `t` standing before its
/// char looking past it (`'cpoptions'` without `;`); under an operator `f`
/// `t` take their char and `F` `T` stop short, an operator's count
/// multiplying the find's; a char the line lacks moves nothing and cuts
/// nothing.
#[test]
fn a_find_lands_where_vims_does() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["one two three"], (0, 0), "fe", &["one two three"], (0, 2), None),
        (&["one two three"], (0, 0), "2fe", &["one two three"], (0, 11), None),
        (&["one two three"], (0, 0), "2te", &["one two three"], (0, 10), None),
        (&["one two three"], (0, 0), "te", &["one two three"], (0, 1), None),
        (&["one two three"], (0, 12), "Fo", &["one two three"], (0, 6), None),
        (&["one two three"], (0, 12), "To", &["one two three"], (0, 7), None),
        (&["a,b,c,d"], (0, 0), "f,;", &["a,b,c,d"], (0, 3), None),
        (&["a,b,c,d"], (0, 0), "t,;", &["a,b,c,d"], (0, 2), None),
        (&["a,b,c,d,e"], (0, 0), "t,2;", &["a,b,c,d,e"], (0, 2), None),
        (&["a,b,c,d"], (0, 0), "f,;,", &["a,b,c,d"], (0, 1), None),
        (&["a,b,c,d"], (0, 0), "t,;,", &["a,b,c,d"], (0, 2), None),
        (&["hello world"], (0, 0), "dfo", &[" world"], (0, 0), Some("hello")),
        (&["hello world"], (0, 0), "dto", &["o world"], (0, 0), Some("hell")),
        (&["hello world"], (0, 10), "dFo", &["hello wd"], (0, 7), Some("orl")),
        (&["hello world"], (0, 10), "dTo", &["hello wod"], (0, 8), Some("rl")),
        (&["ax"], (0, 0), "dtx", &["x"], (0, 0), Some("a")),
        (&["abc"], (0, 0), "fz", &["abc"], (0, 0), None),
        (&["abc"], (0, 0), "dfz", &["abc"], (0, 0), None),
        (&["a;b"], (0, 0), "f;", &["a;b"], (0, 1), None),
        (&["a,b,c,d,e"], (0, 0), "2d2f,", &["e"], (0, 0), Some("a,b,c,d,")),
        (&["hello world"], (0, 0), "cfoX<Esc>", &["X world"], (0, 0), Some("hello")),
        (&["a,b,c,d"], (0, 0), "f,d;", &["ac,d"], (0, 1), Some(",b,")),
    ]);
}

/// **`^` the first non-blank, `}` `{` a paragraph** (`:h ^`, `:h }`) — a
/// count of paragraphs that runs out moves nothing; `}` onto the last line
/// stops on its last char; `d}` from a line's start takes whole lines
/// (`:h d-special`), from inside one the chars.
#[test]
fn first_non_blank_and_paragraphs_land_where_vims_do() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["   indented"], (0, 9), "^", &["   indented"], (0, 3), None),
        (&["   indented"], (0, 9), "d^", &["   ed"], (0, 3), Some("indent")),
        (&["   indented"], (0, 1), "d^", &[" indented"], (0, 1), Some("  ")),
        (&["a", "b", "", "c", "d", "", "e"], (0, 0), "}", &["a", "b", "", "c", "d", "", "e"], (2, 0), None),
        (&["a", "b", "", "c", "d", "", "e"], (0, 0), "2}", &["a", "b", "", "c", "d", "", "e"], (5, 0), None),
        (&["a", "b", "", "c", "d", "", "e"], (0, 0), "3}", &["a", "b", "", "c", "d", "", "e"], (6, 0), None),
        (&["a", "b", "", "c", "d", "", "e"], (0, 0), "4}", &["a", "b", "", "c", "d", "", "e"], (0, 0), None),
        (&["a", "b", "", "c", "d", "", "e"], (6, 0), "{", &["a", "b", "", "c", "d", "", "e"], (5, 0), None),
        (&["a", "b", "", "c", "d", "", "e"], (6, 0), "3{", &["a", "b", "", "c", "d", "", "e"], (0, 0), None),
        (&["a", "b", "", "c", "d", "", "e"], (0, 0), "d}", &["", "c", "d", "", "e"], (0, 0), Some("a\nb\n")),
        (&["a", "b"], (0, 0), "d}", &[""], (0, 0), Some("a\nb\n")),
        (&["ab", "cd"], (0, 1), "d}", &["a"], (0, 0), Some("b\ncd")),
    ]);
}

/// **`ge` `gE` back to a word's end** (`:h ge`) — inclusive, so `dge` takes
/// the char it started on; an empty line is a word; from the buffer's start
/// it moves nothing and cuts nothing.
#[test]
fn back_to_a_words_end_lands_where_vims_does() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["one two three"], (0, 8), "ge", &["one two three"], (0, 6), None),
        (&["one two"], (0, 4), "ge", &["one two"], (0, 2), None),
        (&["one two"], (0, 4), "dge", &["onwo"], (0, 2), Some("e t")),
        (&["a.b c"], (0, 4), "gE", &["a.b c"], (0, 2), None),
        (&["a.b c"], (0, 4), "ge", &["a.b c"], (0, 2), None),
        (&["one", "", "two"], (2, 0), "ge", &["one", "", "two"], (1, 0), None),
        (&["one two"], (0, 0), "ge", &["one two"], (0, 0), None),
        (&["one two"], (0, 0), "dge", &["one two"], (0, 0), None),
    ]);
}

/// **`%` the matching bracket, `N%` a line** (`:h %`, `:h N%`) — from the
/// first bracket at or after the cursor, across lines; none, or no match,
/// moves nothing; `N%` is linewise, as `{n}G` is.
#[test]
fn match_pair_and_percent_land_where_vims_do() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (&["f(a, (b))"], (0, 0), "%", &["f(a, (b))"], (0, 8), None),
        (&["f(a, (b))"], (0, 8), "%", &["f(a, (b))"], (0, 1), None),
        (&["if (x) {", "  y", "}"], (0, 7), "%", &["if (x) {", "  y", "}"], (2, 0), None),
        (&["if (x) {", "  y", "}"], (2, 0), "%", &["if (x) {", "  y", "}"], (0, 7), None),
        (&["f(a)b"], (0, 0), "d%", &["b"], (0, 0), Some("f(a)")),
        (&["abc"], (0, 0), "%", &["abc"], (0, 0), None),
        // Quotes and escapes (stop 16, A M5, C 5 — measured as the rest).
        (&["f(\"(\", x)"], (0, 0), "%", &["f(\"(\", x)"], (0, 8), None),
        (&["f(\"(\", x)"], (0, 1), "%", &["f(\"(\", x)"], (0, 8), None),
        (&["f(a, \")\", b)"], (0, 1), "%", &["f(a, \")\", b)"], (0, 11), None),
        (&["(a \\) b)"], (0, 0), "%", &["(a \\) b)"], (0, 7), None),
        (&["f(\"(\", y)"], (0, 3), "%", &["f(\"(\", y)"], (0, 3), None),
        (&["f('(', y)"], (0, 1), "%", &["f('(', y)"], (0, 8), None),
        (&["x = \"(a)\""], (0, 5), "%", &["x = \"(a)\""], (0, 7), None),
        (&["if (a) {", "  s = \"}\";", "}"], (0, 7), "%", &["if (a) {", "  s = \"}\";", "}"], (2, 0), None),
        (&["f(\"(\", x"], (0, 1), "%", &["f(\"(\", x"], (0, 1), None),
        (&["say \"a\" (b) \"c\""], (0, 0), "%", &["say \"a\" (b) \"c\""], (0, 10), None),
        (&["(abc"], (0, 0), "%", &["(abc"], (0, 0), None),
        // An escaped bracket pairs with an escaped one, and may start;
        // an escaped quote opens nothing; a line with an odd number of
        // quotes has no spans; a literal is passed over but may start
        // (stop 16, mutants m14e–i survived the cases above — measured).
        (&["a \\( (b)"], (0, 0), "%", &["a \\( (b)"], (0, 0), None),
        (&["x \\( y \\) z"], (0, 0), "%", &["x \\( y \\) z"], (0, 8), None),
        (&["x \\( y \\) z"], (0, 8), "%", &["x \\( y \\) z"], (0, 3), None),
        (&["\\( (a) \\)"], (0, 0), "%", &["\\( (a) \\)"], (0, 8), None),
        (&["\\( (a) \\)"], (0, 3), "%", &["\\( (a) \\)"], (0, 5), None),
        (&["(a \\) b)"], (0, 4), "%", &["(a \\) b)"], (0, 4), None),
        (&["p \\\\( q)"], (0, 0), "%", &["p \\\\( q)"], (0, 7), None),
        (&["f(\"\\\")\", x)"], (0, 1), "%", &["f(\"\\\")\", x)"], (0, 10), None),
        (&["g(\" \\\" ) \", y)"], (0, 1), "%", &["g(\" \\\" ) \", y)"], (0, 13), None),
        (&["f(\"a, (b)"], (0, 6), "%", &["f(\"a, (b)"], (0, 8), None),
        (&["f(\"a, (b)"], (0, 1), "%", &["f(\"a, (b)"], (0, 1), None),
        (&["x '(' (a)"], (0, 0), "%", &["x '(' (a)"], (0, 0), None),
        (&["\\( '\\)' \\)"], (0, 0), "%", &["\\( '\\)' \\)"], (0, 9), None),
        (&["f('\\(', y)"], (0, 1), "%", &["f('\\(', y)"], (0, 9), None),
        (&["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], (0, 0), "50%", &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], (4, 0), None),
        (&["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], (0, 0), "15%", &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], (1, 0), None),
        // The column kept, as `{n}G` keeps it; past 100, nothing (stop 16,
        // A L1 and L2, C 5 — measured as the rest).
        (&["    abcdef"; 10], (0, 7), "50%", &["    abcdef"; 10], (4, 7), None),
        (&["    abcdef"; 10], (2, 7), "200%", &["    abcdef"; 10], (2, 7), None),
        (&["    abcdef"; 10], (2, 7), "d200%", &["    abcdef"; 10], (2, 7), None),
        (&["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], (0, 0), "d30%", &["4", "5", "6", "7", "8", "9", "10"], (0, 0), Some("1\n2\n3\n")),
    ]);
}

/// The lines [`an_operators_count_multiplies_its_motions_as_vims_does`] runs
/// on: four `x`s to find, two paragraphs, ten lines for a percentage.
const COUNTED: &[&str] = &[
    "1 x a x b x c x",
    "2",
    "3",
    "",
    "5",
    "6",
    "",
    "8",
    "9",
    "10",
];

/// **An operator's count multiplies its motion's** (`:h operator`: "the
/// counts are multiplied"), and the motion takes the product — `2dG` is
/// `d2G`, `2dtx` is `d2tx`, and `3d2fx` fails whole with four `x`s. As a
/// repeat, `2dG` ran `G` twice and `2dtx` stood still (stop 16, A M6).
#[test]
fn an_operators_count_multiplies_its_motions_as_vims_does() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (COUNTED, (0, 0), "2dG", &["3", "", "5", "6", "", "8", "9", "10"], (0, 0), Some("1 x a x b x c x\n2\n")),
        (COUNTED, (0, 0), "2d3G", &["", "8", "9", "10"], (0, 0), Some("1 x a x b x c x\n2\n3\n\n5\n6\n")),
        (COUNTED, (4, 0), "2dgg", &["1 x a x b x c x", "6", "", "8", "9", "10"], (1, 0), Some("2\n3\n\n5\n")),
        (COUNTED, (0, 0), "2dtx", &["x b x c x", "2", "3", "", "5", "6", "", "8", "9", "10"], (0, 0), Some("1 x a ")),
        (COUNTED, (0, 0), "2d2tx", &["x", "2", "3", "", "5", "6", "", "8", "9", "10"], (0, 0), Some("1 x a x b x c ")),
        (COUNTED, (0, 0), "2d}", &["", "8", "9", "10"], (0, 0), Some("1 x a x b x c x\n2\n3\n\n5\n6\n")),
        (COUNTED, (0, 0), "2d25%", &["6", "", "8", "9", "10"], (0, 0), Some("1 x a x b x c x\n2\n3\n\n5\n")),
        (COUNTED, (0, 0), "2dj", &["", "5", "6", "", "8", "9", "10"], (0, 0), Some("1 x a x b x c x\n2\n3\n")),
        (COUNTED, (0, 0), "2d2w", &["b x c x", "2", "3", "", "5", "6", "", "8", "9", "10"], (0, 0), Some("1 x a x ")),
        (COUNTED, (0, 0), "3d2fx", COUNTED, (0, 0), None),
    ]);
}

/// The lines [`a_count_in_visual_reaches_its_search_as_vims_does`] runs on:
/// four `a`s to search for, one above them.
const SEARCHED: &[&str] = &["x", "a", "b", "a", "c", "a", "d", "a"];

/// **A count typed in Visual reaches its search** (review stop 16, D 3) —
/// `v3n` ends the selection on the third match, as `3n` lands there in
/// Normal; it ended on the first. Measured in Neovim 0.12.6 (`v3ny` yanks
/// the six lines to it, `v3n<Esc>` stands on it).
#[test]
fn a_count_in_visual_reaches_its_search_as_vims_does() {
    #[rustfmt::skip]
    agrees_with_vim(&[
        (SEARCHED, (0, 0), "/a<CR>gg3n", SEARCHED, (5, 0), None),
        (SEARCHED, (0, 0), "/a<CR>ggv3n<Esc>", SEARCHED, (5, 0), None),
        (SEARCHED, (0, 0), "/a<CR>ggv3ny", SEARCHED, (0, 0), Some("x\na\nb\na\nc\na")),
    ]);
}

/// **`;` with no find before it moves nothing and says why** — vim beeps;
/// a dead key says what it would have needed (`App::concrete`). In the
/// transcript too, which has a keymap of its own: the last find is the
/// app's, so `fx` in the prompt then `;` in the transcript repeats it.
#[test]
fn a_repeat_with_no_find_is_said_and_one_carries_across_panes() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().replace_with("a,b,c");
    type_keys(&mut app, "<Esc>0;");
    assert_eq!(app.prompt().cursor(), (0, 0));
    assert!(
        app.notice().is_some_and(|n| n.contains("repeat an `f`")),
        "{:?}",
        app.notice()
    );
    // `c;` with nothing to repeat is refused and leaves Insert unentered, as
    // vim's cancelled change does: the `X` after it deletes (stop 16, C 4).
    app.prompt_mut().place_cursor(0, 1);
    type_keys(&mut app, "c;X");
    assert_eq!(app.prompt_keymap.mode(), Mode::Normal);
    assert_eq!(app.prompt().text(), ",b,c", "`X` deleted the `a`");
    app.prompt_mut().replace_with("a,b,c");
    type_keys(&mut app, "<Esc>0f,");
    assert_eq!(app.prompt().cursor(), (0, 1), "premise: the find");

    // The transcript has the chat keymap, not the prompt's: `;` there finds
    // the prompt's `,`.
    app.output_mut().begin_turn("a question");
    app.output_mut().append_chunk("one,two");
    app.output_mut().end_turn();
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let lines: Vec<String> = app.chat().lines().collect();
    let row = (lines.iter().position(|l| l.contains("one,two")))
        .unwrap_or_else(|| panic!("premise: the reply is in the transcript: {lines:?}"));
    app.chat_mut().expect("a transcript").place_cursor(row, 0);
    type_keys(&mut app, ";");
    let comma = (lines[row].chars().position(|c| c == ',')).expect("its comma");
    assert_eq!(app.chat().cursor(), (row, comma), "the prompt's find, here");
}
