//! Keeping credentials off the wire, off our disk, and out of the model's
//! context — while still letting the agent get work done.
//!
//! See `docs/DesignDocs/secrets.md`, §3a for the general model. Everything
//! crossing the machine/wire boundary passes through **one substitution table**:
//! outbound, local values become names; inbound, names become local values. That
//! single mechanism sits at three settings of four independent axes:
//!
//! | axis | question | values |
//! |---|---|---|
//! | recognition | how is a span found? | literal · issuer prefix · entropy · name-directed · path |
//! | action | what replaces it? | a kind label · a stable name |
//! | reversible | can it come back? | no, we never held it · yes, we do |
//! | sensitive | is spending it dangerous? | the gate stops it · it expands freely |
//!
//! So the modules here are not three mechanisms but three corners of that space:
//!
//! | module | corner |
//! |---|---|
//! | `redact` | recognise by shape or by name; **irreversible**, generated label |
//! | `alias` | recognise by exact value; **reversible**, stable name, sensitivity per entry |
//! | `paths` | recognise a path *before* the read happens, so nothing is produced to substitute |
//!
//! Redaction is a corner, not a separate thing: *an alias with no reverse and a
//! generated label.* Keeping the reversible and irreversible cases distinct is
//! still load-bearing (Presidio's lesson: masking and encryption have different
//! guarantees), but they are settings of one design rather than rival designs.
//!
//! The axis that was missing for three phases is **sensitive**, and folding it
//! into "is it aliased" is what kept the whole thing stuck on credentials.
//! Separated, the same code serves credentials, privacy (usernames, hostnames,
//! the program names in `ps` output) and token economy (a 60-character path
//! repeated 400 times in a build log) — see [`Aliases`].
//!
//! # What the config does and does not hold
//!
//! [`SecretsConfig`] is plain, serialisable policy — it is written to disk in the
//! reference `config.toml` and compared for equality. It therefore **never holds a
//! value**. Values live in [`Aliases`], which is session state, is
//! not `Serialize`, and does not print its contents under `Debug`. That separation
//! is a safety property, not a layering preference.

mod alias;
mod format;
mod json;
mod paths;
mod redact;
mod vault;

pub use alias::{
    Alias, Aliases, ENV_SOURCE, Expansion, KEPT_SOURCE, MARKED_SOURCE, ResolveAt, Restored,
    Unmasked, secret_handles_in,
};
pub use paths::CLAUDE_HOME;
pub(crate) use paths::{placed_entries, reads_as_written};
#[cfg(any(test, feature = "test-util"))]
pub use vault::MemoryVault;
pub use vault::{FileVault, Kept, NoVault, RECORD_CAP, Vault, VaultError};
// Not public API — the permission gate reads the `[secrets] paths` entries
// into its own rule line (`permission::defaults::credential_lines`), and a
// class or a brace keeps its protection as this dialect's own regex.
pub(crate) use paths::glob_entry_regex;
pub use redact::{
    Redaction, added_redactions, classify_kind, crosses_lines, describe, distinct,
    names_a_credential, redact, redact_command, redact_command_shown, redact_json,
    redact_json_shown, redact_lines, redact_shown,
};
// For `Config::clamp`, which reports settings that cannot take effect — asked
// of the redactor, which owns the rules, rather than re-derived in config.
pub(crate) use redact::{entropy_ceiling, inert_excludes};
// For the gate's "a credential's place that writes none down".
pub(crate) use redact::is_a_setting;

// For a test's count of what a file Read scrubbed (`files::run`).
#[cfg(test)]
pub(crate) use redact::scrubbed_by_lines;

use serde::{Deserialize, Serialize};

/// serde default for a bool field that ships enabled.
fn yes() -> bool {
    true
}

/// The defaults for the entropy rule, as serde needs them and as
/// [`SecretsConfig::default`] reads them — one definition each, so a config
/// written by hand and one left empty cannot disagree.
fn token_extra() -> String {
    "+=_-".to_string()
}

/// The arithmetic floor implied by [`entropy_bits`]'s default, not a guess.
///
/// Shannon entropy over one string needs at least `2^B` **distinct** characters
/// to reach `B` bits, so at 4.5 bits a run shorter than 23 can never match and
/// any lower value here is provably inert. The previous 32 was chosen by
/// intuition and measured to buy nothing: over 905,207 runs of real repo text
/// and file paths, dropping the gate from 32 to 1 adds **zero** false
/// positives, while 32 *loses* a real 31-character `ghp_` token and excludes
/// the 23–31 band outright. Be clear what that buys: recall *inside* the band
/// is low — about 5% of random 24-character tokens reach 4.5 bits, and about
/// 1% of `openssl rand -base64 16` output (its `/` splits the run). Lowering
/// the gate removes a cliff; it does not add coverage.
fn min_run() -> usize {
    23
}

/// See [`SecretsConfig::name_min_value`]. 12 is where the measured
/// false-positive curve flattens without costing any obvious secret.
fn name_min_value() -> usize {
    12
}

fn entropy_bits() -> f64 {
    4.5
}

/// See [`SecretsConfig::paths_except`]: Claude Code's files that hold no
/// credential, out of the `~/.claude` seed — and out of [`CLAUDE_HOME`],
/// wherever launch found it, or the guard there made Claude Code's own
/// memories Deny in its sessions. Its memories in EVERY project's
/// directory: carved out is not approved — a read outside the approved
/// places still asks (`[memory]` approves this project's).
fn paths_except() -> Vec<String> {
    [
        "~/.claude/projects/*/memory",
        "~/.claude/CLAUDE.md",
        "~/.claude/skills",
        "{claude-home}/projects/*/memory",
        "{claude-home}/CLAUDE.md",
        "{claude-home}/skills",
    ]
    .map(String::from)
    .to_vec()
}

/// The shipped `entropy_bits`, for `Config::clamp` to restore an out-of-domain
/// value to. One definition, so the reset and the default cannot disagree.
pub(crate) fn default_entropy_bits() -> f64 {
    entropy_bits()
}

/// The user's home directory, read **once**, with one plausibility floor.
///
/// One accessor because there were three, with three different floors that
/// nobody chose — they accreted. The weakest accepted `HOME=/`, which would make
/// every absolute path "under home"; the strictest rejected it. A value this
/// load-bearing (it decides whether a `deny` rule matches) must not mean
/// different things in different modules.
///
/// Cached: matching runs per rule per stage, and per operand per command, so a
/// syscall here is a syscall in a loop.
pub fn home_dir() -> Option<&'static str> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| std::env::var("HOME").ok().and_then(|h| plausible_home(&h)))
        .as_deref()
}

/// `HOME` as a path is compared: in the form every path is normalized to
/// before matching, or `None` when it is not plausibly a home at all.
///
/// Normalized, not only trimmed: a matcher strips this prefix from a
/// NORMALIZED path, so `/Users//you` never prefixed `/Users/you/x` and a
/// `~/…` entry failed open (review 20, Reviewer B). `/` itself would make
/// every absolute path home-relative; a relative `HOME` is nonsense.
fn plausible_home(raw: &str) -> Option<String> {
    Some(crate::shell::pipeline::normalize_path(raw)).filter(|h| h.len() > 1 && h.starts_with('/'))
}

/// Credential policy. Plain data so a frontend can source it from config.
///
/// `PartialEq` but not `Eq`: [`entropy_bits`](Self::entropy_bits) is an `f64`,
/// and bits-per-character is what the knob genuinely is — the unit every
/// comparable tool exposes (`detect-secrets --base64-limit 4.5`). Scaling it to
/// an integer to keep a marker trait would make the config worse to write for
/// the sake of a bound nothing here uses: this is never a map key or sorted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecretsConfig {
    /// Scrub credentials from command output before it is sent or stored.
    /// **On by default**: the cost of a false negative is a key in someone
    /// else's retention window.
    ///
    /// This used to say the cost of a false positive was "a mangled build log",
    /// and that was wrong twice over. It is mangled *source* — the model reasons
    /// and edits from this text, and a substitution is indistinguishable from
    /// the file genuinely saying that. And past the rate at which a user notices
    /// them, false positives *cause* false negatives, because the control they
    /// annoy gets switched off. Precision keeps this knob on, so it is a safety
    /// property here and not a convenience; see `docs/DesignDocs/secrets.md` §5.
    pub redact: bool,
    /// **Keep an unredacted copy of any output that was redacted**, in an
    /// owner-only file under `$XDG_STATE_HOME/shannon/redacted/`, one per
    /// command. **Off by default**, because it is a plaintext file of your
    /// secrets on disk.
    ///
    /// Why it exists: redaction happens before anything records the output, so
    /// without this the value is gone — the transcript, the session file and
    /// the logs all hold the redacted text. For a file that is fine (the value
    /// is still in the file). For a command that *mints* a secret and prints it
    /// once (`aws iam create-access-key`), it is not, and some people want that
    /// record (Steven, 2026-09-27).
    ///
    /// The path is shown to YOU in the notice and never told to the model, and
    /// the directory is on the gate's protected list whether or not this is on,
    /// so a model reading it is asked about first. Not encrypted — docketed
    /// (gpg), deferred.
    pub keep_redacted: bool,
    /// **Password-protect conversations that hold a secret you marked**
    /// (Steven, 10-23). A secret you mark (`:secret`) is kept for later runs
    /// either way, and a conversation reopened shows it to you as itself —
    /// `hunter2`, never `{{secret.06ea}}`; the model only ever has the handle.
    /// This decides one thing: whether reopening asks for a password first.
    ///
    /// - **Off, the default**: kept plain in Shannon's own owner-only store
    ///   ([`FileVault`]); nothing asks.
    /// - **On**: kept in the same store, sealed under a passphrase Shannon
    ///   asks for when a conversation holding one is opened — given, it opens
    ///   with its secrets; not given, it does not open. A forgotten
    ///   passphrase loses the sealed secrets, with no way back (Steven:
    ///   "lose a passphrase, lose the data"). **Not built yet** (the plan's
    ///   P2–P5): today this changes nothing.
    ///
    /// **Your config file only** — `:set` refuses it (`config::set::refuse`):
    /// changing it re-stores your secrets on disk, which a session-only
    /// setting must never do (`069099a`'s `:set` moved them for good).
    pub password_protect: bool,
    /// **Retired** (10-23) — read, so a config that names it still loads
    /// (`deny_unknown_fields` would refuse the whole file), and said at load:
    /// a mark is always kept now, and `password_protect` is the choice.
    /// Never shown or offered (`None` is not serialised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_marked: Option<bool>,
    /// Also flag long high-entropy strings that match no known issuer prefix.
    /// Catches formats we have never seen, at the price of occasionally
    /// redacting a hash or a base64 blob.
    ///
    /// Tuned by [`entropy_token_extra`](Self::entropy_token_extra),
    /// [`entropy_min_run`](Self::entropy_min_run) and
    /// [`entropy_bits`](Self::entropy_bits) — see those for what the rule
    /// actually is and why each knob exists.
    pub redact_high_entropy: bool,

    /// **Non-alphanumeric characters that are part of a candidate token rather
    /// than ending it.** Default `+=_-`.
    ///
    /// A credential is one unbroken run; a name is structured. So the candidate
    /// is a maximal run of `[A-Za-z0-9]` plus these, and everything else — a
    /// dot, a slash, a space — ends the run. `-` and `_` are in the default
    /// because URL-safe base64 embeds them: a 43-character token carrying two
    /// of them has a longest *alphanumeric* run of 23, and a rule that stopped
    /// at alphanumerics would miss it.
    ///
    /// `/` is deliberately **out**, which is where we part company with
    /// `detect-secrets`. Its base64 charset includes it, which is right when
    /// the unit you scan is a quoted string in source code and wrong when it is
    /// free-form command output: `docs/Security/SECURITY-2026-08-08-…md`
    /// fuses into one 56-character run and gets redacted.
    ///
    /// Widen it if your secrets carry a separator we do not expect; empty it
    /// for alphanumeric-only runs.
    #[serde(default = "token_extra")]
    pub entropy_token_extra: String,

    /// **How long a run must be before entropy is even consulted.** Default 23,
    /// which is the arithmetic floor implied by
    /// [`entropy_bits`](Self::entropy_bits), not a judgement: a run needs `2^B`
    /// distinct characters to reach `B` bits, so at 4.5 anything shorter than
    /// `ceil(2^4.5) = 23` can never match and setting it lower only costs work.
    /// `Config::clamp` reports that combination.
    ///
    /// This used to default to 32 and claim to be "most of the precision".
    /// Measured over 905,207 runs of real text and paths, lowering it to 1 adds
    /// *zero* false positives — the character-class gate is what earns them —
    /// while 32 excluded the 23–31 band outright and lost a real 31-character
    /// `ghp_` token. (Recall inside that band is still low — ~5% of random
    /// 24-char tokens clear 4.5 bits — so this removes a cliff, not a gap in
    /// coverage.) Shorter *secrets* than this
    /// generally carry an issuer prefix, matched separately and unaffected here.
    #[serde(default = "min_run")]
    pub entropy_min_run: usize,

    /// **Shannon bits per character a run must reach to count as a secret.**
    /// Default 4.5, matching `detect-secrets`' base64 default.
    ///
    /// Worth knowing what this number can and cannot do. Entropy over one
    /// string's character frequencies measures how evenly its characters are
    /// spread, not how unpredictable they are, and it is bounded by `log2(n)` —
    /// a 32-character run cannot exceed 5.0 bits however random it is. So
    /// random tokens and ordinary text sit closer together than intuition
    /// suggests, and the margins are thin: the random 32-character fixture in
    /// `key_shaped_runs_are_still_caught` clears 4.5 by 0.039 bits.
    ///
    /// **This is the dial**, and it has no right setting — a crypto repository
    /// is full of high-entropy strings and a CRUD app is not. Measured over
    /// 905,207 runs: at 4.5, one false positive and 70% of random 32-character
    /// tokens caught; at 4.35, eight false positives (including this project's
    /// own `SECURITY-2026-07-27-redirect-bypass` filename) and 92%. The curve is
    /// in `DEFAULT_TOML` so that lowering it is an informed choice.
    ///
    /// It replaces a 3.5 that sat below ordinary English prose, so nine of
    /// eleven strings in a realistic corpus were redacted — including
    /// `The build took 45 seconds and passed OK`.
    #[serde(default = "entropy_bits")]
    pub entropy_bits: f64,
    /// Additional credential **prefixes** to recognise, on top of the shipped
    /// set — for issuers we have never heard of, and for internal formats
    /// (`acme_tok_…`). A prefix needs at least 12 trailing characters before it
    /// counts, same as the built-ins, so a bare word never matches.
    pub prefixes: Vec<String>,
    /// Additional paths that should be treated as holding credentials, on top
    /// of the shipped seed. Same pattern grammar: `~/` for home, a trailing `*`
    /// for a prefix, a leading `*` for an extension, otherwise a whole path
    /// segment.
    pub paths: Vec<String>,
    /// **Places carved out of those paths**: a pattern that holds the whole
    /// place (`~/.claude` holds `~/.claude/skills`) is not a credential path
    /// inside it; one that names a file there (`*.pem`) still is. Same
    /// grammar as `paths`. By default Claude Code's own non-secret files —
    /// its memories, `~/.claude/CLAUDE.md` and `~/.claude/skills` — which
    /// `[memory] claude` reads, and which `~/.claude` refused whole (Steven,
    /// 10-06). `[]` makes all of `~/.claude` a credential path again. Only
    /// your own config sets it (`config::stack`'s `YOURS_ONLY`).
    #[serde(default = "paths_except")]
    pub paths_except: Vec<String>,
    /// What to do when a command touches one of those paths.
    #[serde(default)]
    pub path_policy: PathPolicy,
    /// Environment variables holding secrets the agent may **spend but never
    /// see**: each becomes an opaque handle it can write into a command, which
    /// is substituted for the real value at execution — after a human has
    /// looked at the line (`secrets.md` §4).
    ///
    /// This is the fine-grained answer to "let the agent use my key", which is
    /// a different request from "let the agent read my key" and only the second
    /// is dangerous. Empty by default: a secret is lent deliberately, never
    /// because it happened to be in the environment.
    pub env: Vec<String>,
    /// Alias `$HOME` to `~` in everything we send and store. **On by default.**
    ///
    /// The one alias we ship enabled, because it makes what leaves the machine
    /// *more correct*, not merely shorter: an absolute `/Users/you/...` is
    /// machine-specific and true nowhere else, while `~` is what a portable path
    /// looks like to every shell and every model. Privacy (your username stops
    /// travelling) and token economy come along free.
    ///
    /// Turn it off if you would rather see absolute paths in the transcript.
    #[serde(default = "yes")]
    pub alias_home: bool,
    /// Additional **names** that mean "a credential follows", on top of the
    /// shipped seed (`password`, `token`, `api_key`, …).
    ///
    /// This is the matcher for values with no distinguishing shape:
    /// `password: hunter2seekrit` is invisible to prefix and entropy matching,
    /// in every format. Matched as a segment *suffix* of the preceding token, so
    /// `stripe_api_key` counts and `tokenizer` does not. The value must be at
    /// least [`name_min_value`](Self::name_min_value) characters.
    pub names: Vec<String>,
    /// **How long a value must be before a credential-ish NAME is enough to
    /// redact it.** Default 12.
    ///
    /// The name rule qualified on the name alone, and its delimiters (`=`, `:`,
    /// `>`) are ordinary programming syntax, so it fired on **1 line in 677** of
    /// this repo's Rust and 1 in 333 of its prose:
    ///
    /// ```text
    /// let token = &argv[i];        ->  let token = &[redacted: named secret][i];
    /// use crate::secret::Secret;   ->  use crate::secret::[redacted: named secret];
    /// ```
    ///
    /// Measured over `.rs`/`.md`/`.py`, a 12-character floor removes ~95% of
    /// those: the values it stops redacting are `argv`, `Secret`, `new`,
    /// `from`, `key`.
    ///
    /// **Not where the context binds a credential** (11-08) — an assignment
    /// with no spaces to an environment-style name or a flag
    /// (`PGPASSWORD=hunter2`, `--password=hunter2`), a quoted value bound to
    /// a password's name (`password = "hunter2"`): there a value is taken at
    /// any length, because a short password is still a password (Steven).
    /// What the floor still gives up is a short value where the text could be
    /// code — `password: hunter2`, unquoted, as YAML or prose writes it.
    ///
    /// **Measured on the whole VALUE, not its first token**, and the whole value
    /// is redacted: to the closing quote if quoted, else to whitespace or code
    /// syntax. Measuring the token let `password: hunter2!Xk7pQ2mZ9vLab` through
    /// entirely, because `hunter2` is seven characters.
    ///
    /// Raise it if your code still trips it; lower it if you keep short
    /// credentials and would rather have the noise.
    ///
    /// **Shape detection is unaffected.** A recognised issuer prefix still
    /// redacts at any length — this floor governs only the fallback that trusts
    /// a name.
    #[serde(default = "name_min_value")]
    pub name_min_value: usize,

    /// Shipped names to **stop** treating as credential hints, for a codebase
    /// where one of them is ordinary vocabulary.
    ///
    /// [`names`](Self::names) documents itself as adding to the shipped seed, so
    /// it cannot narrow; without this the ring "defaults → config" does not
    /// close and a user drowning in `token`/`secret` has no way out. Compared
    /// by name segments, so `api_key`, `API-KEY` and `apiKey` all exclude the
    /// shipped `api-key` — write the spelling your code uses.
    pub names_exclude: Vec<String>,

    /// Characters that may separate such a name from its value. Empty means the
    /// shipped set (`=`, `:`, `>`), which covers `.env`, INI, TOML, CLI flags,
    /// YAML, JSON, `.pgpass`, `.npmrc` and XML.
    ///
    /// Add `" "` for `.netrc`-style `password hunter2` — deliberately not the
    /// default, because whitespace makes the prose "the password is stored in
    /// Vault" mask the word `is`. Setting this REPLACES the shipped set rather
    /// than adding to it, since the point is usually to narrow it.
    pub value_delimiters: Vec<String>,
    /// Extra literal values to scrub, for secrets whose shape we cannot guess
    /// (a database password, an internal token). These are compared verbatim.
    /// Empty by default, and serialized even so (`extra = []` in the
    /// DEFAULT_TOML reference), like every `[secrets]` list — `:config dump`
    /// shows every setting that exists. `:set` refuses it: a value typed
    /// there is echoed, logged and kept in history (`CREDENTIAL_KEYS`).
    pub extra: Vec<String>,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            redact: true,
            keep_redacted: false,
            password_protect: false,
            keep_marked: None,
            redact_high_entropy: true,
            // Through the same functions serde uses, so an empty config file
            // and `Default` cannot drift apart.
            entropy_token_extra: token_extra(),
            entropy_min_run: min_run(),
            entropy_bits: entropy_bits(),
            name_min_value: name_min_value(),
            names_exclude: Vec::new(),
            prefixes: Vec::new(),
            paths: Vec::new(),
            paths_except: paths_except(),
            path_policy: PathPolicy::default(),
            env: Vec::new(),
            alias_home: true,
            names: Vec::new(),
            value_delimiters: Vec::new(),
            extra: Vec::new(),
        }
    }
}

impl SecretsConfig {
    /// **The policy a rule's words are read under**: this one, with
    /// [`redact`](Self::redact) on whatever it says. The switch is about what
    /// leaves for the provider; a rule is kept as written and `:config save`
    /// publishes it, so `redact = false` lets a value travel unmasked, never
    /// be kept in a rule. The named arm (a `--password`'s value) never asked
    /// the switch and the shaped arm (a URL's userinfo) did, so one rule was
    /// refused and the other taught (review stop 17, E1 L6; Steven, 11-09:
    /// "let's refuse both"). Everything else is still yours: names, prefixes,
    /// the entropy dial.
    ///
    /// Asked where a rule meets the redactor, which is one place:
    /// `permission::reads_as_a_credential`, for each word and for a rule's
    /// whole text.
    pub fn for_rules(&self) -> std::borrow::Cow<'_, SecretsConfig> {
        match self.redact {
            true => std::borrow::Cow::Borrowed(self),
            false => std::borrow::Cow::Owned(SecretsConfig {
                redact: true,
                ..self.clone()
            }),
        }
    }
}

/// What happens when a command touches a path that commonly holds credentials.
///
/// Its own axis, deliberately: "let the agent USE my key" and "let the agent
/// READ my key" are different requests (see `secrets.md` §7a), so this is not
/// folded into one global off switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathPolicy {
    /// Always put it in front of a human, even when policy would otherwise let
    /// it run unattended. **Default** — it does not forbid anything, it only
    /// refuses to do it *silently*.
    #[default]
    Ask,
    /// Refuse outright what a command names for certain. For people who never
    /// want the agent reading these. What it only MAY name — a `$`, a
    /// `${…:-default}`, a glob aimed at one — asks, under every policy but
    /// `Allow`: a refusal is for a match, not a guess (Steven, 10-06).
    Deny,
    /// Treat them like any other path. For people who have decided the
    /// convenience is worth it; their machine, their call.
    Allow,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `HOME` is read in the form paths are matched in — or not at all.
    #[test]
    fn home_is_normalized_and_plausible() {
        assert_eq!(
            plausible_home("/Users//you/").as_deref(),
            Some("/Users/you")
        );
        assert_eq!(
            plausible_home("/Users/./you").as_deref(),
            Some("/Users/you")
        );
        assert_eq!(plausible_home("/work/you").as_deref(), Some("/work/you"));
        for nonsense in ["/", "//", "", "you", "./you"] {
            assert_eq!(plausible_home(nonsense), None, "{nonsense:?}");
        }
    }
}
