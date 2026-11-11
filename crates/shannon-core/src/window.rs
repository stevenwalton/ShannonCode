//! **How full a context window is, when no request measured it.**
//!
//! A backend measures a window exactly: the last request's prompt and its
//! reply ([`Provenance::last_round`](crate::graph::Provenance::last_round)).
//! But that is a measurement of ONE history — the one its turn was answered
//! after ([`Provenance::follows`](crate::graph::Provenance::follows)).
//! Replayed onto another — a `:compact` keeps the last turn and replaces
//! everything before it — it describes a window that no longer exists. There
//! the window is estimated instead, and said to be (`~` on the status row).
//!
//! The estimate is the history's characters at [`CHARS_PER_TOKEN`], plus the
//! overhead a measurement shows the request carries beyond the history (a
//! system prompt, tool definitions): [`overhead`]. One function for the app
//! and for `examples/ctx_estimate.rs`, which measures how good it is against
//! the windows a store holds.

use crate::graph::{Follows, Turn};
use crate::message::{Block, Message};

/// Characters per token — the usual rough rule for English text.
pub const CHARS_PER_TOKEN: usize = 4;

/// The characters `message` sends: its text and every block's (a tool call's
/// input as the JSON it is sent as).
pub fn chars(message: &Message) -> usize {
    let blocks: usize = message
        .blocks
        .iter()
        .map(|block| match block {
            Block::Text { text } => text.chars().count(),
            Block::ToolUse { name, input, .. } => name.len() + input.to_string().chars().count(),
            Block::ToolResult { content, .. } => content.chars().count(),
            Block::Context { text, .. } => text.chars().count(),
            // Sent back, so counted, as text. The signature and an encrypted
            // block are opaque; what they cost is not documented.
            Block::Thinking { thinking, .. } => thinking.chars().count(),
            Block::RedactedThinking { .. } => 0,
        })
        .sum();
    message.content.chars().count() + blocks
}

/// The characters `turn` sends — its messages' ([`chars`]).
pub fn turn_chars(turn: &Turn) -> usize {
    turn.messages.iter().map(chars).sum()
}

/// `chars` of history as tokens, at [`CHARS_PER_TOKEN`] — the one rule the
/// estimate and the [`overhead`] both use, so they cannot disagree.
pub fn tokens(chars: usize) -> u32 {
    u32::try_from(chars / CHARS_PER_TOKEN).unwrap_or(u32::MAX)
}

/// **The estimated tokens of `turns`' messages** — the history alone,
/// without the request's [`overhead`].
pub fn estimate(turns: &[Turn]) -> u32 {
    tokens(turns.iter().map(turn_chars).sum())
}

/// **Does `turns`' last turn's recorded window measure THIS history?** — it
/// was answered after exactly the turn before it here (or first, at the
/// start). A turn sealed before [`Follows`] existed cannot say, and is
/// trusted as it always was.
pub fn measures(turns: &[Turn]) -> bool {
    let Some((last, before)) = turns.split_last() else {
        return false;
    };
    if last.provenance.last_round.is_none() && last.provenance.usage.is_none() {
        return false; // nothing measured at all
    }
    let here = before
        .last()
        .map_or(Follows::Start, |turn| Follows::Turn(turn.id));
    last.provenance
        .follows
        .is_none_or(|follows| follows == here)
}

/// **What a request carried beyond its history**: a `measured` window less
/// the [`tokens`] of the `history_chars` it held — the system prompt, the
/// tool definitions, the estimate's own error. Saturating: an estimate above
/// the measurement is no overhead, not a negative one.
pub fn overhead(measured: u32, history_chars: usize) -> u32 {
    measured.saturating_sub(tokens(history_chars))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::TurnConfig;
    use crate::{Message, Usage};
    use std::time::SystemTime;

    fn turn(text: &str, follows: Option<Follows>, measured: Option<u32>) -> Turn {
        let config = TurnConfig {
            model: "stub".into(),
            max_tokens: None,
            effort: None,
        };
        let mut t = Turn::seal(
            vec![Message::user(text), Message::assistant(text)],
            config,
            SystemTime::UNIX_EPOCH,
        );
        t.provenance.follows = follows;
        t.provenance.last_round = measured.map(|n| Usage {
            input_tokens: n,
            ..Default::default()
        });
        t
    }

    /// **A window measures the history its turn was answered after, and no
    /// other** — kept by a compaction behind a summary, it does not.
    #[test]
    fn a_window_measures_only_the_history_it_was_answered_after() {
        let first = turn("one", Some(Follows::Start), Some(10));
        let second = turn("two", Some(Follows::Turn(first.id)), Some(20));
        assert!(measures(std::slice::from_ref(&first)));
        assert!(measures(&[first.clone(), second.clone()]));
        let summary = turn("what was done", None, None);
        assert!(
            !measures(&[summary.clone(), second.clone()]),
            "kept behind a summary: not the history it measured"
        );
        assert!(
            !measures(std::slice::from_ref(&second)),
            "moved to the start"
        );
        assert!(!measures(&[first, summary]), "nothing measured");
        let legacy = turn("old", None, Some(30));
        assert!(measures(&[legacy]), "written before `follows`: as before");
        // Older still: the turn's usage recorded, no round's — measured too
        // (unpinned until review stop 12, T9).
        let mut usage_only = turn("older", None, None);
        usage_only.provenance.usage = Some(Usage {
            input_tokens: 40,
            ..Default::default()
        });
        assert!(measures(&[usage_only]), "a usage alone is a measurement");
        assert!(!measures(&[]));
    }

    /// The estimate counts every message's characters, blocks included,
    /// and the overhead is what a measurement held beyond it.
    #[test]
    fn the_estimate_counts_text_and_blocks_and_overhead_is_the_rest() {
        let mut t = turn(&"x".repeat(400), None, None);
        t.messages[1].blocks.push(Block::ToolResult {
            tool_use_id: "t".into(),
            content: "y".repeat(400),
            is_error: false,
        });
        assert_eq!(turn_chars(&t), 1200);
        assert_eq!(estimate(std::slice::from_ref(&t)), 300);
        assert_eq!(overhead(10_300, 1200), 10_000);
        assert_eq!(overhead(100, 1200), 0, "saturates");
    }
}
