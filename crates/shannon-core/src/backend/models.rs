//! Friendly model names.
//!
//! Model ids are long, dated, and impossible to type from memory
//! (`claude-haiku-4-5-20251001`). Comparing two models side by side is a
//! headline workflow — `:model` in one pane, `:model` in another, `:Send` to
//! both — and a workflow whose first step is recalling a datestamp is one
//! nobody performs twice.
//!
//! So there is a table, and it is **only** a table: an unrecognised name passes
//! through untouched. That is the concentric-rings rule
//! (`[[feedback_sane_defaults_rings]]`) — `haiku` for most people, the exact id
//! for anyone who needs a model this build has never heard of, and no gate in
//! between. It also means the table cannot go stale in the way that matters: a
//! new model works the day it ships, alias or no alias.

use crate::Effort;

/// Short name → model id, in the order `:model` lists them (cheapest and
/// fastest first, which is also the order you reach for while iterating).
///
/// Deliberately not exhaustive of everything the API accepts. These are the
/// names worth having in the fingers; anything else is typed in full.
// A table, read by column: rustfmt would give each row eight lines.
#[rustfmt::skip]
pub const ALIASES: &[Model] = &[
    // The offline stub, reachable by name so you can flip to it and back
    // mid-session. It is a *model* name rather than a separate switch because
    // that is the question the user is asking — "what answers me" — and because
    // it is the shape a second provider needs: the model names the provider,
    // and the app routes on it. See `STUB`.
    Model::new("stub", STUB, 4_096, 200_000, NO_EFFORT, None),
    // The current models (the API's model table, 2026-10-31): the short
    // names name the latest (Steven, 10-30: "The simple names should point
    // to the latest models"). Opus 5.5 and Haiku 5.5 work at `medium`.
    Model::new("haiku", "claude-haiku-5-5", 128_000, 1_000_000, EVERY_EFFORT, MEDIUM),
    Model::new("sonnet", "claude-sonnet-5-5", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    Model::new("opus", "claude-opus-5-5", 128_000, 1_000_000, EVERY_EFFORT, MEDIUM),
    Model::new("fable", "claude-fable-5-1", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    // Older, still served — kept, so a session recorded on one is still
    // measured by its own row, and listed under `:models`' older models.
    // The 5.0s, which the short names named until 10-04.
    Model::older("sonnet-5", "claude-sonnet-5", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    Model::older("opus-5", "claude-opus-5", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    Model::older("fable-5", "claude-fable-5", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    // The 4.x still served (the API's table, 10-31: Opus 4.5 is legacy and
    // its effort a beta header; Sonnet 4.5 retires 11-30). `xhigh` came with
    // Opus 4.7; `max` with 4.6; Haiku 4.5, which `haiku` named until 10-31,
    // takes none (the API's effort page).
    Model::older("opus-4-8", "claude-opus-4-8", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    Model::older("opus-4-7", "claude-opus-4-7", 128_000, 1_000_000, EVERY_EFFORT, HIGH),
    Model::older("opus-4-6", "claude-opus-4-6", 128_000, 1_000_000, NO_XHIGH, HIGH),
    Model::older("sonnet-4-6", "claude-sonnet-4-6", 128_000, 1_000_000, NO_XHIGH, HIGH),
    Model::older("haiku-4-5", "claude-haiku-4-5-20251001", 64_000, 200_000, NO_EFFORT, None),
];

/// A model with no effort control (Haiku 4.5; the stub).
const NO_EFFORT: &[Effort] = &[];
/// All five levels — every 5.x model, and Opus 4.7 and 4.8 (the API's
/// effort page).
const EVERY_EFFORT: &[Effort] = &Effort::ALL;
/// Every level but `xhigh`, which Opus 4.7 introduced — Opus and Sonnet 4.6.
const NO_XHIGH: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High, Effort::Max];
const HIGH: Option<Effort> = Some(Effort::High);
const MEDIUM: Option<Effort> = Some(Effort::Medium);

/// One row of [`ALIASES`]: the short name, the id, the two limits, and the
/// effort it takes.
///
/// A struct rather than a tuple because there are now two `u32`s in a row and
/// nothing but position tells them apart — `(64_000, 200_000)` reads the same
/// either way round, and getting it backwards would put a wrong percentage on
/// screen with nothing to notice it.
///
/// **The limits live in the row so a new model cannot be added without them.**
/// They were a `match` and a `const` alongside the table: Reviewer C added an
/// alias with no arm and every model test stayed green, because the generic
/// fallback satisfied each assertion by itself
/// (`[[feedback_enumeration_means_unmodeled]]`).
pub struct Model {
    pub alias: &'static str,
    pub id: &'static str,
    /// Largest reply, in tokens — the API's `max_tokens` ceiling.
    pub max_output: u32,
    /// Context window, in tokens.
    pub context: u32,
    /// The effort levels it takes, lowest first — empty for none.
    pub efforts: &'static [Effort],
    /// What it does when no effort is sent; `None` with no effort control.
    pub default_effort: Option<Effort>,
    /// **Superseded, still served** — what `:models` keeps in its older
    /// models fold (Steven, 10-31), shut until asked for.
    pub older: bool,
}

impl Model {
    const fn new(
        alias: &'static str,
        id: &'static str,
        max_output: u32,
        context: u32,
        efforts: &'static [Effort],
        default_effort: Option<Effort>,
    ) -> Self {
        Self {
            alias,
            id,
            max_output,
            context,
            efforts,
            default_effort,
            older: false,
        }
    }

    /// [`new`](Self::new), for a model a newer one has superseded — its
    /// shelf said by the constructor, so the table reads by it.
    const fn older(
        alias: &'static str,
        id: &'static str,
        max_output: u32,
        context: u32,
        efforts: &'static [Effort],
        default_effort: Option<Effort>,
    ) -> Self {
        Self {
            older: true,
            ..Self::new(alias, id, max_output, context, efforts, default_effort)
        }
    }
}

/// **`id`'s row** — by its id, or for Claude Code (`claude-code/opus`) by
/// the name after the prefix, an alias or an id. Claude Code's turns ran on
/// the model behind that name, so its window and its effort levels are
/// that model's: measured against the unknown-model fallback, a
/// `claude-code/sonnet` session read `ctx 100%` at a fifth of its 1M.
///
/// **And by the API's own alias for a dated id** — `claude-haiku-4-5` is
/// `claude-haiku-4-5-20251001` ([`undated`]). Missed, it was an unknown model:
/// sent the run's effort on every turn, each one a 400 (Reviewer, 10-04).
fn row(id: &str) -> Option<&'static Model> {
    let name = id
        .strip_prefix(super::claude_code::MODEL_PREFIX)
        .unwrap_or(id);
    ALIASES.iter().find(|m| {
        m.id == name
            || undated(m.id) == Some(name)
            || (name != id && m.alias.eq_ignore_ascii_case(name))
    })
}

/// `id` without the snapshot date the API puts on the end (`-20251001`), if it
/// has one — which is the API's alias for that model.
pub fn undated(id: &str) -> Option<&str> {
    let (base, date) = id.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit())).then_some(base)
}

/// **The effort levels `id` takes**, lowest first — empty for a model with
/// no effort control; `None` for a model this build does not know, whose
/// levels it cannot vouch for (the API answers that with a 400 naming it).
pub fn efforts(id: &str) -> Option<&'static [Effort]> {
    row(id).map(|m| m.efforts)
}

/// What `id` does when no effort is sent, if this build knows.
///
/// **Never for Claude Code.** `claude` with no `--effort` runs at the user's
/// own Claude Code setting (their `/effort`; `CLAUDE_CODE_EFFORT_LEVEL` is
/// Shannon's start level and kept from `claude`), and only failing that at
/// its model default — which is not even the API's
/// (Claude Code's docs: Sonnet 5.5 at `medium`). Reading through to the API
/// row told a `claude-code/opus` session it "works at medium" while it ran at
/// the user's `xhigh` (Reviewers, 10-04).
pub fn default_effort(id: &str) -> Option<Effort> {
    if super::claude_code::is_claude_code(id) {
        return None;
    }
    row(id).and_then(|m| m.default_effort)
}

/// **The effort a turn on `id` is sent, when `wanted` is asked for**:
/// `wanted` where the model takes it, and for a model this build does not
/// know — the API is then the judge, and its refusal is on screen. Nothing
/// for a model that does not take it: a session's effort (or the run's
/// default it follows) outlives a `:model` to Haiku, and must not turn every
/// one of its turns into a 400.
pub fn effort_for(id: &str, wanted: Option<Effort>) -> Option<Effort> {
    let wanted = wanted?;
    match efforts(id) {
        Some(levels) if !levels.contains(&wanted) => None,
        _ => Some(wanted),
    }
}

/// **How a model thinks** — a category, not a property of each row (Steven,
/// 10-27: "there's a category here and we should leverage that").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingKind {
    /// 4.6 and later: `{type: "adaptive", display}` — on by default from 5.x,
    /// off until asked on 4.6–4.8; `"enabled"` a 400 from 4.7 on.
    Adaptive,
    /// 4.5 and earlier: `{type: "enabled", budget_tokens}` only, or none.
    Budget,
}

/// **How `id` thinks, read off its generation** (`claude-opus-4-8` is 4.8)
/// — so a model typed in full works without a row. `None` for the stub, for
/// Claude Code (which keeps its own), and for an id with no version in it:
/// sent nothing, the API's default.
pub fn thinking_kind(id: &str) -> Option<ThinkingKind> {
    if id == STUB || super::claude_code::is_claude_code(id) {
        return None;
    }
    let (major, minor) = generation(id)?;
    Some(match (major, minor) >= (4, 6) {
        true => ThinkingKind::Adaptive,
        false => ThinkingKind::Budget,
    })
}

/// **`id`'s generation** — its first number and the one after it, if that
/// is a minor version rather than a snapshot date: `claude-haiku-4-5-20251001`
/// is 4.5, `claude-sonnet-4-20250514` 4.0, `claude-opus-5` 5.0,
/// `claude-3-7-sonnet-20250219` 3.7.
fn generation(id: &str) -> Option<(u32, u32)> {
    let version = |part: &str| {
        (!part.is_empty() && part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let mut parts = id.strip_prefix("claude-")?.split('-');
    let major = parts.by_ref().find_map(version)?;
    Some((major, parts.next().and_then(version).unwrap_or(0)))
}

/// **The offline stub, as a model id.**
///
/// `CannedStub::model()` has returned this since it was written, deliberately:
/// a turn produced offline records `canned-stub` in its `TurnConfig`, so a
/// resumed session makes plain which turns came from a real model and which did
/// not. That honesty half already existed; naming it here is what lets a
/// session be *pointed* at the stub rather than only landing there when no key
/// is configured.
pub const STUB: &str = "canned-stub";

/// The context window, in tokens, for `id`.
///
/// **This was a constant asserting the whole family shipped 200k**, with the
/// reasoning that the larger window was gated behind a beta header we do not
/// send. Steven checked, and then I checked the API's own model table: Opus 5,
/// Sonnet 5 and Fable 5 are **1M**, Haiku 4.5 is 200k, and no beta header is
/// involved. The `context-1m-2025-08-07` beta still exists — it is how the
/// *older* 200k models opt in — which is where the belief came from and why it
/// was true once. A doc comment is a claim, and this one had gone stale under
/// a correct-sounding argument (`[[feedback_doc_comments_are_claims]]`).
///
/// The consequence was not abstract: `ctx %` divided by 200k and saturated at
/// 100, so an Opus session 300k tokens in read **100% full** when it was 30%
/// used — and the context-exhausted notice would then tell the user to squash a
/// conversation with 700k of room left.
///
/// A model we have never heard of is measured against the smallest window we
/// know, so the gauge over-reports rather than under-reports: being told you
/// are fuller than you are costs a needless squash, while the other direction
/// walks you into a wall.
pub fn context_window(id: &str) -> u32 {
    row(id).map_or(UNKNOWN_MODEL_CONTEXT, |m| m.context)
}

/// The context window assumed for a model not in [`ALIASES`]. The smallest we
/// know — see [`context_window`] for why the gauge errs toward "fuller".
pub const UNKNOWN_MODEL_CONTEXT: u32 = 200_000;

/// The two fallbacks have to be coherent with each other: a model cannot be
/// asked for a reply longer than the context it is measured against.
///
/// A **compile-time** check, not a test. clippy pointed out that asserting it
/// at runtime is a constant expression and therefore measures nothing — which
/// is right, and the fix is to make it a build error rather than to delete it.
const _: () = assert!(UNKNOWN_MODEL_CEILING <= UNKNOWN_MODEL_CONTEXT);

/// The largest reply we ask a model for, in tokens, when the user has not said
/// otherwise.
///
/// **Not a budget guard.** The old `MAX_OUTPUT_TOKENS = 4096` was one — its
/// comment said "so a test key with a small budget cannot accidentally be
/// drained by a runaway turn" — and it silently guillotined every turn of a
/// live demo, four of which returned NO text at all while billing the full
/// 4,096. A spend guard belongs on spend, visibly; it does not belong on the
/// length of an answer, invisibly.
///
/// So the default is the model's own ceiling: Shannon imposes nothing the
/// provider would not. `[turn] max_output_tokens` lowers it for anyone who
/// wants the old behaviour deliberately.
///
/// **Checked against the live API, 10-31** (`examples/live_probe.rs models`:
/// every row asked at its own ceiling, each answered). Rerun it when a row
/// is added. A value above a model's real ceiling is rejected at validation
/// — before any tokens are generated, so a wrong entry costs a legible error
/// and nothing else: a number that is too big fails loudly, where a number
/// that is too small failed silently (a typed `claude-opus-4-8` ran at the
/// unknown model's 4,096 until it had a row — 10-31).
pub fn max_output_tokens(id: &str) -> u32 {
    if let Some(m) = row(id) {
        return m.max_output;
    }
    // A model this build has never heard of. **Conservative, not generous.**
    // The asymmetry decides it: a ceiling that is too LOW now announces itself
    // — the reply says it was cut off and `:continue` resumes it — while one
    // that is too HIGH is rejected by the API on every single turn, and the
    // message that would explain it names a config key the user then has to
    // find. 4,096 is the floor every Claude model has accepted.
    UNKNOWN_MODEL_CEILING
}

/// The reply ceiling for a model not in [`ALIASES`]. See
/// [`max_output_tokens`] for why it is the conservative number.
pub const UNKNOWN_MODEL_CEILING: u32 = 4_096;

/// Resolve a user-typed model name to the id the API expects.
///
/// Case-insensitive on the alias, because `Opus` is how people write it.
/// **Passes anything unrecognised straight through** rather than refusing:
/// refusing would make this table a gate on which models exist, which is a
/// promise no compiled-in list can keep. A wrong id fails at the API with a
/// message naming it, which is a better error than one we invent.
pub fn resolve(name: &str) -> String {
    let name = name.trim();
    let lower = name.to_ascii_lowercase();
    ALIASES
        .iter()
        .find(|m| m.alias == lower)
        .map(|m| m.id.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Whether this build **recognises** `name` — an alias or an id in [`ALIASES`].
///
/// Not a gate. [`resolve`] passes an unknown name straight through on purpose,
/// so a model released after this build, or a dated id like
/// `claude-opus-4-5-20251101`, still works without waiting for a new version.
/// The cost of that door is that `:model nonesuch` is accepted too, and the
/// API rejects it one turn later with a 400 the user has to connect back.
///
/// So this exists for the **message**, not the decision: the caller may still
/// set whatever it was handed, but it can say which of the two things it just
/// did. Claiming to understand a name we have never seen is the kind of small
/// lie `[[project_shannon_message_honesty]]` is about.
pub fn is_known(name: &str) -> bool {
    let name = name.trim();
    let lower = name.to_ascii_lowercase();
    ALIASES.iter().any(|m| {
        m.alias == lower
            || m.id.eq_ignore_ascii_case(name)
            || undated(m.id).is_some_and(|u| u.eq_ignore_ascii_case(name))
    })
}

/// The short name for an id, when there is one — for showing a model back to
/// the user in the space a status line has. The API's undated alias is its
/// row's, as `row` reads it: `:model claude-haiku-4-5` was "not a known
/// alias" while effort was filtered as Haiku's.
pub fn short_name(id: &str) -> &str {
    ALIASES
        .iter()
        .find(|m| m.id == id || undated(m.id) == Some(id))
        .map(|m| m.alias)
        .unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The category is the generation** — every row, and ids no row holds
    /// (Steven: "support a few of the <5.0 models (like Opus 4.8)"); a date
    /// is never a minor version; the stub, Claude Code and an id with no
    /// version are asked nothing.
    #[test]
    fn a_model_thinks_by_its_generation() {
        use ThinkingKind::{Adaptive, Budget};
        for (id, kind) in [
            ("claude-sonnet-5-5", Some(Adaptive)),
            ("claude-opus-5", Some(Adaptive)),
            ("claude-fable-5-1", Some(Adaptive)),
            ("claude-opus-4-8", Some(Adaptive)),
            ("claude-opus-4-6", Some(Adaptive)),
            ("claude-sonnet-4-6", Some(Adaptive)),
            ("claude-haiku-4-5-20251001", Some(Budget)),
            ("claude-opus-4-1-20250805", Some(Budget)),
            ("claude-sonnet-4-20250514", Some(Budget)),
            ("claude-3-7-sonnet-20250219", Some(Budget)),
            (STUB, None),
            ("claude-code/opus", None),
            ("claude-next", None),
            ("gpt-5", None),
        ] {
            assert_eq!(thinking_kind(id), kind, "{id}");
        }
        for row in ALIASES {
            assert_eq!(
                thinking_kind(row.id).is_some(),
                row.id != STUB,
                "{}: every row but the stub has a generation",
                row.alias
            );
        }
    }

    /// `resolve` passing an unknown name through is the feature; `is_known`
    /// is how a caller tells that apart from a typo. Both halves asserted,
    /// because a predicate that answers `true` for everything would leave
    /// `resolve`'s tests green and the message wrong.
    #[test]
    fn an_unknown_name_resolves_but_is_not_claimed_as_known() {
        assert_eq!(resolve("nonesuch"), "nonesuch", "the door stays open");
        assert!(!is_known("nonesuch"));
        assert!(!is_known("opusss"), "a near-miss is still a miss");
        assert!(is_known("opus"));
        assert!(is_known("OPUS"), "aliases are case-insensitive");
        assert!(is_known(" opus "), "and trimmed, like `resolve`");
        assert!(
            is_known("claude-opus-5"),
            "a full id is known too — `:model claude-opus-5` must not be \
             called unrecognised"
        );
        assert!(
            !is_known("claude-opus-4-5-20251101"),
            "premise: a dated id this build has never seen is NOT known — \
             which is exactly why `resolve` must still let it through"
        );
        assert_eq!(
            resolve("claude-opus-4-5-20251101"),
            "claude-opus-4-5-20251101"
        );
    }

    /// The short names name the CURRENT models (the API's model table,
    /// 2026-10-31); an older one keeps a name of its own.
    #[test]
    fn aliases_resolve_and_are_case_insensitive() {
        assert_eq!(resolve("haiku"), "claude-haiku-5-5");
        assert_eq!(resolve("Opus"), "claude-opus-5-5");
        assert_eq!(resolve("  sonnet  "), "claude-sonnet-5-5");
        assert_eq!(resolve("fable"), "claude-fable-5-1");
        assert_eq!(resolve("opus-5"), "claude-opus-5");
        assert_eq!(resolve("haiku-4-5"), "claude-haiku-4-5-20251001");
        assert_eq!(resolve("opus-4-8"), "claude-opus-4-8");
    }

    /// **The older shelf is the superseded rows, and only those** — what
    /// `:models` folds away. Each short name is a current model; every older
    /// row has a newer one of its kind on the current shelf.
    #[test]
    fn the_short_names_are_current_and_the_rest_are_older() {
        for alias in ["haiku", "sonnet", "opus", "fable", "stub"] {
            let row = ALIASES.iter().find(|m| m.alias == alias).unwrap();
            assert!(!row.older, "{alias} is current");
        }
        let older: Vec<&str> = ALIASES
            .iter()
            .filter(|m| m.older)
            .map(|m| m.alias)
            .collect();
        assert_eq!(
            older,
            [
                "sonnet-5",
                "opus-5",
                "fable-5",
                "opus-4-8",
                "opus-4-7",
                "opus-4-6",
                "sonnet-4-6",
                "haiku-4-5"
            ]
        );
    }

    /// **Effort, per model, as the API's effort page has it**: the 5.x models
    /// take all five levels; Opus 5.5 and Haiku 5.5 default to `medium`, the
    /// rest to `high`; Opus 4.7 and 4.8 take all five, 4.6 all but `xhigh`;
    /// Haiku 4.5 takes none; a model this build does not know is not
    /// vouched for either way. A `claude-code/…` name takes its model's
    /// LEVELS, but its default is Claude Code's own — the user's setting —
    /// which this build cannot know.
    #[test]
    fn each_model_takes_the_effort_its_row_says() {
        use Effort::{High, Low, Max, Medium};
        assert_eq!(efforts("claude-sonnet-5-5"), Some(&Effort::ALL[..]));
        assert_eq!(default_effort("claude-opus-5-5"), Some(Medium));
        assert_eq!(default_effort("claude-fable-5-1"), Some(High));
        assert_eq!(efforts("claude-haiku-5-5"), Some(&Effort::ALL[..]));
        assert_eq!(default_effort("claude-haiku-5-5"), Some(Medium));
        assert_eq!(efforts("claude-opus-4-8"), Some(&Effort::ALL[..]));
        assert_eq!(efforts("claude-opus-4-7"), Some(&Effort::ALL[..]));
        for id in ["claude-opus-4-6", "claude-sonnet-4-6"] {
            assert_eq!(efforts(id), Some(&[Low, Medium, High, Max][..]), "{id}");
            assert_eq!(default_effort(id), Some(High), "{id}");
        }
        assert_eq!(efforts("claude-haiku-4-5-20251001"), Some(&[][..]));
        assert_eq!(default_effort("claude-haiku-4-5-20251001"), None);
        assert_eq!(
            efforts("claude-something-9"),
            None,
            "unknown: not vouched for"
        );
        assert_eq!(
            default_effort("claude-code/opus"),
            None,
            "Claude Code's own"
        );
        assert_eq!(efforts("claude-code/opus"), Some(&Effort::ALL[..]));
        assert_eq!(efforts("claude-code/haiku"), Some(&Effort::ALL[..]));
        assert_eq!(efforts("claude-code/haiku-4-5"), Some(&[][..]));
    }

    /// **The API's undated alias is its dated row** — `claude-haiku-4-5` takes
    /// no effort and has Haiku's limits, through Claude Code too — and only a
    /// DATE is dropped: `claude-opus-5` is not `claude-opus-5-5`.
    #[test]
    fn an_undated_alias_is_its_dated_row() {
        let max = Some(Effort::Max);
        assert_eq!(effort_for("claude-haiku-4-5", max), None);
        assert_eq!(effort_for("claude-code/claude-haiku-4-5", max), None);
        assert_eq!(max_output_tokens("claude-haiku-4-5"), 64_000);
        assert_eq!(context_window("claude-haiku-4-5"), 200_000);
        assert_eq!(
            undated("claude-haiku-4-5-20251001"),
            Some("claude-haiku-4-5")
        );
        assert_eq!(undated("claude-opus-5-5"), None, "a version, not a date");
        assert_eq!(undated("claude-x-2025100"), None, "seven digits");
        assert!(is_known("claude-haiku-4-5"), "known, as `row` knows it");
        assert_eq!(short_name("claude-haiku-4-5"), "haiku-4-5");
        assert_eq!(
            short_name("claude-code/opus"),
            "claude-code/opus",
            "its route kept"
        );
        assert_eq!(efforts("claude-opus-5"), Some(&Effort::ALL[..]));
        assert_eq!(
            default_effort("claude-opus-5"),
            Some(Effort::High),
            "its own row"
        );
    }

    /// **What is sent**: the level asked for where the model takes it or is
    /// unknown (the API judges), nothing where it takes none, and nothing
    /// when nothing was asked.
    #[test]
    fn effort_is_sent_where_the_model_takes_it() {
        let max = Some(Effort::Max);
        assert_eq!(effort_for("claude-opus-5-5", max), max);
        assert_eq!(effort_for("claude-code/sonnet", max), max);
        assert_eq!(
            effort_for("claude-something-9", max),
            max,
            "unknown: the API judges"
        );
        assert_eq!(effort_for("claude-haiku-4-5-20251001", max), None);
        assert_eq!(effort_for(STUB, max), None);
        assert_eq!(effort_for("claude-opus-5-5", None), None);
    }

    #[test]
    fn an_unknown_name_passes_through_untouched() {
        // The ring that matters: a model this build has never heard of must
        // still be reachable, or the table becomes a gate on what exists.
        assert_eq!(resolve("claude-something-9"), "claude-something-9");
    }

    #[test]
    fn round_trip_through_the_short_name() {
        for m in ALIASES {
            assert_eq!(resolve(m.alias), m.id);
            assert_eq!(short_name(m.id), m.alias);
        }
    }

    #[test]
    fn the_default_model_has_a_short_name() {
        // If the default were not in the table, the status line would show a
        // datestamped id for the model everyone is using by default. Which
        // one is Steven's call (10-06: "Let's set Sonnet to the default").
        assert_eq!(
            short_name(super::super::anthropic::DEFAULT_MODEL),
            "sonnet",
            "the default model should be nameable, and Sonnet"
        );
    }
}

#[cfg(test)]
mod ceiling_tests {
    use super::*;

    /// **No model is capped at 4,096 any more.**
    ///
    /// That constant was a spend guard ("a test key with a small budget cannot
    /// accidentally be drained") sitting on the length of an answer, and it
    /// guillotined all seven turns of a live demo — four of which returned no
    /// text at all while billing the full 4,096. A guard on spend belongs on
    /// spend, and visibly.
    #[test]
    fn no_model_inherits_the_old_spend_guard() {
        for m in ALIASES {
            if m.id == STUB {
                continue;
            }
            assert!(
                max_output_tokens(m.id) > 4096,
                "{} is still capped at the old guard's value",
                m.id
            );
        }
    }

    /// **A model we have never heard of gets the CONSERVATIVE number**, which
    /// is the opposite of what this file first asserted.
    ///
    /// The asymmetry decides it: a ceiling too LOW announces itself — the reply
    /// says it was cut off and `:continue` resumes — while one too HIGH is
    /// rejected by the API on every single turn, and the message explaining it
    /// names a config key the user then has to go and find. "Generous enough
    /// not to cut a real answer" is the wrong optimisation for a model whose
    /// real ceiling we do not know (Reviewer A).
    #[test]
    fn an_unknown_model_is_never_given_a_larger_ceiling_than_a_known_one() {
        let unknown = max_output_tokens("claude-something-nobody-has-shipped");
        assert_eq!(unknown, UNKNOWN_MODEL_CEILING);
        let smallest_known = ALIASES
            .iter()
            .filter(|m| m.id != STUB)
            .map(|m| m.max_output)
            .min()
            .expect("the table is not empty");
        assert!(
            unknown <= smallest_known,
            "a guess ({unknown}) must not exceed the smallest ceiling we \
             actually know ({smallest_known}) — too high fails every turn"
        );
    }

    /// **A new alias cannot be added without a ceiling.**
    ///
    /// It used to fall through to the generic fallback, silently: Reviewer C
    /// added `("turbo", "claude-turbo-9")` and all six model tests stayed
    /// green, because every assertion was satisfied by the fallback itself. The
    /// ceiling is now a column of the alias row, so omitting one is a compile
    /// error — this records that the structure is load-bearing rather than
    /// re-checking what the type already enforces
    /// (`[[feedback_enumeration_means_unmodeled]]`).
    #[test]
    fn every_alias_carries_its_own_ceiling() {
        for m in ALIASES {
            assert!(m.max_output > 0, "{} has no ceiling", m.alias);
            assert_eq!(
                max_output_tokens(m.id),
                m.max_output,
                "{} resolves to a ceiling other than the one in its row",
                m.alias
            );
            if m.id != STUB {
                assert_ne!(
                    max_output_tokens(m.id),
                    UNKNOWN_MODEL_CEILING,
                    "{} is silently taking the unknown-model fallback",
                    m.alias
                );
            }
        }
    }

    /// The default model's ceiling is a real one, not the unknown-model
    /// fallback — the table has to cover what everybody actually runs.
    #[test]
    fn the_default_model_has_its_own_ceiling() {
        let known = max_output_tokens(super::super::anthropic::DEFAULT_MODEL);
        let unknown = max_output_tokens("claude-something-nobody-has-shipped");
        assert_ne!(
            known, unknown,
            "the model every session starts on is falling through to the default"
        );
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    /// **The numbers, against the API's own model table** (checked 2026-08-10,
    /// `platform.claude.com/docs/en/about-claude/models/overview`).
    ///
    /// The constant this replaced asserted the whole family shipped 200k, on
    /// the reasoning that the larger window was gated behind a beta header we
    /// do not send. That was true once — `context-1m-2025-08-07` is how the
    /// *older* 200k models opt in — and had quietly stopped being true. The
    /// argument was sound and the conclusion was stale, which is the shape a
    /// persuasive comment hides best.
    #[test]
    fn the_windows_match_the_published_table() {
        assert_eq!(context_window("claude-opus-5"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window("claude-fable-5"), 1_000_000);
        assert_eq!(context_window("claude-haiku-4-5-20251001"), 200_000);
        // The current ones (checked again 2026-10-31), and the 4.x still
        // served — all 1M and 128k but Haiku 4.5.
        for id in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-fable-5-1",
            "claude-haiku-5-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
        ] {
            assert_eq!(context_window(id), 1_000_000, "{id}");
            assert_eq!(max_output_tokens(id), 128_000, "{id}");
        }
        // **A Claude Code session is measured by its model's window**: it fell
        // back to the unknown 200k and read `ctx 100%` at a fifth of its 1M.
        assert_eq!(context_window("claude-code/sonnet"), 1_000_000);
        assert_eq!(context_window("claude-code/claude-opus-5-5"), 1_000_000);
        assert_eq!(context_window("claude-code/haiku"), 1_000_000);
        assert_eq!(context_window("claude-code/haiku-4-5"), 200_000);
        assert_eq!(
            context_window("claude-code/nonesuch"),
            UNKNOWN_MODEL_CONTEXT,
            "an unknown one still errs fuller"
        );

        assert_eq!(max_output_tokens("claude-opus-5"), 128_000);
        assert_eq!(max_output_tokens("claude-sonnet-5"), 128_000);
        assert_eq!(max_output_tokens("claude-fable-5"), 128_000);
        assert_eq!(max_output_tokens("claude-haiku-4-5-20251001"), 64_000);
    }

    /// A model's reply ceiling cannot exceed its own context window — a
    /// coherence check on the table rather than on any one number, so a typo
    /// that swaps the two columns is caught even where I have no external
    /// figure to compare against.
    #[test]
    fn no_model_can_reply_with_more_than_it_can_hold() {
        for m in ALIASES {
            assert!(
                m.max_output <= m.context,
                "{}: max_output {} exceeds its {} context window",
                m.alias,
                m.max_output,
                m.context
            );
        }
    }

    /// **An unknown model errs toward "fuller", not emptier.**
    ///
    /// Being told you have less room than you do costs a needless squash;
    /// being told you have more walks you into a wall unwarned. So the
    /// fallback is the smallest window we know, not the largest.
    #[test]
    fn an_unknown_model_is_measured_against_the_smallest_window_we_know() {
        let unknown = context_window("claude-something-nobody-has-shipped");
        assert_eq!(unknown, UNKNOWN_MODEL_CONTEXT);
        let smallest = ALIASES.iter().map(|m| m.context).min().expect("non-empty");
        assert!(
            unknown <= smallest,
            "the fallback ({unknown}) must not exceed the smallest known window ({smallest})"
        );
    }
}
