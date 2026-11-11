//! Syntax highlighting for fenced code blocks (syntect).
//!
//! A `` ``` ``-fenced block whose info string names a known language is
//! highlighted line-by-line into the same render-agnostic [`CellStyle`] map the
//! rest of the styling uses, so it flows through the *same* per-block wrap+style
//! cache (computed once at commit, sliced per row — never per frame). syntect's
//! syntax set is heavy to build, so it loads once via `LazyLock`.
//!
//! **Into roles, not colours** (`style::Ink`): syntect names a token by its
//! TextMate scope (`keyword.control.rust`, `string.quoted.double`), and
//! `ROLES` says which role each scope is — so a model's code block is
//! painted by the active theme (`theme::Theme`), as the shell lexer's and
//! markdown's roles are, and inherits the terminal's colours as they do. It
//! painted from a bundled 24-bit theme (`base16-ocean.dark`) the terminal
//! never saw (Steven, 10-06).
//!
//! syntect is **stateful within a block** (a multi-line string keeps its
//! highlight across lines), so a [`CodeHighlighter`] is created when a fence
//! opens and advanced one line at a time until it closes — owned by the markdown
//! pass that drives the fence state.

use std::str::FromStr;
use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;

use crate::style::{CellStyle, Ink};

/// Line-based syntaxes (our block lines carry no trailing `'\n'`), loaded once.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);

/// **Which role a TextMate scope is.** syntect picks, per token, the
/// selector that matches its scope stack most specifically — so
/// `keyword.operator` beats `keyword`, and `variable.parameter` beats
/// `variable` — and these are its selectors. A scope none matches is plain.
///
/// `storage` — `storage.type` too — is a keyword: grammars put `let`, `fn`
/// and `class` there, and themes paint it so. No `meta.*` selector: a meta
/// scope spans a whole construct (`meta.function-call` is the call AND its
/// arguments), so it would paint a bare argument as the function's name.
const ROLES: &[(&str, Ink)] = &[
    ("comment", Ink::Comment),
    ("keyword, storage", Ink::Keyword),
    ("keyword.operator", Ink::Operator),
    ("string", Ink::String),
    (
        "string.regexp, constant.character.escape",
        Ink::StringSpecial,
    ),
    ("constant.numeric", Ink::Number),
    ("constant", Ink::Constant),
    ("constant.language", Ink::ConstantBuiltin),
    ("entity.name.function, variable.function", Ink::Function),
    ("support.function", Ink::FunctionBuiltin),
    (
        "entity.name.function.macro, support.macro",
        Ink::FunctionMacro,
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
         entity.name.trait, entity.other.inherited-class",
        Ink::Type,
    ),
    ("support.type, support.class", Ink::TypeBuiltin),
    ("variable", Ink::Variable),
    ("variable.language", Ink::VariableBuiltin),
    ("variable.parameter", Ink::Parameter),
    (
        "variable.other.member, variable.other.property",
        Ink::Member,
    ),
    ("entity.name.namespace, entity.name.module", Ink::Module),
    ("entity.other.attribute-name", Ink::Attribute),
    ("entity.name.tag", Ink::Tag),
    ("entity.name.label", Ink::Label),
    ("punctuation", Ink::Punctuation),
    ("markup.heading", Ink::Heading),
    ("markup.raw", Ink::Raw),
    ("markup.inserted", Ink::Added),
    ("markup.deleted", Ink::Removed),
    ("invalid", Ink::Error),
];

/// [`ROLES`] as a syntect theme whose "colour" for each selector carries the
/// role's index (`r`, with `a` marking it set) — syntect's own selector
/// scoring then names the role, and [`to_cell`] reads it back. A selector
/// that does not parse is skipped, not a panic on the render path.
static SCOPES: LazyLock<Theme> = LazyLock::new(|| Theme {
    settings: ThemeSettings {
        foreground: Some(UNSET),
        ..ThemeSettings::default()
    },
    scopes: ROLES
        .iter()
        .filter_map(|&(selector, ink)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(selector).ok()?,
                style: StyleModifier {
                    foreground: Some(Color {
                        r: ink as u8,
                        g: 0,
                        b: 0,
                        a: SET,
                    }),
                    background: None,
                    font_style: None,
                },
            })
        })
        .collect(),
    ..Theme::default()
});

/// A token no selector matched: plain.
const UNSET: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

/// The mark of a colour that carries a role.
const SET: u8 = 0xff;

/// Skip syntect above this many bytes on a single line. syntect's `fancy-regex`
/// engine is superlinear on adversarial input, so one multi-kilobyte line — a
/// minified bundle pasted into a fence — could stall the (synchronous,
/// commit-time) highlight for seconds and freeze the frame. A real code line is
/// far shorter; past the cap we fall back to flat code styling. (The skipped
/// line doesn't advance syntect's cross-line state, so following lines in the
/// same fence may mis-highlight — an acceptable trade against a multi-second
/// freeze on pathological input.)
const MAX_HIGHLIGHT_LINE_BYTES: usize = 4096;

/// A per-fenced-block highlighter, advanced one line at a time.
pub struct CodeHighlighter {
    inner: HighlightLines<'static>,
}

impl CodeHighlighter {
    /// `Some` if `lang` (the fence's info string, e.g. `rust`/`py`/`js`) maps to
    /// a known syntax; `None` for an unknown or empty language (the caller then
    /// falls back to flat code styling).
    pub fn for_lang(lang: &str) -> Option<Self> {
        let lang = lang.trim();
        if lang.is_empty() {
            return None;
        }
        let syntax = SYNTAXES
            .find_syntax_by_token(lang)
            .or_else(|| SYNTAXES.find_syntax_by_extension(lang))?;
        Some(Self {
            inner: HighlightLines::new(syntax, &SCOPES),
        })
    }

    /// Per-char styles for one code line (no trailing newline). Falls back to an
    /// empty (all-plain) map if syntect errors, so a highlight failure never
    /// breaks the render.
    pub fn line(&mut self, text: &str) -> Vec<CellStyle> {
        // Bound the worst case: an enormous single line falls back to flat code
        // (empty here → the caller paints the whole line `CODE`) rather than
        // risking a multi-second syntect stall on the render path.
        if text.len() > MAX_HIGHLIGHT_LINE_BYTES {
            return Vec::new();
        }
        let Ok(ranges) = self.inner.highlight_line(text, &SYNTAXES) else {
            return Vec::new();
        };
        let mut styles = Vec::with_capacity(text.chars().count());
        for (sty, piece) in ranges {
            let cell = to_cell(sty);
            styles.extend(std::iter::repeat_n(cell, piece.chars().count()));
        }
        styles
    }
}

/// The role [`SCOPES`] put in a token's colour, as a [`CellStyle`]; plain
/// where none did.
fn to_cell(s: syntect::highlighting::Style) -> CellStyle {
    match (s.foreground.a, Ink::ALL.get(s.foreground.r as usize)) {
        (SET, Some(&ink)) => CellStyle::ink(ink),
        _ => CellStyle::PLAIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_language_highlights_per_char() {
        let mut h = CodeHighlighter::for_lang("rust").expect("rust syntax exists");
        let styles = h.line("let x = 1;");
        // One style per char, and not entirely plain (the keyword/punct differ).
        assert_eq!(styles.len(), "let x = 1;".chars().count());
        assert!(styles.iter().any(|c| !c.is_plain()));
    }

    /// **A token is a role, never a colour** — the theme paints it (Steven,
    /// 10-06: the terminal's mappings, not a bundled theme's values).
    #[test]
    fn a_token_is_its_role() {
        let ink_at = |lang: &str, line: &str, needle: &str| {
            let mut h = CodeHighlighter::for_lang(lang).expect("a syntax");
            let styles = h.line(line);
            let at = line[..line.find(needle).expect("in the line")]
                .chars()
                .count();
            styles[at].ink
        };
        assert_eq!(ink_at("rust", "let x = 1;", "let"), Some(Ink::Keyword));
        assert_eq!(ink_at("rust", "let x = 1;", "1"), Some(Ink::Number));
        // A token no selector names is plain — not the role at index 0,
        // which syntect's default foreground (black, set) would read as
        // without `UNSET` (review of f50fda5, Reviewer B: unheld).
        assert_eq!(ink_at("rust", "let x = 1;", " x"), None);
        assert_eq!(ink_at("rust", "let x = 1;", "x "), None);
        assert_eq!(ink_at("rust", "let s = \"hi\";", "hi"), Some(Ink::String));
        assert_eq!(ink_at("rust", "// note", "note"), Some(Ink::Comment));
        assert_eq!(
            ink_at("python", "def go(): pass", "go"),
            Some(Ink::Function)
        );
        // Every selector parses: none is skipped in silence.
        assert_eq!(SCOPES.scopes.len(), ROLES.len());
    }

    #[test]
    fn an_overlong_line_falls_back_to_flat() {
        // Past the cap, `line` returns empty styles so the markdown caller paints
        // the line flat `CODE` instead of stalling syntect on a pathological line.
        let mut h = CodeHighlighter::for_lang("rust").expect("rust syntax exists");
        let huge = "a".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1);
        assert!(h.line(&huge).is_empty());
        // A normal line still highlights (the skip didn't wedge the highlighter).
        assert!(!h.line("let x = 1;").is_empty());
    }

    #[test]
    fn unknown_or_empty_language_is_none() {
        assert!(CodeHighlighter::for_lang("").is_none());
        assert!(CodeHighlighter::for_lang("   ").is_none());
        assert!(CodeHighlighter::for_lang("not-a-real-language-xyz").is_none());
    }
}
