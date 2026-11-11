"""Vim-fidelity sweep against the running binary.

Each case seeds the prompt, runs a key sequence from Normal mode, and compares
the prompt's rendered text with what real vim would leave. Expectations are
written from vim's semantics, not from Shannon's -- the point is to find where
they differ. Anything marked KNOWN is a difference we have already decided
about; anything else that fails is a finding.
"""
import sys, os, re
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from term import start

def prompt_body(s):
    """The prompt pane's content lines, trailing blanks stripped."""
    lines = s.repaint().split("\n")
    top = next((i for i, l in enumerate(lines) if l.startswith("┌ prompt")), None)
    if top is None:
        return None
    out = []
    for l in lines[top + 1:]:
        if l.startswith("└"):
            break
        out.append(l[1:].rstrip().rstrip("│").rstrip() if l.startswith("│") else l)
    while out and not out[-1]:
        out.pop()
    return out

def reset(s, text):
    """Empty the prompt, then type `text` and return to Normal."""
    s.send("\x1b", settle=0.15)
    s.send("gg", settle=0.15)
    s.send("dG", settle=0.25)
    if text:
        s.send("i", settle=0.15)
        # CR, not LF: Enter is a carriage return on the wire, and a bare LF is
        # not a key any terminal sends. Seeding with "\n" silently produced a
        # ONE-line buffer and made nine motion cases look broken.
        s.send(text.replace("\n", "\r"), settle=0.5)
        s.send("\x1b", settle=0.25)
    return prompt_body(s)

CASES = [
    # (name, seed, keys, expected-lines)
    ("x deletes under cursor",        "abc",        "0x",        ["bc"]),
    ("X deletes before cursor",       "abc",        "$X",        ["ac"]),
    ("3x deletes three",              "abcdef",     "03x",       ["def"]),
    ("dw deletes a word",             "foo bar",    "0dw",       ["bar"]),
    ("d2w deletes two words",         "a b c",      "0d2w",      ["c"]),
    ("2dw is the same as d2w",        "a b c",      "02dw",      ["c"]),
    ("db deletes back",               "foo bar",    "$db",       ["foo r"]),
    # Trailing space is invisible here: the pane pads its rows and
    # `prompt_body` rstrips, so a buffer of "foo " and one of "foo" render the
    # same. The check is that `D` removed "bar", not what it left behind.
    ("D kills to line end",           "foo bar",    "0wD",       ["foo"]),
    ("dd kills the line",             "one\ntwo",   "ggdd",      ["two"]),
    ("2dd kills two lines",           "a\nb\nc",    "gg2dd",     ["c"]),
    ("cw changes a word",             "foo bar",    "0cwbaz\x1b", ["baz bar"]),
    ("C changes to line end",         "foo bar",    "0wCx\x1b",  ["foo x"]),
    ("cc changes the line",           "one\ntwo",   "ggccX\x1b", ["X", "two"]),
    ("s substitutes a char",          "abc",        "0sZ\x1b",   ["Zbc"]),
    ("~ toggles case",                "abc",        "0~",        ["Abc"]),
    ("J joins lines",                 "one\ntwo",   "ggJ",       ["one two"]),
    ("o opens below",                 "one",        "ooX\x1b",   ["one", "oX"]),
    ("O opens above",                 "one",        "OX\x1b",    ["X", "one"]),
    ("A appends at line end",         "ab",         "0AZ\x1b",   ["abZ"]),
    ("I inserts at line start",       "ab",         "$IZ\x1b",   ["Zab"]),
    ("a appends after cursor",        "ab",         "0aZ\x1b",   ["aZb"]),
    ("yy p duplicates a line",        "one",        "yyp",       ["one", "one"]),
    ("yy P pastes above",             "one\ntwo",   "ggyyP",     ["one", "one", "two"]),
    ("2yy p pastes two",              "a\nb",       "gg2yyp",    ["a", "a", "b", "b"]),
    ("u undoes",                      "abc",        "0xu",       ["abc"]),
    ("C-r redoes",                    "abc",        "0xu\x12",   ["bc"]),
    ("f finds forward",               "a.b.c",      "0f.x",      ["ab.c"]),
    # `x` always deletes a char on a non-empty line, so an expectation of "the
    # seed, unchanged" is an outcome vim cannot produce. `$`→c, `F.`→the second
    # dot, `,` (reverse) → the first dot, `x` deletes it.
    (", reverses a find",             "a.b.c",      "$F.F.,x",   ["a.bc"]),
    ("n repeats a search",            "aXbXc",      "/X\rnx",    ["aXbc"]),
    # Seeded so that DOING NOTHING and doing it RIGHT differ. `0t.x` on
    # "a.b.c" passed while `t` was unbound, because `t.` from column 0 does
    # not move in vim either -- the assertion measured no part of `t`.
    ("t stops before",                "ab.c",       "0t.x",      ["a.c"]),
    ("; repeats a find",              "a.b.c",      "0f.;x",     ["a.bc"]),
    ("% matches a bracket",           "(ab)c",      "0%x",       ["(abc"]),
    ("w moves by word",               "foo bar",    "0wx",       ["foo ar"]),
    ("e moves to word end",           "foo bar",    "0ex",       ["fo bar"]),
    ("b moves back a word",           "foo bar",    "$bx",       ["foo ar"]),
    ("^ is first non-blank",          "  ab",       "^x",        ["  b"]),
    ("$ is line end",                 "abc",        "0$x",       ["ab"]),
    ("gg goes to the top",            "a\nb\nc",    "Gggx",      ["", "b", "c"]),
    ("G goes to the bottom",          "a\nb\nc",    "ggGx",      ["a", "b"]),
    ("2G goes to line 2",             "a\nb\nc",    "2Gx",       ["a", "", "c"]),
    ("3j moves down three",           "a\nb\nc\nd", "gg3jx",     ["a", "b", "c"]),
    ("dip paragraph object",          "a\n\nb",     "ggdip",     ["", "b"]),
    ("di( cursor ON the paren",       "x(ab)y",     "0ldi(",     ["x()y"]),
    ("di( cursor inside",             "x(ab)y",     "0lldi(",    ["x()y"]),
    ("da( around",                    "x(ab)y",     "0llda(",    ["xy"]),
    ('di" inside',                    'x"ab"y',     '0lldi"',    ['x""y']),
    (". repeats the last change",     "aaaa",       "0x.",       ["aa"]),
    # Trailing spaces are invisible here — `prompt_body` rstrips, and the pane
    # pads its rows, so `"a.b "` and `"a.b"` render identically. An expectation
    # ending in a space can NEVER match; these two would have gone red the day
    # `W`/`B` landed (Reviewer C). Seeded so the surviving text differs instead.
    ("W big-word forward",            "a.b c.d",    "0Wx",       ["a.b .d"]),
    ("E big-word end",                "a.b c",      "0Ex",       ["a. c"]),
    ("B big-word back",               "a.b c.d",    "$Bx",       ["a.b .d"]),
    ("gU uppercases",                 "ab",         "0gUl",      ["Ab"]),
    ("F back-find",                   "a.b",        "$F.x",      ["ab"]),
    ("+ next line first non-blank",   "a\nb",       "gg+x",      ["a"]),
    ("C-a increments a number",       "a1",         "0\x01",     ["a2"]),
    ("m sets a mark",                 "ab",         "0max",      ["b"]),
    ("v y p yanks a selection",       "abc",        "0vly$p",    ["abcab"]),
    ("V y p yanks a line",            "one",        "Vyp",       ["one", "one"]),
    ("V d deletes a line",            "a\nb",       "ggVd",      ["b"]),
    ("v d deletes a selection",       "abcd",       "0vld",      ["cd"]),
    ("V G y G p (Steven's test)",     "a\nb",       "ggVGyGp",   ["a", "b", "a", "b"]),
    (">> indents",                    "ab",         ">>",        ["    ab"]),
    ("<< outdents",                   "    ab",     "<<",        ["ab"]),
    ("ciw changes inner word",        "foo bar",    "0ciwZ\x1b", ["Z bar"]),
    ("r replaces a char",             "abc",        "0rZ",       ["Zbc"]),
    ("P pastes before",               "ab",         "0ylP",      ["aab"]),
]

# Vim keys Shannon does not implement yet. Verified absent from `:keymap` as
# well as from behaviour, so these are unimplemented rather than broken.
# A name here that starts PASSING is reported: an out-of-date exclusion list
# quietly shrinks the suite.
KNOWN_MISSING = {
    "s substitutes a char", "~ toggles case", "J joins lines",
    "f finds forward", "F back-find", "t stops before", "; repeats a find",
    ", reverses a find", "n repeats a search",
    "% matches a bracket", "^ is first non-blank", ". repeats the last change",
    "W big-word forward", "E big-word end", "B big-word back",
    "gU uppercases", "+ next line first non-blank",
    "C-a increments a number", "m sets a mark",
    ">> indents", "<< outdents", "r replaces a char",
    "dip paragraph object",
}


def main():
    s = start(cols=100, rows=30)
    probe = reset(s, "seed")
    if probe != ["seed"]:
        print("!! reset mechanism is broken, got %r -- aborting" % (probe,))
        s.close(); return
    fails = []
    for name, seed, keys, want in CASES:
        reset(s, seed)
        for chunk in re.split(r'(\x1b)', keys):
            if chunk:
                s.send(chunk, settle=0.25 if chunk == "\x1b" else 0.4)
        got = prompt_body(s)
        ok = got == want
        known = name in KNOWN_MISSING
        if ok and known:
            print("NEW    " + name + "   (works now -- drop it from KNOWN_MISSING)")
        elif ok:
            print("PASS   " + name)
        elif known:
            print("missing" + " " + name)
        else:
            print("FAIL   " + name)
            fails.append((name, seed, keys, want, got))
    supported = [c for c in CASES if c[0] not in KNOWN_MISSING]
    print("\n%d/%d implemented keys behave like vim (%d known-missing)"
          % (len(supported) - len(fails), len(supported), len(KNOWN_MISSING)))
    for name, seed, keys, want, got in fails:
        print("\n--- %s\n    seed %r + %r\n    want %r\n    got  %r"
              % (name, seed, keys, want, got))
    s.close()

if __name__ == "__main__":
    main()
