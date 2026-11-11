//! Catching credentials on their way out — to a provider, or to our own disk.
//!
//! See `docs/DesignDocs/secrets.md` §5. This is **P1: egress redaction**, the net
//! under everything else: a one-way scrub applied to text that is about to leave
//! the machine or be persisted.
//!
//! # What it is, and what it is not
//!
//! It is the answer for **secrets we do not hold** — the arbitrary output of an
//! arbitrary command, where we cannot substitute a value we never had. That
//! makes it detection, and detection has **false negatives by construction**. It
//! is a net, not a wall, and nothing else in the design should be relaxed on the
//! assumption that it catches everything.
//!
//! It is deliberately **one-way**. Presidio's lesson: masking, hashing and
//! redaction are irreversible by design, and only an explicit encrypt/decrypt
//! pair round-trips. Reversible substitution — letting the model *use* a secret
//! it cannot see — is a different mechanism ([`Aliases`](super::Aliases)) and must not
//! be confused with this one. The two meet in [`redact`], which runs the table's
//! exact, reversible pass **first**, so a secret we hold comes back as the handle
//! the model already knows rather than as an anonymous placeholder it can do
//! nothing with.
//!
//! # Both sinks, not just the wire
//!
//! A key scrubbed from the request but written to the episode log is still a key
//! on disk, in a file that outlives the session. Every caller that sends text to
//! a provider **or** persists it runs it through here.

use super::format::{SHAPES, Shape};
use super::{Aliases, SecretsConfig};

/// Names that mean "a credential follows" — a **seed, not the list**.
///
/// Matched as a **segment suffix** of the preceding token, never as a substring.
/// `ANTHROPIC_API_KEY` splits to `[anthropic, api, key]` and matches the hint
/// `api-key`; `tokenizer` splits to `[tokenizer]` and matches nothing, where a
/// substring test would have masked the value in `tokenizer: gpt2`. That false
/// positive lands squarely in this user's own work, which is why the stricter
/// rule is the shipped one.
///
/// Users extend this with [`SecretsConfig::names`].
const DEFAULT_SECRET_NAMES: &[&str] = &[
    "password",
    "passwd",
    "pass",
    "passphrase",
    "secret",
    "token",
    "auth-token",
    "access-token",
    "refresh-token",
    "id-token",
    "api-key",
    "apikey",
    "secret-key",
    "private-key",
    "access-key",
    "client-secret",
    "auth",
    "authorization",
    "credential",
    "credentials",
    "session-key",
    "encryption-key",
    "signing-key",
    "dsn",
    // `sshpass` reads its password from `SSHPASS`; `MYSQL_PWD` is MySQL's.
    "sshpass",
    "pwd",
];

/// Hints that name something else when they stand alone, and a credential
/// only after a qualifier: `PWD` is the shell's working directory, in every
/// `env` dump; `MYSQL_PWD`, `DB_PWD` are passwords.
const NEEDS_A_QUALIFIER: &[&str] = &["pwd"];

/// Hints that name a password and nothing else — the ones a quoted short
/// value is taken under ([`context_names_a_value`]). `token`, `secret`,
/// `auth` and `key` also name a lexer's token, a setting about secrets, an
/// auth mode: `token == "--"`, `secret = "redact"` are code and config.
const PASSWORD_NAMES: &[&str] = &["password", "passwd", "passphrase", "pwd", "sshpass"];

/// What the structure said of a value before its text is read: nothing
/// (free text), that its key names a credential, or a password — a JSON
/// value's key ([`redact_json`]), where the text's own separator is gone —
/// or that it is a command line, which the gate reads word by word
/// ([`redact_command`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keyed {
    No,
    Credential,
    Password,
    Command,
}

/// The shortest hint matched GLUED to its owner (`PGPASSWORD`): five
/// letters, so `pass`, `auth` and `dsn` are not — `BYPASS`, `COMPASS` and
/// `OAUTH` name no credential.
const GLUED_MIN: usize = 5;

/// Characters that can separate a credential's NAME from its VALUE.
///
/// The one place per-format configuration genuinely earns its keep. `=` covers
/// `.env`/INI/TOML/CLI, `:` covers YAML/JSON/`.pgpass`/`.npmrc`, `>` covers XML.
/// Whitespace is **not** included by default: it would make the prose "the
/// password is stored in Vault" mask the word `is`. Anyone who needs
/// `.netrc`-style `password hunter2` adds `" "` and accepts that trade
/// knowingly.
const DEFAULT_VALUE_DELIMITERS: &[char] = &['=', ':', '>'];

/// The kinds we ever emit inside `[redacted: …]`.
///
/// A **closed set**, matched exactly, so a placeholder is recognised as *our*
/// output rather than by its shape. Shape-matching is precisely the F1 bug in a
/// second costume: a real key written inside our marker's shape by an attacker
/// (`[redacted: ghp_…]`) would otherwise be skipped whole, hiding the key it
/// contains.
fn is_our_kind(kind: &str) -> bool {
    SHAPES.iter().any(|s| s.kind == kind)
        || matches!(
            kind,
            "configured prefix"
                | "configured value"
                | "high-entropy string"
                | "named secret"
                | "private key"
        )
}

// The entropy rule's three knobs live in `SecretsConfig`
// (`entropy_token_extra`, `entropy_min_run`, `entropy_bits`), not here.
// Configurability is the point of this project — defaults aim at the median
// user and the knobs widen the distribution — and this rule in particular is a
// heuristic whose right setting depends on what a given user's output looks
// like.

/// What scrubbing did to a piece of text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Redaction {
    /// The text, safe to send and to persist.
    pub text: String,
    /// Credential kinds **removed one-way**, in order, for telling the user and
    /// the model. Empty when nothing matched — the overwhelmingly common case.
    pub kinds: Vec<String>,
    /// Labels of held secrets **replaced by their handle** — reversible, and a
    /// different event from a redaction: the model can still act on these, so
    /// the two must not be reported as the same thing. See
    /// [`Aliases::mask`](super::Aliases::mask).
    pub masked: Vec<String>,
}

impl Redaction {
    /// Did anything change?
    pub fn any(&self) -> bool {
        !self.kinds.is_empty() || !self.masked.is_empty()
    }

    /// One line naming what was removed one-way, for a notice or a tool result.
    pub fn summary(&self) -> String {
        distinct(&self.kinds)
    }
}

/// Order-preserving dedup **with counts**, joined for display.
///
/// One copy: `ExecOutcome` carries the kinds out of a `Redaction` as a plain
/// `Vec<String>` and had grown a byte-for-byte duplicate of this.
///
/// **The count is the point.** This used to collapse to distinct kinds alone,
/// so three removals and one read identically — and that is exactly how a
/// `find` listing came back two filenames short while the notice said only
/// "high-entropy string", the model presented the list as complete, and neither
/// party could tell. Saying *how many* is what makes a short answer visibly
/// short (`[[feedback_no_silent_failure]]`).
///
/// A single removal stays unnumbered (`high-entropy string`, not
/// `1 × high-entropy string`) — the count earns its place only when it tells
/// you something.
pub fn distinct(kinds: &[String]) -> String {
    let mut tally: Vec<(&str, usize)> = Vec::new();
    for k in kinds {
        match tally.iter_mut().find(|(seen, _)| *seen == k.as_str()) {
            Some((_, n)) => *n += 1,
            None => tally.push((k, 1)),
        }
    }
    tally
        .into_iter()
        .map(|(kind, n)| {
            if n == 1 {
                kind.to_string()
            } else {
                format!("{n} × {kind}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Scrub `input` of anything that looks like a credential, and replace any secret
/// we actually hold with its handle.
///
/// Returns the text unchanged (and both lists empty) when the policy is off and
/// the vault is empty, so the caller can cheaply detect the common case.
///
/// Order matters: the vault's **exact** substitution runs first, so a held value
/// that happens to match a known issuer prefix comes back as its usable handle
/// rather than a dead `[redacted: github token]`.
pub fn redact(input: &str, cfg: &SecretsConfig, aliases: &Aliases) -> Redaction {
    redact_from(
        input,
        Keyed::No,
        cfg,
        aliases,
        Aliases::mask_in,
        Lines::Join,
    )
}

/// [`redact`] for text read **by its line numbers** — a file Read's, before
/// they are put on it (`Lines::Keep`): the k-th line out is the k-th line
/// in, so a number stays the file's and a cut falls on scrubbed text.
///
/// Numbered and cut first, a held value that ran over lines was no longer
/// one substring (`1\tline one\n2\tline two`), and one the cut fell inside
/// was a prefix of itself: neither matched, and both reached the model
/// (review stop 16, E 1, measured). `[[feedback_scrub_before_cut]]`: every
/// transform of a model-facing text comes after its scrub.
pub fn redact_lines(input: &str, cfg: &SecretsConfig, aliases: &Aliases) -> Redaction {
    #[cfg(test)]
    SCRUBBED_BY_LINES.with(|bytes| bytes.set(bytes.get() + input.len()));
    redact_from(
        input,
        Keyed::No,
        cfg,
        aliases,
        Aliases::mask_in,
        Lines::Keep,
    )
}

#[cfg(test)]
thread_local! {
    /// How many bytes [`redact_lines`] was handed, on this thread — a count,
    /// not a time, so "a Read scrubs what it shows, not the file" holds on
    /// any machine. Timed, it held only where it was measured: the Read's
    /// other whole-file work (reading, decoding, hashing) is not the scrub's
    /// kind of work and weighs differently elsewhere — a 4 MiB file's 10-line
    /// Read took 113 ms against the whole scrub's 1.90 s on this Mac, 606 ms
    /// against 1.90 s on Steven's Linux (11-07), and failed a ratio set here.
    static SCRUBBED_BY_LINES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The bytes scrubbed by lines so far on this thread ([`SCRUBBED_BY_LINES`]).
#[cfg(test)]
pub(crate) fn scrubbed_by_lines() -> usize {
    SCRUBBED_BY_LINES.with(std::cell::Cell::get)
}

/// [`redact`] for a VIEW of a command the user approved: credentials
/// redacted and held values masked as ever, the home's path left as written
/// (`Aliases::mask_shown`) — an edit to the full path read as `~` and as
/// though it never ran. Never for what is stored or sent.
pub fn redact_shown(input: &str, cfg: &SecretsConfig, aliases: &Aliases) -> Redaction {
    redact_from(
        input,
        Keyed::No,
        cfg,
        aliases,
        Aliases::mask_shown_in,
        Lines::Join,
    )
}

/// [`redact`] for a **command line** — a call's, a held or an audited one:
/// the free-text passes, and before them each value a credential's name
/// announces where the shell reads it, at any length
/// (`redact_named_positions`). A call's `"command"` in JSON is read so
/// ([`redact_json`]); a command kept or shown as text asks for it here.
pub fn redact_command(input: &str, cfg: &SecretsConfig, aliases: &Aliases) -> Redaction {
    redact_from(
        input,
        Keyed::Command,
        cfg,
        aliases,
        Aliases::mask_in,
        Lines::Join,
    )
}

/// [`redact_command`] for a VIEW, as [`redact_shown`] is [`redact`]'s.
pub fn redact_command_shown(input: &str, cfg: &SecretsConfig, aliases: &Aliases) -> Redaction {
    redact_from(
        input,
        Keyed::Command,
        cfg,
        aliases,
        Aliases::mask_shown_in,
        Lines::Join,
    )
}

/// **What a replacement does with the lines it ran over.** Four passes can
/// replace text holding a newline: a held value's mask, a private key's
/// block, a configured literal and a URL's userinfo. Each puts what stands
/// for it through [`instead`](Self::instead), so the choice is made once.
/// The token pass never crosses a line: a token holds no whitespace, a
/// quoted named value stops at a newline ([`named_value_end`]), and the one
/// value taken to the end of its text, a JSON string's, is never redacted
/// by lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lines {
    /// Joined: one piece of text stands where the lines stood — for text
    /// read whole, a command's output, which a handle then round-trips.
    Join,
    /// Kept: what stands for the text on the line it starts on, and
    /// [`CONTINUED`] on each later line it has a character on — for text
    /// read by its line numbers ([`redact_lines`]).
    Keep,
}

/// What a line holds in place of a value that began on a line above it.
pub(crate) const CONTINUED: &str = "[… continued]";

impl Lines {
    /// What stands for `replaced`: `with`, and when lines are kept, each
    /// newline `replaced` held, then [`CONTINUED`] if that line has any of
    /// it. A value ending at its newline leaves the next line as it was.
    pub(crate) fn instead<'a>(self, with: &'a str, replaced: &str) -> std::borrow::Cow<'a, str> {
        if self == Lines::Join || !replaced.contains('\n') {
            return std::borrow::Cow::Borrowed(with);
        }
        let mut out = with.to_string();
        for piece in replaced.split('\n').skip(1) {
            out.push('\n');
            if !piece.is_empty() {
                out.push_str(CONTINUED);
            }
        }
        std::borrow::Cow::Owned(out)
    }
}

/// **The redactions `scrubbed` shows that `raw` did not hold** — each
/// `[redacted: <kind>]` of ours, by kind, less those the text itself
/// already held (this repository's own tests and docs are full of them).
/// For a Read's window ([`redact_lines`]): the lines it shows, scrubbed,
/// against the same lines as the file has them — what was removed from
/// what the model was sent, not from the rest of the file.
pub fn added_redactions(raw: &str, scrubbed: &str) -> Vec<String> {
    let mut held = ours(raw);
    let mut added = Vec::new();
    for kind in ours(scrubbed) {
        match held.iter().position(|k| *k == kind) {
            Some(at) => {
                held.swap_remove(at);
            }
            None => added.push(kind.to_string()),
        }
    }
    added
}

/// Each `[redacted: <kind>]` in `text` whose kind is ours, in order.
fn ours(text: &str) -> Vec<&str> {
    const PLACEHOLDER: &str = "[redacted: ";
    (text.match_indices(PLACEHOLDER))
        .filter_map(|(at, _)| {
            let rest = &text[at + PLACEHOLDER.len()..];
            let kind = &rest[..find_near(rest, "]")?];
            is_our_kind(kind).then_some(kind)
        })
        .collect()
}

/// **`value` scrubbed as [`redact`] scrubs the same JSON as text** — every
/// key, and every string as the VALUE of the key it sits under: the name rule
/// reads `"password": "hunter2seekrit"` in a tool's output, so a call's input
/// holding the same pair must not pass because its name and its value are two
/// strings (review stop 4, E 2: three named values went through in clear
/// where the text was redacted). A key is scrubbed as text: a marked value or
/// a key-shaped string can be a key as well as a value.
///
/// Two scrubbed keys can come out alike and keep one entry — a display
/// that lost an entry, never a value that leaked.
pub fn redact_json(
    value: &serde_json::Value,
    cfg: &SecretsConfig,
    aliases: &Aliases,
) -> serde_json::Value {
    json_with(value, cfg, aliases, Aliases::mask_in)
}

/// [`redact_json`] for a VIEW, as [`redact_shown`] is [`redact`]'s.
pub fn redact_json_shown(
    value: &serde_json::Value,
    cfg: &SecretsConfig,
    aliases: &Aliases,
) -> serde_json::Value {
    json_with(value, cfg, aliases, Aliases::mask_shown_in)
}

/// How [`redact_from`] puts held values back as their handles: every alias
/// (`Aliases::mask`), or all but the home's, for a view (`Aliases::mask_shown`)
/// — over lines joined or kept ([`Lines`]), as the passes after it are.
type Mask = fn(&Aliases, &str, Lines) -> (String, Vec<String>);

fn json_with(
    value: &serde_json::Value,
    cfg: &SecretsConfig,
    aliases: &Aliases,
    mask: Mask,
) -> serde_json::Value {
    super::json::map_strings(value, &mut |s, under| {
        let keyed = match under {
            // A call's command line is no free text: the shell's reading
            // says where a credential's name put its value.
            Some("command") => Keyed::Command,
            // A switch's own word is no credential under any name, as in
            // text: this arm came first without the exemption, so
            // `{"HAS_PASSWORD": "true"}` was masked and `HAS_PASSWORD=true`
            // was not (stop 17, V2a LOW 2).
            Some(key) if names_a_password(key, cfg) && !is_a_setting(s) => Keyed::Password,
            // `{"TOKEN": "v"}`, as `TOKEN="v"` is in text (review stop 17,
            // E1 L1).
            Some(key) if is_env_name(key) && is_secret_name(key, cfg) && !is_a_setting(s) => {
                Keyed::Password
            }
            Some(key) if is_secret_name(key, cfg) => Keyed::Credential,
            _ => Keyed::No,
        };
        redact_from(s, keyed, cfg, aliases, mask, Lines::Join).text
    })
}

/// [`redact`], and when `keyed` says so, `input` is the value of a name that
/// announces a credential — the whole of it, as JSON gave its extent. Every
/// replacement treats the lines it runs over as `lines` says.
fn redact_from(
    input: &str,
    keyed: Keyed,
    cfg: &SecretsConfig,
    aliases: &Aliases,
    mask: Mask,
    lines: Lines,
) -> Redaction {
    let (input, masked) = mask(aliases, input, lines);
    if !cfg.redact {
        return Redaction {
            text: input,
            kinds: Vec::new(),
            masked,
        };
    }
    let mut kinds = Vec::new();
    // A command's named values next: after the mask, so a held value leaves
    // as its handle (which writes no value down), and before the passes that
    // read the rest as free text.
    let (input, keyed) = match keyed {
        Keyed::Command => {
            let named = redact_named_positions(&input, cfg, aliases, &mut kinds, lines);
            (named, Keyed::No)
        }
        keyed => (input, keyed),
    };
    // PEM blocks next: a private key is a multi-line construct, so token-wise
    // scanning would leave the armour and shred the middle into fragments that
    // are individually below the entropy floor.
    let text = redact_pem_blocks(&input, &mut kinds, lines);
    let text = redact_url_credentials(&text, &mut kinds, lines);
    let text = redact_literals(&text, &cfg.extra, &mut kinds, lines);
    let mut out = redact_tokens(&text, keyed, cfg, aliases, kinds);
    out.masked = masked;
    out
}

/// **A command's values where a credential's name put them** — `mysql
/// --password hunter2`, `-phunter2`, at any length — by the gate's reading of
/// the line (`permission::named_credential_spans`). Free text has no
/// position to go on, so the name rule takes only a value bound to its name
/// and keeps a length floor for the rest; a command's words have one, and
/// the gate already read them: it refused to make a rule of this value while
/// every sink kept it in clear (review stop 17, E1 M1; Steven, 11-09).
///
/// A span that is already ours is left: a second scrub reads our marker's
/// first word as the value (`--password [redacted: …`), and a scrub must be
/// idempotent ([`our_own_span`]).
fn redact_named_positions(
    input: &str,
    cfg: &SecretsConfig,
    aliases: &Aliases,
    kinds: &mut Vec<String>,
    lines: Lines,
) -> String {
    const NAMED: &str = "[redacted: named secret]";
    let mut out = String::with_capacity(input.len());
    let mut at = 0;
    for span in crate::permission::named_credential_spans(input, cfg) {
        if our_own_span(&input[span.start..], aliases).is_some() {
            continue;
        }
        out.push_str(&input[at..span.start]);
        out.push_str(&lines.instead(NAMED, &input[span.clone()]));
        kinds.push("named secret".to_string());
        at = span.end;
    }
    out.push_str(&input[at..]);
    out
}

/// Replace whole `-----BEGIN … PRIVATE KEY-----` … `-----END …-----` blocks.
///
/// A private key is a multi-line construct, so token-wise scanning would leave
/// the armour and shred the body into fragments individually below the entropy
/// floor. This finds the whole block instead.
///
/// **The armour is delimited by its own dashes, not by a newline.** Requiring
/// `-----\n` to end the header meant a key with its newlines stripped
/// (`cat id_rsa | tr -d '\n'`, or any tool that folds it) was not recognised as
/// a block at all — and with `redact_high_entropy = false` it then passed
/// through completely intact. Structure, not formatting, decides.
fn redact_pem_blocks(input: &str, kinds: &mut Vec<String>, lines: Lines) -> String {
    const BEGIN: &str = PEM_BEGIN;
    const END: &str = "-----END ";
    const DASHES: &str = "-----";

    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start..];
        // `-----BEGIN <TYPE>-----` — the header ends at its own closing dashes,
        // wherever they are.
        let Some(header_end) = after[BEGIN.len()..]
            .find(DASHES)
            .map(|i| BEGIN.len() + i + DASHES.len())
        else {
            break;
        };
        let header = &after[..header_end];
        // Only armour that actually names a key; certificates are public.
        if !header.contains("PRIVATE KEY") {
            out.push_str(&rest[..start + header_end]);
            rest = &rest[start + header_end..];
            continue;
        }
        out.push_str(&rest[..start]);
        kinds.push("private key".to_string());
        // Consume through the END armour's *closing* dashes. Searching for
        // `-----` from the start of `-----END ` always matched at offset zero,
        // which left `END OPENSSH PRIVATE KEY-----` sitting in the output.
        let end = match after.find(END) {
            Some(e) => match after[e + END.len()..].find(DASHES) {
                Some(t) => e + END.len() + t + DASHES.len(),
                // Unterminated END armour: drop the remainder rather than emit a
                // half-key. Failing closed costs a truncated log; failing open
                // costs the key.
                None => after.len(),
            },
            None => after.len(),
        };
        out.push_str(&lines.instead("[redacted: private key]", &after[..end]));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Replace the **userinfo** of any `scheme://user:password@host` URL.
///
/// Measured gap, not a hypothetical: a realistic `.env` had six credentials
/// caught by the passes around this one and **six URL-embedded ones survive**
/// — `postgres://`, `mongodb://`, `redis://`, `amqp://`, an `https://` git
/// remote and an HTTP proxy. Even a `ghp_` GitHub token survived, because the
/// issuer-prefix scanner anchors at a token START and the whole URL is one
/// token.
///
/// This matters more than any single permission-gate spelling: redaction is
/// **independent of how the file was read**, so it covers `xargs`, `$VAR`, a
/// symlink and a glob alike, where the gate has to recognise each one.
///
/// **Syntax, not entropy.** RFC 3986 says userinfo is what sits between `://`
/// and an `@` before the next `/`, `?` or `#`. That is unambiguous, so there is
/// no threshold to tune and no false positive on an ordinary URL — one without
/// an `@` is untouched, which is nearly all of them.
///
/// The **host is kept**: `postgres://[redacted: url credential]@db/app` still
/// tells the reader (and the model) which service was configured, which is the
/// part that is usually the point. Redacting the whole URL would cost real
/// debugging value for no extra safety.
fn redact_url_credentials(input: &str, kinds: &mut Vec<String>, lines: Lines) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(scheme_end) = rest.find("://") {
        let after = &rest[scheme_end + 3..];
        let Some(at) = userinfo_end(after) else {
            out.push_str(&rest[..scheme_end + 3]);
            rest = after;
            continue;
        };
        out.push_str(&rest[..scheme_end + 3]);
        // Userinfo stops at a space but not a newline, so it can run onto
        // the next line (`x://a` then `b@host`).
        out.push_str(&lines.instead("[redacted: url credential]", &after[..at]));
        kinds.push("url credential".to_string());
        rest = &after[at..];
    }
    out.push_str(rest);
    out
}

/// **Where the userinfo after a `scheme://` ends** — at its `@` — or `None`
/// when there is none: one in a URL's path, query or fragment is not
/// (`https://host/a@b`). Userinfo ends at the first `@`, and only if no
/// path/query/fragment delimiter comes first. So the FIRST of them decides,
/// and it is found without reading past it: looking for the next `@`
/// anywhere after every `://` read the rest of the text each time —
/// quadratic over a long output full of URLs and no `@` (review stop 3,
/// B 5: 4 MB took 2.5 s). The URL pass's rule, and [`crosses_lines`]'.
fn userinfo_end(after: &str) -> Option<usize> {
    after
        .find(['@', '/', '?', '#', ' '])
        .filter(|&at| at > 0 && after[at..].starts_with('@'))
}

/// What opens a key block ([`redact_pem_blocks`]).
const PEM_BEGIN: &str = "-----BEGIN ";

/// **Whether a replacement could run over a line of `input`**, under `cfg`
/// and the values `aliases` holds — one of the four that can: a held value
/// with a newline in it, and with redaction on, a configured literal with
/// one, a key block's armour, a URL's userinfo with a newline before its
/// `@`. Where none can, every replacement falls within a line, so a line
/// scrubbed alone is scrubbed as it would be in the whole text: a Read
/// scrubs only the lines it shows (`files::run::shown`). Scrubbing a whole
/// 16 MiB file for every Read took 2.1 s, its window 11 ms (review stop
/// 16, measured).
pub fn crosses_lines(input: &str, cfg: &SecretsConfig, aliases: &Aliases) -> bool {
    if aliases.holds_a_value_over_lines() {
        return true;
    }
    if !cfg.redact {
        return false;
    }
    let userinfo_crosses = || {
        (input.match_indices("://")).any(|(at, scheme)| {
            let after = &input[at + scheme.len()..];
            userinfo_end(after).is_some_and(|end| after[..end].contains('\n'))
        })
    };
    cfg.extra
        .iter()
        .any(|v| v.len() >= LITERAL_MIN && v.contains('\n'))
        || input.contains(PEM_BEGIN)
        || userinfo_crosses()
}

/// The shortest configured literal redacted ([`redact_literals`]).
const LITERAL_MIN: usize = 8;

/// Replace user-configured literal values.
fn redact_literals(input: &str, extra: &[String], kinds: &mut Vec<String>, lines: Lines) -> String {
    let mut text = input.to_string();
    for value in extra {
        // A short "secret" would match everywhere; refuse rather than shred the
        // output. (A one-character secret is a configuration mistake, not a
        // request to blank the transcript.)
        if value.len() < LITERAL_MIN || !text.contains(value.as_str()) {
            continue;
        }
        text = text.replace(
            value.as_str(),
            &lines.instead("[redacted: configured value]", value),
        );
        kinds.push("configured value".to_string());
    }
    text
}

/// Replace individual tokens that a matcher recognises.
///
/// Tokenising is what makes recognition **format-independent**, and this is the
/// point the design got wrong for a while by describing it as an `.env` feature.
/// Token characters are what credential formats actually use
/// (`alnum _ - . + ~`), so *every* delimiter any config format uses — `=`, `:`,
/// `"`, `<`, `>`, whitespace, `,` — is already a boundary. `.env`, YAML, JSON,
/// TOML, `.netrc`, `.pgpass`, `.npmrc` and XML all fall out of one rule, and
/// "keys visible, values masked" is simply what tokenising does. No per-format
/// parser is needed, and writing one would have been the wrong generalisation.
///
/// A substitution handle is skipped whole ([`handle_span`]). The *shape*
/// matchers would have left it alone anyway — `{`, `}` and `:` are boundaries,
/// so it breaks into short, unremarkable tokens — but the name-directed matcher
/// would not have: `{{db-url.4f19}}` reads as the hint `secret`
/// followed by a `:`, and the pass would eat its own output.
///
/// Two matchers run per token, and they are independent:
/// - **shape** — an issuer prefix or the entropy heuristic ([`classify`]);
/// - **name-directed** — the *preceding* token said a credential follows
///   ([`names_a_secret`]). This is the only thing that catches a value with no
///   distinguishing shape (`password: hunter2seekrit`), and it works in every
///   format for the same reason the rest does. The value is taken whole, not
///   token by token — see [`named_value_end`].
///
/// `keyed`: the name came from structure, not from the text — `input` is the
/// value of a JSON key that names a credential ([`redact_json`]). Its first
/// token starts named, and its value runs to the end: JSON gave its extent.
/// A password's key takes it whatever its length, as the quoted text form
/// does ([`context_names_a_value`]).
fn redact_tokens(
    input: &str,
    mut keyed: Keyed,
    cfg: &SecretsConfig,
    aliases: &Aliases,
    mut kinds: Vec<String>,
) -> Redaction {
    let mut out = String::with_capacity(input.len());
    let mut token = String::new();
    // The last token flushed, and the separator run since — together they answer
    // "is this token the VALUE of a credential-shaped name?".
    let mut previous = String::new();
    let mut separator = String::new();
    // Decided when the token STARTS (that is when `separator` holds the complete
    // run since `previous`) and used when it is flushed. Deciding at flush time
    // instead was a real bug: the separator had already been cleared by then.
    let mut named = false;
    let mut carry_scheme = false;

    let mut cursor = 0usize;
    while cursor < input.len() {
        // A region WE produced is inert: copy it through untouched, so the
        // name-directed matcher cannot eat our own output. See `our_own_span`.
        if let Some(end) = our_own_span(&input[cursor..], aliases) {
            if !token.is_empty() {
                flush(&mut token, &mut out, &mut kinds, cfg, named, &mut previous);
                named = false;
            }
            out.push_str(&input[cursor..cursor + end]);
            previous.clear();
            separator.clear();
            cursor += end;
            continue;
        }
        let at = cursor;
        let ch = input[cursor..].chars().next().expect("in bounds");
        cursor += ch.len_utf8();
        // A RUN of punctuation is a separator, not part of a name. Found by the
        // property test: `-----END … KEY-----AKIA…` tokenised as one word
        // starting `KEY`, so the prefix check (which anchors at the token start)
        // never saw `AKIA` and the key survived. Runs mean "separator" in every
        // format — PEM armour, markdown rules, log dividers — while a *single*
        // `-` or `_` is ordinary inside a name, so this splits armour without
        // splitting `sk-ant-…` or `some-file-name`.
        if in_punctuation_run(input, at, ch) {
            if !token.is_empty() {
                carry_scheme = flush(&mut token, &mut out, &mut kinds, cfg, named, &mut previous);
                separator.clear();
                named = false;
            }
            separator.push(ch);
            out.push(ch);
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '+' | '~') {
            if token.is_empty() {
                // The credential after a scheme word, whose delimiter is a
                // space rather than `:` or `=`. Gated on `carry_scheme`, which
                // only the flush of a NAMED scheme sets — so `Authorization:
                // Bearer x` scrubs `x`, and prose containing the word "token"
                // does not scrub the word after it.
                let after_scheme = carry_scheme
                    && !separator.is_empty()
                    && separator.chars().all(|c| c == ' ' || c == '\t');
                let by_key = std::mem::replace(&mut keyed, Keyed::No);
                let whole = by_key != Keyed::No;
                named = whole || after_scheme || names_a_secret(&previous, &separator, cfg);
                // A named VALUE is redacted whole, measured whole. See
                // `named_value_end` for why the token is the wrong unit.
                if named && !is_auth_scheme(leading_token(&input[at..])) {
                    let end = if whole {
                        input.len()
                    } else {
                        at + named_value_end(&input[at..], &separator)
                    };
                    let value = &input[at..end];
                    // A JSON value under a password's key is the quoted
                    // form, its key and quotes given by the structure.
                    if value.chars().count() >= cfg.name_min_value
                        || by_key == Keyed::Password
                        || (context_names_a_value(&previous, &separator, cfg)
                            && !is_a_setting(value))
                    {
                        let first = leading_token(value);
                        let kind = classify(first, cfg).unwrap_or("named secret");
                        out.push_str(&format!("[redacted: {kind}]"));
                        kinds.push(kind.to_string());
                        previous.clear();
                        previous.push_str(value);
                        separator.clear();
                        carry_scheme = false;
                        named = false;
                        cursor = end;
                        continue;
                    }
                    // Too short to be the credential this rule is for. Shape
                    // matchers still see it, token by token, below.
                    named = false;
                }
            }
            token.push(ch);
        } else {
            if !token.is_empty() {
                carry_scheme = flush(&mut token, &mut out, &mut kinds, cfg, named, &mut previous);
                separator.clear();
                named = false;
            }
            separator.push(ch);
            out.push(ch);
        }
    }
    if !token.is_empty() {
        let _ = flush(&mut token, &mut out, &mut kinds, cfg, named, &mut previous);
    }

    Redaction {
        text: out,
        kinds,
        masked: Vec::new(),
    }
}

/// Emit one token, redacted if any matcher recognises it, and remember it as the
/// `previous` token for the name-directed matcher.
/// Emit one token, redacted or not. Returns **true** when it emitted an auth
/// SCHEME that was itself named — `Bearer` in `Authorization: Bearer x` — so
/// the caller carries the name to the next token, which is the credential.
///
/// The carry needs the scheme's own context, not just the word: `token`,
/// `basic` and `digest` are ordinary English, and a rule keyed on the word
/// alone redacted `before` in "rotate the token before Friday". The existing
/// prose tests caught that within a minute of writing it, which is the entire
/// argument for having them.
fn flush(
    token: &mut String,
    out: &mut String,
    kinds: &mut Vec<String>,
    cfg: &SecretsConfig,
    named: bool,
    previous: &mut String,
) -> bool {
    // **An auth SCHEME is part of the name, not the credential.** The
    // name-directed pass fired on `Authorization:` and redacted the very next
    // token, which is `Bearer` — leaving the token itself in the clear under a
    // `[redacted: …]` marker. Output that LOOKS scrubbed and is not is worse
    // than output that plainly is not: a reader stops looking.
    //
    // The name-directed redaction itself no longer happens here: a named value
    // is taken whole in `redact_tokens` (see `named_value_end`), so the only
    // named token that reaches a flush is a scheme, or a value too short to be
    // the credential the rule is for — and both are emitted as written.
    let scheme = named && is_auth_scheme(token);
    match classify(token, cfg) {
        Some(kind) => {
            out.push_str(&format!("[redacted: {kind}]"));
            kinds.push(kind.to_string());
        }
        None => out.push_str(token),
    }
    // Remember the token as WRITTEN, not as emitted: `password=x=y` should not
    // treat a redaction placeholder as a name.
    previous.clear();
    previous.push_str(token);
    token.clear();
    scheme
}

/// If `text` starts with a region **we produced**, its length.
///
/// # An inert-region skip is an attacker-controlled off switch
///
/// Two kinds of region are ours: a substitution handle we minted, and a
/// `[redacted: …]` placeholder we wrote. Both must be recognised by
/// **provenance**, never by shape — that distinction is the whole finding.
///
/// The handle test used to accept anything *shaped* like one (`{{`, inert
/// characters, `}}`), and every credential format is built from exactly those
/// characters, so `{{ghp_abcdefghijklmnopqrstuvwxyz0123456789}}` turned off **all four**
/// matchers at once and delivered the key verbatim — reachable by injected
/// output telling the model to "wrap secrets in `{{ }}`", on a `ReadOnly`
/// command that never faces a human.
///
/// So: a handle is ours if we [`knows`](Aliases::knows) it; a placeholder is ours
/// if its kind is in the closed set we emit ([`is_our_kind`]).
///
/// # Why the skip must exist at all
///
/// Both of our own outputs are re-matched by the *name-directed* pass otherwise:
/// `{{db-url.…}}` reads as the hint `secret` + a `:` delimiter, and
/// `password: [redacted: named secret]` reads as `password` + `: [` — so a second
/// scrub produced `password: [[redacted: named secret]: named secret]`, degrading
/// further on every pass. Not hypothetical: `audit()` scrubs a report whose
/// output was already scrubbed at capture. **Redaction must be idempotent**, and
/// this is what makes it so.
fn our_own_span(text: &str, aliases: &Aliases) -> Option<usize> {
    const PLACEHOLDER: &str = "[redacted: ";
    if text.starts_with("{{") {
        let end = find_near(text, "}}")? + 2;
        return aliases.knows(&text[..end]).then_some(end);
    }
    if let Some(rest) = text.strip_prefix(PLACEHOLDER) {
        let close = find_near(rest, "]")?;
        return is_our_kind(&rest[..close]).then_some(PLACEHOLDER.len() + close + 1);
    }
    None
}

/// The **bounded** search this function needs: our own shapes are short, so a
/// closing delimiter further away than [`OUR_SHAPE_MAX`] is not ours.
///
/// Unbounded `find` made `redact` quadratic on ordinary output. The caller
/// advances **one character** when a check fails, so text that keeps opening
/// `{{` without closing — minified JSON, LaTeX, JS: `{"a":{"b":{"c":…` — paid a
/// near-full-length scan per character. Measured before this: 2.0s at 128k
/// `{`, 8.0s at 256k, clean O(n²) (Reviewer B). That is on `exec`'s
/// **always-on** path, run over every command's captured output — and it runs
/// AFTER the timeout wraps the child, so it is time no configured limit bounds.
///
/// A handle is `{{name.nnnn}}` and a placeholder kind comes from the closed
/// `is_our_kind` set; both are tens of bytes, so the window is generous by an
/// order of magnitude and a miss now costs O(1).
///
/// The window walks **back to a char boundary** rather than asking `get` and
/// falling back to the whole string on `None`: that fallback would have handed
/// the unbounded scan straight back for any input whose 128th byte lands
/// mid-character, which is a thing output can simply be
/// ([[feedback_char_boundary_slicing]]).
fn find_near(text: &str, needle: &str) -> Option<usize> {
    const OUR_SHAPE_MAX: usize = 128;
    let mut end = OUR_SHAPE_MAX.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].find(needle)
}

/// Is the character at byte offset `at` part of a **run** of three or more of
/// itself?
///
/// Looks both ways, so every character of the run is a separator and not just
/// its first — checking forward only split `-----` after the first dash and left
/// the rest glued to the next word.
///
/// Three, not two: `--flag` and `__init__` are ordinary names, `-----` is armour.
///
/// **Counts at most two each way, because that is the whole question.** It
/// counted the entire run in both directions, from *every* character of the
/// run — so one row of `N` dashes cost O(N²), and the main loop calls this once
/// per character. Measured: 3.9 ms at 2k dashes, 608 ms at 32k, textbook
/// quadratic (Reviewer D). A markdown rule, a log divider, PEM armour or an
/// underline row all produce exactly this shape, and this is `redact`'s
/// always-on path over command output, running after the timeout has stopped
/// applying — the third quadratic found in this file in one round, two of them
/// in the same function's neighbours.
///
/// The predicate is `>= 3`, so two on each side is all the evidence there can
/// be. `take_while(...).take(2)` stops the walk there.
fn in_punctuation_run(input: &str, at: usize, ch: char) -> bool {
    if !matches!(ch, '-' | '.' | '_' | '=' | '*' | '~' | '+') {
        return false;
    }
    let before = input[..at]
        .chars()
        .rev()
        .take_while(|c| *c == ch)
        .take(2)
        .count();
    let after = input[at + ch.len_utf8()..]
        .chars()
        .take_while(|c| *c == ch)
        .take(2)
        .count();
    before + 1 + after >= 3
}

/// Does `previous`, separated by `separator`, announce that this token is a
/// credential?
///
/// Two guards keep this from eating prose:
/// 1. The separator must contain a configured **value delimiter** and no newline.
///    `password: x` and `password=x` qualify; "the password is stored in Vault"
///    does not, because `is` follows a bare space.
/// 2. The name matches as a **segment suffix**, never a substring — so
///    `tokenizer: gpt2` is left alone while `access_token: gpt2` is not.
fn names_a_secret(previous: &str, separator: &str, cfg: &SecretsConfig) -> bool {
    if previous.is_empty() || separator.contains('\n') || separator.contains('\r') {
        return false;
    }
    let delimiters: Vec<char> = if cfg.value_delimiters.is_empty() {
        DEFAULT_VALUE_DELIMITERS.to_vec()
    } else {
        cfg.value_delimiters
            .iter()
            .filter_map(|s| s.chars().next())
            .collect()
    };
    if !separator.chars().any(|c| delimiters.contains(&c)) {
        return false;
    }
    is_secret_name(previous, cfg)
}

/// Whether `name` announces a credential — a hint, shipped and not turned off
/// or added by the user, matching as a **segment suffix**. The name half of
/// [`names_a_secret`], asked alone of a JSON key ([`redact_json`]), whose
/// value's separation is the structure.
fn is_secret_name(name: &str, cfg: &SecretsConfig) -> bool {
    names_by(name, cfg, |_| true)
}

/// Whether `name` announces a PASSWORD ([`PASSWORD_NAMES`]), by the same
/// test [`is_secret_name`] makes — glued, qualified, the user's exclusions
/// honoured.
fn names_a_password(name: &str, cfg: &SecretsConfig) -> bool {
    names_by(name, cfg, |hint| {
        // A name you added says what it says: `names = ["pin"]`, `pin = "v"`
        // (review stop 17, E1 L2).
        PASSWORD_NAMES.contains(&hint) || cfg.names.iter().any(|mine| mine == hint)
    })
}

/// The name test, over the hints `keep` accepts.
fn names_by(name: &str, cfg: &SecretsConfig, keep: impl Fn(&str) -> bool) -> bool {
    let segments = name_segments(name);
    let env = is_env_name(name);
    DEFAULT_SECRET_NAMES
        .iter()
        .copied()
        // A shipped hint the user has turned off. `names` documents itself as
        // ADDING to the seed, so it cannot narrow — without this the ring
        // "defaults → config → forkability" does not close, and someone whose
        // codebase is full of `token` has no way out but to disable the rule
        // entirely (`[[feedback_sane_defaults_rings]]`).
        //
        // Compared as SEGMENTS, like every other name here: a user writes the
        // spelling their code uses (`api_key`, `TOKEN`), and an exact string
        // compare against our `api-key` silently did nothing.
        .filter(|hint| !cfg.names_exclude.iter().any(|off| same_name(off, hint)))
        .chain(cfg.names.iter().map(|s| s.as_str()))
        .filter(|hint| keep(hint))
        .any(|hint| {
            if is_segment_suffix(&segments, hint) {
                // Alone, `PWD` is the working directory, not a password.
                return !(NEEDS_A_QUALIFIER.contains(&hint) && segments.len() == 1);
            }
            env && is_glued(&segments, hint)
        })
}

/// Is `name` spelled as the environment spells a variable — capitals,
/// digits and `_`, at least one capital (`PGPASSWORD`, `DB_PASSWORD`)? The
/// spelling a shell, a `.env` file and a CI config bind credentials under,
/// and that code does not use for a local.
fn is_env_name(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Does a one-word `hint` end the last segment, GLUED to its owner
/// (`PGPASSWORD`, `CLIENTSECRET`, `GHTOKEN`)? Five letters at least
/// ([`GLUED_MIN`]); asked of an environment-style name alone, where the
/// segment rule cannot see the hint (one segment, `pgpassword`).
fn is_glued(segments: &[String], hint: &str) -> bool {
    let hint = hint.to_ascii_lowercase();
    hint.len() >= GLUED_MIN
        && hint.chars().all(|c| c.is_ascii_alphanumeric())
        && segments
            .last()
            .is_some_and(|last| last.len() > hint.len() && last.ends_with(&hint))
}

/// **Where the value floor does not apply: the context binds a value, not
/// code.** The floor (`name_min_value`) exists because a name alone is weak
/// evidence where code is the text — `let token = &argv[i]` (see
/// [`named_value_end`]). Two shapes say more, and in them a short value is
/// still the credential (Steven, 11-08: what matters is when something is
/// *obviously* a password):
/// - **an assignment with no spaces**, to an environment-style name or a
///   flag, its value quoted or not — `PGPASSWORD=hunter2`, `TOKEN="v"`,
///   `export DB_PASSWORD=hunter2`, `--password=hunter2`: how a shell, a
///   `.env` file and a command line bind values, where formatted code puts
///   spaces around `=` and Python's lowercase keyword arguments
///   (`connect(password=pw)`) are not this spelling;
/// - **a quoted value bound to a password's name** ([`PASSWORD_NAMES`], or a
///   name you added in `secrets.names`) — `password = "hunter2"`,
///   `"passwd": "hunter2"`: one `=`, `:` or `>` between them, so
///   `password == "x"` and `Password => "x"` (a comparison, a match arm) are
///   not. Quoted alone it was too wide, measured over this repository:
///   `token == "--"`, `secret = "redact"`.
///
/// Not lifted, a named limit: YAML's `KEY: value` and prose's `password:
/// hunter2` — `: ` is also how Rust declares a field or a constant's type.
fn context_names_a_value(name: &str, separator: &str, cfg: &SecretsConfig) -> bool {
    // Quoted or not: `TOKEN="v"` binds as `TOKEN=v` does (review stop 17,
    // E1 L1). A lowercase name's quoted form is the password rule below.
    let bound = separator.trim_end_matches(['"', '\'']) == "="
        && (is_env_name(name) || name.starts_with('-'));
    let binds_once = separator
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '"' | '\''))
        .count()
        == 1;
    let quoted = separator.ends_with(['"', '\'']) && binds_once && names_a_password(name, cfg);
    bound || quoted
}

/// A switch's own words: `--secret=false`, `--auth=none`. The context binds
/// a value, and these say there is none (review stop 17, A L4). A bare
/// number stays taken: a PIN is one.
pub(crate) fn is_a_setting(value: &str) -> bool {
    [
        "true", "false", "none", "null", "yes", "no", "on", "off", "enabled", "disabled",
    ]
    .iter()
    .any(|word| value.eq_ignore_ascii_case(word))
}

/// **Whether `name` — a flag's (`--password`, `--api-key`) or a variable's —
/// announces a credential**: the name rule's own test (`is_secret_name`),
/// the user's `names` and `names_exclude` honoured. The permission gate asks
/// it of a command's flags, so a rule never carries the value after
/// `--password`, whatever its length: there the position is the evidence,
/// which in free text the value floor has to be (`named_value_end`).
///
/// `--no-password` says there is none: the word after it is an operand
/// (`pg_dump --no-password mydb`, review stop 17, A M1).
pub fn names_a_credential(name: &str, cfg: &SecretsConfig) -> bool {
    !name.trim_start_matches('-').starts_with("no-") && is_secret_name(name, cfg)
}

/// Do two spellings name the same thing? `api_key`, `API-KEY` and `apiKey` do.
/// The one comparison `names_exclude` is matched by — in the matcher and in
/// the config check that reports an entry excluding nothing, so the two cannot
/// disagree about what an entry means.
fn same_name(a: &str, b: &str) -> bool {
    name_segments(a) == name_segments(b)
}

/// `names_exclude` entries that match no shipped name, and so exclude nothing.
///
/// An inert entry reads as a decision the user made and took effect: they add
/// `tokne`, the noise continues, and nothing says why. `Config::clamp` reports
/// these (`[[project_shannon_argument_is_a_program]]`: an inert rule is a
/// load-time error).
pub(crate) fn inert_excludes(cfg: &SecretsConfig) -> Vec<&str> {
    cfg.names_exclude
        .iter()
        .map(String::as_str)
        .filter(|off| !DEFAULT_SECRET_NAMES.iter().any(|hint| same_name(off, hint)))
        .collect()
}

/// The most Shannon bits per character ANY run can reach under `cfg` —
/// `log2` of the run alphabet, `[A-Za-z0-9]` plus `entropy_token_extra`.
///
/// Entropy is at most `log2(distinct characters)`, so an `entropy_bits` above
/// this can never be met and the high-entropy pass is off while the config
/// says it is on — the silent off switch `nan` was, reached with an ordinary
/// number (Reviewers A, B, C and F, converging). Counted over the WIDER
/// alphabet on purpose (every extra, even one a token can never contain), so
/// "can never be reached" is strictly true when `Config::clamp` says it.
pub(crate) fn entropy_ceiling(cfg: &SecretsConfig) -> f64 {
    let extra: std::collections::BTreeSet<char> = cfg
        .entropy_token_extra
        .chars()
        .filter(|c| !c.is_ascii_alphanumeric())
        .collect();
    ((62 + extra.len()) as f64).log2()
}

/// The token at the start of `text`: its longest prefix of token characters —
/// the same class `redact_tokens` accumulates.
fn leading_token(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '~')))
        .unwrap_or(text.len());
    &text[..end]
}

/// Where the **value** of a named credential ends, as a byte offset into
/// `text`, which starts at the value's first character.
///
/// # A name is not enough; the value must look like one — measured WHOLE
///
/// The delimiters the name rule keys on (`=`, `:`, `>`) are ordinary programming
/// syntax and `token`/`secret`/`pass`/`auth` are ordinary identifiers, so
/// name-alone fired on 1 line in 677 of this repo's Rust and 1 in 333 of its
/// prose: `let token = &argv[i]` became `let token = &[redacted: named
/// secret][i]`, handing the model source that does not compile and saying
/// nothing to anyone. A 12-character floor (`name_min_value`) removes ~95% of
/// that, measured, because what it spares is `argv`, `Secret`, `new`, `key`.
///
/// **The floor must measure the value, not the token, and it did not at
/// first.** A token ends at any punctuation, so `password: hunter2!Xk7pQ2mZ9vLab`
/// reached the floor as `hunter2` — seven characters, spared — and a
/// twenty-character password went out whole with no marker at all. Two
/// reviewers reproduced that independently. The token-sized unit was also the
/// older leak: before the floor, only the first fragment was ever redacted,
/// so `SECRET_KEY='django-insecure-k#2m$9x!vq@7z&w^p'` kept everything after
/// the `#`. Measuring and redacting the whole extent fixes both.
///
/// # Where a value ends
///
/// - **Quoted** — the separator's last character is `"` or `'`: the value runs
///   to the matching quote. With no closing quote on the line it runs to the
///   end of the line: failing closed, since a torn quote is exactly where a
///   secret gets cut in half. A backtick is *not* a quote here — it is
///   markdown and shell syntax, not a string delimiter in any config format,
///   and treating it as one redacted `` credential: `Cause::SecretHandle` ``
///   out of this repo's own docs.
/// - **Unquoted** — the value ends at whitespace, at a quote or backtick, or at
///   the syntax of code: `( ) [ ] { } < > ; , :`. That is what keeps source
///   spared: `let token = tokens.next().unwrap();` measures `tokens.next`, and
///   `Secret => Command::Secrets` measures `Command`. The price is an unquoted
///   password *containing* one of those characters, whose tail survives — a
///   named limit, pinned by a test; quote it and it is caught whole. `/` is
///   deliberately NOT a stop: base64 uses it, and stopping there leaked about
///   one AWS secret key in six.
///
/// Measured over this repository (104k lines of `.rs` and `.md`, `secrets/`
/// excluded for its planted fixtures): seven genuine false positives, six of
/// which the token-sized rule already had.
fn named_value_end(text: &str, separator: &str) -> usize {
    let quote = separator.chars().last().filter(|c| matches!(c, '"' | '\''));
    let stop = |c: char| match quote {
        Some(q) => c == q || c == '\n' || c == '\r',
        None => {
            c.is_whitespace()
                || matches!(
                    c,
                    '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ';' | ',' | ':'
                )
                || matches!(c, '"' | '\'' | '`')
        }
    };
    text.find(stop).unwrap_or(text.len())
}

/// Is this token an HTTP authentication **scheme** rather than a credential?
///
/// The closed set from the IANA registry that we actually see, lowercased for
/// comparison. Deliberately a small hand-list: adding to it is a decision about
/// what counts as a name, and it should read like one.
fn is_auth_scheme(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "bearer" | "basic" | "digest" | "token" | "apikey" | "negotiate" | "bot"
    )
}

/// Split an identifier into lowercase segments: `ANTHROPIC_API_KEY` and
/// `anthropic.apiKey` both become `[anthropic, api, key]`.
fn name_segments(name: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if matches!(ch, '_' | '-' | '.' | '+' | '~') {
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            prev_lower = false;
        } else {
            // camelCase boundary: a lowercase run followed by an uppercase.
            if prev_lower && ch.is_ascii_uppercase() && !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
            current.push(ch.to_ascii_lowercase());
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Does `hint` (itself segmented) match the END of `segments`?
///
/// Suffix rather than "contains" because the last segments are what name the
/// thing: `stripe_api_key` is a key, `api_key_rotation_policy` is a policy.
fn is_segment_suffix(segments: &[String], hint: &str) -> bool {
    let hint: Vec<String> = name_segments(hint);
    if hint.is_empty() || hint.len() > segments.len() {
        return false;
    }
    segments[segments.len() - hint.len()..] == hint[..]
}

/// What kind of credential `value` looks like, if any — the classifier, exposed.
///
/// Used when naming a user-marked secret: `github-token-1` reads better in
/// `:alias` and in the model's tool description than `secret-1`, and we already
/// know how to tell.
pub fn classify_kind(value: &str, cfg: &SecretsConfig) -> Option<&'static str> {
    classify(value, cfg)
}

/// What kind of credential this token looks like, if any.
fn classify(token: &str, cfg: &SecretsConfig) -> Option<&'static str> {
    for shape in SHAPES {
        if carries_shape(token, shape) {
            return Some(shape.kind);
        }
    }
    for prefix in &cfg.prefixes {
        // A user's own format: we cannot know its length, so the old general
        // floor is the only honest choice for it.
        let shape = Shape {
            prefix: "",
            min_body: 12,
            kind: "configured prefix",
            check: None,
        };
        if !prefix.is_empty() && carries(token, prefix, &shape) {
            return Some("configured prefix");
        }
    }
    if cfg.redact_high_entropy && looks_high_entropy(token, cfg) {
        return Some("high-entropy string");
    }
    None
}

/// Does `token` carry `prefix` at a position where a credential could start,
/// with enough trailing material to be one?
///
/// # Why not just `starts_with`
///
/// Anchoring at the token start missed `prefix-AKIAIOSFODNN7EXAMPLE` — the token
/// begins `prefix`, so the check never saw the key. This is a **known
/// limitation of the whole field**, not a gap peculiar to us: gitleaks' own rule
/// anatomy is `identifier … assignment … secret-body`, so a credential welded
/// into a longer identifier with no delimiter is missed there too.
///
/// # Why the naive widening is wrong
///
/// Letting a prefix match anywhere makes `some-sk-directory-name` and
/// `x-ASIA-PACIFIC-REGION` credentials, and a scrubber that mangles ordinary
/// output gets switched off — after which it protects nothing.
///
/// # What the field actually does, and what we take from it
///
/// Gitleaks pairs every prefix with a **character class and a length**
/// (`[a-z0-9_-]{32}`), so the body has to *look* like key material rather than
/// merely be long. TruffleHog instead verifies candidates against the issuer's
/// live API — not open to us, since that is itself egress and would *spend* the
/// credential to test it.
///
/// So: the prefix must sit at a **sub-token boundary** (start, or after a
/// separator — regex `\b` semantics), and when it is *not* at the token start we
/// require **more evidence**: the trailing run must contain a digit, which is
/// what tells `PLANTEDVALUE0123` from `directory-name` and `PACIFIC-REGION`.
/// Asking for extra evidence exactly where the false-positive risk is means the
/// token-start case keeps its old behaviour unchanged.
fn carries_shape(token: &str, shape: &Shape) -> bool {
    carries(token, shape.prefix, shape)
}

fn carries(token: &str, prefix: &str, shape: &Shape) -> bool {
    // Once per token, not per occurrence: a long run repeating a prefix took
    // 43 s at 488 KB (review stop 17, B F5).
    let last_digit = token.bytes().rposition(|b| b.is_ascii_digit());
    let mut from = 0usize;
    while let Some(rel) = token[from..].find(prefix) {
        let at = from + rel;
        let body = &token[at + prefix.len()..];
        let at_boundary = at == 0
            || token[..at]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_ascii_alphanumeric());
        // A published checksum is evidence the heuristics were only ever standing
        // in for, so it licenses matching at ANY offset — boundary or not. That
        // is what closes the un-separated case.
        //
        // **Additive, never a replacement.** The first draft made it exclusive
        // and the existing tests caught the regression immediately: a *truncated*
        // token (our own output cap can do that) or one with a typo fails its
        // checksum, and would then have stopped being redacted — a partial
        // credential is still sensitive, since it narrows a search space. A
        // checksum may only turn a miss into a catch.
        if shape.check.is_some_and(|check| check(body)) {
            return true;
        }
        let enough = body.len() >= shape.min_body;
        // Extra evidence only where the risk is: a prefix mid-token is far more
        // likely to be a coincidence inside a compound word.
        let looks_like_key = at == 0 || last_digit.is_some_and(|d| d >= at + prefix.len());
        if at_boundary && enough && looks_like_key {
            return true;
        }
        from = at + 1;
        if from >= token.len() {
            break;
        }
    }
    false
}

/// **An unbroken run that is long, mixed-class, and high-entropy.**
///
/// A credential is *one token*; a name is *structured*. So the unit tested is
/// not the whole token but each maximal **run** over `[A-Za-z0-9]` plus
/// [`entropy_token_extra`](crate::SecretsConfig::entropy_token_extra) — a dot,
/// a slash or a space ends a run. This is the industry shape: TruffleHog
/// scans "every blob of text greater than 20 characters *comprised of* [the]
/// character sets", and `detect-secrets` matches its charset first and measures
/// entropy second. Entropy over a whole line is nobody's design.
///
/// **What replaced what, and why.** The previous rule measured entropy over the
/// entire token against 3.5 bits, which sits *below ordinary English prose*. On
/// a realistic corpus it redacted nine of eleven benign strings, among them
/// `The build took 45 seconds and passed OK` and both of this repo's own
/// `SECURITY-2026-…-….md` filenames — quietly dropping them from a `find`
/// listing, so the model answered "list all md files" with a list that was
/// short by two and nobody could tell. The two strings it spared had *higher*
/// entropy than several it flagged; they were saved by the character-class
/// gate. The entropy term was decorative.
///
/// **The bound that makes this hard.** Shannon entropy over one string's
/// character frequencies measures how evenly its characters are spread, not how
/// unpredictable they are, and it cannot exceed `log2(n)` — a 32-character run
/// tops out at 5.0 bits however random. Real tokens and prose therefore sit
/// closer together than intuition suggests (measured: benign 3.92–4.51, secrets
/// 4.54–5.27). The *threshold* is the dial that trades precision for recall;
/// the length gate adds no precision (over 905k real runs, lowering
/// `entropy_min_run` to 1 added no false positives) and exists only because
/// below `ceil(2^bits)` characters a match is arithmetically impossible.
///
/// The mixed-case-and-digit requirement still earns its place: it is what
/// spares a git SHA and a UUID, both lowercase hex, both long enough to clear
/// the length gate on their own.
fn looks_high_entropy(token: &str, cfg: &SecretsConfig) -> bool {
    runs_of(token, &cfg.entropy_token_extra)
        .any(|run| run_is_secret_shaped(run, cfg.entropy_min_run, cfg.entropy_bits))
}

/// Each maximal run of alphanumerics plus `extra`, in order.
fn runs_of<'a>(token: &'a str, extra: &'a str) -> impl Iterator<Item = &'a str> {
    token
        .split(|c: char| !(c.is_ascii_alphanumeric() || extra.contains(c)))
        .filter(|run| !run.is_empty())
}

/// One run, against the three gates. Split out so a test can drive the rule on
/// a run directly, and so the order reads as the argument: cheapest and most
/// discriminating first.
fn run_is_secret_shaped(run: &str, min_run: usize, min_bits: f64) -> bool {
    // `chars().count()`, not `len()`: the old rule gated on BYTES while
    // measuring entropy over chars, so multibyte input was held to a shorter
    // effective length than intended (`[[feedback_char_boundary_slicing]]` —
    // the same byte-vs-char confusion, in its counting form).
    if run.chars().count() < min_run {
        return false;
    }
    let has_lower = run.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = run.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = run.chars().any(|c| c.is_ascii_digit());
    if !(has_lower && has_upper && has_digit) {
        return false;
    }
    shannon_bits(run) >= min_bits
}

/// A description of a value, computed **locally**, that can be sent where the
/// value itself cannot.
///
/// The escape hatch for "the model needs to reason about this secret"
/// (`secrets.md` §10a). Almost always what is actually wanted is an answer
/// *about* the value — is it strong, is it the right kind, is it truncated —
/// and every input to that answer can be computed here and sent as a fact.
/// Sending a *property* costs nothing and leaks nothing; sending the value is
/// irreversible.
///
/// Deliberately coarse. Exact length plus exact entropy plus the character
/// classes present is already a small amount of information about the value, and
/// there is no reason to hand over more precision than a human question needs.
pub fn describe(value: &str, cfg: &SecretsConfig) -> String {
    let kind = classify(value, cfg).unwrap_or("no known issuer format");
    // Bucketed, not printed. A test caught the first draft reporting entropy to
    // one decimal, which distinguishes two values a single character apart —
    // more precision than any real question needs, handed to a party we are
    // trying not to inform. Coarse answers "is it strong"; exact answers "which
    // value is it".
    let bits = match shannon_bits(value) {
        b if b < 2.0 => "low randomness",
        b if b < 3.5 => "moderate randomness",
        _ => "high randomness (key-like)",
    };
    let mut classes = Vec::new();
    if value.chars().any(|c| c.is_ascii_lowercase()) {
        classes.push("lowercase");
    }
    if value.chars().any(|c| c.is_ascii_uppercase()) {
        classes.push("uppercase");
    }
    if value.chars().any(|c| c.is_ascii_digit()) {
        classes.push("digits");
    }
    if value.chars().any(|c| !c.is_ascii_alphanumeric()) {
        classes.push("symbols");
    }
    // Length in a bucket, not exactly: "about 40" answers every real question and
    // narrows a search space less than "41" does.
    let len = match value.chars().count() {
        0..=15 => "under 16 characters",
        16..=31 => "16-31 characters",
        32..=63 => "32-63 characters",
        _ => "64 or more characters",
    };
    format!(
        "{len}, contains {}, {bits}, {kind}",
        if classes.is_empty() {
            "no recognised character classes".to_string()
        } else {
            classes.join(" + ")
        }
    )
}

/// Shannon entropy in bits per character.
fn shannon_bits(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let len = s.chars().count() as f64;
    -counts
        .values()
        .map(|&n| {
            let p = n as f64 / len;
            p * p.log2()
        })
        .sum::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> SecretsConfig {
        SecretsConfig::default()
    }

    /// Scrub with no vault — the ordinary case for these tests.
    fn scrub(input: &str, cfg: &SecretsConfig) -> Redaction {
        redact(input, cfg, &Aliases::default())
    }

    /// **A JSON value loses what the same JSON loses as text** — each value a
    /// name announces, nested, in an array, after an auth scheme, as a
    /// number — and keeps what the name rule spares there: a name that is not
    /// one, a value under the floor (review stop 4, E 2: the TUI's walk kept
    /// all three of the first values).
    #[test]
    fn a_json_value_is_scrubbed_as_its_text_is() {
        use serde_json::json;
        let cfg = on();
        let input = json!({
            "password": crate::fakes::PASSWORD_WITH_BANGS,
            "passphrase": "correct horse battery staple",
            "api_token": crate::fakes::API_TOKEN,
            "env": {"DB_PASSWORD": crate::fakes::PASSPHRASE},
            "credentials": ["opensesame-to-the-vault"],
            "headers": {"Authorization": "Bearer abcdefghijklmnop1234"},
            "secret": 987654321098765_u64,
            "tokenizer": "gpt2 small",
            "token": "argv",
            "count": 123456789012345_u64,
        });
        let out = redact_json(&input, &cfg, &Aliases::default());
        let written = out.to_string();
        for value in [
            crate::fakes::PASSWORD_WITH_BANGS,
            "battery staple",
            crate::fakes::API_TOKEN,
            crate::fakes::PASSPHRASE,
            "opensesame-to-the-vault",
            "abcdefghijklmnop1234",
            "987654321098765",
        ] {
            assert!(!written.contains(value), "{value} in {written}");
            assert!(
                !scrub(&input.to_string(), &cfg).text.contains(value),
                "the text rule loses {value} too"
            );
        }
        assert_eq!(out["password"], "[redacted: named secret]");
        let bearer = out["headers"]["Authorization"].as_str().unwrap();
        assert!(bearer.starts_with("Bearer [redacted: "), "{bearer}");
        assert_eq!(out["tokenizer"], "gpt2 small");
        assert_eq!(out["token"], "argv", "under the floor");
        assert_eq!(
            out["count"],
            json!(123456789012345_u64),
            "a number nobody named"
        );
        assert_eq!(
            redact_json(&out, &cfg, &Aliases::default()),
            out,
            "idempotent"
        );
        let off = SecretsConfig {
            redact: false,
            ..on()
        };
        assert_eq!(redact_json(&input, &off, &Aliases::default()), input);
    }

    /// **A key is scrubbed as text** — a marked value and a key-shaped string
    /// can be keys as well as values; and a marked number leaves as its
    /// handle, through the secrets mask's walk too (the one walk both take).
    #[test]
    fn a_json_key_is_scrubbed_and_a_marked_number_masked() {
        use serde_json::json;
        let mut aliases = Aliases::default();
        let pw = aliases
            .insert("pw", "hunter2seekrit", "test", true)
            .unwrap();
        let acct = aliases
            .insert("acct", "123456789012", "test", true)
            .unwrap();
        let input = json!({
            "hunter2seekrit": 1,
            "sk-ant-api03-abcdefghijklmnopqrstuvw": 2,
            "account": 123456789012_u64,
        });
        let out = redact_json(&input, &on(), &aliases);
        let written = out.to_string();
        assert!(!written.contains("hunter2seekrit"), "{written}");
        assert!(written.contains(&pw), "{written}");
        assert!(!written.contains("sk-ant-api03"), "{written}");
        assert_eq!(out["account"], json!(acct));

        let call = crate::Message {
            role: crate::Role::Assistant,
            content: String::new(),
            blocks: vec![crate::Block::ToolUse {
                id: "t".into(),
                name: "x".into(),
                input,
            }],
        };
        let (masked, _) = aliases.mask_message_secrets(&call);
        let crate::Block::ToolUse { input, .. } = &masked.blocks[0] else {
            panic!("a call")
        };
        assert_eq!(input[&pw], json!(1));
        assert_eq!(input["account"], json!(acct));
    }

    /// **A command's credential goes where its name put it, at any length**
    /// (review stop 17, E1 M1; Steven, 11-09: the gate's reading, "(b)").
    /// The gate refused to make a rule of `--password hunter2` while the
    /// kept call, the log and every later request held it: free text has no
    /// position, so the name rule takes only a bound value (`=`) and keeps a
    /// floor. A command's words have one — quoted, a variable's, inside a
    /// substitution, in a line the shell cannot read — and the rest of the
    /// line stays as written.
    #[test]
    fn a_command_loses_a_named_value_by_where_it_sits() {
        let cfg = on();
        let none = Aliases::default();
        let line = "mysql --password correcthorse9staple -h db.example.org";
        assert!(
            scrub(line, &cfg).text.contains("correcthorse9staple"),
            "premise: as free text, no position"
        );
        assert_eq!(
            redact_command(line, &cfg, &none).text,
            "mysql --password [redacted: named secret] -h db.example.org"
        );
        for (line, kept) in [
            (
                "deploy --password hunter2 now",
                "deploy --password [redacted: named secret] now",
            ),
            (
                "deploy --password 'hunter 2' now",
                "deploy --password [redacted: named secret] now",
            ),
            (
                "deploy --token=hunter2 now",
                "deploy --token=[redacted: named secret] now",
            ),
            (
                "deploy --token=\"hunter 2\" now",
                "deploy --token=[redacted: named secret] now",
            ),
            (
                "db_password=hunter2 psql",
                "db_password=[redacted: named secret] psql",
            ),
            (
                "echo $(deploy --password hunter2 -q)",
                "echo $(deploy --password [redacted: named secret] -q)",
            ),
            // A value inside a value: the outer word's span holds the inner
            // stage's, and the two are one span, or the cut panicked
            // (`&input[at..span.start]`) — the merge was held by nothing
            // (stop 17, V2a M3).
            (
                "deploy --password $(fetch --token v) now",
                "deploy --password [redacted: named secret] now",
            ),
            (
                "DB_PASSWORD=$(fetch --token v) psql",
                "DB_PASSWORD=[redacted: named secret] psql",
            ),
            // The shell cannot read it (a stray quote); its words still can.
            (
                "deploy --password hunter2 '",
                "deploy --password [redacted: named secret] '",
            ),
        ] {
            let out = redact_command(line, &cfg, &none);
            assert_eq!(out.text, kept, "{line}");
            assert_eq!(out.kinds, ["named secret"], "{line}");
            assert_eq!(
                redact_command(&out.text, &cfg, &none).text,
                kept,
                "idempotent: {line}"
            );
        }
        // What writes no value down is left as written.
        for line in [
            "deploy --password $DEPLOY_PW now",
            "deploy --password * now",
            "deploy --auth none now",
            "deploy --password= now",
        ] {
            assert_eq!(redact_command(line, &cfg, &none).text, line);
        }
        let off = SecretsConfig {
            redact: false,
            ..on()
        };
        assert_eq!(
            redact_command(line, &off, &none).text,
            line,
            "`redact = false` leaves a command's named value, as the text rule's"
        );
    }

    /// **A value you hold leaves as its handle, not a dead marker**: the mask
    /// runs before the position, and a handle writes no value down — the
    /// model can still spend it (`redact`'s order: "a held value … comes back
    /// as its usable handle").
    #[test]
    fn a_held_value_in_a_credentials_place_keeps_its_handle() {
        let mut aliases = Aliases::default();
        let handle = aliases.insert("db", "plainword7", "test", true).unwrap();
        let out = redact_command("deploy --password plainword7 now", &on(), &aliases);
        assert_eq!(out.text, format!("deploy --password {handle} now"));
        assert!(out.kinds.is_empty(), "{:?}", out.kinds);
    }

    /// **A call's `"command"` is read as a command, every other string as
    /// text** — the JSON walk is where a kept call, a provider's record and
    /// an import's replay are scrubbed, so the key decides there; a
    /// description is prose, where `--password hunter2` would be a guess.
    #[test]
    fn a_calls_command_is_read_by_position() {
        use serde_json::json;
        let input = json!({
            "command": "deploy --password hunter2 now",
            "description": "deploy --password hunter2 now",
        });
        let out = redact_json(&input, &on(), &Aliases::default());
        assert_eq!(
            out["command"],
            "deploy --password [redacted: named secret] now"
        );
        assert_eq!(out["description"], "deploy --password hunter2 now");
        assert_eq!(
            redact_json(&out, &on(), &Aliases::default()),
            out,
            "idempotent"
        );
        let shown =
            redact_command_shown("deploy --password hunter2 now", &on(), &Aliases::default());
        assert_eq!(shown.text, "deploy --password [redacted: named secret] now");
    }

    /// **URL credentials are found in linear time** — after every `://`, the
    /// scan looked for the next `@` in the whole rest of the text: quadratic
    /// over a long output with many URLs and no credential (review stop 3,
    /// B 5: 4 MB took 2.5 s). The URL pass alone, so the others' cost does
    /// not hide it; the bound's margin is measured (see the commit that
    /// added it), not assumed. And the rule it keeps: userinfo only when an
    /// `@` comes before any `/ ? #` or space.
    #[test]
    fn url_credentials_are_found_in_linear_time() {
        let text = "see a://b ".repeat(100_000);
        let start = std::time::Instant::now();
        let mut kinds = Vec::new();
        let out = redact_url_credentials(&text, &mut kinds, Lines::Join);
        let took = start.elapsed();
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
        assert_eq!((out.len(), kinds.len()), (text.len(), 0));

        for (url, credential) in [
            (crate::fakes::URL_MINIMAL, true),
            ("https://host/a@b", false),
            ("https://host?q=a@b", false),
            ("https://host#a@b", false),
            ("ftp://a b@c", false),
            ("x://@host", false),
        ] {
            let mut kinds = Vec::new();
            let out = redact_url_credentials(url, &mut kinds, Lines::Join);
            assert_eq!(kinds.len(), usize::from(credential), "{url} -> {out}");
        }
    }

    /// `NAME=value` masks the VALUE and keeps the NAME. This is the property
    /// `.env` handling rests on (secrets.md §7): the agent needs to know which
    /// variables exist, not what they equal. An earlier version treated `=` as
    /// part of the token and ate the whole assignment, name included.
    #[test]
    fn an_assignment_keeps_its_name_and_loses_its_value() {
        let r = scrub("ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnop", &on());
        assert!(r.text.starts_with("ANTHROPIC_API_KEY="), "{}", r.text);
        assert!(
            !r.text.contains("sk-ant-api03-abcdefghijklmnop"),
            "{}",
            r.text
        );

        // Same for a high-entropy value with no recognisable prefix.
        let r = scrub(
            &format!("DB_PASSWORD={}", crate::fakes::HIGH_ENTROPY_44),
            &on(),
        );
        assert!(r.text.starts_with("DB_PASSWORD="), "{}", r.text);
        assert!(!r.text.contains("Xk7pQ2mZ"), "{}", r.text);
    }

    #[test]
    fn known_issuer_prefixes_are_caught() {
        for (text, kind) in [
            ("sk-ant-api03-abcdefghijklmnop", "anthropic key"),
            ("ghp_abcdefghijklmnopqrstuvwxyz0123456789", "github token"),
            ("AKIAIOSFODNN7EXAMPLE", "aws access key"),
            ("xoxb-123456789012-abcdefghijkl", "slack token"),
        ] {
            let r = scrub(&format!("token is {text} ok"), &on());
            assert!(!r.text.contains(text), "not scrubbed: {}", r.text);
            assert!(r.text.contains("[redacted:"), "{}", r.text);
            assert!(r.kinds.iter().any(|k| k == kind), "{:?}", r.kinds);
            // Surrounding text survives — a redaction that eats the log is its
            // own kind of damage.
            assert!(r.text.starts_with("token is ") && r.text.ends_with(" ok"));
        }
    }

    /// Ordinary command output must come through untouched. A redactor that
    /// mangles a build log gets turned off, and then it protects nothing.
    #[test]
    fn ordinary_output_is_left_alone() {
        for text in [
            "Compiling shannon-core v0.1.0 (/home/sam/src/shannon)",
            "commit 9dd0ad4c1f2b3a4d5e6f708192a3b4c5d6e7f809",
            "test result: ok. 314 passed; 0 failed",
            "https://api.github.com/repos/owner/name/pulls/42",
            "-rw-r--r--  1 sam  staff  1024 Jul 25 10:00 README.md",
            "error[E0433]: cannot find type `ToolPrompt` in this scope",
        ] {
            let r = scrub(text, &on());
            assert_eq!(r.text, text, "false positive on: {text}");
            assert!(!r.any());
        }
    }

    /// A private key is a multi-line construct: scrubbing it token-by-token
    /// would leave the armour and shred the body into pieces.
    #[test]
    fn a_private_key_block_goes_whole() {
        // In two pieces, so the source holds no block a secret scanner
        // reads as a key (the public repo's push protection; 11-07).
        let pem = concat!(
            "before\n-----BEGIN ",
            "OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAABG5vbmUAAAAEbm9uZQ==\n",
            "-----END OPENSSH PRIVATE KEY-----\nafter"
        );
        let r = scrub(pem, &on());
        assert!(!r.text.contains("BEGIN OPENSSH"), "{}", r.text);
        assert!(!r.text.contains("b3BlbnNzaC1rZXktdjEA"), "{}", r.text);
        assert!(r.text.contains("before") && r.text.contains("after"));
        assert!(r.kinds.iter().any(|k| k == "private key"));
    }

    /// Unterminated armour drops the remainder rather than emitting half a key.
    #[test]
    fn an_unterminated_key_block_fails_closed() {
        let pem = crate::fakes::PEM_RSA_UNTERMINATED;
        let r = scrub(pem, &on());
        assert!(!r.text.contains("MIIEowIBAAKCAQEAvXqLmn"), "{}", r.text);
    }

    /// A certificate is public — redacting it would be noise.
    #[test]
    fn a_certificate_is_not_a_secret() {
        let cert = "-----BEGIN CERTIFICATE-----\nMIIB1234\n-----END CERTIFICATE-----";
        let r = scrub(cert, &on());
        assert!(r.text.contains("BEGIN CERTIFICATE"), "{}", r.text);
    }

    /// **Redacting this project's own source changes nothing.**
    ///
    /// The mechanical guard against the whole class, driven from real files
    /// rather than strings I chose — a hand-picked corpus only ever proves the
    /// cases its author thought of, and the author of the previous rule thought
    /// of exactly one (`tokenizer: gpt2`).
    ///
    /// Without the value floor these two files produce nine redactions:
    /// `let token = &argv[i];` became
    /// `let token = &[redacted: named secret][i];`, because `=` and `:` are
    /// ordinary syntax and `token` is an ordinary identifier. The model was
    /// handed source that does not compile, and nothing said so.
    ///
    /// Both files are chosen for being full of the hint words
    /// (`token`/`secret`/`pass`/`auth`) and holding **no planted credentials**
    /// — the `secrets/` module's own sources are deliberately full of those, so
    /// counting them as false positives measures the fixtures.
    ///
    /// **Asserts its premise.** These files will change, and if the hint sites
    /// drift out of them this test would keep passing while measuring nothing.
    /// So it first checks that, floor removed, the corpus still fires.
    #[test]
    fn redacting_our_own_source_is_a_no_op() {
        let no_floor = SecretsConfig {
            name_min_value: 0,
            ..on()
        };
        for (name, src) in [
            ("shell/normalize.rs", include_str!("../shell/normalize.rs")),
            (
                "permission/decide.rs",
                include_str!("../permission/decide.rs"),
            ),
        ] {
            assert!(
                scrub(src, &no_floor).any(),
                "premise: {name} no longer exercises the name rule — pick a new corpus file"
            );
            let r = scrub(src, &on());
            assert!(
                r.kinds.is_empty(),
                "{name}: redacted {} span(s) of ordinary source: {:?}",
                r.kinds.len(),
                r.kinds,
            );
            assert_eq!(r.text, src, "{name} came back altered");
        }
    }

    /// **The floor is what does it** — and it is reachable from config.
    ///
    /// Pins the mechanism rather than the outcome: the same line is mangled
    /// again the moment the floor is lowered, so this cannot pass for some
    /// unrelated reason.
    #[test]
    fn the_value_floor_is_what_spares_source_code() {
        let line = "let token = &argv[i];";
        assert_eq!(scrub(line, &on()).text, line, "spared at the default");

        let no_floor = SecretsConfig {
            name_min_value: 0,
            ..on()
        };
        assert!(
            scrub(line, &no_floor).text.contains("[redacted"),
            "premise: without the floor this is exactly the bug"
        );
    }

    /// **A named value is measured and redacted WHOLE — no fragment survives.**
    ///
    /// The floor first measured the token, and a token ends at any punctuation,
    /// so each of these reached it as a short first fragment (`hunter2`, `x7`,
    /// `correct`), was spared, and went out whole with no marker at all. Two
    /// reviewers reproduced it independently. Before the floor the same unit
    /// leaked everything after the first fragment. Asserting only "something
    /// was redacted" would pass on both bugs, so this asserts on the TAIL.
    #[test]
    fn a_named_secret_long_enough_to_matter_is_still_redacted() {
        // The cases, each with why, are in the fakes (`NAMED_SECRETS_WITH_TAILS`).
        for &(text, tail) in crate::fakes::NAMED_SECRETS_WITH_TAILS {
            let r = scrub(text, &on());
            assert!(r.any(), "missed a real one: {text}");
            assert!(!r.text.contains(tail), "{text} leaked its tail: {}", r.text);
        }
    }

    /// **The floor's boundary and unit are pinned, not described.** Eleven is
    /// spared, twelve is not — and the count is in CHARACTERS, so eleven
    /// characters in 21 bytes is still eleven. (The value starts with an ASCII
    /// letter because a named value begins at a token character; one of pure
    /// `é` would never be named at all, and the check would pass vacuously.)
    /// Unquoted: quoted under a password's name, a value has no floor since
    /// 11-08 (`the_floor_yields_where_the_context_binds_a_credential`).
    #[test]
    fn the_value_floor_is_twelve_characters() {
        let at = |v: &str| scrub(&format!("password: {v}"), &on()).any();
        assert!(!at("abcdefghijk"), "11 chars must be spared");
        assert!(at("abcdefghijkl"), "12 chars must be redacted");
        let wide = format!("a{}", "é".repeat(10));
        assert_eq!((wide.chars().count(), wide.len()), (11, 21), "premise");
        assert!(!at(&wide), "11 chars in 21 bytes must be spared");
        assert!(at(&format!("{wide}é")), "premise: 12 of them are redacted");
        assert!(
            at("\"abc\""),
            "quoted under a password's name, three characters are taken"
        );
    }

    /// **The floor yields where the context binds a credential** (Steven,
    /// 11-08: what matters is when something is *obviously* a password) —
    /// an assignment with no spaces to an environment-style name or a flag,
    /// a quoted value bound to a password's name, a JSON value under a
    /// password's key; a glued name (`PGPASSWORD`), `SSHPASS` and a
    /// qualified `pwd` are names. Every value here is short enough that the
    /// floor alone spares it.
    #[test]
    fn the_floor_yields_where_the_context_binds_a_credential() {
        let password = crate::fakes::SHORT_PASSWORD;
        // The lines are in the fakes (`CONTEXT_BOUND_CREDENTIALS`).
        for &line in crate::fakes::CONTEXT_BOUND_CREDENTIALS {
            assert!(line.contains(password), "premise: {line} holds it");
            let r = scrub(line, &on());
            assert!(!r.text.contains(password), "{line} kept it: {}", r.text);
        }
        let input = serde_json::json!({"passwd": password});
        let out = redact_json(&input, &on(), &Aliases::default());
        assert_eq!(out["passwd"], "[redacted: named secret]", "as its text is");
    }

    /// **Quoted, a binding is still one** (review stop 17, E1 L1): the floor
    /// yielded to `TOKEN=v` and held for `TOKEN="v"`, the same assignment.
    /// JSON's env-style key is the same reading, its `=` a `:`.
    #[test]
    fn a_quoted_binding_yields_the_floor_as_a_bare_one_does() {
        let password = crate::fakes::SHORT_PASSWORD;
        for line in [
            format!("TOKEN={password}"),
            format!("TOKEN=\"{password}\""),
            format!("TOKEN='{password}'"),
            format!("--token=\"{password}\""),
        ] {
            let r = scrub(&line, &on());
            assert!(!r.text.contains(password), "{line} kept it: {}", r.text);
        }
        let input = serde_json::json!({"TOKEN": password});
        let out = redact_json(&input, &on(), &Aliases::default());
        assert_eq!(out["TOKEN"], "[redacted: named secret]", "as its text is");
        // A switch's own word is no credential, in JSON as in text (close
        // verifier LOW 3).
        // A name that says "password" too: the password arm came first and
        // masked it, the exemption only the env arm's (stop 17, V2a LOW 2).
        let setting = serde_json::json!({
            "SKIP_AUTH": "true",
            "CACHE_AUTH": "disabled",
            "HAS_PASSWORD": "true",
            "AUTH": "off",
        });
        let kept = redact_json(&setting, &on(), &Aliases::default());
        assert_eq!(kept, setting);
        for text in ["SKIP_AUTH=true", "HAS_PASSWORD=true", "AUTH=off"] {
            assert_eq!(scrub(text, &on()).text, text, "and its text, alike");
        }
        // Only when spelled as an assignment: lowercase, and with spaces,
        // the floor stands (`the_floor_holds_where_the_context_is_code`).
        let spaced = format!("TOKEN == \"{password}\"");
        assert_eq!(scrub(&spaced, &on()).text, spaced);
    }

    /// **A name you added says what it says** (review stop 17, E1 L2):
    /// `names = ["pin"]` took `--pin=v`, and `pin = "v"` got past the
    /// password tier, which read only the shipped names.
    #[test]
    fn a_name_you_added_reaches_the_password_tier() {
        let password = crate::fakes::SHORT_PASSWORD;
        let (line, input) = (
            format!("pin = \"{password}\""),
            serde_json::json!({ "pin": password }),
        );
        let without = redact_json(&input, &on(), &Aliases::default());
        assert_eq!(scrub(&line, &on()).text, line, "premise: no such name");
        assert_eq!(without["pin"], password, "premise: no such name");
        let mut cfg = on();
        cfg.names.push("pin".into());
        let r = scrub(&line, &cfg);
        assert!(!r.text.contains(password), "{line} kept it: {}", r.text);
        let out = redact_json(&input, &cfg, &Aliases::default());
        assert_eq!(out["pin"], "[redacted: named secret]", "as its text is");
    }

    /// **And holds where the context is code** — a short value under a
    /// credential's name, spared as the floor spares it: a comparison and a
    /// match arm, a setting about secrets, a lexer's token, a keyword
    /// argument, a local, the working directory (a long one), a name that
    /// ends in `pass` but names none. Measured over this repository (11-08):
    /// quoting alone took `token == "--"` and `secret = "redact"`. And the
    /// named limit: YAML's or prose's `password: hunter2`, unquoted.
    #[test]
    fn the_floor_holds_where_the_context_is_code() {
        for line in [
            r#"if token == "--" {"#,
            r#"Password => "pw","#,
            r#"secret = "redact""#,
            r#""token": "argv""#,
            "connect(password=pw)",
            "let token = tokens.next();",
            "PWD=/home/me/projects/app",
            "OLDPWD=/tmp",
            "BYPASS=1",
            "password: hunter2",
        ] {
            let r = scrub(line, &on());
            assert_eq!(r.text, line, "{line} was redacted");
        }
    }

    /// **Known limit: an unquoted value containing code syntax keeps its tail.**
    ///
    /// Unquoted, a value ends at `( ) [ ] { } < > ; , :` because that is what
    /// keeps source code intact (`tokens.next().unwrap()` measures
    /// `tokens.next`). The price is here: the part before the bracket is short,
    /// so nothing is redacted. Quoted, the same password is caught whole —
    /// which is the recovery, and the half of this test that must never fail.
    #[test]
    fn an_unquoted_password_with_code_syntax_is_a_known_miss() {
        let unquoted = "password=P@ss(word)Xk7pQ2mZ9";
        assert_eq!(
            scrub(unquoted, &on()).text,
            unquoted,
            "limit moved: update the doc"
        );
        let quoted = r#"password="P@ss(word)Xk7pQ2mZ9""#;
        assert!(
            !scrub(quoted, &on()).text.contains("Xk7pQ2mZ9"),
            "quoted must be whole"
        );
    }

    /// **A torn quote fails closed to the end of its line — and no further.**
    /// The value is redacted whole even with no closing quote, but the next
    /// line is someone else's output and must survive.
    #[test]
    fn an_unclosed_quote_stops_at_the_line_end() {
        let text = "password: \"Xk7pQ2mZ9vLab and more\nbuild finished in 42 seconds";
        let r = scrub(text, &on());
        assert!(!r.text.contains("Xk7pQ"), "value leaked: {}", r.text);
        assert!(
            r.text.ends_with("\nbuild finished in 42 seconds"),
            "{}",
            r.text
        );
    }

    /// **Code syntax ends an unquoted value; a backtick is not a quote.** Each
    /// line is a false positive the whole-value rule produced on this repo's
    /// own sources before these stops were added (a corpus walk, 104k lines).
    #[test]
    fn code_syntax_ends_an_unquoted_value() {
        for line in [
            "Command::Secret => Command::Secrets,",
            "- **Secrets/handles.** A job cannot spend a lent credential: `Cause::SecretHandle`",
            "let token = tokens.next().unwrap();",
        ] {
            let r = scrub(line, &on());
            assert_eq!(r.text, line, "source mangled: {:?}", r.kinds);
        }
    }

    /// **`names_exclude` takes the spelling the user's code uses.** It used to
    /// compare strings exactly against our `api-key`, so `api_key` and `TOKEN`
    /// were accepted and did nothing.
    #[test]
    fn an_excluded_name_matches_by_segment_not_by_spelling() {
        let line = format!("TOKEN={}", crate::fakes::TOKEN_17);
        assert!(scrub(&line, &on()).any(), "premise: redacted by default");
        for spelling in ["token", "TOKEN", "Token"] {
            let cfg = SecretsConfig {
                names_exclude: vec![spelling.into()],
                ..on()
            };
            assert_eq!(scrub(&line, &cfg).text, line, "{spelling} did not exclude");
        }
        let line = "API_KEY=secretvalue12345";
        for spelling in ["api_key", "API-KEY", "apiKey"] {
            let api = SecretsConfig {
                names_exclude: vec![spelling.into()],
                ..on()
            };
            assert_eq!(
                scrub(line, &api).text,
                line,
                "{spelling} did not exclude api-key"
            );
        }
    }

    /// A shipped hint can be turned OFF, so the ring closes.
    #[test]
    fn a_shipped_name_hint_can_be_excluded() {
        let text = format!("token = {}", crate::fakes::PASSWORD_15);
        assert!(!scrub(&text, &on()).kinds.is_empty(), "premise: caught");

        let off = SecretsConfig {
            names_exclude: vec!["token".to_string()],
            ..on()
        };
        assert_eq!(scrub(&text, &off).text, text, "names_exclude was ignored");
        // Narrowing one hint does not disarm the others.
        let other = format!("password = {}", crate::fakes::PASSWORD_15);
        assert!(!scrub(&other, &off).kinds.is_empty(), "over-narrowed");
    }

    /// **The 23–31 character band the old floor excluded.**
    ///
    /// `entropy_min_run` was 32 by intuition; 23 is the arithmetic floor
    /// implied by `entropy_bits = 4.5` (entropy needs 2^4.5 ≈ 23 distinct
    /// characters to reach 4.5, so anything lower is inert).
    ///
    /// Recall in this band is low whatever the gate — ~5% of random 24-char
    /// tokens reach 4.5 bits — so these fixtures are chosen to clear it (all
    /// characters distinct), and pin the GATE, not a recall claim.
    #[test]
    fn a_24_character_key_is_caught() {
        // 24 distinct chars, mixed class, 4.58 bits.
        let key = crate::fakes::KEY_24;
        assert!(
            key.chars().count() < 32 && key.chars().count() >= 23,
            "premise: in the band the old floor excluded"
        );
        let r = scrub(&format!("value={key}"), &on());
        assert!(!r.text.contains(key), "missed the 23-31 band: {}", r.text);

        // And the floor is what admits it.
        let old = SecretsConfig {
            entropy_min_run: 32,
            ..on()
        };
        assert!(
            scrub(&format!("value={key}"), &old).text.contains(key),
            "premise: the old floor is what used to lose it"
        );

        // The bottom of the band: 23 distinct characters is log2(23) = 4.52
        // bits, the shortest run that can clear 4.5 at all. Pins 23 exactly —
        // a drift to 24 would lose it and fail only config tautologies.
        let shortest = "aB3cD4eF5gH6iJ7kL8mN9pQ";
        assert_eq!(shortest.chars().count(), 23, "premise");
        let r = scrub(&format!("value={shortest}"), &on());
        assert!(
            !r.text.contains(shortest),
            "missed the 23-char floor: {}",
            r.text
        );
    }

    /// **Ordinary output survives the entropy rule.**
    ///
    /// The corpus that condemned the previous one, which measured entropy over
    /// a whole token against 3.5 bits — below ordinary English prose — and
    /// redacted nine of these eleven. Steven found it live: a `find` listing
    /// lost both `SECURITY-*.md` files, so the model answered "list all md
    /// files" with a list short by two and neither party could tell.
    ///
    /// Each entry is here because it breaks a *different* way, so a future
    /// loosening has to argue with all of them:
    #[test]
    fn ordinary_output_is_not_mistaken_for_a_secret() {
        let benign = [
            // Long, dated, mixed-case: the ones that actually bit. At the old
            // hardcoded 3.5 bits these came back from `find` as
            // `[redacted: high-entropy string]`, so a listing read as complete
            // while two entries short; 4.5 fixed it (`SECURITY-…-redirect-bypass`
            // measures 4.376). The fix is a NUMBER, so it un-fixes itself if
            // `entropy_bits` is turned down — which is what these lines pin.
            "SECURITY-2026-07-27-redirect-bypass.md",
            "SECURITY-2026-08-08-unattended-execution.md",
            // A path — `/` must not fuse segments into one long run, which is
            // where `detect-secrets`' charset would take us.
            "docs/DesignDocs/SECURITY-2026-08-08-unattended-execution.md",
            // rustdoc's content-hashed asset.
            "SourceSerif4-LICENSE-a2cfd9d5.md",
            // Plain English with a capital and a number.
            "The build took 45 seconds and passed OK",
            "Reviewer A found 3 CRITICALs in Milestone 4",
            // A type signature and a manifest line.
            "std::collections::HashMap<String, Vec<u8>>",
            "Cargo.toml requires Rust 1.95 for edition 2024",
            // Lowercase hex, long enough to clear every length gate — spared by
            // the character-class requirement, which is why it stays.
            "commit 7bc7193f2a4e8d1c0b5a6e9f3d2c1b0a9e8d7c6b",
            "550e8400-e29b-41d4-a716-446655440000",
        ];
        for text in benign {
            let r = scrub(text, &on());
            assert_eq!(r.text, text, "redacted ordinary output: {text}");
            assert!(r.kinds.is_empty(), "{text} -> {:?}", r.kinds);
        }
    }

    /// **Lowercase hex stays safe even when the threshold is lowered.**
    ///
    /// The character-class gate is what spares a git SHA and a UUID — but at the
    /// default 4.5 bits it never gets the chance, because entropy alone already
    /// spares them (3.91 and 3.39). So removing the gate broke no test, and the
    /// corpus above could not tell you whether it mattered
    /// (`[[feedback_fixture_omits_the_axis]]`).
    ///
    /// It matters as soon as someone turns `entropy_bits` down, which is now a
    /// supported thing to do. A coding agent's output is full of commit hashes;
    /// redacting them because the user wanted a tighter net would be its own
    /// kind of broken. Driven at 3.0 so the gate is the only thing standing
    /// between these and redaction.
    #[test]
    fn lowercase_hex_is_spared_by_class_not_only_by_entropy() {
        let tight = SecretsConfig {
            entropy_bits: 3.0,
            ..on()
        };
        for text in [
            "7bc7193f2a4e8d1c0b5a6e9f3d2c1b0a9e8d7c6b",
            "550e8400-e29b-41d4-a716-446655440000",
        ] {
            assert!(
                shannon_bits(text) >= 3.0,
                "premise: {text} clears the lowered bar on entropy alone"
            );
            assert_eq!(scrub(text, &tight).text, text, "redacted: {text}");
        }
        // And the gate is not simply refusing everything at this threshold: a
        // mixed-class key of the same length still goes.
        let key = crate::fakes::KEY_40;
        assert!(
            !scrub(key, &tight).text.contains(key),
            "premise: still catches"
        );
    }

    /// **Real key material is still caught**, including the shape that decides
    /// the default charset.
    ///
    /// URL-safe base64 embeds `-` and `_`; a 43-character token carrying two of
    /// them has a longest *alphanumeric* run of 23. A rule that stopped at
    /// alphanumerics would miss it, which is why they are in
    /// `entropy_token_extra` by default. These are generated-looking but fixed,
    /// so the test cannot drift.
    #[test]
    fn key_shaped_runs_are_still_caught() {
        let secrets = [
            crate::fakes::KEY_32,
            crate::fakes::KEY_40,
            "71fTquWoGsbeKXgzg2sye9b2Rann76dEyTzAeKOmXRr",
            // The url-safe ones: separators inside the token.
            "jjlAFbVV6q9rXxtNDFyuzX9k1gnneGEYG1-LwiqD9jJ",
            "j1d_bWWbjkloG1QX647kdNl9cDo_-GbVMszvR4_EPZGz3zBXCOArr_Sfi",
        ];
        for secret in secrets {
            let r = scrub(&format!("value={secret}"), &on());
            assert!(!r.text.contains(secret), "missed a key: {}", r.text);
        }
    }

    /// **Every knob actually reaches the rule.**
    ///
    /// Configurability is the point of this project, and a setting nothing reads
    /// is worse than none — it reads as honoured (`[[feedback_setting_has_three_halves]]`).
    /// Each is driven in the direction that CHANGES the verdict on one string,
    /// so a knob wired to nothing fails here.
    #[test]
    fn each_entropy_knob_changes_the_verdict() {
        // Spared by default: a 35-char run at ~4.38 bits.
        let name = "SECURITY-2026-07-27-redirect-bypass";
        assert_eq!(scrub(name, &on()).text, name, "premise: spared by default");

        // Lower the bar and it is caught — so `entropy_bits` is consulted.
        let loose = SecretsConfig {
            entropy_bits: 4.0,
            ..on()
        };
        assert!(
            !scrub(name, &loose).text.contains(name),
            "entropy_bits inert"
        );

        // Drop `-` from the token charset and the run splits into short words,
        // so it is spared even at the lower threshold — `entropy_token_extra`
        // is consulted too.
        let split = SecretsConfig {
            entropy_bits: 4.0,
            entropy_token_extra: "+=".to_string(),
            ..on()
        };
        assert_eq!(scrub(name, &split).text, name, "entropy_token_extra inert");

        // A real key, spared by raising the length gate above it.
        let key = crate::fakes::KEY_32;
        assert!(!scrub(key, &on()).text.contains(key), "premise: caught");
        let long_only = SecretsConfig {
            entropy_min_run: 64,
            ..on()
        };
        assert_eq!(scrub(key, &long_only).text, key, "entropy_min_run inert");
    }

    #[test]
    fn high_entropy_strings_are_caught_and_can_be_disabled() {
        let blob = crate::fakes::HIGH_ENTROPY_44;
        let r = scrub(&format!("value={blob}"), &on());
        assert!(!r.text.contains(blob), "{}", r.text);

        let off = SecretsConfig {
            redact_high_entropy: false,
            ..on()
        };
        let r = scrub(&format!("value={blob}"), &off);
        assert!(r.text.contains(blob), "opt-out honoured");
    }

    /// The shipped prefix list is a seed. Anyone with an internal token format
    /// has a shape we never heard of, and a hardcoded list protects them not at
    /// all. (Steven, 2026-07-25.)
    #[test]
    fn users_can_add_their_own_credential_shapes() {
        let cfg = SecretsConfig {
            prefixes: vec!["acme_tok_".to_string()],
            ..on()
        };
        let r = scrub("token acme_tok_abcdefghijklmn here", &cfg);
        assert!(!r.text.contains("acme_tok_abcdefghijklmn"), "{}", r.text);
        assert!(r.kinds.iter().any(|k| k == "configured prefix"));
        // Still bounded: a bare occurrence of the prefix is not a secret.
        let r = scrub("see the acme_tok_ docs", &cfg);
        assert_eq!(r.text, "see the acme_tok_ docs");
        // An EMPTY prefix — a stray `""` in the list — is no shape: carried
        // by every long word, it would mask all prose. Unheld until review
        // stop 12 (T8, S-M10 green).
        let blank = SecretsConfig {
            prefixes: vec![String::new()],
            ..on()
        };
        let prose = "an ordinaryverylongword in a sentence";
        let r = scrub(prose, &blank);
        assert_eq!(r.text, prose, "{:?}", r.kinds);
    }

    #[test]
    fn configured_literals_are_scrubbed() {
        let cfg = SecretsConfig {
            extra: vec!["hunter2-the-database-password".to_string()],
            ..on()
        };
        let r = scrub("psql url=hunter2-the-database-password end", &cfg);
        assert!(!r.text.contains("hunter2"), "{}", r.text);
        assert!(r.kinds.iter().any(|k| k == "configured value"));
    }

    /// A too-short configured value would match everywhere; refuse it rather
    /// than blank the output.
    #[test]
    fn a_trivially_short_literal_is_ignored() {
        let cfg = SecretsConfig {
            extra: vec!["ab".to_string()],
            ..on()
        };
        let r = scrub("abandon the cabbage", &cfg);
        assert_eq!(r.text, "abandon the cabbage");
    }

    #[test]
    fn disabling_redaction_passes_text_through() {
        let off = SecretsConfig {
            redact: false,
            ..on()
        };
        let key = "sk-ant-api03-abcdefghijklmnop";
        let r = scrub(key, &off);
        assert_eq!(r.text, key);
        assert!(!r.any());
    }

    /// **The summary says how many, not just what kind.**
    ///
    /// This test previously asserted `"anthropic key"` for two removals — the
    /// contract that let a `find` listing come back two filenames short while
    /// the notice read the same as for one, the model presented the list as
    /// complete, and neither party could tell. It failed when the behaviour
    /// changed, which is what a test that measures something does.
    #[test]
    fn the_summary_counts_each_kind() {
        let r = scrub(
            "sk-ant-api03-aaaaaaaaaaaaaaaa and sk-ant-api03-bbbbbbbbbbbbbbbb",
            &on(),
        );
        assert_eq!(r.kinds.len(), 2, "both occurrences scrubbed");
        assert_eq!(r.summary(), "2 × anthropic key");

        // One removal stays unnumbered: a count earns its place only when it
        // tells you something.
        let one = scrub("sk-ant-api03-aaaaaaaaaaaaaaaa", &on());
        assert_eq!(one.summary(), "anthropic key");

        // Kinds stay distinct, in order of first appearance.
        let mixed = scrub(
            "sk-ant-api03-aaaaaaaaaaaaaaaa AKIAIOSFODNN7EXAMPLE sk-ant-api03-bbbbbbbbbbbbbbbb",
            &on(),
        );
        assert_eq!(mixed.summary(), "2 × anthropic key, aws access key");
    }

    /// Multibyte text must survive: redaction runs on output we do not control.
    #[test]
    fn multibyte_output_is_not_corrupted() {
        let text = "résumé 🙂 日本語 — ordinary";
        assert_eq!(scrub(text, &on()).text, text);
    }

    // --- where the two mechanisms meet (P3) ----------------------------------

    /// A secret we HOLD comes back as its handle, so the model can keep acting on
    /// it. A secret we do NOT hold is still scrubbed one-way in the same pass,
    /// and the two are reported separately because they mean different things.
    #[test]
    fn a_held_secret_returns_as_its_handle_others_are_scrubbed() {
        let mut aliases = Aliases::default();
        let handle = aliases
            .insert("gh", crate::fakes::GITHUB_TOKEN_HELD, "test", true)
            .unwrap();
        let r = redact(
            &format!(
                "held={} other={}",
                crate::fakes::GITHUB_TOKEN_HELD,
                crate::fakes::GITHUB_TOKEN_SOMEONE_ELSES
            ),
            &on(),
            &aliases,
        );
        assert!(r.text.contains(&handle), "held value is usable: {}", r.text);
        assert!(!r.text.contains("ghp_pretendheld"), "{}", r.text);
        assert!(!r.text.contains("ghp_someoneelses"), "{}", r.text);
        assert!(r.text.contains("[redacted: github token]"), "{}", r.text);
        assert_eq!(r.masked, vec!["gh".to_string()]);
        assert_eq!(r.kinds, vec!["github token".to_string()]);
    }

    /// The handle must survive the one-way pass that runs after it — masking is
    /// pointless if the scrubber then eats the placeholder it produced.
    #[test]
    fn a_handle_is_not_itself_redacted() {
        let mut aliases = Aliases::default();
        let handle = aliases
            .insert("db-url", crate::fakes::DB_URL, "t", true)
            .unwrap();
        let r = redact(
            &format!("DATABASE_URL={}", crate::fakes::DB_URL),
            &on(),
            &aliases,
        );
        assert_eq!(r.text, format!("DATABASE_URL={handle}"), "{}", r.text);
    }

    /// **A handle after a credential's name is kept** — where the
    /// name-directed pass bites: `GITHUB_TOKEN={{github-token.fd7d}}` read as
    /// a token's value and was replaced by a placeholder the moment the skip
    /// for our own handles was off. Every fixture used a name no hint
    /// matches, so the skip was held by nothing (review stop 12, T8 H6).
    /// Every hint the pass reads, in the shapes it reads it: an assignment,
    /// a YAML key, a JSON pair, a flag — the handle written there, and the
    /// value it stands for masked to it.
    #[test]
    fn a_handle_after_a_credentials_name_is_kept() {
        for hint in DEFAULT_SECRET_NAMES {
            let mut aliases = Aliases::default();
            let value = format!("v{}-Xk7pQ2mZ9vL4nR8tW1yB", hint.len());
            let handle = aliases.insert(hint, &value, "t", true).unwrap();
            let env = hint.to_ascii_uppercase().replace('-', "_");
            for shape in [
                format!("GITHUB_{env}=@"),
                format!("{hint}: @"),
                format!("{{\"{hint}\": \"@\"}}"),
                format!("--{hint} @"),
            ] {
                let written = shape.replace('@', &handle);
                assert_eq!(redact(&written, &on(), &aliases).text, written, "{hint}");
                let held = redact(&shape.replace('@', &value), &on(), &aliases).text;
                assert_eq!(held, written, "{hint}: the value comes back as its handle");
            }
        }
    }

    /// Masking is not conditional on `redact`: turning the one-way net off is a
    /// statement about detection heuristics, not permission to send back a value
    /// the user explicitly lent us as a handle.
    #[test]
    fn a_held_secret_is_masked_even_with_redaction_off() {
        let mut aliases = Aliases::default();
        let handle = aliases
            .insert("gh", crate::fakes::GITHUB_TOKEN_HELD, "test", true)
            .unwrap();
        let off = SecretsConfig {
            redact: false,
            ..on()
        };
        let r = redact(
            &format!("token {}", crate::fakes::GITHUB_TOKEN_HELD),
            &off,
            &aliases,
        );
        assert_eq!(r.text, format!("token {handle}"));
        assert_eq!(r.masked, vec!["gh".to_string()]);
    }

    /// **A value over lines is one handle in text read whole, and keeps its
    /// lines in text read by number** (review stop 16, E 1): a command's
    /// output round-trips through the handle; a Read's k-th line stays the
    /// file's k-th, the handle where the value starts and [`CONTINUED`]
    /// where it runs on. A value ending at its newline leaves the next line
    /// as it was; a key block keeps every line it had.
    #[test]
    fn a_value_over_lines_keeps_them_only_when_read_by_number() {
        let mut aliases = Aliases::default();
        let h = aliases.insert("db", "lineOne\nlineTwo", "t", true).unwrap();
        let text = "PASS=lineOne\nlineTwo tail\nnext\n";
        assert_eq!(
            redact(text, &on(), &aliases).text,
            format!("PASS={h} tail\nnext\n")
        );
        assert_eq!(
            redact_lines(text, &on(), &aliases).text,
            format!("PASS={h}\n{CONTINUED} tail\nnext\n")
        );
        let ends = aliases.insert("ends", "endsHere\n", "t", true).unwrap();
        assert_eq!(
            redact_lines("K=endsHere\nnext\n", &on(), &aliases).text,
            format!("K={ends}\nnext\n")
        );
        let pem = crate::fakes::PEM_RSA_FRAMED;
        assert_eq!(
            redact_lines(pem, &on(), &Aliases::default()).text,
            format!("a\n[redacted: private key]\n{CONTINUED}\n{CONTINUED}\nb\n")
        );
        assert_eq!(
            redact(pem, &on(), &Aliases::default()).text,
            "a\n[redacted: private key]\nb\n"
        );
    }

    /// **What can run over a line is known before any scrub** — each of the
    /// four: a held value with a newline, a configured literal with one, a
    /// key block's armour, a URL's userinfo broken by one; and with
    /// redaction off, only the held value, whose mask still runs. A URL
    /// whose `@` is past its path, or on its own line, crosses nothing.
    #[test]
    fn what_can_run_over_a_line_is_known_first() {
        let none = Aliases::default();
        let plain = "a line\nhttps://host/path@x\nuser@host\nhttps://u:p@db/app\n";
        assert!(!crosses_lines(plain, &on(), &none));
        assert!(crosses_lines("x://user\nname@host", &on(), &none));
        assert!(crosses_lines(
            crate::fakes::PEM_RSA_OPENING_LINE,
            &on(),
            &none
        ));
        let literal = SecretsConfig {
            extra: vec!["confi\ngured value".into()],
            ..on()
        };
        assert!(crosses_lines(plain, &literal, &none));
        let mut held = Aliases::default();
        held.insert("db", "line one\nline two", "t", true).unwrap();
        assert!(crosses_lines(plain, &on(), &held));
        let off = SecretsConfig {
            redact: false,
            ..literal
        };
        assert!(!crosses_lines(crate::fakes::PEM_BEGIN_RSA, &off, &none));
        assert!(
            crosses_lines(plain, &off, &held),
            "a held value is masked, off or on"
        );
    }

    /// **The redactions counted are those the scrub put in** — each of
    /// ours it reported, and not one the text already held as written.
    #[test]
    fn only_the_redactions_a_scrub_put_in_are_counted() {
        let raw = "seen [redacted: github token] before\n\
                   key=ghp_PLANTEDTOKENVALUE0123456789\n\
                   x=sk-ant-api03-PLANTEDVALUE0123456789\n";
        let r = redact(raw, &on(), &Aliases::default());
        assert!(!r.kinds.is_empty(), "premise: something was redacted");
        assert_eq!(added_redactions(raw, &r.text), r.kinds);
        assert!(added_redactions(raw, raw).is_empty());
        // A placeholder whose kind is not ours is the text's, not a count.
        assert!(added_redactions("", "[redacted: whatever]").is_empty());
    }

    /// **The plug, working.** A valid GitHub token glued to a word with **no
    /// separator at all** — the case that was a documented miss — is now caught,
    /// because its checksum is evidence no heuristic could supply.
    ///
    /// The token is *synthesised* with a correct CRC32 rather than pasted: a
    /// real credential must never appear in this repository, and a checksum is
    /// precisely the thing a made-up string cannot exercise.
    #[test]
    fn a_valid_token_is_caught_with_no_separator_at_all() {
        let token =
            super::super::format::synthetic_github_token("ghp_", "abcdefghijklmnopqrstuvwxyz0123");
        for text in [
            format!("prefix{token}"),
            token.clone(),
            format!("url=https://x@{token}.example"),
            format!("noise{token}noise"),
        ] {
            let r = redact(&text, &on(), &Aliases::default());
            assert!(!r.text.contains(&token), "survived: {text} -> {}", r.text);
            assert!(r.kinds.iter().any(|k| k == "github token"), "{:?}", r.kinds);
        }
    }

    /// **The limit that remains**, and it is now narrow: a token that does *not*
    /// carry a valid checksum, welded into a word with no separator. There is no
    /// evidence at all there — no boundary, no structure — so matching it would
    /// mean matching a prefix at any offset in any word.
    ///
    /// Kept as a named test so the boundary is visible and cannot move quietly.
    #[test]
    fn a_checksumless_credential_with_no_separator_is_a_known_miss() {
        let welded = "prefixAKIAIOSFODNN7EXAMPLE";
        let out = redact(welded, &on(), &Aliases::default());
        assert_eq!(out.text, welded, "known limit — see the doc comment");

        // Any separator at all, and it is caught.
        for delimited in [
            "prefix AKIAIOSFODNN7EXAMPLE",
            "prefix=AKIAIOSFODNN7EXAMPLE",
            "prefix-AKIAIOSFODNN7EXAMPLE",
            "prefix_AKIAIOSFODNN7EXAMPLE",
            "prefix.AKIAIOSFODNN7EXAMPLE",
            "prefix-----AKIAIOSFODNN7EXAMPLE",
        ] {
            let out = redact(delimited, &on(), &Aliases::default());
            assert!(!out.text.contains("AKIAIOSFODNN"), "{}", out.text);
        }
    }

    /// **A checksum may only ADD catches.** A truncated or typo'd token fails
    /// its checksum — our own output cap can truncate one — and must still be
    /// redacted by the ordinary path. The first implementation made the checksum
    /// exclusive and silently stopped redacting exactly these.
    ///
    /// **With no name before it, and by the path that catches it**: under
    /// `token=` the name rule masked the value whatever its shape, so the
    /// checksum made exclusive again passed here, red only through five other
    /// tests (review stop 12, T8 M13). Each premised to fail its checksum:
    /// a typo at full length is the shape's to catch (`github token`); one
    /// cut short, as an output cap might, is too short for the shape and
    /// caught as random (`high-entropy string`), as a real token's body is.
    ///
    /// **Measured, a known miss**: cut short AND low in entropy —
    /// `ghp_thisisnotavalidchecksumatallxyz01`, which this test fed — is
    /// caught by neither, only by a name before it. No real token has such a
    /// body; a `ghp_` prefix and 30 characters (the entropy part) would be
    /// the shape to widen to, if one ever did.
    #[test]
    fn a_token_that_fails_its_checksum_is_still_redacted() {
        for (token, kind) in [
            ("ghp_abcdefghijklmnopqrstuvwxyz0123456789", "github token"),
            ("ghp_Zk3pQ9vX2mL7tR4wB8nYa1Bc2De3F", "high-entropy string"),
        ] {
            let body = token.strip_prefix("ghp_").expect("a ghp_ token");
            assert!(
                !super::super::format::github_classic(body),
                "premise: {token} fails its checksum"
            );
            let text = format!("it printed {token} and stopped");
            let r = redact(&text, &on(), &Aliases::default());
            assert_eq!(
                r.text,
                format!("it printed [redacted: {kind}] and stopped"),
                "{token}"
            );
        }
    }

    // --- name-directed masking (the format-independent gap) -------------------

    /// The gap the format probe exposed: a value with **no distinguishing
    /// shape** survives every prefix and entropy test, in every format. What
    /// identifies it is its neighbour — the name in front of it.
    ///
    /// One rule, every format, because tokenising already erased the format.
    #[test]
    fn a_shapeless_value_is_masked_because_its_name_says_what_it_is() {
        for line in [
            "password: hunter2seekrit",
            "password=hunter2seekrit",
            "password = hunter2seekrit",
            "PASSWORD=hunter2seekrit",
            "\"client_secret\": \"hunter2seekrit\"",
            "db.apiKey: hunter2seekrit",
            "<passphrase>hunter2seekrit</passphrase>",
            "STRIPE_SECRET_KEY=hunter2seekrit",
            "//registry.npmjs.org/:_authToken=hunter2seekrit",
        ] {
            let r = scrub(line, &on());
            assert!(
                !r.text.contains("hunter2seekrit"),
                "missed: {line} -> {}",
                r.text
            );
            assert!(r.kinds.iter().any(|k| k == "named secret"), "{line}");
        }
    }

    /// **The false positive that decides whether this ships.** Substring
    /// matching would mask `gpt2` here, because `tokenizer` contains `token` —
    /// and this user writes ML code all day. Segment-suffix matching is what
    /// makes the rule safe.
    #[test]
    fn a_name_that_merely_contains_a_hint_is_left_alone() {
        for line in [
            "tokenizer: gpt2-large",
            "tokenizer_config: bert-base",
            "keychain: /Users/me/Library",
            "authors: sam-and-friends",
            "secretary: alice",
            "password_policy_url: example.com",
        ] {
            let r = scrub(line, &on());
            assert_eq!(r.text, line, "false positive: {line}");
        }
    }

    /// Prose must survive. A delimiter is required between the name and the
    /// value, and whitespace is not one by default — otherwise "the password is
    /// stored in Vault" would redact the word `is`.
    ///
    /// **And a long word after the hint**, which only the guards spare: every
    /// word after a hint here was short, so the twelve-character floor kept
    /// them with the delimiter guard off, or an unnamed scheme word (`token`,
    /// `basic`) carried to the next (review stop 12, T8 M13).
    #[test]
    fn prose_about_secrets_is_not_redacted() {
        for line in [
            "the password is stored in Vault",
            "rotate the token before Friday",
            "# secret sauce: nothing to see",
            "password:",
            "rotate the token periodically-for-safety",
            "keep the password somewhere-safe-and-sound",
            "use basic authentication-over-tls-everywhere",
        ] {
            let r = scrub(line, &on());
            assert_eq!(r.text, line, "false positive: {line}");
        }
    }

    /// A newline is never a value delimiter, so a name at the end of one line
    /// cannot claim the first word of the next.
    #[test]
    fn a_name_does_not_reach_across_a_newline() {
        let r = scrub("password:\nnothing-to-see-here", &on());
        assert!(r.text.contains("nothing-to-see-here"), "{}", r.text);
    }

    /// `.netrc` is whitespace-positional, which is exactly why the delimiter set
    /// is a knob rather than a constant. Off by default (it would eat prose),
    /// available to anyone who needs it.
    #[test]
    fn whitespace_delimiters_are_opt_in_for_netrc_style_files() {
        let line = "machine api.example.com login me password hunter2seekrit";
        assert_eq!(scrub(line, &on()).text, line, "not by default");

        let netrc = SecretsConfig {
            value_delimiters: vec![" ".to_string()],
            ..on()
        };
        let r = scrub(line, &netrc);
        assert!(!r.text.contains("hunter2seekrit"), "{}", r.text);
        assert!(r.text.contains("login me"), "the rest survives: {}", r.text);
    }

    /// The hint list is a seed, like every other list here.
    #[test]
    fn users_can_add_their_own_secret_names() {
        let cfg = SecretsConfig {
            names: vec!["acme-handshake".to_string()],
            ..on()
        };
        let r = scrub("acme_handshake: hunter2seekrit", &cfg);
        assert!(!r.text.contains("hunter2seekrit"), "{}", r.text);
        // And the shipped seed still applies alongside it.
        assert!(
            !scrub("password: hunter2seekrit", &cfg)
                .text
                .contains("hunter2")
        );
    }

    #[test]
    fn names_split_into_segments_on_every_convention() {
        assert_eq!(
            name_segments("ANTHROPIC_API_KEY"),
            ["anthropic", "api", "key"]
        );
        assert_eq!(
            name_segments("anthropic.apiKey"),
            ["anthropic", "api", "key"]
        );
        assert_eq!(name_segments("client-secret"), ["client", "secret"]);
        assert_eq!(name_segments("tokenizer"), ["tokenizer"]);
    }

    /// **The inert-region off switch (security review F1).** A handle-shaped
    /// wrapper must not disable the scrubber; only a handle we actually minted
    /// is inert. Every credential format is built from the characters the old
    /// shape test accepted, so this was a deterministic four-character bypass —
    /// reachable by injected output telling the model to "wrap secrets in
    /// `{{ }}`", on a ReadOnly command that never faces a human.
    #[test]
    fn a_handle_shaped_wrapper_does_not_disable_the_scrubber() {
        for text in [
            "token {{ghp_abcdefghijklmnopqrstuvwxyz0123456789}} end",
            "{{sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789}}",
            "{{AKIAIOSFODNN7EXAMPLE}}",
            "{{password:hunter2seekrit}}",
            "{{Xk7pQ2mZ9vL4nR8tW1yB3cF6hJ0sD5gA2eN7uI4oP1qT}}",
        ] {
            let r = scrub(text, &on());
            assert!(
                r.text.contains("[redacted:"),
                "not scrubbed: {text} -> {}",
                r.text
            );
            assert!(!r.text.contains("ghp_abcdefghij"), "{}", r.text);
            assert!(!r.text.contains("sk-ant-api03-abc"), "{}", r.text);
            assert!(!r.text.contains("hunter2seekrit"), "{}", r.text);
        }
    }

    /// **Security review F7.** A key with its newlines stripped is still a key.
    /// The header terminator used to require `-----\n`, so `tr -d '\n'` output
    /// was not recognised as a block — and with the entropy heuristic off it
    /// passed through **completely intact**. Structure, not formatting.
    #[test]
    fn a_key_folded_onto_one_line_is_still_caught() {
        let no_entropy = SecretsConfig {
            redact_high_entropy: false,
            ..on()
        };
        let folded = crate::fakes::PEM_FOLDED_OPENSSH;
        let r = scrub(folded, &no_entropy);
        assert!(!r.text.contains("b3BlbnNzaC1rZXktdjEA"), "{}", r.text);
        assert!(r.kinds.iter().any(|k| k == "private key"), "{:?}", r.kinds);
    }

    /// The closing armour must be consumed too. Searching for `-----` from the
    /// start of `-----END ` matched at offset zero, leaving the tail behind.
    #[test]
    fn the_closing_armour_is_consumed_whole() {
        let pem = crate::fakes::PEM_OPENSSH_FRAMED;
        let r = scrub(pem, &on());
        assert_eq!(r.text, "before\n[redacted: private key]", "{}", r.text);

        // ...and text after the block survives.
        let r = scrub(&format!("{pem}\nafter"), &on());
        assert_eq!(
            r.text, "before\n[redacted: private key]\nafter",
            "{}",
            r.text
        );
    }

    /// A credential welded into a longer token **at a separator** is now caught.
    /// This was a documented miss; the field's answer (gitleaks pairs each
    /// prefix with a charset and length) is what makes widening safe.
    #[test]
    fn a_credential_after_a_separator_is_caught() {
        for text in [
            "prefix-AKIAIOSFODNN7EXAMPLE",
            "bucket_AKIAIOSFODNN7EXAMPLE",
            "a.b.ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "x~xoxb-123456789012-abcdefghijkl",
        ] {
            let r = scrub(text, &on());
            assert!(
                r.text.contains("[redacted:"),
                "missed: {text} -> {}",
                r.text
            );
        }
    }

    /// **Found by a property test, and squarely in this user's domain.** A
    /// global 12-character floor made `hf_` + 12 characters a HuggingFace token,
    /// so ordinary ML filenames were being redacted. Each issuer now carries its
    /// own minimum, taken from the real format — which is gitleaks' "pair every
    /// prefix with a length" rule, arrived at the hard way.
    #[test]
    fn a_short_prefix_does_not_make_ordinary_filenames_credentials() {
        for text in [
            "hf_model_config-v2",
            "hf_00aa_aaa-a00",
            "hf_dataset_loader.py",
            "sk-notes-for-later",
            "SG.report.for.july",
            "npm_cache_dir_v1",
        ] {
            let r = scrub(text, &on());
            assert_eq!(r.text, text, "false positive on: {text}");
        }
    }

    /// ...and a real one, at its real length, is still caught. A floor that
    /// stopped false positives by stopping detection would be no use.
    #[test]
    fn a_real_length_token_is_still_caught() {
        for text in [
            "hf_abcdefghijklmnopqrstuvwxyz01234567",
            "AKIAIOSFODNN7EXAMPLE",
            "sk-ant-api03-abcdefghijklmnopqrstuvw",
            "npm_abcdefghijklmnopqrstuvwxyz0123456789",
        ] {
            let r = scrub(&format!("value {text} end"), &on());
            assert!(
                r.text.contains("[redacted:"),
                "missed: {text} -> {}",
                r.text
            );
        }
    }

    /// **The corpus that decides whether widening was safe.** Each of these
    /// carries a shipped prefix after a separator and is ordinary text; the
    /// digit requirement is what keeps them intact.
    #[test]
    fn compound_words_containing_a_prefix_are_left_alone() {
        for text in [
            "some-sk-directory-name",
            "x-ASIA-PACIFIC-REGION",
            "risk-management-service",
            "task-sk-handler-registry",
            "release-AKIA-CANDIDATE-NOTES",
            "my_hf_dataset_loader_utils",
            "the.SG.report.summary.page",
        ] {
            let r = scrub(text, &on());
            assert_eq!(r.text, text, "false positive on: {text}");
        }
    }

    /// A prefix glued INSIDE a word — no separator — is still not a match, so
    /// `risk` never reads as `sk-`.
    #[test]
    fn a_prefix_inside_a_word_is_not_a_boundary() {
        let sk = Shape {
            prefix: "sk-",
            min_body: 20,
            kind: "api key",
            check: None,
        };
        assert!(!carries_shape("risk-management-registry-2024", &sk));
        assert!(carries_shape("task-sk-management-registry-2024", &sk));
        // Mid-token, the digit must be in the BODY: one ahead of the prefix
        // is no evidence (the position, once per token: stop 17, B F5).
        assert!(!carries_shape("2024-sk-management-registry-only", &sk));
    }

    /// A password's name is a secret's name: the quoted form reads
    /// [`PASSWORD_NAMES`] only of what the name rule already takes.
    #[test]
    fn every_password_name_is_a_secret_name() {
        for name in PASSWORD_NAMES {
            assert!(DEFAULT_SECRET_NAMES.contains(name), "{name}");
        }
    }

    /// **A switch's own words are not a value**, where the context would
    /// lift the floor (review stop 17, A L4) — and a short word that is
    /// none of them still is, as a PIN is.
    #[test]
    fn a_switch_set_to_a_setting_is_not_a_secret() {
        for line in [
            "--secret=false",
            "--auth=none",
            "SKIP_AUTH=true",
            "HAS_PASSWORD=TRUE",
        ] {
            assert_eq!(scrub(line, &on()).text, line, "taken: {line}");
        }
        assert_ne!(scrub("--password=1234", &on()).text, "--password=1234");
        assert_ne!(scrub("--auth=opensesame", &on()).text, "--auth=opensesame");
    }
}

/// Property tests for the substitution boundary.
///
/// **"What is our unexpected input is someone else's expected"** (Steven,
/// 2026-07-26). Command output is arbitrary bytes from an arbitrary program in
/// an arbitrary locale: CRLF, bidi controls, emoji in paths, a config file in a
/// format nobody here has seen. Example-based tests pin the cases we thought of;
/// these pin the cases we did not, which is where a *correctness* claim actually
/// has to hold.
///
/// The generators are deliberately adversarial rather than random — random
/// letters never find the edges. Every one of these properties is a claim
/// `secrets.md` makes in prose.
#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    /// Output-shaped noise: our own placeholders and handles (the two things a
    /// second pass must not chew on), credential shapes, delimiters from every
    /// config format, and text that is nobody's idea of ASCII.
    fn arb_output() -> impl Strategy<Value = String> {
        let atom = prop_oneof![
            Just("password".to_string()),
            Just("api_key".to_string()),
            Just("tokenizer".to_string()),
            Just("=".to_string()),
            Just(":".to_string()),
            Just("\"".to_string()),
            Just("'".to_string()),
            Just(">".to_string()),
            Just("\n".to_string()),
            Just("\r\n".to_string()),
            Just("{{".to_string()),
            Just("}}".to_string()),
            Just("{{x.dead}}".to_string()),
            Just("{{a:repo}}".to_string()),
            Just("[redacted: named secret]".to_string()),
            Just("[redacted: ".to_string()),
            Just("]".to_string()),
            Just(crate::fakes::PEM_BEGIN_OPENSSH.to_string()),
            Just("-----END OPENSSH PRIVATE KEY-----".to_string()),
            Just("-----".to_string()),
            Just("~".to_string()),
            Just("~root".to_string()),
            Just("=~".to_string()),
            Just("é€🙂".to_string()),
            Just("\u{202e}".to_string()), // bidi override
            Just("\u{200b}".to_string()), // zero-width space
            Just("\u{00a0}".to_string()), // NBSP
            "[a-zA-Z0-9_./+~-]{0,12}",
        ];
        prop::collection::vec(atom, 0..40).prop_map(|v| v.join(" "))
    }

    /// Credential shapes, used to plant a secret inside arbitrary noise —
    /// each built from two pieces, so the source holds no token a secret
    /// scanner reads as one (the public repo's push protection: a GitLab
    /// shape here could have refused the push; 11-07).
    fn arb_secret() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(concat!("sk-ant-", "api03-PLANTEDVALUE0123456789").to_string()),
            Just(concat!("ghp_", "PLANTEDVALUE0123456789abcdefghijklmn").to_string()),
            Just(concat!("AKIA", "PLANTEDVALUE0123").to_string()),
            Just(concat!("xoxb-", "123456789012-PLANTEDVALUE").to_string()),
            Just(concat!("glpat-", "PLANTEDVALUE0123456789").to_string()),
        ]
    }

    fn cfg() -> SecretsConfig {
        SecretsConfig::default()
    }

    proptest! {
        /// **A handle we hold survives any output around it** — written after
        /// a credential's name, in noise of names, delimiters and our own
        /// shapes: the skip that keeps our handles from the name-directed pass
        /// was held by nothing (review stop 12, T8 H6). The noise before it
        /// opens no key block (`BEGIN` read as `BEGAN`): a `BEGIN` with no
        /// `END` drops the rest of the output, a handle in it too —
        /// fail-closed, by design
        /// (`an_unterminated_key_block_fails_closed_and_terminates`; found here).
        #[test]
        fn a_held_handle_is_always_kept(
            before in arb_output().prop_map(|noise| noise.replace("BEGIN", "BEGAN")),
            after in arb_output(),
            hint in prop::sample::select(DEFAULT_SECRET_NAMES.to_vec()),
        ) {
            let mut aliases = Aliases::default();
            let handle = aliases.insert(hint, "Xk7pQ2mZ9vL4nR8tW1yB-held", "t", true).unwrap();
            let text = format!("{before} {hint}={handle} {after}");
            let out = redact(&text, &cfg(), &aliases).text;
            prop_assert!(out.contains(&handle), "{text}\n{out}");
        }

        /// The scrubber sits on the path everything takes to the wire and to
        /// disk. A panic here is not a viewer crashing — it is the last thing
        /// between a credential and a provider, dying.
        #[test]
        fn scrubbing_never_panics(text in arb_output()) {
            let out = redact(&text, &cfg(), &Aliases::default());
            // A `String` is valid UTF-8 by construction; this asserts we did not
            // build one by slicing mid-character (which would have panicked).
            prop_assert!(out.text.is_char_boundary(0));
        }

        /// **Redaction is idempotent.** Not decoration: `audit()` scrubs a report
        /// whose output was already scrubbed at capture, so a non-idempotent pass
        /// degrades real transcripts. It did — `password: [redacted: named
        /// secret]` became `password: [[redacted: named secret]: named secret]`,
        /// worse on every pass — because our own placeholder was re-matched by
        /// the name-directed rule.
        #[test]
        fn scrubbing_twice_changes_nothing(text in arb_output()) {
            let once = redact(&text, &cfg(), &Aliases::default()).text;
            let twice = redact(&once, &cfg(), &Aliases::default()).text;
            prop_assert_eq!(once, twice);
        }

        /// **A scrub by lines keeps every line** (review stop 16, E 1) — the
        /// four passes that can replace a newline (a held value, a key
        /// block whole or torn, a configured literal, a URL's userinfo),
        /// planted in output-shaped noise: as many newlines out as in, and
        /// as many lines, so a Read's numbers stay the file's.
        #[test]
        fn a_scrub_by_lines_keeps_every_line(
            before in arb_output(),
            after in arb_output(),
            planted in prop::sample::select(vec![
                crate::fakes::PEM_RSA_SHORT,
                crate::fakes::PEM_RSA_TRUNCATED,
                "x://user\nname@host",
                "confi\ngured value",
                "held\nvalue\n",
                "\nheld\nlead",
            ]),
        ) {
            let mut aliases = Aliases::default();
            aliases.insert("held", "held\nvalue\n", "t", true).unwrap();
            aliases.insert("lead", "\nheld\nlead", "t", true).unwrap();
            let cfg = SecretsConfig {
                extra: vec!["confi\ngured value".into()],
                ..cfg()
            };
            let text = format!("{before}{planted}{after}");
            let out = redact_lines(&text, &cfg, &aliases).text;
            prop_assert_eq!(
                out.matches('\n').count(),
                text.matches('\n').count(),
                "{:?}\n{:?}",
                text,
                out
            );
            prop_assert_eq!(out.lines().count(), text.lines().count());
        }

        /// **The core claim, over arbitrary surroundings.** A credential that
        /// begins at a token boundary must not survive, wherever it sits —
        /// including wrapped in text shaped like our own inert regions, which is
        /// exactly how the shape-based skip was defeated.
        ///
        /// A *delimiter* on each side is the honest precondition, not a
        /// convenience: see `a_credential_welded_into_a_longer_token_is_a_known
        /// _miss` for the class this deliberately does not claim.
        #[test]
        fn a_planted_credential_at_a_boundary_never_survives(
            before in arb_output(),
            delim in prop_oneof![
                Just(" ".to_string()), Just("=".to_string()), Just(":".to_string()),
                Just("\"".to_string()), Just("\n".to_string()), Just("'".to_string()),
            ],
            key in arb_secret(),
            after in arb_output(),
        ) {
            let text = format!("{before}{delim}{key} {after}");
            let out = redact(&text, &cfg(), &Aliases::default());
            prop_assert!(!out.text.contains(&key), "survived: {}", out.text);
        }

        /// The same, planted inside a handle-shaped wrapper an attacker chose.
        /// Skipping a region must depend on provenance, never on shape.
        #[test]
        fn a_credential_in_a_forged_inert_region_never_survives(key in arb_secret()) {
            for wrapper in [
                format!("{{{{{key}}}}}"),
                format!("{{{{{key}}}}}"),
                format!("[redacted: {key}]"),
                format!("[redacted: {key} ]"),
            ] {
                let out = redact(&wrapper, &cfg(), &Aliases::default());
                prop_assert!(!out.text.contains(&key), "survived: {}", out.text);
            }
        }

        /// Text carrying no credential comes back **byte-identical**. The whole
        /// design rests on this: a scrubber that mangles ordinary output gets
        /// switched off, and then it protects nothing.
        ///
        /// The generator is lowercase-with-digits on purpose. Mixed case *plus*
        /// digits *plus* 32 characters is the entropy heuristic's definition of
        /// key material, so generating that and calling it "ordinary" would be
        /// asserting the heuristic does not work.
        #[test]
        fn ordinary_text_is_returned_unchanged(text in "[a-z0-9 ./_-]{0,120}") {
            let out = redact(&text, &cfg(), &Aliases::default());
            prop_assert_eq!(&out.text, &text);
            prop_assert!(!out.any());
        }
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// **Scrubbing ordinary output stays linear.**
    ///
    /// `redact` runs on every command's captured output (`exec.rs`), and after
    /// the timeout has already wrapped the child — so time spent here is time
    /// no configured limit bounds. `our_own_span` scanned the whole remaining
    /// string for a closing `}}` while its caller advanced one character per
    /// failure, which is O(n²) on any text that opens `{{` without closing:
    /// minified JSON, LaTeX, JS. Measured before the fix: 2.02 s at 128k, 7.96 s
    /// at 256k (Reviewer B, round 3).
    ///
    /// A ceiling with a margin the mutation **produces**: unbounding the window
    /// runs this input in **10.96 s** where the fixed scan takes **0.07 s**,
    /// against a 500 ms ceiling.
    /// **Both fail-closed arms of the PEM walk, and that it terminates.**
    ///
    /// `redact_pem_blocks` drops the remainder when the END armour is missing
    /// or unterminated. Neither arm was pinned: replacing either with "emit the
    /// rest" left all 1372 tests green, and the outer one is worse than it
    /// looks — emitting the rest without advancing `rest` is an **infinite
    /// loop**, on `exec`'s always-on path, after the timeout stops applying
    /// (Reviewer E, round 3).
    ///
    /// The outer arm is reachable in ordinary use: our own 64 KiB output cap
    /// truncates a long key mid-block.
    #[test]
    fn an_unterminated_key_block_fails_closed_and_terminates() {
        let aliases = Aliases::default();
        let cfg = SecretsConfig::default();
        for (what, text) in [
            (
                "no END at all — what our own output cap produces",
                format!(
                    "before\n{}\n{}\n",
                    crate::fakes::PEM_BEGIN_PRIVATE,
                    "A".repeat(200)
                ),
            ),
            (
                "END armour with no closing dashes",
                format!(
                    "before\n{}\n{}\n-----END PRIVATE KEY\ntail",
                    crate::fakes::PEM_BEGIN_PRIVATE,
                    "A".repeat(200)
                ),
            ),
        ] {
            // **On its own thread, with a deadline.** The mutation this pins
            // is an infinite loop, and a test that hangs is a worse citizen
            // than one that fails: it takes the whole suite with it and reads
            // as a stuck machine rather than as a finding.
            let (tx, rx) = std::sync::mpsc::channel();
            let (t, c, a) = (text.clone(), cfg.clone(), aliases.clone());
            std::thread::spawn(move || {
                let _ = tx.send(redact(&t, &c, &a));
            });
            let out = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("{what}: the walk did not terminate"));
            assert!(
                out.text.contains("[redacted: private key]"),
                "{what}: the block must be replaced: {}",
                out.text
            );
            assert!(
                !out.text.contains(&"A".repeat(200)),
                "{what}: key material survived: {}",
                out.text
            );
            assert!(
                !out.text.contains("PRIVATE KEY-----\n"),
                "{what}: armour survived, which is how the body did last time"
            );
        }
    }

    /// **The token is redacted, not the word `Bearer`.**
    ///
    /// The name-directed pass fired on `Authorization:` and scrubbed the very
    /// next token — the scheme word — leaving the credential in the clear under
    /// a `[redacted: …]` marker one field to its left. That is worse than a
    /// miss: output that looks scrubbed and is not makes a reader stop looking.
    ///
    /// Asserted on BOTH halves, because "something was redacted" was true of the
    /// bug ([[feedback_assert_the_subject]]).
    #[test]
    fn an_auth_header_redacts_the_credential_not_the_scheme() {
        let cfg = SecretsConfig::default();
        let aliases = Aliases::default();
        for (scheme, secret) in [
            ("Bearer", "abcdef1234567890abcdef1234567890"),
            ("Basic", "dXNlcjpodW50ZXIyOm5vdGFyZWFsc2VjcmV0"),
            ("Token", crate::fakes::HEX_32),
        ] {
            let line = format!("Authorization: {scheme} {secret}");
            let out = redact(&line, &cfg, &aliases);
            assert!(
                !out.text.contains(secret),
                "{scheme}: the credential survived: {}",
                out.text
            );
            assert!(
                out.text.contains(scheme),
                "{scheme}: the scheme is not a secret and reading it is how you \
                 know what kind of credential it was: {}",
                out.text
            );
        }

        // A scheme word on its own is not a credential, and prose after a
        // secret NAME still does not qualify — the delimiter rule that keeps
        // "the password is stored in Vault" intact is unchanged.
        for plain in [
            "supported schemes: Bearer and Basic",
            "the password is stored in Vault",
        ] {
            assert_eq!(
                redact(plain, &cfg, &aliases).text,
                plain,
                "prose must survive intact"
            );
        }
    }

    /// **A credential inside a URL is a credential.**
    ///
    /// Measured on a realistic `.env`: the passes around this one caught five of
    /// six secrets, and **every URL-embedded credential survived** — including a
    /// `ghp_` GitHub token, because the issuer-prefix scanner anchors at a token
    /// start and the whole URL is one token. This is the layer that generalises:
    /// it does not care whether the file was read by `cat`, `xargs`, a glob or a
    /// symlink, where the permission gate has to recognise each spelling.
    #[test]
    fn a_credential_inside_a_url_does_not_survive() {
        let cfg = SecretsConfig::default();
        let aliases = Aliases::default();
        // The shapes are in the fakes (`CREDENTIALS_IN_URLS`).
        for &(what, line, secret) in crate::fakes::CREDENTIALS_IN_URLS {
            let out = redact(line, &cfg, &aliases);
            assert!(
                !out.text.contains(secret),
                "{what}: the credential survived: {}",
                out.text
            );
            // The HOST stays: which service was configured is usually the point,
            // and redacting the whole URL costs debugging value for no safety.
            assert!(
                out.text.contains("@"),
                "{what}: the host must survive: {}",
                out.text
            );
        }

        // **And an ordinary URL is untouched**, which is nearly all of them.
        // A rule that fires on every link would be noise, and noise is what
        // makes a real redaction unreadable.
        for plain in [
            "see https://github.com/owner/repo/blob/main/README.md",
            "GET http://localhost:8080/health -> 200",
            "curl https://api.example.com/v1/items?q=1#frag",
            // An `@` AFTER the path is not userinfo.
            "https://example.com/users/me@example.com",
        ] {
            let out = redact(plain, &cfg, &aliases);
            assert_eq!(out.text, plain, "an ordinary URL must not be rewritten");
        }
    }

    /// **A long run of one punctuation mark is not quadratic either.**
    ///
    /// `in_punctuation_run` counted the whole run in both directions from every
    /// character of it, and the main scan calls it once per character: 608 ms at
    /// 32k dashes, growing four-fold per doubling (Reviewer D). A markdown rule,
    /// a log divider or PEM armour is exactly this shape.
    ///
    /// Margin the mutation **produces**: counting the full run takes
    /// **258.78 s** on this input where the bounded walk takes ~0.02 s, against
    /// a 500 ms ceiling.
    #[test]
    fn scrubbing_a_long_punctuation_run_does_not_go_quadratic() {
        let aliases = Aliases::default();
        let cfg = SecretsConfig::default();
        let text = "-".repeat(128_000);
        let start = std::time::Instant::now();
        let out = redact(&text, &cfg, &aliases);
        let elapsed = start.elapsed();
        assert_eq!(out.text, text, "a row of dashes is not a secret");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "128k dashes took {elapsed:?} — the run scan is unbounded again"
        );
    }

    #[test]
    fn scrubbing_brace_soup_does_not_go_quadratic() {
        let aliases = Aliases::default();
        let cfg = SecretsConfig::default();
        assert!(cfg.redact, "premise: the default policy actually scrubs");
        let text = "{".repeat(128_000);
        let start = std::time::Instant::now();
        let out = redact(&text, &cfg, &aliases);
        let elapsed = start.elapsed();
        assert_eq!(out.text, text, "nothing in it is a secret");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "128k of open braces took {elapsed:?} — the scan is unbounded again"
        );
    }
}
