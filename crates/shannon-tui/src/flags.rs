//! CLI-style flag parsing — the **base grammar** every flag-taking command
//! shares (`:child -m haiku`, `/child --model=opus`, and whatever comes next),
//! so the flag dialect is defined once and reads like the tools CLI users
//! already live in.
//!
//! Accepted forms (all four equivalent): `-m haiku`, `-m=haiku`,
//! `--model haiku`, `--model=haiku`. Rules:
//!
//! - Flags parse from the **front** of the argument string; the first non-flag
//!   token starts the free-text **remainder**, returned verbatim (spacing
//!   preserved) — so a prompt like `fix the -m flag handling` can't have words
//!   eaten out of its middle. A bare `--` ends flags explicitly (the standard
//!   escape when the free text itself must start with a dash).
//! - A [`FlagSpec`] declares each flag's long name, optional short char, and
//!   whether it **takes a value** — which is what disambiguates `-m haiku`
//!   (value) from a boolean flag followed by prompt text.
//! - Shorts normalize to their long name in the output ([`Flag::name`] is
//!   always the long spelling), so consumers match one name.
//! - Unknown flags and missing values are `Err` with the user-facing reason —
//!   surfaced on the notice line, never just logged ([[visible-refusals]]).

/// One flag a command understands: its canonical long name (no dashes), an
/// optional single-char short form, and whether it consumes a value.
pub struct FlagSpec {
    pub long: &'static str,
    pub short: Option<char>,
    pub takes_value: bool,
}

/// A parsed flag: the canonical (long) name and its value, if the spec takes
/// one. `-m haiku`, `-m=haiku`, `--model haiku`, `--model=haiku` all yield
/// `Flag { name: "model", value: Some("haiku") }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub name: String,
    pub value: Option<String>,
}

/// Parse leading flags off `input` per `specs`, returning the flags and the
/// untouched free-text remainder. See the module docs for the grammar.
pub fn parse_flags<'a>(input: &'a str, specs: &[FlagSpec]) -> Result<(Vec<Flag>, &'a str), String> {
    let mut flags = Vec::new();
    let mut rest = input.trim_start();
    // `while let`: the loop ends exactly when there is no next token, which is
    // the same condition spelled once instead of as a body-break.
    while let Some(token) = rest.split_whitespace().next() {
        // The explicit end-of-flags marker: skip it, everything after is text.
        if token == "--" {
            rest = rest[token.len()..].trim_start();
            break;
        }
        // First non-flag token → the remainder starts here.
        if !token.starts_with('-') || token == "-" {
            break;
        }
        // Split an inline `=value` off the flag word.
        let (word, inline) = match token.split_once('=') {
            Some((w, v)) => (w, Some(v.to_string())),
            None => (token, None),
        };
        let spec = lookup(word, specs)?;
        rest = rest[token.len()..].trim_start();
        let value = match (spec.takes_value, inline) {
            (true, Some(v)) => Some(v),
            (true, None) => {
                // Space-delimited value: the next token, whatever it is.
                let Some(v) = rest.split_whitespace().next() else {
                    return Err(format!("--{} needs a value", spec.long));
                };
                let v = v.to_string();
                rest = rest[v.len()..].trim_start();
                Some(v)
            }
            (false, Some(_)) => {
                return Err(format!("--{} does not take a value", spec.long));
            }
            (false, None) => None,
        };
        flags.push(Flag {
            name: spec.long.to_string(),
            value,
        });
    }
    Ok((flags, rest))
}

/// Resolve a flag word (`--model` or `-m`) to its spec, or the user-facing
/// reason it doesn't exist.
fn lookup<'s>(word: &str, specs: &'s [FlagSpec]) -> Result<&'s FlagSpec, String> {
    if let Some(long) = word.strip_prefix("--") {
        return specs
            .iter()
            .find(|s| s.long == long)
            .ok_or_else(|| format!("unknown flag --{long}"));
    }
    let short = word.strip_prefix('-').unwrap_or(word);
    let mut chars = short.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => specs
            .iter()
            .find(|s| s.short == Some(c))
            .ok_or_else(|| format!("unknown flag -{c}")),
        // `-abc` bundles are ambiguous with values and typos; we don't accept
        // them — say so rather than guess.
        _ => Err(format!("unknown flag {word} (shorts are single-char: -m)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs() -> Vec<FlagSpec> {
        vec![
            FlagSpec {
                long: "model",
                short: Some('m'),
                takes_value: true,
            },
            FlagSpec {
                long: "quiet",
                short: Some('q'),
                takes_value: false,
            },
        ]
    }

    fn model(v: &str) -> Flag {
        Flag {
            name: "model".into(),
            value: Some(v.into()),
        }
    }

    #[test]
    fn all_four_forms_are_equivalent() {
        // The user's exact examples: `-m=haiku`, `-m sonnet`, `--model Sonnet`,
        // `--model=Opus` — values pass through verbatim (casing is the
        // consumer's business).
        for (input, want) in [
            ("-m=haiku", model("haiku")),
            ("-m sonnet", model("sonnet")),
            ("--model Sonnet", model("Sonnet")),
            ("--model=Opus", model("Opus")),
        ] {
            let (flags, rest) = parse_flags(input, &specs()).unwrap();
            assert_eq!(flags, vec![want.clone()], "{input}");
            assert_eq!(rest, "", "{input}");
        }
    }

    #[test]
    fn flags_then_free_text_remainder() {
        let (flags, rest) = parse_flags("-m haiku summarize the   branch", &specs()).unwrap();
        assert_eq!(flags, vec![model("haiku")]);
        assert_eq!(rest, "summarize the   branch", "spacing preserved verbatim");
    }

    #[test]
    fn the_remainder_may_contain_dashes_safely() {
        // Flags parse from the FRONT only: once free text starts, `-m` is text.
        let (flags, rest) = parse_flags("fix the -m flag handling", &specs()).unwrap();
        assert!(flags.is_empty());
        assert_eq!(rest, "fix the -m flag handling");
    }

    #[test]
    fn double_dash_ends_flags_explicitly() {
        let (flags, rest) = parse_flags("-q -- --model is my topic", &specs()).unwrap();
        assert_eq!(
            flags,
            vec![Flag {
                name: "quiet".into(),
                value: None
            }]
        );
        assert_eq!(rest, "--model is my topic");
    }

    #[test]
    fn boolean_flags_do_not_eat_the_prompt() {
        // `takes_value: false` is what keeps `-q write a poem` intact.
        let (flags, rest) = parse_flags("-q write a poem", &specs()).unwrap();
        assert_eq!(flags[0].name, "quiet");
        assert_eq!(flags[0].value, None);
        assert_eq!(rest, "write a poem");
    }

    #[test]
    fn unknown_flags_and_missing_values_refuse_with_the_reason() {
        assert!(
            parse_flags("--nope x", &specs())
                .unwrap_err()
                .contains("--nope")
        );
        assert!(parse_flags("-z", &specs()).unwrap_err().contains("-z"));
        assert!(
            parse_flags("--model", &specs())
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            parse_flags("--quiet=loud", &specs())
                .unwrap_err()
                .contains("does not take a value")
        );
        assert!(
            parse_flags("-abc", &specs())
                .unwrap_err()
                .contains("single-char")
        );
    }

    #[test]
    fn empty_and_flagless_inputs_pass_through() {
        let (flags, rest) = parse_flags("", &specs()).unwrap();
        assert!(flags.is_empty());
        assert_eq!(rest, "");
        let (flags, rest) = parse_flags("   just a prompt", &specs()).unwrap();
        assert!(flags.is_empty());
        assert_eq!(rest, "just a prompt");
    }

    #[test]
    fn a_lone_dash_is_text_not_a_flag() {
        // `-` alone is a common stdin-ish token; treat it as the remainder.
        let (flags, rest) = parse_flags("- weird prompt", &specs()).unwrap();
        assert!(flags.is_empty());
        assert_eq!(rest, "- weird prompt");
    }
}
