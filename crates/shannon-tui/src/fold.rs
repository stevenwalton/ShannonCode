//! **Folds** — rows shut to one line that says what they hold, opened and
//! shut by vim's `z` commands (Steven, 10-31: "I chose the term 'fold'
//! intentionally, in hopes that these could in fact share the same
//! primitives").
//!
//! What every fold shares lives here: the verbs and what each does to a
//! fold, and the glyph its line starts with. What a fold holds, and where
//! its state is kept, is each consumer's.
//!
//! Consumers: `:models`' older models (`model_picker`), and the transcript's
//! folds (N5a, `docs/DesignDocs/navigation.md` §6), whose `z` commands are
//! these (`bindings`' `Context::AfterZ`, `App::apply_fold`), and whose shut
//! row starts with this glyph (`output::FoldRow::lead`). Not yet wired: the
//! gutter's fold column (N6), which will make the glyph configurable.

/// **What a `z` command does to a fold** — vim's verbs, by vim's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldVerb {
    /// `zo` (and `zO`, which opens nested folds too): open.
    Open,
    /// `zc` (and `zC`): shut.
    Close,
    /// `za` (and `zA`): open a shut fold, shut an open one.
    Toggle,
    /// `zR`: open every fold.
    OpenAll,
    /// `zM`: shut every fold.
    CloseAll,
}

/// **vim's `z` keys for folds, and the verb each names** — the one list:
/// [`FoldVerb::after_z`] reads it (`:models` asks that), and the keymap
/// binds each (`bindings`' `Context::AfterZ`). Listed twice, in a `match`
/// and in the key table, a key added to one reached `:models` and not the
/// transcript (review stop 15, D L9). `zO` `zC` `zA` open and shut nested
/// folds too; none nest yet, so they are `zo` `zc` `za`.
pub(crate) const Z_KEYS: [(char, FoldVerb); 8] = [
    ('o', FoldVerb::Open),
    ('O', FoldVerb::Open),
    ('c', FoldVerb::Close),
    ('C', FoldVerb::Close),
    ('a', FoldVerb::Toggle),
    ('A', FoldVerb::Toggle),
    ('R', FoldVerb::OpenAll),
    ('M', FoldVerb::CloseAll),
];

impl FoldVerb {
    /// The verb `z` then `key` names ([`Z_KEYS`]), or `None` for a key that
    /// names no fold command — the chord ends there, as vim's does.
    pub(crate) fn after_z(key: char) -> Option<FoldVerb> {
        (Z_KEYS.iter()).find_map(|&(k, verb)| (k == key).then_some(verb))
    }

    /// Whether a fold that is `open` is open after this verb.
    pub(crate) fn apply(self, open: bool) -> bool {
        match self {
            FoldVerb::Open | FoldVerb::OpenAll => true,
            FoldVerb::Close | FoldVerb::CloseAll => false,
            FoldVerb::Toggle => !open,
        }
    }
}

/// **The glyph a fold's line starts with** — vim's `fillchars` defaults,
/// `foldclose:+` and `foldopen:-`: plain ASCII, which every font draws, and
/// unlike the triangles a list's cursor already uses (`▸`).
pub(crate) fn glyph(open: bool) -> char {
    if open { '-' } else { '+' }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **vim's `z` keys, vim's verbs** — and a key that names none ends the
    /// chord with nothing done.
    #[test]
    fn the_z_keys_are_vims() {
        use FoldVerb::*;
        for (key, verb) in [
            ('o', Open),
            ('O', Open),
            ('c', Close),
            ('C', Close),
            ('a', Toggle),
            ('A', Toggle),
            ('R', OpenAll),
            ('M', CloseAll),
        ] {
            assert_eq!(FoldVerb::after_z(key), Some(verb), "z{key}");
        }
        for key in ['r', 'm', 'z', 'j', ' '] {
            assert_eq!(FoldVerb::after_z(key), None, "z{key} is no fold command");
        }
    }

    /// Each verb from each state: open and `zR` open, shut and `zM` shut,
    /// `za` flips.
    #[test]
    fn each_verb_leaves_the_fold_as_it_says() {
        for open in [false, true] {
            assert!(FoldVerb::Open.apply(open));
            assert!(FoldVerb::OpenAll.apply(open));
            assert!(!FoldVerb::Close.apply(open));
            assert!(!FoldVerb::CloseAll.apply(open));
            assert_eq!(FoldVerb::Toggle.apply(open), !open);
        }
        assert_eq!((glyph(false), glyph(true)), ('+', '-'));
    }
}
