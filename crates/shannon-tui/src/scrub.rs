//! **Scrubbing what is bound for a view**: a text, a JSON value, a block —
//! every string in it — against the secrets as configured and the session's
//! aliases (`shannon_core::redact`).
//!
//! One rule, read in two places: `App`'s own scrub, and a [`Scrubber`] taken
//! from it to scrub off the UI thread — a Claude Code reading of 20 MB took
//! 2.7 s to scrub in release (10-17), and `:graph` did it on every opening
//! (`[[feedback_one_door_per_invariant]]`: the worker does not get a copy of
//! the rule, it gets the rule).

use std::sync::Arc;

use serde_json::Value;
use shannon_core::claude_import::{self, Imported, ImportedTurn};
use shannon_core::{Aliases, Block, SecretsConfig};

/// `text`, scrubbed.
pub(crate) fn text(text: &str, secrets: &SecretsConfig, aliases: &Aliases) -> String {
    shannon_core::redact(text, secrets, aliases).text
}

/// `value` — a tool call's input — scrubbed as its text would be: keys too,
/// each string as the value of its key (`shannon_core::redact_json`, the one
/// walk; this crate's own kept keys raw, review stop 4, E 2).
pub(crate) fn value(value: &Value, secrets: &SecretsConfig, aliases: &Aliases) -> Value {
    shannon_core::redact_json(value, secrets, aliases)
}

/// [`text`] for a VIEW of a command the user approved — the home's path
/// left as written (`shannon_core::redact_shown`). Never for what is stored
/// or sent.
pub(crate) fn shown_text(text: &str, secrets: &SecretsConfig, aliases: &Aliases) -> String {
    shannon_core::redact_shown(text, secrets, aliases).text
}

/// [`value`] for a VIEW, as [`shown_text`] is [`text`]'s.
pub(crate) fn shown_value(value: &Value, secrets: &SecretsConfig, aliases: &Aliases) -> Value {
    shannon_core::redact_json_shown(value, secrets, aliases)
}

/// [`text`] for a **command line** kept or logged as text — an audited, a
/// logged, a held one: also each value a credential's name announces, by
/// where the shell reads it (`shannon_core::redact_command`), as a call's
/// `"command"` is in [`value`]. A command scrubbed as [`text`] kept
/// `--password hunter2` (review stop 17, E1 M1).
pub(crate) fn command(text: &str, secrets: &SecretsConfig, aliases: &Aliases) -> String {
    shannon_core::redact_command(text, secrets, aliases).text
}

/// [`command`] for a VIEW, as [`shown_text`] is [`text`]'s.
pub(crate) fn shown_command(text: &str, secrets: &SecretsConfig, aliases: &Aliases) -> String {
    shannon_core::redact_command_shown(text, secrets, aliases).text
}

/// A call, a result or a text, its every string scrubbed.
pub(crate) fn block(block: &Block, secrets: &SecretsConfig, aliases: &Aliases) -> Block {
    let text = |s: &str| text(s, secrets, aliases);
    match block {
        Block::ToolUse { id, name, input } => Block::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: value(input, secrets, aliases),
        },
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Block::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: text(content),
            is_error: *is_error,
        },
        Block::Text { text: t } => Block::Text { text: text(t) },
        Block::Context {
            kind,
            source,
            text: t,
        } => Block::Context {
            kind: *kind,
            source: text(source),
            text: text(t),
        },
        // Shown, so scrubbed as text is. Its one caller, `Scrubber::turn`,
        // walks a Claude Code turn, whose import brings no thinking in
        // (`claude_import` counts it skipped); were one to, a byte changed is
        // a block the API drops — the right price for a value kept out.
        Block::Thinking {
            thinking,
            signature,
            at,
        } => Block::Thinking {
            thinking: text(thinking),
            signature: signature.clone(),
            at: *at,
        },
        Block::RedactedThinking { .. } => block.clone(),
    }
}

/// **What scrubs, taken to scrub with elsewhere** — the secrets and the
/// aliases as they were when it was taken. The app replaces its aliases
/// whole when they change, never in place, so the table this holds is the
/// one it scrubbed with, and [`same`](Self::same) knows a reading scrubbed
/// before a secret was marked from one scrubbed after.
#[derive(Clone)]
pub(crate) struct Scrubber {
    secrets: SecretsConfig,
    aliases: Arc<Aliases>,
}

/// Opaque: what it scrubs is the secrets themselves, and a section that
/// holds one is printed by tests that look for a value.
impl std::fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Scrubber")
    }
}

/// As [`same`](Scrubber::same): a section listed under one scrubs as
/// another listed under the same rule.
impl PartialEq for Scrubber {
    fn eq(&self, other: &Self) -> bool {
        self.same(other)
    }
}

impl Scrubber {
    pub(crate) fn new(secrets: &SecretsConfig, aliases: &Arc<Aliases>) -> Self {
        Scrubber {
            secrets: secrets.clone(),
            aliases: Arc::clone(aliases),
        }
    }

    /// Whether `other` scrubs as this does: the same secrets, the same
    /// table — held, so its address cannot be another's since.
    pub(crate) fn same(&self, other: &Scrubber) -> bool {
        self.scrubs_as(&other.secrets, &other.aliases)
    }

    /// **The defaults, one table for the whole run** — what a test's section
    /// is listed under: [`same`](Self::same) holds the table by address, so
    /// two made here scrub alike.
    #[cfg(test)]
    pub(crate) fn defaults() -> Self {
        static TABLE: std::sync::OnceLock<Arc<Aliases>> = std::sync::OnceLock::new();
        Scrubber::new(
            &SecretsConfig::default(),
            TABLE.get_or_init(Default::default),
        )
    }

    /// [`same`](Self::same), asked of the secrets and the table themselves —
    /// no scrubber taken to ask with: a tree rebuilt at every background
    /// event asks it of its section (`App::listed_as_now`).
    pub(crate) fn scrubs_as(&self, secrets: &SecretsConfig, aliases: &Arc<Aliases>) -> bool {
        self.secrets == *secrets && Arc::ptr_eq(&self.aliases, aliases)
    }

    /// `text`, scrubbed.
    pub(crate) fn text(&self, s: &str) -> String {
        text(s, &self.secrets, &self.aliases)
    }

    /// **A Claude Code turn, every word of it scrubbed** — its prompt, its
    /// reply, each round's text, calls and results. The one walk over a
    /// turn's words: a reading a view shows ([`imported`](Self::imported))
    /// and an import's replay (`App::replay`) both take it, and the match
    /// for older imports compares the two with `==` — two walks could part
    /// and a match fail without a word (review stop 4, C M6).
    pub(crate) fn turn(&self, turn: &ImportedTurn) -> ImportedTurn {
        let mut turn = turn.clone();
        turn.prompt = self.text(&turn.prompt);
        turn.reply = self.text(&turn.reply);
        for round in &mut turn.rounds {
            round.text = self.text(&round.text);
            for b in round.calls.iter_mut().chain(round.results.iter_mut()) {
                *b = block(b, &self.secrets, &self.aliases);
                // Cut once scrubbed, never before: a credential across the
                // cut kept its start (review stop 4, E 4).
                if let Block::ToolResult { content, .. } = b {
                    *content = claude_import::cut_result(std::mem::take(content));
                }
            }
        }
        turn
    }

    /// **A Claude Code reading as a view may show it** — each turn scrubbed
    /// ([`turn`](Self::turn)), as an import scrubs what it brings in. Its
    /// first `kept` turns come from `before` — the same file's reading as
    /// scrubbed before, by a scrubber alike, whose first `kept` turns were
    /// read unchanged since (`claude_import::Transcript::lineage`); the rest
    /// are scrubbed now. `kept` beyond `before` is a caller's mistake, and
    /// scrubs those again rather than reaching past it.
    pub(crate) fn imported(
        &self,
        raw: &Imported,
        kept: usize,
        before: Option<&Imported>,
    ) -> Imported {
        let kept = before.map_or(0, |b| kept.min(b.turns.len()).min(raw.turns.len()));
        let mut shown = Imported {
            id: raw.id.clone(),
            cwd: raw.cwd.clone(),
            turns: Vec::with_capacity(raw.turns.len()),
            skipped: raw.skipped,
            earlier: raw.earlier,
            leaf: raw.leaf.clone(),
        };
        if let Some(before) = before {
            shown.turns.extend_from_slice(&before.turns[..kept]);
        }
        shown
            .turns
            .extend(raw.turns[kept..].iter().map(|t| self.turn(t)));
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(prompts: &[&str]) -> Imported {
        Imported {
            id: "cc".into(),
            cwd: None,
            turns: prompts
                .iter()
                .map(|p| ImportedTurn {
                    prompt: (*p).into(),
                    ..ImportedTurn::default()
                })
                .collect(),
            skipped: Default::default(),
            earlier: None,
            leaf: None,
        }
    }

    /// **The turns that came through unchanged are taken as they were
    /// scrubbed, the rest scrubbed now** — the reason a grown file's re-read
    /// costs its new turns (review stop 4, C: taking none passed every test);
    /// and a `kept` past what was kept scrubs again, never reaching past it
    /// (B 8).
    #[test]
    fn a_reading_keeps_the_turns_scrubbed_before() {
        let scrubber = Scrubber::new(&SecretsConfig::default(), &Arc::new(Aliases::default()));
        let raw = reading(&["first", "second", "third"]);
        // Stand-ins only an unchanged turn taken as it was could carry.
        let before = reading(&["FIRST, as kept", "SECOND, as kept"]);
        let shown = scrubber.imported(&raw, 2, Some(&before));
        let prompts: Vec<&str> = shown.turns.iter().map(|t| t.prompt.as_str()).collect();
        assert_eq!(prompts, ["FIRST, as kept", "SECOND, as kept", "third"]);
        let past = scrubber.imported(&raw, 5, Some(&before));
        assert_eq!(past.turns.len(), 3, "no further than was kept");
        assert_eq!(past.turns[2].prompt, "third");
    }
}
