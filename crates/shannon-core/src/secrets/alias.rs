//! The substitution table: local values out as names, names back in as values.
//!
//! See `docs/DesignDocs/secrets.md` §3a and §4. This is the general mechanism the
//! first three phases were special cases of — *everything crossing the
//! machine/wire boundary passes through one table.* Outbound, local values become
//! names; inbound, names become local values.
//!
//! ```text
//!   GITHUB_TOKEN={{github-token.4f19}}   ← what the model sees
//!   cd {{a:repo}} && cargo test                 ← what the model writes
//! ```
//!
//! # Sensitivity is its own axis, and it is the one that gates
//!
//! Folding "is it secret" into "is it aliased" is what kept this design stuck on
//! credentials. Separated, the same code serves three needs:
//!
//! | need | example | sensitive | why alias it |
//! |---|---|---|---|
//! | credentials | `GITHUB_TOKEN` | yes | it must not be seen |
//! | privacy | `/home/sam`, hostnames, the program names in `ps` output | no | not our business to ship upstream |
//! | token economy | a 60-char path appearing 400 times in a build log | no | repeating it is expensive |
//!
//! The third is not a bonus, it is evidence the abstraction found the real joint:
//! a substitution that saves tokens *and* protects privacy with one mechanism,
//! and that would be worth having with no security argument at all.
//!
//! **Only sensitivity gates.** A [secret](Alias::sensitive) faces a human,
//! defeats an `allow` rule, and refuses when unknown. A plain alias expands
//! silently, because expanding it grants no authority the machine did not already
//! have — the resulting command still meets the permission gate on its own
//! effect. Gating plain aliases would train people to dismiss prompts, applied to
//! the one mechanism whose value depends on being invisible.
//!
//! # Where each kind is resolved, and why they differ
//!
//! - **Plain aliases resolve at the GATE**, before analysis. The human then sees
//!   the command that will really run, and an alias cannot launder a path past a
//!   path-scoped `deny` rule.
//! - **Secrets resolve at EXEC**, after the gate, because the whole point is that
//!   the value is never displayed, recorded, or logged.
//!
//! Same axis, opposite answer, for the same reason in both cases: show the human
//! everything that is safe to show, and nothing that is not.
//!
//! # The nonce belongs exactly where the capability is
//!
//! A secret's handle carries a per-session nonce derived as
//! `blake3(session seed ‖ name)`. The adversary is prompt injection through tool
//! output, authored by someone who cannot see this session: with predictable
//! handles they write `{{github-token}}` blind into a poisoned README and
//! it resolves. Deriving rather than drawing makes "same name, same handle all
//! session" free, so the model's reasoning does not fragment.
//!
//! Plain aliases carry **no nonce, deliberately**. There is no capability to
//! protect, and a guessable `{{a:repo}}` is one a human can type.
//! *Unguessable where it grants something; memorable where it does not.*
//!
//! # Why `{{…}}`
//!
//! It must survive two parsers unchanged. Bash leaves `{{a:x}}` alone (brace
//! expansion needs a comma or a `..` range inside, and names are restricted to
//! `[a-z0-9-]` so neither can appear), and our own analyzer treats a `{`-bearing
//! token as an **unresolved operand** — which is exactly right, and gives §6's "a
//! handle is an unresolved operand, so trace it" for free. The `alias:`/`secret:`
//! prefix also keeps us clear of the `{{name}}` templating in Handlebars, Jinja
//! and Go templates, which absolutely does appear in build output.
//!
//! # What this does not solve
//!
//! For secrets it moves the risk rather than removing it: from *"the model sees
//! the secret"* to *"the model can spend the secret"* — the confused deputy. The
//! value stays out of context, out of the transcript, and out of anyone's
//! retention window, which is a large win; claiming more would be the dishonesty
//! this project exists to avoid. Bounding *where* a secret may be spent is
//! audience binding (§6.2, P4).
//!
//! # Honest limit: `ps`
//!
//! Secret expansion substitutes the value into the command line handed to the
//! shell, so it is visible in `ps` to other processes of the same user for the
//! duration of the run. Passing it through the environment instead would hide it
//! there, but would silently break whenever the handle sits inside single quotes
//! — a wrong-behaviour trap in exchange for a same-user disclosure. Documented
//! rather than papered over; see the docket for the env-injection variant.

use super::redact::Lines;
use crate::message::{Block, Message};
use crate::secret::Secret;

/// The wire form of the home directory.
///
/// **Not a minted handle — a canonical form that already exists.** `~` is what a
/// portable path looks like, in every shell and to every model, so aliasing
/// `$HOME` to it makes what we send *more* correct rather than merely shorter:
/// an absolute `/home/sam/...` is machine-specific, true nowhere else, and
/// teaching it to a model is teaching a fact that does not travel (Steven,
/// 2026-07-26: *"it makes it portable. Using a full path name does more harm
/// than good"*).
///
/// The privacy and token wins come along free, which is the §3a test for whether
/// an abstraction found the real joint.
const HOME_HANDLE: &str = "~";

/// Opening delimiter of a **secret** handle — a capability.
///
/// Bare `{{`, so a secret reads `{{secret.2864}}` or `{{github-token.7f1a}}`.
/// The *plain* alias carries the marker instead ([`PLAIN_OPEN`]), because
/// secrets are the ones a human meets in a resumed transcript and the ones the
/// model is told about — they should be the clean form, not the decorated one.
///
/// Terse on purpose. The audience for a handle is the **model**, not a human:
/// since marking stopped rewriting the buffer, a person sees their own text
/// styled and only meets a handle in a resumed transcript. And a model needs
/// far less ceremony than `{{…}}` to understand a placeholder — Steven,
/// 2026-07-26: *"the LLM is smart enough to understand something simple… we
/// don't need to be super complicated."*
///
/// So the ceremony shrank and the nonce halved. §3c is the reason to care: a
/// handle is paid for in tokens, and the nonce is the expensive half because
/// random hex is exactly what tokenises worst.
const SECRET_OPEN: &str = "{{";
/// Opening delimiter of a **plain** alias — a convenience.
///
/// Carries the marker so that a **table-free** scanner can still tell the two
/// apart: the permission gate must be able to say "this line spends a secret"
/// without holding any, and an *unknown* handle has to reach a human too.
const PLAIN_OPEN: &str = "{{a:";
/// Closing delimiter, shared.
const CLOSE: &str = "}}";

/// **When an entry must be resolved** — the axis the "expand at the gate exactly
/// what a human cannot read" rule turns on.
///
/// Named as its own thing because it is *not* a synonym for sensitivity, and
/// keying the code on sensitivity instead is what forced `~` to be handled by a
/// bolted-on second pass. Two entries can disagree with `sensitive` in opposite
/// directions, and every future recognition mode (interned detections,
/// entropy-aliased noise) adds another.
/// **What a value is spliced into** (`Aliases::expand_for`): a shell line,
/// under the shell's rules (`~`, a value that would change the line), or a
/// file's text, under none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Splice {
    Shell,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveAt {
    /// Before the human reads the command. A minted name means nothing without
    /// us, so leaving it in place would put an unreadable token in front of the
    /// person approving — and would let an alias launder a path past a
    /// path-scoped `deny`.
    Gate,
    /// At the last moment before the shell sees the line. Two very different
    /// entries land here for opposite reasons, which is the sign the axis is cut
    /// at the right joint: a **secret** must never be displayed at all, and `~`
    /// is universally legible so expanding it early would buy the reader nothing
    /// and cost them a portable path.
    Exec,
}

/// One entry in the substitution table.
///
/// The value is private and reachable only through [`Secret::expose`], which is
/// what makes every read of it a greppable act; `Debug` is derived precisely
/// *because* [`Secret`] redacts itself, so the no-leak property is structural
/// rather than a hand-written impl someone can forget to update. Plain aliases
/// get the same treatment — not because their values are secret, but because a
/// uniform rule has no exception for someone to get wrong.
#[derive(Debug, Clone)]
pub struct Alias {
    /// What a human and the model read: `github-token`, `repo`.
    pub name: String,
    /// The full placeholder, including the session nonce when sensitive.
    pub handle: String,
    /// Where it came from, for the listing — `env GITHUB_TOKEN`, `config`.
    pub source: String,
    /// Whether spending this is dangerous. **The only thing that gates**, and
    /// the only thing that makes a substitution an *event* worth announcing.
    pub sensitive: bool,
    /// May a command expand this back into the real value?
    ///
    /// **Its own axis, and the default is NO for anything a user marked.** Two
    /// very different intents were being conflated:
    ///
    /// - *"the agent may USE this but never see it"* — a lent credential, where
    ///   spending is the entire point (`[secrets] env`);
    /// - *"this must not leave my machine"* — a password mentioned in passing,
    ///   a customer name, PII. Here spending was never asked for, and granting
    ///   it hands out a capability nobody wanted.
    ///
    /// The second is what `:secret` and the paste offer are for, and it is the
    /// commoner case. It also removes an unavoidable channel: a spendable secret
    /// can be *computed on* by an approved command whose output the model reads,
    /// so the model can derive facts about it a few bits at a time. That is
    /// inherent to spending — not a bug to fix — so the answer is to stop
    /// granting spend where nobody asked for it.
    pub spendable: bool,
    /// When this must be resolved. Independent of [`sensitive`](Alias::sensitive)
    /// — see [`ResolveAt`].
    pub resolve: ResolveAt,
    value: Secret,
}

impl Alias {
    /// The value, **for the user's own eyes** — the `:secrets` pane, which
    /// shows what was hidden so the user can check they marked the right
    /// thing. Steven, 2026-10-01: "we're not trying to hide the secrets from
    /// the user, we're trying to hide them from the agents."
    ///
    /// Named for its one audience so every read of a value onto a screen is a
    /// greppable, deliberate act (through [`Secret::expose`]). Four readers:
    /// in `App`, the pane; `:secret`'s completion, which compares against it
    /// so a value already marked is not offered again; and `:nosecret`'s,
    /// which offers it — and the clipboard's (`clipboard.rs`), which finds a
    /// shown value in what you copied so `tui.copy_secrets` can act on it. A
    /// pane, the wildmenu and your own clipboard are not a log, a notice, or
    /// the wire; nothing bound for those may call this.
    pub fn shown_to_user(&self) -> &str {
        self.value.expose()
    }

    /// Whether the user marked this (this run or an earlier one, kept) — as
    /// opposed to lent by `[secrets] env` or config, which the next launch
    /// lends again whatever this run does.
    pub fn is_marked(&self) -> bool {
        self.source.starts_with(MARKED_SOURCE)
    }

    /// **The variable a secret was lent from** — `[secrets] env`, or an
    /// `[[alias]]` with `env` and `sensitive` — or `None`: marked, from
    /// config, or not a secret at all (a plain alias's variable is no one's
    /// to hide).
    pub fn lent_from(&self) -> Option<&str> {
        self.source
            .strip_prefix(ENV_SOURCE)
            .filter(|_| self.sensitive)
    }
}

/// **How a held secret's handle is shown** in text the user reads
/// ([`Aliases::unmask_for_display`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmasked {
    /// Its value, in the secret's colour, as the user's own line shows a
    /// secret — a reply's, a command's output (Steven, 10-19: "In chats we
    /// should just show `hunter2`… No need to show the hash"). Where it was
    /// put back is said ([`Aliases::unmask_shown`]), so a value the model
    /// wrote itself — it had it — is drawn apart (review stop 6, F 5; Steven:
    /// "an underline along with a color"). A plain alias shows its
    /// value too, uncoloured, as on the user's own line (Steven, 10-19:
    /// "secrets and aliases follow the same rule").
    Value,
    /// `value (handle)` — the offline stub's, a test tool echoing what it was
    /// sent, so you see the handle is what went over the wire (Steven, 10-02:
    /// "`hunter2 ({{secret.cf4e}})` with both highlighted"; 10-19: "that
    /// notation _only_ in the stub").
    WithHandle,
}

impl Unmasked {
    /// For text `model` produced — the stub's echo with its handles, any
    /// other model's values alone. `model` is what answered (`canned-stub` for
    /// the stub, as a turn is sealed), never what was asked for.
    pub fn for_model(model: &str) -> Unmasked {
        match model == crate::backend::models::STUB {
            true => Unmasked::WithHandle,
            false => Unmasked::Value,
        }
    }
}

/// A session's substitution table: names out, values in.
///
/// Deliberately **not** `Serialize` and **not** part of `SecretsConfig` (which is
/// written to disk): a stale handle must never expand to a rotated secret, so the
/// mapping dies with the session.
#[derive(Clone, Default)]
pub struct Aliases {
    /// Session entropy, obtained on first use — and only ever for a *sensitive*
    /// entry, so a table of plain aliases costs no syscall and cannot fail.
    seed: Option<[u8; 32]>,
    entries: Vec<Alias>,
}

impl std::fmt::Debug for Aliases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secrets = self.entries.iter().filter(|a| a.sensitive).count();
        write!(
            f,
            "Aliases({} plain, {secrets} secret)",
            self.entries.len() - secrets
        )
    }
}

impl PartialEq for Aliases {
    /// Compares handles, never values: equality is asked by derived `PartialEq`
    /// on config-shaped structs, and a value comparison there would be both
    /// meaningless and a timing side channel.
    fn eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(a, b)| a.handle == b.handle)
    }
}

impl Eq for Aliases {}

impl Aliases {
    /// Build the session's table from config: `[[alias]]` entries first, then
    /// the `[secrets] env` shorthand.
    ///
    /// One constructor, so a source cannot be wired in one place and forgotten in
    /// another. Returns every entry that could **not** be created, with the
    /// reason — a declared alias that silently isn't there looks, from the user's
    /// chair, exactly like the feature not working, and sends them hunting in the
    /// wrong place.
    pub fn from_config(
        specs: &[crate::AliasSpec],
        env: &[String],
        alias_home: bool,
    ) -> (Aliases, Vec<String>) {
        Self::from_config_with(specs, env, alias_home, &|name| std::env::var(name).ok())
    }

    /// [`from_config`](Self::from_config) over `lookup` for the environment —
    /// the seam a test lends through without touching the process's own
    /// (setting a variable is process-wide, and `unsafe` since 2024). The
    /// `[secrets] env` ingest was tested by nothing (review stop 12, T8 M13).
    fn from_config_with(
        specs: &[crate::AliasSpec],
        env: &[String],
        alias_home: bool,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> (Aliases, Vec<String>) {
        let mut table = Aliases::default();
        let mut problems = Vec::new();
        for spec in specs {
            let sensitive = spec.is_sensitive();
            let (value, source) = match (&spec.value, &spec.env) {
                (Some(v), None) => (Some(v.clone()), "config".to_string()),
                (None, Some(name)) => (
                    lookup(name).filter(|v| !v.is_empty()),
                    format!("{ENV_SOURCE}{name}"),
                ),
                (Some(_), Some(_)) => {
                    problems.push(format!("{}: give either value or env, not both", spec.name));
                    continue;
                }
                (None, None) => {
                    problems.push(format!("{}: needs a value or an env", spec.name));
                    continue;
                }
            };
            let Some(value) = value else {
                problems.push(format!("{}: {source} is not set", spec.name));
                continue;
            };
            if let Err(why) = table.insert(&spec.name, &value, &source, sensitive) {
                problems.push(format!("{}: {why}", spec.name));
            }
        }
        for name in table.lend_env(env, lookup) {
            problems.push(format!("{name}: not set in this environment"));
        }
        // Last, so a user alias for a path *under* home is still the longer
        // value and therefore wins at mask time.
        if alias_home {
            table.alias_home();
        }
        (table, problems)
    }

    /// Alias `$HOME` to `~`, the shipped-on-by-default entry.
    ///
    /// Skipped silently when `$HOME` is unset or implausibly short — there is
    /// nothing the user could do about it and nothing is lost but brevity.
    fn alias_home(&mut self) {
        let Some(home) = super::home_dir() else {
            return;
        };
        self.entries.push(Alias {
            name: "home".to_string(),
            handle: HOME_HANDLE.to_string(),
            source: "$HOME".to_string(),
            sensitive: false,
            spendable: true,
            // Not sensitive, and yet resolved at exec — the entry that disagrees
            // with sensitivity in the direction that made the axis necessary.
            // `~` needs no expanding for a human to read it.
            resolve: ResolveAt::Exec,
            value: Secret::new(home),
        });
    }

    /// Lend the named environment variables as **secrets**.
    ///
    /// Returns the names that were **not set**, so the caller can say so: a
    /// declared secret that silently isn't there looks, from the user's chair,
    /// like the feature not working. Read through `lookup`
    /// ([`from_config_with`](Self::from_config_with)).
    fn lend_env(
        &mut self,
        names: &[String],
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<String> {
        let mut missing = Vec::new();
        for name in names {
            // Set but empty is not set. `insert` refuses an empty value too, and
            // its refusal is said the same way — so this guard is the reason
            // stated where it applies, not a second check (measured
            // equivalent, review stop 12).
            match lookup(name) {
                Some(value) if !value.is_empty() => {
                    // A failure to mint is reported the same way as absence: what
                    // matters is that the user learns the handle does not exist.
                    if self
                        .insert(name, &value, &format!("{ENV_SOURCE}{name}"), true)
                        .is_err()
                    {
                        missing.push(name.clone());
                    }
                }
                _ => missing.push(name.clone()),
            }
        }
        missing
    }

    /// **Every variable a secret was lent from** ([`Alias::lent_from`]) —
    /// what no command may be handed: lending gives the model the handle, and
    /// a variable left in a command's environment hands back the value
    /// (`echo $GITHUB_TOKEN`). Read by `exec::configured_env` (`[exec] env`
    /// may not name one) and by Claude Code's turn, whose `claude` inherits
    /// Shannon's environment (review stop 14, Reviewer E F1).
    pub fn lent_vars(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(Alias::lent_from)
            .map(str::to_string)
            .collect()
    }

    /// **Every variable of `env` whose value this table holds as a secret**,
    /// with that secret's handle — lent by `[secrets] env`, or marked (a
    /// paste, `:secret`) while your shell also exports it. Keyed on what the
    /// variable HOLDS, not on how the secret came: a token pasted into
    /// Shannon and exported as `GITHUB_TOKEN` stayed in `claude`'s
    /// environment, and `echo $GITHUB_TOKEN` read it, allowed (the close of
    /// review stop 14, verifier V 3).
    pub fn held_vars(
        &self,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<(String, String)> {
        env.into_iter()
            .filter_map(|(name, value)| {
                let held = self.secret_holding(&value)?;
                Some((name, held.handle.clone()))
            })
            .collect()
    }

    /// Take a value into the table under `name`, returning its handle.
    ///
    /// Re-inserting the same name replaces the value and keeps the handle, so a
    /// rotated secret does not fragment the model's reasoning mid-session.
    pub fn insert(
        &mut self,
        name: &str,
        value: &str,
        source: &str,
        sensitive: bool,
    ) -> Result<String, String> {
        // Spendable by default: a plain alias must always expand (that is its
        // whole job), and a secret named in `[secrets] env` was lent precisely
        // so it could be used. Only "keep this off the wire" opts out, and it
        // says so at its call site.
        self.insert_with(name, value, source, sensitive, true)
    }

    /// [`insert`](Aliases::insert), choosing whether a command may expand it.
    ///
    /// Separate constructor rather than a flag on the common one: the *intent*
    /// differs between "lent for use" and "kept off the wire", and a call site
    /// should have to say which.
    pub fn insert_with(
        &mut self,
        name: &str,
        value: &str,
        source: &str,
        sensitive: bool,
        spendable: bool,
    ) -> Result<String, String> {
        let name = sanitize_name(name);
        if name.is_empty() {
            return Err("an alias needs a name with at least one letter or digit".to_string());
        }
        // **An empty value is not an alias.** Masking searches the output for
        // this text; an empty needle is found everywhere and consumes nothing,
        // so `mask` spun forever and grew the output without bound — a hung TUI
        // with no panic and no message. The `env` path already filtered this
        // (`from_config`); the config-value path did not, so `value = ""` in
        // `[[alias]]` was a one-line way to wedge the program.
        //
        // Refused here rather than silently skipped: the user wrote a line that
        // cannot mean anything, and a rule that parses into no rule is the
        // defect `Rule::parse` refuses for the same reason.
        if value.is_empty() {
            return Err(format!(
                "alias `{name}` has an empty value — there would be nothing to mask"
            ));
        }
        let handle = if sensitive {
            let seed = self.seed()?;
            format!("{SECRET_OPEN}{name}.{}{CLOSE}", nonce(&seed, &name))
        } else {
            format!("{PLAIN_OPEN}{name}{CLOSE}")
        };
        if let Some(existing) = self.entries.iter_mut().find(|e| e.name == name) {
            existing.value = Secret::new(value);
            existing.source = source.to_string();
            existing.sensitive = sensitive;
            existing.spendable = spendable;
            existing.resolve = if sensitive {
                ResolveAt::Exec
            } else {
                ResolveAt::Gate
            };
            existing.handle = handle.clone();
            return Ok(handle);
        }
        self.entries.push(Alias {
            name,
            handle: handle.clone(),
            source: source.to_string(),
            sensitive,
            spendable,
            // A minted name is unreadable without us, so it must resolve before
            // a human is asked to approve it — unless it is a secret, whose
            // whole point is never being shown.
            resolve: if sensitive {
                ResolveAt::Exec
            } else {
                ResolveAt::Gate
            },
            value: Secret::new(value),
        });
        Ok(handle)
    }

    /// Every entry, for listing to the user. Values are reachable only through
    /// [`Secret::expose`]; the one display door is [`Alias::shown_to_user`],
    /// for the user's own eyes.
    pub fn entries(&self) -> &[Alias] {
        &self.entries
    }

    /// The handle a secret named `name` would get this run — so a caller can
    /// check it against a vault BEFORE minting it. The nonce is 16 bits (four
    /// hex characters, `NONCE_HEX`), so across months of runs the same name
    /// meets the same handle; a vault keyed by handle must not let a later
    /// mark overwrite an earlier one, and this is how the caller finds out in
    /// time.
    pub fn secret_handle(&mut self, name: &str) -> Result<String, String> {
        let name = sanitize_name(name);
        if name.is_empty() {
            return Err("an alias needs a name with at least one letter or digit".to_string());
        }
        let seed = self.seed()?;
        Ok(format!(
            "{SECRET_OPEN}{name}.{}{CLOSE}",
            nonce(&seed, &name)
        ))
    }

    /// Put back a secret **kept from an earlier run**, under exactly the
    /// handle its history holds — never re-derived, and never matched by name:
    /// a handle nobody minted is the shape of an injection, and the exact
    /// string is the whole of the nonce's protection.
    ///
    /// A handle this run already holds for the SAME value is [`Restored::Already`].
    /// One it holds for a DIFFERENT value — a 16-bit nonce colliding with a
    /// secret lent this run — is [`Restored::Conflict`] and changes nothing:
    /// the old history's handle must not silently come to mean another secret.
    /// A name in use this run gets a suffix; the handle is what resolves.
    pub fn restore(&mut self, handle: &str, kept: &super::Kept) -> Result<Restored, String> {
        if !is_secret_shaped(handle) {
            return Err(format!("{handle} is not shaped like a secret's handle"));
        }
        if let Some(held) = self.entries.iter().find(|e| e.handle == handle) {
            return Ok(if held.value.expose() == kept.value.expose() {
                Restored::Already
            } else {
                Restored::Conflict
            });
        }
        if kept.value.expose().is_empty() {
            return Err(format!("{handle} was kept with no value"));
        }
        let base = match sanitize_name(&kept.name) {
            n if n.is_empty() => "secret".to_string(),
            n => n,
        };
        let taken = |n: &str| self.entries.iter().any(|e| e.name == n);
        let name = std::iter::once(base.clone())
            .chain((2..).map(|k| format!("{base}-{k}")))
            .find(|n| !taken(n))
            .expect("an unbounded sequence has a free name");
        self.entries.push(Alias {
            name,
            handle: handle.to_string(),
            source: KEPT_SOURCE.to_string(),
            sensitive: true,
            spendable: kept.spendable,
            resolve: ResolveAt::Exec,
            value: kept.value.clone(),
        });
        Ok(Restored::Added)
    }

    /// Take secrets off the table, named by a handle or by a value — the two
    /// things `:secrets` shows — and return them; empty when no secret is
    /// either. From then on each handle resolves to nothing (a command naming
    /// it is refused as unknown) and the value is no longer masked or scrubbed.
    /// Plain aliases are not secrets and are never taken.
    ///
    /// **By value, every secret holding it.** One value can sit under two
    /// handles — lent by `[secrets] env` and also pasted, or restored from an
    /// earlier run and marked again — and taking only the first left the value
    /// masked while the notice said it no longer was (Reviewers A and B).
    pub fn unmark(&mut self, key: &str) -> Vec<Alias> {
        let (taken, kept): (Vec<Alias>, Vec<Alias>) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|a| a.sensitive && (a.handle == key || a.value.expose() == key));
        self.entries = kept;
        taken
    }

    /// **What a turn records as its provenance** (`Provenance::handles`): the
    /// handles `messages` name that were really held — each once, in order —
    /// read from every part that carries text, a tool's output included (a
    /// value a command printed was masked to its handle, and that handle is
    /// ours). Recall later asks the vault for exactly these, so a guess
    /// planted in a file, never held, is never recorded and recall cannot be
    /// walked by guessing.
    ///
    /// Held means one of two things:
    /// - **a secret YOU marked, in this table** — a lent `[secrets] env` or
    ///   config secret is lent again by the next launch, never kept, so
    ///   asking the vault for it would only spend a lookup (Reviewers A and
    ///   B, round 19);
    /// - **`vouched`**: what the turns it was composed from recorded
    ///   (`Graph::recorded_handles`). A return, summary, edit or continuation
    ///   carries its sources' handles, and in a later run this table may not
    ///   hold them until something recalls them. A turn sealed live has no
    ///   sources and passes `&[]`.
    pub fn handles_to_record(&self, messages: &[Message], vouched: &[String]) -> Vec<String> {
        let held = |handle: &str| {
            self.secrets().any(|a| a.is_marked() && a.handle == handle)
                || vouched.iter().any(|v| v == handle)
        };
        let mut out: Vec<String> = Vec::new();
        let mut note = |text: &str| {
            for handle in secret_handles_in(text) {
                if held(handle) && !out.iter().any(|o| o == handle) {
                    out.push(handle.to_string());
                }
            }
        };
        for message in messages {
            note(&message.content);
            for block in &message.blocks {
                match block {
                    Block::Text { text } | Block::Context { text, .. } => note(text),
                    Block::Thinking { thinking, .. } => note(thinking),
                    Block::ToolUse { input, .. } => note(&input.to_string()),
                    Block::ToolResult { content, .. } => note(content),
                    // Encrypted: no handle can be read in it.
                    Block::RedactedThinking { .. } => {}
                }
            }
        }
        out
    }

    /// The secret holding `value`, if one does — so marking a value already
    /// held answers with its handle instead of minting a second one.
    pub fn secret_holding(&self, value: &str) -> Option<&Alias> {
        self.secrets().find(|a| a.value.expose() == value)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every secret, spendable or not.
    pub fn secrets(&self) -> impl Iterator<Item = &Alias> {
        self.entries.iter().filter(|a| a.sensitive)
    }

    /// **What [`secret_ranges`](Self::secret_ranges) finds, as one number**
    /// — every held value and its handle, hashed: equal for two tables that
    /// find the same, so a cache of what one found knows when to ask again
    /// (a transcript's shut rows: `view::hides_a_leak`). Keyed once per
    /// process, at random, so the number says nothing of a value outside
    /// the run that made it; it is never written anywhere.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{BuildHasher, Hash, Hasher};
        static KEYS: std::sync::OnceLock<std::collections::hash_map::RandomState> =
            std::sync::OnceLock::new();
        let mut hasher = KEYS
            .get_or_init(std::collections::hash_map::RandomState::new)
            .build_hasher();
        for alias in self.secrets() {
            alias.value.expose().hash(&mut hasher);
            alias.handle.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// **The secrets among `names`** — of what a [`mask`](Self::mask)
    /// matched, the entries that are secrets: what "withheld" may be said
    /// of. A plain alias — the home's `~`, an `[[alias]]` path — is a
    /// spelling, nothing kept from the model, and "1 secret withheld (home)"
    /// was said of a conversation holding none (Steven, 10-23, image 7).
    /// Read by every sentence that says what was withheld (`App::
    /// say_withheld`, `:Send`'s, `:child`'s).
    pub fn secrets_among(&self, names: &[String]) -> Vec<String> {
        (names.iter())
            .filter(|name| self.secrets().any(|s| &s.name == *name))
            .cloned()
            .collect()
    }

    /// The secrets a command may actually expand — what the model is told it can
    /// use. Telling it about the others would only invite a wasted turn.
    pub fn spendable(&self) -> impl Iterator<Item = &Alias> {
        self.entries.iter().filter(|a| a.sensitive && a.spendable)
    }

    /// What can be said about a held value **without sending it**.
    ///
    /// The first layer of the "I need the model to reason about this" escape
    /// hatch (`secrets.md` §10a): almost every real question about a secret — is
    /// it strong, is it the right format, was it truncated — is a question about
    /// its *properties*, and those can be computed here and sent as facts. Only
    /// the questions that genuinely need the characters themselves fall through
    /// to the irreversible option.
    pub fn describe(&self, name: &str, cfg: &crate::SecretsConfig) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| super::describe(e.value.expose(), cfg))
    }

    /// Show every handle we minted as `how` says, for **display only** — a
    /// marked secret's and a plain alias's alike (Steven, 10-19: "secrets and
    /// aliases follow the same rule").
    ///
    /// The other half of the boundary. The model receives handles, so it echoes
    /// handles back — and a user reading `{{secret.2864}}` in a reply is being
    /// shown our plumbing instead of their conversation. So a reply shows the
    /// value, as the user's own line does ([`Unmasked::Value`]), a secret's in
    /// its colour ([`secret_ranges`](Self::secret_ranges)); the offline stub's
    /// echo shows `value (handle)` ([`Unmasked::WithHandle`]). A user's own
    /// line is always `Value`: the conversation stores what was SENT (a marked
    /// value as its handle, a plain alias's value as `{{a:name}}`), and "the
    /// input should still be the literal input" (Steven, 2026-10-02). This was
    /// two functions — the line's resolving aliases too, a reply's secrets
    /// alone — until the reply's rule became the line's (review stop 6, C 8).
    ///
    /// Only exact handles we hold resolve, each once
    /// ([`unmask_shown`](Self::unmask_shown)).
    ///
    /// **Never call this on anything that is stored or sent.** The conversation
    /// keeps the handle, which is what makes the transcript on disk safe and
    /// what lets a resumed session — which no longer holds the values — stay
    /// honest about what it does not know.
    pub fn unmask_for_display(&self, text: &str, how: Unmasked) -> String {
        self.unmask_shown(text, how).0
    }

    /// [`unmask_for_display`](Self::unmask_for_display), and **where in what
    /// it returns each held SECRET was put back** — char ranges, ascending —
    /// so a held value in model-written text that is not one of them is one
    /// the model wrote itself: it had the value (review stop 6, F 5; drawn
    /// apart by `view::row_secrets`). Positions, never values, as
    /// [`secret_ranges`](Self::secret_ranges) gives.
    ///
    /// One left-to-right pass, because a replacement is never read again.
    /// Replacing entry by entry, a value that happened to contain another
    /// entry's handle had that handle resolved inside it, and `value (handle)`
    /// relied on table order not to resolve its own parentheses a second time
    /// (Reviewer B, round 19). The longest handle at a position wins, as in
    /// [`mask`](Self::mask).
    ///
    /// Minted handles only ([`minted`](Self::minted)) — which is also what
    /// guarantees progress: an empty handle (none can be built, but this runs
    /// every frame) would match at every position and never advance.
    pub fn unmask_shown(&self, text: &str, how: Unmasked) -> (String, Vec<(usize, usize)>) {
        let mut out = String::with_capacity(text.len());
        let mut shown = Vec::new();
        // Chars in `out` so far: the ranges are in chars, as every
        // renderer's are.
        let mut at_char = 0;
        let mut rest = text;
        while let Some(at) = rest.find(SECRET_OPEN) {
            let before = &rest[..at];
            out.push_str(before);
            at_char += before.chars().count();
            let here = &rest[at..];
            let hit = self
                .minted()
                .filter(|e| here.starts_with(&*e.handle))
                .max_by_key(|e| e.handle.len());
            match hit {
                Some(entry) => {
                    let put = match how {
                        Unmasked::Value => entry.value.expose().to_string(),
                        Unmasked::WithHandle => {
                            format!("{} ({})", entry.value.expose(), entry.handle)
                        }
                    };
                    let len = put.chars().count();
                    if entry.sensitive {
                        shown.push((at_char, at_char + len));
                    }
                    out.push_str(&put);
                    at_char += len;
                    rest = &here[entry.handle.len()..];
                }
                None => {
                    // One byte (`{` is ASCII, so still a char boundary), not
                    // the whole opener: a handle can start inside `{{{`.
                    out.push('{');
                    at_char += 1;
                    rest = &here[1..];
                }
            }
        }
        out.push_str(rest);
        (out, shown)
    }

    /// **Every entry whose handle we minted** — `{{…}}`, a secret's or a
    /// plain alias's — and not the home form `~`: a canonical spelling the
    /// user types too, so putting it back would rewrite what they wrote
    /// rather than restore it. What display resolves
    /// ([`unmask_shown`](Self::unmask_shown)), and so what a stream holds a
    /// tail back for (`App::unmask_stream`).
    pub fn minted(&self) -> impl Iterator<Item = &Alias> {
        self.entries
            .iter()
            .filter(|e| e.handle.starts_with(SECRET_OPEN))
    }

    /// The longest handle we could mint, so a streaming reader knows how much
    /// tail to hold back while waiting for one to complete.
    pub fn longest_handle(&self) -> usize {
        self.entries
            .iter()
            .map(|e| e.handle.len())
            .max()
            .unwrap_or(0)
    }

    /// Character ranges in `text` covered by a secret we hold — its value, or
    /// its handle: the display shows the value, and the stub's echo the
    /// handle beside it (`hunter2 ({{secret.cf4e}})`, see
    /// [`unmask_for_display`](Self::unmask_for_display)); both are styled.
    ///
    /// For **styling**, which is why it returns positions and never values:
    /// `expose` stays inside this module, and a renderer cannot accidentally
    /// come to hold a credential just because it wanted to colour one.
    ///
    /// Longest first, so an overlapping pair highlights as the more specific
    /// one — the same ordering rule as [`mask`](Aliases::mask).
    ///
    /// **Linear in the line, not quadratic.** This compared a slice at every
    /// char for every needle and checked each hit against every run so far,
    /// and a long line — a minified file a tool printed — cost 12–100 ms a
    /// frame (Reviewer B, round 19). Now: `str::find` per needle, an
    /// already-styled byte map, and one walk turning bytes into chars. A hit
    /// that overlaps a styled region steps on by one char, so a later,
    /// overlapping hit of the same needle is still found — the old scan's
    /// rule, pinned against it by `secret_ranges_match_the_per_char_scan`.
    pub fn secret_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        let mut order: Vec<&str> = self
            .entries
            .iter()
            .filter(|a| a.sensitive)
            .flat_map(|a| [a.value.expose(), a.handle.as_str()])
            .filter(|needle| !needle.is_empty())
            .collect();
        order.sort_by_key(|needle| std::cmp::Reverse(needle.len()));
        // Byte ranges first; allocated only once something matches, so a line
        // without a secret — nearly every line — costs the searches alone.
        let mut styled: Vec<bool> = Vec::new();
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for needle in order {
            let mut from = 0;
            while let Some(found) = text[from..].find(needle) {
                let (lo, hi) = (from + found, from + found + needle.len());
                if styled.is_empty() {
                    styled = vec![false; text.len()];
                }
                // An already-styled region wins; overlapping highlights would
                // render as a muddle rather than as two facts.
                if styled[lo..hi].contains(&true) {
                    let step = text[lo..].chars().next().map_or(1, char::len_utf8);
                    from = lo + step;
                    continue;
                }
                styled[lo..hi].fill(true);
                runs.push((lo, hi));
                from = hi;
            }
        }
        runs.sort_unstable();
        // Disjoint and sorted, so every bound is at or after the last: one
        // walk over the text converts them all.
        let (mut at, mut chars) = (0, 0);
        let mut to_char = |byte: usize| {
            chars += text[at..byte].chars().count();
            at = byte;
            chars
        };
        runs.into_iter()
            .map(|(lo, hi)| (to_char(lo), to_char(hi)))
            .collect()
    }

    /// Is this handle one we minted?
    pub fn knows(&self, handle: &str) -> bool {
        self.entries.iter().any(|a| a.handle == handle)
    }

    /// Resolve everything due **before the human reads the command**.
    ///
    /// So the pane shows a command whose every part means something to the person
    /// approving it, and so an alias cannot launder a path past a path-scoped
    /// `deny` rule.
    pub fn expand_at_gate(&self, text: &str) -> Result<Expansion, String> {
        self.expand_for(text, ResolveAt::Gate, Splice::Shell)
    }

    /// Resolve everything due **at the last moment before the shell sees it**:
    /// secrets, which must never be displayed, and canonical forms like `~`,
    /// which need no displaying.
    pub fn expand_at_exec(&self, text: &str) -> Result<Expansion, String> {
        self.expand_for(text, ResolveAt::Exec, Splice::Shell)
    }

    /// **`text`'s secret handles, each its value, for a file** (S21-Q27,
    /// Steven: "as bash") — a Write's content, an Edit's strings: the secret
    /// pass [`expand_at_exec`](Self::expand_at_exec) makes for a command,
    /// refusing as it does an unknown handle and one kept off the wire; but
    /// none of the shell's rules. `~` is text in a file, and a value's quote
    /// or newline changes no line a shell reads: what you approved is the
    /// file with the value where the handle is.
    pub fn put_in_file(&self, text: &str) -> Result<Expansion, String> {
        self.expand_for(text, ResolveAt::Exec, Splice::File)
    }

    /// Resolve the entries due at `at`, and no others.
    ///
    /// **Fails closed on an unknown handle in a namespace this pass owns.** The
    /// design's first sketch had an unrecognised handle expand to nothing; that is
    /// worse than refusing, because `rm -rf {{x}}/` would become
    /// `rm -rf /`. An unresolvable handle is either a mistake or an injection
    /// attempt, and neither should run.
    fn expand_for(&self, text: &str, at: ResolveAt, into: Splice) -> Result<Expansion, String> {
        let mut spent = Vec::new();

        // Canonical forms first, by their own matcher. `~` is shell syntax rather
        // than a token we minted, so it is recognised by shell rules, not by a
        // delimiter scan (see `expand_tilde`). Which PASS it belongs to is now
        // decided by `resolve` like everything else — that is the special case
        // the axis dissolved. Not in a file: there `~` is text.
        let mut text = std::borrow::Cow::Borrowed(text);
        let shell = (self.entries.iter()).filter(|_| into == Splice::Shell);
        for entry in shell.filter(|e| e.resolve == at) {
            if entry.handle != HOME_HANDLE {
                continue;
            }
            let expanded = expand_tilde(&text, entry.value.expose());
            if expanded != *text {
                if entry.sensitive {
                    spent.push(entry.name.clone());
                }
                text = std::borrow::Cow::Owned(expanded);
            }
        }

        // Then minted handles, whose namespace says which pass owns them.
        let open = match at {
            ResolveAt::Gate => PLAIN_OPEN,
            ResolveAt::Exec => SECRET_OPEN,
        };
        let mut out = String::with_capacity(text.len());
        let mut rest: &str = &text;
        while let Some(start) = rest.find(open) {
            let after = &rest[start..];
            let Some(end) = after.find(CLOSE) else {
                // An unterminated handle is not a handle; leave it as text. It
                // has still faced a human, because the gate keys on the opening
                // delimiter rather than on a well-formed span.
                break;
            };
            let handle = &after[..end + CLOSE.len()];
            // The secret pass opens on bare `{{`, which plain aliases and every
            // template language share. Anything not shaped like one of ours
            // belongs to somebody else — step over it rather than refusing it.
            if at == ResolveAt::Exec && !is_secret_shaped(handle) {
                out.push_str(&rest[..start + open.len()]);
                rest = &after[open.len()..];
                continue;
            }
            let Some(entry) = self.entries.iter().find(|e| e.handle == handle) else {
                let what = match into {
                    Splice::Shell => "to run the command",
                    Splice::File => "to write the file",
                };
                return Err(format!(
                    "{handle} is not an alias this session holds — refusing {what} \
                     rather than guess what it meant"
                ));
            };
            // Held, but not lent for use. Refusing is the whole point: a value
            // marked "keep this off the wire" was never made spendable, and
            // expanding it would hand out a capability nobody asked for.
            // Only meaningful for a secret: a plain alias grants no capability,
            // so "not spendable" would just break it for nothing.
            if entry.sensitive && !entry.spendable {
                return Err(format!(
                    "{handle} is kept off the wire, not lent for use — re-mark \
                     it with `:secret <text>` (without `--no-spend`) if a \
                     command really needs the value"
                ));
            }
            // **A spliced value must not change the STRUCTURE of the line a
            // human approved.** This is the one span in the system that is
            // never analysed: plain aliases expand at the gate precisely so the
            // analyzer and the reader see what will run, but a secret expands
            // *after* it, straight into `bash -c` (Reviewer E).
            //
            // The test is **what changes which context we are in** — not what
            // survives the current one. That distinction is the whole fix: the
            // previous version asked "what stays live inside double quotes",
            // and so allowed a `"`, which does not survive the quoting, it ENDS
            // it. `x" ; id ; echo "` in the common shape
            // (`curl -H "Bearer {{gh.1234}}"`) spliced a complete second
            // command. A newline was allowed on the same reasoning and is not
            // loud either — it is a second command line.
            //
            // So: refuse, and say so. Never splice something whose meaning we
            // cannot vouch for ([[feedback_no_silent_failure]]). Quoting the
            // value instead would be friendlier and needs the handle's lexical
            // context — blind quoting breaks the quoted shape above — which is
            // docketed.
            //
            // **The remedy names one route, because there is one.** This used
            // to add "or pass it through the environment", which cannot work in
            // either reading: a tool child gets `env_clear()` plus a twelve-name
            // allowlist with no knob to extend it, and the in-product
            // `[secrets] env` route builds an ordinary alias spliced by THIS
            // function, so it lands on this same refusal. A user who followed
            // it would `export` the value and watch an approved command run
            // with an empty header — the refusal converting a loud stop into a
            // silent wrong result (Reviewer F). "Re-mark the secret without it"
            // went too: it reads as *edit your credential*, which is not a
            // thing the user can do.
            if let Some(what) = shell_active(entry.value.expose()).filter(|_| into == Splice::Shell)
            {
                return Err(format!(
                    "{handle} holds a value containing {what} — refusing rather \
                     than running a command that no longer means what you \
                     approved. Rotate the credential to one without it"
                ));
            }
            out.push_str(&rest[..start]);
            out.push_str(entry.value.expose());
            if entry.sensitive && !spent.contains(&entry.name) {
                spent.push(entry.name.clone());
            }
            rest = &after[end + CLOSE.len()..];
        }
        out.push_str(rest);
        Ok(Expansion { text: out, spent })
    }

    /// Replace any held value appearing in `text` with its handle.
    ///
    /// Runs before the one-way scrub, so a value we hold comes back as the handle
    /// the model already knows rather than an anonymous `[redacted: …]`. That
    /// closes the loop in both directions: the model can act on the result without
    /// ever having seen the value.
    ///
    /// **Longest value first**, so `/Users/me/project` wins over `/Users/me` and
    /// the more specific alias is the one that shows.
    ///
    /// Secrets match **anywhere**; plain aliases match only at **word
    /// boundaries**. That asymmetry is the failure-mode lens: for a secret a false
    /// positive costs a mangled log and a false negative costs the key, so we
    /// over-match; for a cosmetic alias it is the reverse, and masking `sam`
    /// inside `samson` is just damage.
    pub fn mask(&self, text: &str) -> (String, Vec<String>) {
        self.mask_in(text, Lines::Join)
    }

    /// [`mask`](Self::mask), a held value that runs over lines standing for
    /// them as `lines` says (`secrets::redact_lines`: kept, for a Read).
    pub(crate) fn mask_in(&self, text: &str, lines: Lines) -> (String, Vec<String>) {
        Self::mask_with(self.entries.iter(), text, lines)
    }

    /// Whether any value held has a newline in it — so its mask can run
    /// over a line (`secrets::crosses_lines`).
    pub(crate) fn holds_a_value_over_lines(&self) -> bool {
        (self.entries.iter()).any(|entry| entry.value.expose().contains('\n'))
    }

    /// [`mask`](Self::mask) with the secrets alone — no plain alias. What the
    /// outbound door applies to every message it sends: a last line for
    /// secrets, which must not reshape cosmetic text nobody asked it to.
    pub fn mask_secrets(&self, text: &str) -> (String, Vec<String>) {
        Self::mask_with(self.secrets(), text, Lines::Join)
    }

    /// [`mask`](Self::mask) for a VIEW of a command the user approved —
    /// every entry but the home's (`~`). The shell spells the home `~` too,
    /// so a `~` on screen cannot be turned back into the path that was
    /// typed: an edit to `/Users/you/x` read as the model's own `~/x`, as
    /// though it never ran (Steven, 10-20, images 24–25: "it should show the
    /// literal"). What is stored and sent keeps `~` ([`mask`](Self::mask)).
    pub fn mask_shown(&self, text: &str) -> (String, Vec<String>) {
        self.mask_shown_in(text, Lines::Join)
    }

    /// [`mask_shown`](Self::mask_shown) over lines joined or kept, as
    /// [`mask_in`](Self::mask_in) is [`mask`](Self::mask)'s.
    pub(crate) fn mask_shown_in(&self, text: &str, lines: Lines) -> (String, Vec<String>) {
        Self::mask_with(
            self.entries.iter().filter(|e| e.handle != HOME_HANDLE),
            text,
            lines,
        )
    }

    /// [`mask`](Self::mask) over a whole message — its text and every block
    /// that carries text: a tool call's input strings, a tool result. Returns
    /// the masked message and the names that matched. For text the user
    /// authored outside the prompt (an `e` edit, an `s` summary), which must
    /// reach the store as handles exactly as a typed prompt does.
    pub fn mask_message(&self, message: &Message) -> (Message, Vec<String>) {
        Self::mask_message_with(message, |text, points| {
            Self::mask_moving(self.entries.iter(), text, points, Lines::Join)
        })
    }

    /// [`mask_message`](Self::mask_message) with the secrets alone — see
    /// [`mask_secrets`](Self::mask_secrets).
    pub fn mask_message_secrets(&self, message: &Message) -> (Message, Vec<String>) {
        Self::mask_message_with(message, |text, points| {
            Self::mask_moving(self.secrets(), text, points, Lines::Join)
        })
    }

    fn mask_message_with(
        message: &Message,
        mask: impl Fn(&str, &mut [usize]) -> (String, Vec<String>),
    ) -> (Message, Vec<String>) {
        let mut hit = Vec::new();
        let mut take_moving = |text: &str, points: &mut [usize]| {
            let (out, names) = mask(text, points);
            for name in names {
                if !hit.contains(&name) {
                    hit.push(name);
                }
            }
            out
        };
        // **Thinking goes back as it came** — a byte changed is a block the
        // API drops; the model wrote it from what it was sent, so a value in
        // it reached the API already — **and where it stood**: after the text
        // it followed. The text is masked whole (piece by piece, a value
        // split around a block would leave both halves in clear), and each
        // thought's place moves with it. Kept as it was, a thought after a
        // value stood past the masked text's end, or inside a handle longer
        // than the value: sealed so, its turn failed its content hash on
        // load, and the branch an `e` made was gone at the next launch
        // (review stop 15, E H1).
        let mut places: Vec<usize> = message.blocks.iter().filter_map(Block::at).collect();
        let content = take_moving(&message.content, &mut places);
        let mut places = places.into_iter();
        let mut take = |text: &str| take_moving(text, &mut []);
        let blocks = message
            .blocks
            .iter()
            .map(|block| match block {
                Block::Text { text } => Block::Text { text: take(text) },
                Block::ToolUse { id, name, input } => Block::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    // Keys too: a value can sit anywhere a model chose to put it.
                    input: super::json::map_strings(input, &mut |s, _| take(s)),
                },
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Block::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: take(content),
                    is_error: *is_error,
                },
                // A file Shannon read is text bound for the model, as a tool's
                // output is: a value marked secret in it leaves as its handle.
                Block::Context { kind, source, text } => Block::Context {
                    kind: *kind,
                    source: take(source),
                    text: take(text),
                },
                Block::Thinking { .. } | Block::RedactedThinking { .. } => {
                    let at = places.next().unwrap_or(content.len());
                    block.clone().placed(at)
                }
            })
            .collect();
        let masked = Message {
            role: message.role,
            content,
            blocks,
        };
        (masked, hit)
    }

    fn mask_with<'a>(
        entries: impl Iterator<Item = &'a Alias>,
        text: &str,
        lines: Lines,
    ) -> (String, Vec<String>) {
        Self::mask_moving(entries, text, &mut [], lines)
    }

    /// [`mask_with`](Self::mask_with), moving `points` — byte offsets into
    /// `text` — to where each stands in the masked text
    /// ([`replace_outside`]): a thought's place in its message's text.
    fn mask_moving<'a>(
        entries: impl Iterator<Item = &'a Alias>,
        text: &str,
        points: &mut [usize],
        lines: Lines,
    ) -> (String, Vec<String>) {
        let mut order: Vec<&Alias> = entries.collect();
        order.sort_by_key(|a| std::cmp::Reverse(a.value.expose().len()));
        let mut out = text.to_string();
        // **What is already a secret's handle is not text to mask** (review
        // stop 12, T8 M13): a held value inside a handle — `pw` in
        // `{{pw.8d31}}`, a short value in the handle this pass just put in
        // for a longer one — was masked again, nesting the handle in itself
        // on every pass. Only the table's own secret handles, found or put
        // in: braces a model wrote shield nothing, and a match reaching past
        // a handle's edge is masked as ever.
        let secret_handles: Vec<&str> = (order.iter())
            .filter(|a| a.sensitive)
            .map(|a| a.handle.as_str())
            .collect();
        let mut kept = spans_of(&out, &secret_handles);
        let mut hit = Vec::new();
        for entry in order {
            let value = entry.value.expose();
            // **Sensitivity decides how to match; length decides nothing.**
            // There used to be a length floor on marking and a length-based
            // matching threshold, and both were wrong. The user types the
            // explicit value, so refusing it is us imposing our implementation on
            // their situation — and for a SECRET a missed match is a *leak*
            // while an over-match is only a mangled message, so over-match.
            let accept = |text: &str, at: usize| {
                entry.sensitive || flanked_by_non_alphanumerics(text, at, value.len())
            };
            let with = lines.instead(&entry.handle, value);
            let (masked, moved, put) = replace_outside(&out, value, &with, &kept, points, accept);
            if !put.is_empty() {
                out = masked;
                kept = moved;
                if entry.sensitive {
                    kept.extend(put);
                }
                hit.push(entry.name.clone());
            }
        }
        (out, hit)
    }

    /// Session entropy, obtained on first use.
    fn seed(&mut self) -> Result<[u8; 32], String> {
        if let Some(seed) = self.seed {
            return Ok(seed);
        }
        let seed = session_entropy()?;
        self.seed = Some(seed);
        Ok(seed)
    }
}

/// Where a secret the user marked came from, for the `:secrets` listing.
pub const MARKED_SOURCE: &str = "marked by you";

/// Where a value lent from the environment came from, before the variable's
/// name — `env GITHUB_TOKEN`, for the `:secrets` listing. The one spelling
/// both lenders write and [`Alias::lent_from`] reads.
pub const ENV_SOURCE: &str = "env ";

/// Where a secret restored from a vault came from, for the `:secrets` listing.
/// Begins with [`MARKED_SOURCE`]: a kept secret is one the user marked.
pub const KEPT_SOURCE: &str = "marked by you, kept";

/// What [`Aliases::restore`] did with one kept secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// Put back under its handle.
    Added,
    /// This run already holds that handle for that value.
    Already,
    /// This run holds that handle for a DIFFERENT value; nothing changed.
    Conflict,
}

/// Text with the entries for one [`ResolveAt`] resolved, plus what it cost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Expansion {
    /// The resolved text. If any secret was resolved this is the only place that
    /// cleartext exists outside the table — **never log, store, or display it**.
    pub text: String,
    /// Names of the **sensitive** entries substituted, in first-use order.
    ///
    /// Sensitive only, deliberately. A substitution is only an *event* when
    /// spending it is dangerous; announcing an ordinary one would put
    /// `secret spent: home` in front of the user on most commands and bury the
    /// `github-token` line that actually matters. Non-sensitive substitutions are
    /// counted for the ledger, not announced — the design's own rule that gating
    /// (or shouting about) a plain alias is "the prompt that trains dismissal".
    pub spent: Vec<String>,
}

/// The **secret** handles appearing in `text`, purely syntactically.
///
/// Deliberately table-free: the permission engine must be able to say "this line
/// spends a secret" without holding any, and an *unknown* handle has to reach a
/// human too — it is the shape of an injection attempt. Plain aliases are
/// excluded because they do not gate (see the module doc).
pub fn secret_handles_in(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(SECRET_OPEN) {
        let after = &rest[start..];
        let Some(end) = after.find(CLOSE) else { break };
        let handle = &after[..end + CLOSE.len()];
        if is_secret_shaped(handle) {
            found.push(handle);
            rest = &after[end + CLOSE.len()..];
        } else {
            // **Advance past the OPENER, not past the close.** A candidate that
            // is not shaped like ours belongs to somebody else — a Helm or Jinja
            // template — but its `}}` may well be closing a handle of ours that
            // started later: `{{x {{gh.1234}}` is one non-handle followed by one
            // real handle. Skipping to the close swallowed the real one, so the
            // gate saw nothing while `expand_for` — which steps over the opener
            // and re-scans — spent it anyway (Reviewer A, HIGH).
            //
            // The two walks must agree, and this is the direction that agrees
            // SAFELY: seeing a handle that will not be spent costs a prompt,
            // missing one that will be spent costs the guarantee
            // (`[[feedback_one_door_per_invariant]]`). Pinned by
            // `the_gate_sees_every_handle_execution_would_spend`.
            // One byte, not the whole opener: `{"{{tok.X}}"` loses its
            // quotes to `{{{tok.X}}`, where stepping over `{{` lands past
            // the real handle's first brace (review 2026-10-10, E2).
            rest = &after[1..];
        }
    }
    found
}

/// Characters that change **which quoting context the rest of the line is in**,
/// and the words a refusal uses for each.
///
/// One table, so the character and its explanation cannot drift and the test
/// that proves each is refused is driven from the list it protects.
///
/// **Membership is "does it matter where the handle sits?"** Everything here is
/// dangerous in *every* position — bare, in `"…"`, in `'…'` — because it ends
/// or escapes the context rather than living inside it. Characters that are
/// dangerous only *unquoted* (`;`, `|`, `&`, `<`, `>`) are deliberately absent:
/// they are a real injection when the handle sits bare (`export TOKEN={{h}}`
/// with a value of `x;id` runs `id`, and does not "break loudly" as this
/// module once claimed), but refusing them unconditionally would reject an
/// ordinary punctuated password, which `a_value_the_shell_would_act_on_is_
/// refused_with_a_reason` pins as a requirement. Deciding those needs the
/// handle's lexical context, which is the same thing the quote-at-splice-time
/// fix needs — both are docketed together rather than half-guessed here.
const STRUCTURAL: &[(char, &str)] = &[
    ('`', "a backtick, which runs a command"),
    // **The three that killed the previous version of this check.** A quote is
    // not inert inside quotes — it ENDS them, and everything after it is
    // unquoted. `x" ; id ; echo "` spliced into `curl -H "Bearer {{h}}"`
    // produces a complete second command. A trailing backslash escapes the
    // closing quote and does the same. And a newline is not a loud failure, it
    // is a second command line.
    ('"', "a double quote, which would end the quoting around it"),
    (
        '\'',
        "a single quote, which would end the quoting around it",
    ),
    (
        '\\',
        "a backslash, which can escape the quote that follows it",
    ),
    ('\n', "a newline, which would start a second command"),
    (
        '\r',
        "a carriage return, which would start a second command",
    ),
];

/// The part of a value the shell would act on, if any — the thing that makes a
/// splice change the structure of the line a human approved.
///
/// **The previous version of this reasoned from the wrong question.** It asked
/// *"what stays live inside double quotes"*, concluded backticks and `$`, and
/// said in its own comment that *"everything else (spaces, `;`, `|`) is inert
/// when quoted and, when unquoted, breaks the command LOUDLY"*. Both halves are
/// false, and the first is false in the example the comment itself gives:
///
/// ```text
/// value = x" ; id ; echo "     handle in  curl -H "Bearer {{gh.1234}}"
///   was ACCEPTED  ->  bash -c: curl -H "Bearer x" ; id ; echo ""
/// ```
///
/// A quote does not survive its context — it **ends** it. A newline is not
/// loud; it is a second command line, silently. The right question is not
/// *"what survives in this context"* but *"what changes which context we are
/// in"* (Reviewer E, verified by expansion).
///
/// The residual is named in [`STRUCTURAL`] and docketed: `;`, `|`, `&`, `<` and
/// `>` still splice through, and are a real injection when the handle sits
/// unquoted. Refusing them costs an ordinary punctuated password, so the honest
/// fix is the lexical one, not a wider blacklist.
///
/// Refusing rather than quoting is still the posture: this is the one span in
/// the system that is never analysed, plain aliases expand *at* the gate so the
/// reader sees them, and a secret expands after it straight into `bash -c`. The
/// friendlier fix — quote at splice time using the handle's lexical context,
/// which `lex()` can supply — is docketed. Refusal is only over-strict for a
/// credential that contains shell syntax, and it says exactly what and why.
fn shell_active(value: &str) -> Option<&'static str> {
    if let Some((_, why)) = STRUCTURAL.iter().find(|(c, _)| value.contains(*c)) {
        return Some(why);
    }
    // `$` is the multi-character case: a bare `$` is literal (`hunter2$`), so it
    // has to OPEN something — `$(`, `${`, or `$name`.
    let b = value.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c != b'$' {
            continue;
        }
        match b.get(i + 1) {
            Some(b'(') => return Some("`$(`, which runs a command"),
            Some(b'{') => return Some("`${`, which expands a variable"),
            Some(n) if n.is_ascii_alphanumeric() || *n == b'_' => {
                return Some("`$name`, which expands to a variable");
            }
            _ => {}
        }
    }
    None
}

/// Does `handle` have the shape we mint — `{{name.nnnn}}`, name in `[a-z0-9-]`
/// and exactly four hex?
///
/// **Shape, not a sigil**, and the reason is collision. A bare `{{…}}` would
/// have made every Helm, Jinja, Handlebars, Go and Vue template in a repository
/// look like a secret being spent — and a gate that fires on every template is
/// the "prompt that trains dismissal" failure, aimed at the loudest signal we
/// have. A sigil (`{{…}}`) avoids that but spends characters on every mention
/// of every secret, for a distinction the *nonce already makes*: `{{ .Values.x }}`
/// is not `name` + `.` + four hex, and nothing anyone writes by hand is.
///
/// This is recognition by what we PRODUCE — the same rule as the inert-region
/// skip in `redact` — rather than by decoration we hope nobody else uses.
fn is_secret_shaped(handle: &str) -> bool {
    let Some(body) = handle
        .strip_prefix(SECRET_OPEN)
        .and_then(|h| h.strip_suffix(CLOSE))
    else {
        return false;
    };
    let Some((name, nonce)) = body.rsplit_once('.') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && nonce.len() == NONCE_HEX
        && nonce.chars().all(|c| c.is_ascii_hexdigit())
}

/// Replace `~` with `home` where a **shell** would treat it as the home
/// directory, and nowhere else.
///
/// Its own rules rather than the generic word-boundary test, because `~` is
/// shell syntax rather than a name we minted, and the shell's rules are the ones
/// that matter: `~/x` and a bare `~` are home; `~root` is a *different* user's
/// home (which we must not guess at), `=~` is bash's regex-match operator, and
/// `backup~` is an editor's leftover. Requiring whitespace-or-quote before, and
/// `/` or a word end after, excludes all three.
fn expand_tilde(text: &str, home: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < text.len() {
        let ch = text[i..].chars().next().expect("in bounds");
        if ch == '~' {
            // `=` is deliberately NOT a valid predecessor. Bash does expand
            // `VAR=~/x`, but it also has `=~` as the regex-match operator, and
            // rewriting `[[ $x =~ y ]]` into a path would be a wrong answer to a
            // command we were only asked to analyse. Losing `VAR=~/x` (rare from
            // a model) is the cheaper error.
            let before_ok = i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'"' | b'\'');
            let after = text[i + 1..].chars().next();
            let after_ok = matches!(after, None | Some('/'))
                || after.is_some_and(|c| c.is_whitespace() || c == '"' || c == '\'');
            if before_ok && after_ok {
                out.push_str(home);
                i += 1;
                continue;
            }
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Plain `str::replace`, named for symmetry with its word-boundary sibling.
/// Where each of `handles` stands in `text`, as byte spans.
fn spans_of(text: &str, handles: &[&str]) -> Vec<std::ops::Range<usize>> {
    (handles.iter())
        .filter(|h| !h.is_empty())
        .flat_map(|h| text.match_indices(h).map(|(at, h)| at..at + h.len()))
        .collect()
}

/// **`needle` replaced by `with` wherever `accept` says, but never where the
/// match lies wholly inside one of `kept`** — spans of `text` that are
/// handles. Left to right, a match not taken skipped whole, as
/// `str::replace` walks; a plain alias's `accept` is the word boundary
/// ([`flanked_by_non_alphanumerics`]: an alias for `sam` does not rewrite
/// the middle of `samson`). Returns the text, the kept spans no
/// replacement touched (moved to where they now stand — one a match reached
/// into is no longer a handle), and where each `with` was put.
///
/// **An empty needle replaces nothing.** `str::find("")` is `Some(0)`
/// forever, so a loop advancing by the needle's length spun and pushed
/// `with` every iteration — a hang with unbounded allocation. The caller
/// refuses an empty alias value, and this stays as the guard that makes the
/// function safe for any caller.
///
/// **`points` move with the text** — byte offsets into it (a thought's
/// place, `Block::at`), each where it now stands: past what was replaced
/// before it, and one inside a replaced match after its `with`.
fn replace_outside(
    text: &str,
    needle: &str,
    with: &str,
    kept: &[std::ops::Range<usize>],
    points: &mut [usize],
    accept: impl Fn(&str, usize) -> bool,
) -> (
    String,
    Vec<std::ops::Range<usize>>,
    Vec<std::ops::Range<usize>>,
) {
    let mut cuts: Vec<std::ops::Range<usize>> = Vec::new();
    // An empty needle matches everywhere and would never advance.
    if !needle.is_empty() {
        let mut from = 0;
        while let Some(i) = text[from..].find(needle) {
            let (at, end) = (from + i, from + i + needle.len());
            let inside = kept.iter().any(|k| k.start <= at && end <= k.end);
            if !inside && accept(text, at) {
                cuts.push(at..end);
            }
            from = end;
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut put = Vec::new();
    let mut last = 0;
    for cut in &cuts {
        out.push_str(&text[last..cut.start]);
        put.push(out.len()..out.len() + with.len());
        out.push_str(with);
        last = cut.end;
    }
    out.push_str(&text[last..]);
    let moved = (kept.iter())
        .filter(|k| !cuts.iter().any(|c| c.start < k.end && k.start < c.end))
        .map(|k| {
            let before = cuts.iter().filter(|c| c.end <= k.start);
            let grown: isize = before
                .map(|c| with.len() as isize - (c.end - c.start) as isize)
                .sum();
            let shift = |at: usize| (at as isize + grown) as usize;
            shift(k.start)..shift(k.end)
        })
        .collect();
    for point in points.iter_mut() {
        // Inside a match, at its end: what was before it stands before the
        // `with` put there.
        let at = (cuts.iter())
            .find(|c| c.start < *point && *point < c.end)
            .map_or(*point, |c| c.end);
        let grown: isize = (cuts.iter())
            .filter(|c| c.end <= at)
            .map(|c| with.len() as isize - (c.end - c.start) as isize)
            .sum();
        *point = (at as isize + grown) as usize;
    }
    (out, moved, put)
}

/// Is the `len`-long match at byte offset `at` flanked by non-alphanumerics?
fn flanked_by_non_alphanumerics(text: &str, at: usize, len: usize) -> bool {
    let before = text[..at]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric());
    let after = text[at + len..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric());
    before && after
}

/// A name reduced to `[a-z0-9-]`, so a handle can never contain a character that
/// means something to a shell — or a `,`/`..`, which would wake up bash's brace
/// expansion and turn the handle into something else entirely.
fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Length of the nonce, in hex characters. Also what makes a handle
/// recognisable by shape — see [`is_secret_shaped`].
const NONCE_HEX: usize = 4;

/// Four hex characters derived from the session seed and the name.
///
/// 16 bits, down from 32. The nonce defeats a *blind* handle written by injected
/// content, and a wrong guess is a **visible refusal** with no oracle to iterate
/// against — so the attacker gets a handful of loud attempts, not a search. That
/// is worth far more than the extra four characters cost in tokens on every
/// mention of every secret.
fn nonce(seed: &[u8; 32], name: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(seed);
    hasher.update(name.as_bytes());
    hasher
        .finalize()
        .as_bytes()
        .iter()
        .take(2)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 32 bytes from the operating system.
///
/// Fails rather than falling back to time-and-pid. A predictable seed would be a
/// silent downgrade of the one property the nonce exists for, and "the feature is
/// off, here is why" is honest where "the feature is weak, quietly" is not.
fn session_entropy() -> Result<[u8; 32], String> {
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("no system entropy ({e}) — secret handles are unavailable"))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A table's fingerprint changes with what it finds, and only then**
    /// — a secret marked moves it, and unmarked it comes back; a plain alias
    /// is no secret and moves nothing. (Two tables marking one value differ:
    /// each mints its own handle, which a secret's ranges find too.)
    #[test]
    fn a_fingerprint_moves_with_what_the_table_finds() {
        let mut t = Aliases::default();
        let empty = t.fingerprint();
        assert_eq!(t.fingerprint(), empty, "the same table, the same number");
        t.insert("home", "/home/me", "config", false).unwrap();
        assert_eq!(t.fingerprint(), empty, "an alias finds no secret");
        let pw = t.insert("pw", "hunter2", "marked by you", true).unwrap();
        let one = t.fingerprint();
        assert_ne!(one, empty, "a secret marked");
        t.insert("key", "sk-another", "marked by you", true)
            .unwrap();
        assert_ne!(t.fingerprint(), one, "another marked");
        assert_eq!(t.unmark("sk-another").len(), 1, "premise: by its value");
        assert_eq!(t.fingerprint(), one, "and unmarked");
        // Rotated: the same name, a new value, the handle kept (`insert`) —
        // what the table finds changed, so the number does.
        let rotated = t.insert("pw", "hunter3", "marked by you", true).unwrap();
        assert_eq!(rotated, pw, "premise: the handle kept");
        assert_ne!(t.fingerprint(), one, "a rotated value");
    }

    /// **What is a handle is not text to mask** (review stop 12, T8 M13): a
    /// held `pw` masks to `{{pw.…}}`, and masking that again found `pw`
    /// inside it — `{{{{pw.…}}.…}}`. Within one pass alike: a shorter value
    /// inside the handle the pass had just put in for a longer one.
    #[test]
    fn a_value_inside_a_handle_is_not_masked_again() {
        let mut t = Aliases::default();
        let pw = t.insert("pw", "pw", "marked by you", true).unwrap();
        let (once, _) = t.mask("the pw is set");
        assert_eq!(once, format!("the {pw} is set"), "premise");
        assert_eq!(t.mask(&once).0, once, "a second pass changes nothing");

        let mut t = Aliases::default();
        let long = t
            .insert("a", "longvalue123", "marked by you", true)
            .unwrap();
        t.insert("b", "a", "marked by you", true).unwrap();
        assert!(
            long.contains('a'),
            "premise: the short value is in the handle"
        );
        assert_eq!(t.mask("longvalue123").0, long, "one pass");
    }

    /// **Only the table's own handles are not text to mask** — a model can
    /// write anything in braces, and a value inside braces the table never
    /// minted, or straddling a handle's edge, is masked as ever: a secret
    /// over-matches, it never under-matches.
    #[test]
    fn a_handle_the_table_does_not_hold_shields_no_value() {
        let mut t = Aliases::default();
        t.insert("tok", "hunter2seekrit", "marked by you", true)
            .unwrap();
        let (out, _) = t.mask("{{hunter2seekrit}} and {{tok.0000}}hunter2seekrit");
        assert!(!out.contains("hunter2seekrit"), "{out}");
    }

    /// **`restore` puts a kept secret back under exactly its handle**: added
    /// once, `Already` for the same value, `Conflict` — changing nothing — for
    /// a different one, a taken name suffixed while the handle stays, and a
    /// handle not shaped like ours refused.
    #[test]
    fn restore_is_by_exact_handle_and_never_overwrites() {
        let kept = |name: &str, value: &str| crate::secrets::Kept {
            name: name.into(),
            value: Secret::new(value),
            spendable: true,
        };
        let mut t = Aliases::default();
        // No quotes in these values: a quote is refused at splice time
        // (`shell_active`), which is not what this test is about.
        let live = t
            .insert("secret", "this run", "marked by you", true)
            .unwrap();

        let old = "{{secret.beef}}";
        assert_eq!(
            t.restore(old, &kept("secret", "earlier")),
            Ok(Restored::Added)
        );
        let entry = t.entries().iter().find(|e| e.handle == old).unwrap();
        assert_eq!(
            entry.name, "secret-2",
            "the name was taken; the handle is kept"
        );
        assert_eq!(entry.source, KEPT_SOURCE);
        assert_eq!(t.expand_at_exec(old).unwrap().text, "earlier");

        assert_eq!(t.restore(old, &kept("x", "earlier")), Ok(Restored::Already));
        assert_eq!(
            t.restore(&live, &kept("x", "other")),
            Ok(Restored::Conflict)
        );
        assert_eq!(
            t.expand_at_exec(&live).unwrap().text,
            "this run",
            "unchanged"
        );
        assert!(
            t.restore("{{a:repo}}", &kept("x", "v")).is_err(),
            "not a secret's shape"
        );
    }

    /// **A turn records only what was held** — every part that carries text, a
    /// tool's output included, each handle once in order of first appearance
    /// — and never a guess, a plain alias or a lent secret: what recall may
    /// later ask the vault about (`Provenance::handles`). Each held handle
    /// sits in exactly one kind of part, so dropping any part from the walk
    /// drops it — a context block's (a memory the turn was told) too, which
    /// no row held, so its arm passed dropped (review stop 12, T8 M13).
    #[test]
    fn handles_to_record_records_held_secrets_only_once_each() {
        use crate::message::{ContextKind, Role};
        let mut t = Aliases::default();
        let typed = t.insert("typed", "value-a", MARKED_SOURCE, true).unwrap();
        let said = t.insert("said", "value-b", MARKED_SOURCE, true).unwrap();
        let ran = t.insert("ran", "value-c", MARKED_SOURCE, true).unwrap();
        let printed = t.insert("printed", "value-d", KEPT_SOURCE, true).unwrap();
        let told = t.insert("told", "value-f", MARKED_SOURCE, true).unwrap();
        let repo = t.insert("repo", "/src/repo", "config", false).unwrap();
        let lent = t.insert("token", "value-e", "env GH_TOKEN", true).unwrap();
        let guess = "{{typed.0000}}";
        assert!(!t.entries().iter().any(|a| a.handle == guess), "premise");

        let messages = vec![
            Message::user(format!("use {typed} and {guess}, {typed} again, {lent}")),
            Message {
                role: Role::Assistant,
                content: String::new(),
                blocks: vec![
                    Block::Text {
                        text: format!("ok, {said}"),
                    },
                    Block::ToolUse {
                        id: "t1".into(),
                        name: "bash".into(),
                        input: serde_json::json!({ "command": format!("echo {ran}") }),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: String::new(),
                blocks: vec![
                    Block::ToolResult {
                        tool_use_id: "t1".into(),
                        content: format!("{printed} {repo} {typed}"),
                        is_error: false,
                    },
                    Block::Context {
                        kind: ContextKind::Memory,
                        source: "/m/MEMORY.md".into(),
                        text: format!("- the deploy key is {told}"),
                    },
                ],
            },
        ];
        assert_eq!(
            t.handles_to_record(&messages, &[]),
            vec![
                typed.clone(),
                said.clone(),
                ran.clone(),
                printed.clone(),
                told.clone()
            ]
        );
        assert!(
            Aliases::default()
                .handles_to_record(&messages, &[])
                .is_empty(),
            "an empty table holds nothing"
        );
        // A source's record vouches for a handle this table does not hold —
        // the composed turn of a later run — and never for one the text does
        // not name.
        let vouched = [said.clone(), "{{other.beef}}".to_string()];
        assert_eq!(
            Aliases::default().handles_to_record(&messages, &vouched),
            vec![said],
            "vouched, and named"
        );
    }

    /// **Display puts each minted handle back once, in one pass.** A value
    /// that contains another entry's handle keeps it as text; a handle right
    /// after a stray `{` still resolves; a user's line gets a plain alias back
    /// as well as a secret — and never the canonical `~`, which the user types
    /// too (Reviewers A, B and C, round 19).
    #[test]
    fn display_puts_each_minted_handle_back_once() {
        let mut t = Aliases::default();
        // `outer` ahead of `inner` in the table: replacing entry by entry
        // would resolve `inner` inside `outer`'s value on the way past.
        t.insert("outer", "placeholder", MARKED_SOURCE, true)
            .unwrap();
        let inner = t.insert("inner", "in-value", MARKED_SOURCE, true).unwrap();
        let outer = t
            .insert("outer", &format!("<{inner}>"), MARKED_SOURCE, true)
            .unwrap();
        assert_eq!(t.entries[0].handle, outer, "premise: outer first");
        let repo = t.insert("repo", "/src/repo", "config", false).unwrap();
        t.entries.push(Alias {
            name: "home".into(),
            handle: HOME_HANDLE.into(),
            source: "$HOME".into(),
            sensitive: false,
            spendable: true,
            resolve: ResolveAt::Exec,
            value: Secret::new("/home/me"),
        });

        assert_eq!(
            t.unmask_for_display(&format!("{outer} in {repo} at ~/x"), Unmasked::Value),
            format!("<{inner}> in /src/repo at ~/x"),
            "a value's own text is not read again; `~` stays"
        );
        // A plain alias as its value too, as a secret is (review stop 6, C 8;
        // a reply showed `{{a:repo}}` where the user's line showed the path).
        assert_eq!(
            t.unmask_for_display(&format!("{{{outer} {repo}"), Unmasked::Value),
            format!("{{<{inner}> /src/repo"),
            "a reply: each once, after a stray brace — the values"
        );
        assert_eq!(
            t.unmask_for_display(&format!("{{{outer} {repo}"), Unmasked::WithHandle),
            format!("{{<{inner}> ({outer}) /src/repo ({repo})"),
            "the stub's echo: the handle beside each"
        );
        let stub = crate::backend::models::STUB;
        assert_eq!(Unmasked::for_model(stub), Unmasked::WithHandle);
        for model in ["claude-code/sonnet", "sonnet", "claude-opus-5-5", ""] {
            assert_eq!(Unmasked::for_model(model), Unmasked::Value, "{model}");
        }
        assert_eq!(
            t.unmask_for_display("{{nobody.0000}} {{ {", Unmasked::Value),
            "{{nobody.0000}} {{ {",
            "what we do not hold, and half-openers, pass through"
        );

        // An entry no constructor can build — an empty handle — must not
        // stall the walk: it is not a minted handle, so it never matches.
        t.entries.push(Alias {
            name: "empty".into(),
            handle: String::new(),
            source: "test".into(),
            sensitive: true,
            spendable: true,
            resolve: ResolveAt::Exec,
            value: Secret::new("v"),
        });
        assert_eq!(t.unmask_for_display("a {{ b", Unmasked::Value), "a {{ b");
    }

    /// **Where each held secret was put back** — char ranges in what is
    /// shown, a secret's alone (an alias's value is not one), counted in
    /// chars past multibyte text; and not where the text already held the
    /// value itself, which is what tells the model had it (review stop 6,
    /// F 5).
    #[test]
    fn unmask_shown_says_where_each_secret_went_back() {
        let mut t = Aliases::default();
        let pw = t.insert("pw", "hunter2", MARKED_SOURCE, true).unwrap();
        let repo = t.insert("repo", "/src/repo", "config", false).unwrap();
        let (shown, at) = t.unmask_shown(&format!("é {pw} {repo} hunter2"), Unmasked::Value);
        assert_eq!(shown, "é hunter2 /src/repo hunter2");
        assert_eq!(
            at,
            vec![(2, 9)],
            "the handle's, not the alias, not the literal"
        );
        let (shown, at) = t.unmask_shown(&pw, Unmasked::WithHandle);
        assert_eq!(shown, format!("hunter2 ({pw})"));
        assert_eq!(
            at,
            vec![(0, shown.chars().count())],
            "the handle beside it too"
        );
    }

    /// **`unmark` takes a secret out by its handle or by its value** — never a
    /// plain alias, never by name — and the handle stops resolving at once.
    #[test]
    fn unmark_takes_a_secret_by_handle_or_value() {
        let mut t = Aliases::default();
        let pw = t.insert("pw", "hunter2", "marked by you", true).unwrap();
        let token = t.insert("token", "s3cr3t", "marked by you", true).unwrap();
        t.insert("repo", "/src/repo", "config", false).unwrap();

        let handles =
            |taken: Vec<Alias>| -> Vec<String> { taken.into_iter().map(|a| a.handle).collect() };
        assert!(t.unmark("pw").is_empty(), "a name is not a key");
        assert!(
            t.unmark("/src/repo").is_empty(),
            "a plain alias is not a secret"
        );
        assert_eq!(handles(t.unmark(&pw)), std::slice::from_ref(&pw));
        assert!(
            t.expand_at_exec(&pw).is_err(),
            "gone: the handle resolves to nothing"
        );
        assert_eq!(handles(t.unmark("s3cr3t")), [token]);
        assert_eq!(t.secrets().count(), 0);
        assert_eq!(t.entries().len(), 1, "the plain alias stays");
    }

    /// **A whole message is masked, every block that carries text** — the
    /// content, a text block, a tool call's input (keys and values, nested),
    /// a tool result — and the secrets-only form leaves a plain alias alone.
    #[test]
    fn mask_message_reaches_every_block() {
        use crate::message::{Block, Message, Role};
        let mut t = Aliases::default();
        let pw = t.insert("pw", "hunter2", MARKED_SOURCE, true).unwrap();
        let repo = t.insert("repo", "/src/repo", "config", false).unwrap();
        let message = Message {
            role: Role::Assistant,
            content: "hunter2 in /src/repo".into(),
            blocks: vec![
                Block::Text {
                    text: "text hunter2".into(),
                },
                Block::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "echo hunter2", "hunter2": ["x hunter2"]}),
                },
                Block::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "out hunter2".into(),
                    is_error: false,
                },
                // A file Shannon read for the model, its name included.
                Block::Context {
                    kind: crate::ContextKind::Memory,
                    source: "/m/hunter2.md".into(),
                    text: "ctx hunter2".into(),
                },
            ],
        };
        let (masked, names) = t.mask_message(&message);
        let rendered = format!("{masked:?}");
        assert!(!rendered.contains("hunter2"), "every block: {rendered}");
        assert_eq!(rendered.matches(pw.as_str()).count(), 8, "{rendered}");
        assert!(rendered.contains(&repo), "the plain alias too: {rendered}");
        let mut names = names;
        names.sort();
        assert_eq!(names, ["pw", "repo"], "each named once");

        let (secrets_only, names) = t.mask_message_secrets(&message);
        let rendered = format!("{secrets_only:?}");
        assert!(
            !rendered.contains("hunter2") && rendered.contains("/src/repo"),
            "{rendered}"
        );
        assert_eq!(names, ["pw"]);
    }

    /// **A thought keeps its place through a masked message** — after the
    /// text it followed, wherever a value before it was masked; one placed
    /// inside a value, after its handle. Masked whole with its places kept,
    /// a thought after a value stood past the masked text's end — sealed so,
    /// the turn failed its content hash on load and the branch an `e` made
    /// was gone at the next launch — or, where the handle is longer than the
    /// value, inside the handle on the wire (review stop 15, E H1).
    #[test]
    fn a_thought_keeps_its_place_through_a_masked_message() {
        use crate::message::{Block, Message, Role};
        let mut t = Aliases::default();
        let long = "a-value-of-some-length-1234567890";
        let short = "ab12";
        let pw = t.insert("pw", long, "test", true).unwrap();
        let k = t.insert("k", short, "test", true).unwrap();
        let thought = |at| Block::Thinking {
            thinking: "weighing".into(),
            signature: "S".into(),
            at,
        };
        for (value, handle) in [(long, &pw), (short, &k)] {
            let before = format!("found {value} here");
            let content = format!("{before}, then on");
            let message = Message {
                role: Role::Assistant,
                content: content.clone(),
                blocks: vec![thought(before.len()), thought(content.len())],
            };
            let (masked, _) = t.mask_message(&message);
            let after = format!("found {handle} here").len();
            assert_eq!(
                masked.blocks[0].at(),
                Some(after),
                "{value}: after its text"
            );
            let end = Some(masked.content.len());
            assert_eq!(masked.blocks[1].at(), end, "{value}: at the end");
        }
        let message = Message {
            role: Role::Assistant,
            content: format!("say {short} now"),
            blocks: vec![thought("say ab".len())],
        };
        let (masked, _) = t.mask_message(&message);
        let past = Some(format!("say {k}").len());
        assert_eq!(
            masked.blocks[0].at(),
            past,
            "inside a value: after its handle"
        );
    }

    /// **One value under two handles comes off whole.** Lent by `[secrets]
    /// env` and also pasted, the same value sits under two names; taking only
    /// the first left it masked while `:nosecret` said it no longer was.
    #[test]
    fn unmark_by_value_takes_every_secret_holding_it() {
        let mut t = Aliases::default();
        let lent = t
            .insert("github", "ghp_same", "env GITHUB_TOKEN", true)
            .unwrap();
        let marked = t.insert("secret", "ghp_same", MARKED_SOURCE, true).unwrap();
        let other = t.insert("other", "elsewhere", MARKED_SOURCE, true).unwrap();
        assert_eq!(t.secret_ranges("x ghp_same").len(), 1, "premise: masked");

        let taken: Vec<String> = t.unmark("ghp_same").into_iter().map(|a| a.handle).collect();
        assert_eq!(taken, [lent, marked], "both, in table order");
        assert!(t.secret_ranges("x ghp_same").is_empty(), "no longer masked");
        assert_eq!(
            t.secrets().map(|a| a.handle.clone()).collect::<Vec<_>>(),
            [other]
        );
    }

    /// **An empty alias value is refused, and the masking loop cannot spin.**
    ///
    /// `value = ""` in an `[[alias]]` block hung the whole program: masking
    /// searches the output for the alias text, `str::find("")` is `Some(0)`
    /// forever, and the loop advanced by `needle.len()` — so it never advanced
    /// and pushed the replacement every iteration. A single-threaded TUI, gone,
    /// with no panic and no message. The `env` path filtered empty values; the
    /// config-value path did not.
    ///
    /// Both ends are pinned. The refusal is the door; the guard is what makes
    /// the helper safe for a caller that does not exist yet.
    ///
    /// **If the guard is removed, the second half of this test HANGS rather
    /// than fails** — there is no way to assert termination in-process without
    /// risking that. A timeout in CI is the signal; this comment is why.
    #[test]
    fn an_empty_alias_value_is_refused_and_cannot_hang_the_masker() {
        let mut a = Aliases::default();
        let err = a
            .insert("x", "", "config", false)
            .expect_err("an empty value must be refused");
        assert!(
            err.contains("empty value"),
            "the reason names the problem: {err:?}"
        );
        // …and the table did not keep a half-made entry.
        assert_eq!(a.mask("anything at all").0, "anything at all");

        // The guard, directly: an empty needle replaces nothing and returns.
        for text in ["some text", ""] {
            let (out, _, put) = replace_outside(text, "", "X", &[], &mut [], |_, _| true);
            assert_eq!((out.as_str(), put.len()), (text, 0));
        }
    }

    /// A table with one secret, and the handle minted for it.
    fn secret(name: &str, value: &str) -> (Aliases, String) {
        let mut a = Aliases::default();
        let h = a.insert(name, value, "test", true).expect("mint");
        (a, h)
    }

    /// A table with one plain alias.
    fn plain(name: &str, value: &str) -> (Aliases, String) {
        let mut a = Aliases::default();
        let h = a.insert(name, value, "config", false).expect("mint");
        (a, h)
    }

    /// The properties the design rests on: a handle names the entry readably,
    /// carries an unguessable nonce **when sensitive**, and is stable.
    #[test]
    fn a_secret_handle_is_readable_stable_and_unguessable() {
        let (mut a, h) = secret("GITHUB_TOKEN", crate::fakes::GITHUB_TOKEN);
        assert!(h.starts_with("{{github-token."), "{h}");
        assert!(h.ends_with("}}"), "{h}");

        // Stable: asking again in the same session gives the same handle, or the
        // model's reasoning fragments across a turn.
        let again = a
            .insert("GITHUB_TOKEN", crate::fakes::GITHUB_TOKEN, "test", true)
            .unwrap();
        assert_eq!(h, again);

        // Unguessable: a different session mints a different handle for the same
        // name, so an injected blind guess cannot resolve.
        let (_, other) = secret("GITHUB_TOKEN", crate::fakes::GITHUB_TOKEN);
        assert_ne!(h, other, "the nonce is per-session");
    }

    /// **The nonce belongs exactly where the capability is.** A plain alias has
    /// nothing to protect, so it is guessable on purpose — which is what lets a
    /// human type it.
    #[test]
    fn a_plain_alias_is_memorable_and_carries_no_nonce() {
        let (_, h) = plain("repo", "/home/sam/src/project");
        assert_eq!(h, "{{a:repo}}");

        // And it is the SAME in another session — that is the point.
        let (_, again) = plain("repo", "/home/sam/src/project");
        assert_eq!(h, again);
    }

    /// A table of plain aliases needs no entropy at all, so the common case
    /// cannot fail for a reason the user cannot act on.
    #[test]
    fn plain_aliases_need_no_entropy() {
        let mut a = Aliases::default();
        a.insert("repo", "/some/long/path", "config", false)
            .expect("no syscall needed");
        assert!(a.seed.is_none(), "entropy is only drawn for a secret");
    }

    /// The handle must survive a shell and our analyzer as one inert token — no
    /// brace expansion, no metacharacters.
    #[test]
    fn a_name_cannot_smuggle_shell_syntax() {
        let (_, h) = plain("a,b..c $(id) `id` ;rm", "value-long-enough");
        let inner = h
            .trim_start_matches(PLAIN_OPEN)
            .trim_end_matches(CLOSE)
            .to_string();
        assert!(
            inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "handle body must be inert: {h}"
        );
        assert!(!inner.contains(".."), "a range would brace-expand: {h}");
    }

    // --- resolving ------------------------------------------------------------

    #[test]
    fn expanding_secrets_substitutes_the_value_and_names_what_it_spent() {
        let (a, h) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let e = a
            .expand_at_exec(&format!("curl -H \"Bearer {h}\" https://api.github.com"))
            .expect("expands");
        assert!(e.text.contains(crate::fakes::GITHUB_TOKEN), "{e:?}");
        assert!(!e.text.contains("{{"), "{e:?}");
        assert_eq!(e.spent, vec!["gh".to_string()]);
    }

    /// Each pass resolves **only its own kind**, so the gate can show a plain
    /// alias's real value while a secret stays a placeholder until exec.
    #[test]
    fn each_pass_leaves_the_other_kind_alone() {
        let mut a = Aliases::default();
        let s = a
            .insert("gh", crate::fakes::GITHUB_TOKEN, "t", true)
            .unwrap();
        let p = a.insert("repo", "/Users/me/proj", "c", false).unwrap();
        let line = format!("cd {p} && curl -H {s} x");

        let gated = a.expand_at_gate(&line).expect("plain resolves");
        assert!(gated.text.contains("/Users/me/proj"), "{gated:?}");
        assert!(gated.text.contains(&s), "the secret is still a handle");

        let run = a.expand_at_exec(&gated.text).expect("secret resolves");
        assert!(run.text.contains(crate::fakes::GITHUB_TOKEN));
        assert!(run.text.contains("/Users/me/proj"));
    }

    /// **Fail closed.** An unminted handle is the shape of an injection attempt;
    /// expanding it to nothing would turn `rm -rf {{a:x}}/` into `rm -rf /`.
    #[test]
    fn an_unknown_handle_refuses_the_command() {
        let (a, _) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let err = a
            .expand_at_exec("rm -rf {{not-real.dead}}/")
            .expect_err("must refuse");
        assert!(err.contains("not an alias this session holds"), "{err}");

        // Plain aliases fail closed identically — one rule, no exception.
        let err = Aliases::default()
            .expand_at_gate("rm -rf {{a:nope}}/")
            .expect_err("must refuse");
        assert!(err.contains("refusing to run"), "{err}");
    }

    #[test]
    fn an_ordinary_command_passes_through_unchanged() {
        let (a, _) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let e = a.expand_at_exec("ls -la src/").unwrap();
        assert_eq!(e.text, "ls -la src/");
        assert!(e.spent.is_empty());
    }

    // --- masking (the return leg) ---------------------------------------------

    #[test]
    fn a_held_value_is_masked_back_to_its_handle() {
        let (a, h) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let (text, hit) = a.mask(&format!("token is {} ok", crate::fakes::GITHUB_TOKEN));
        assert_eq!(text, format!("token is {h} ok"));
        assert_eq!(hit, vec!["gh".to_string()]);
    }

    /// The token-economy case, which is also the privacy case: a long path
    /// repeated through a build log collapses to a short name, and the username
    /// stops travelling with it.
    #[test]
    fn a_plain_alias_shortens_and_anonymises_output() {
        let (a, h) = plain("repo", "/home/sam/src/project");
        let log = "compiling /home/sam/src/project/src/a.rs\n\
                   compiling /home/sam/src/project/src/b.rs";
        let (text, hit) = a.mask(log);
        assert!(!text.contains("sam"), "the username is gone: {text}");
        assert_eq!(text.matches(&h).count(), 2);
        assert!(text.len() < log.len(), "and it is shorter");
        assert_eq!(hit, vec!["repo".to_string()]);
    }

    /// Longest value first, so the more specific alias is the one that shows.
    #[test]
    fn a_more_specific_alias_wins_over_a_prefix_of_it() {
        let mut a = Aliases::default();
        a.insert("home", "/home/sam", "c", false).unwrap();
        a.insert("repo", "/home/sam/proj", "c", false).unwrap();
        let (text, _) = a.mask("cd /home/sam/proj && ls /home/sam");
        assert_eq!(text, "cd {{a:repo}} && ls {{a:home}}", "{text}");
    }

    /// A plain alias matches on word boundaries only — masking `sam` inside
    /// `samson` is damage, not protection. A SECRET matches anywhere, because
    /// there the two error costs are reversed.
    #[test]
    fn plain_matches_on_word_boundaries_but_a_secret_matches_anywhere() {
        let (a, h) = plain("me", "sam");
        let (text, _) = a.mask("sam and samson and sam-x");
        assert_eq!(text, format!("{h} and samson and {h}-x"), "{text}");

        let (a, h) = secret("tok", "abcd1234efgh");
        let (text, _) = a.mask("xxabcd1234efghyy");
        assert_eq!(text, format!("xx{h}yy"), "a secret is masked regardless");
    }

    /// **Length is not a requirement.** The user types the explicit value, so
    /// refusing `hunter2` for being seven characters was us imposing our
    /// implementation on their situation (Steven, dogfooding 2026-07-26). And a
    /// short secret is masked *anywhere*, not just at word boundaries: for a
    /// secret a missed match is a **leak** while an over-match is only a mangled
    /// message, and that asymmetry does not change with length.
    #[test]
    fn a_secret_of_any_length_is_held_and_matched_everywhere() {
        let mut a = Aliases::default();
        let handle = a
            .insert("pw", "hunter2", "marked by you", true)
            .expect("held");

        let (text, hit) = a.mask("my password is hunter2 ok");
        assert_eq!(text, format!("my password is {handle} ok"));
        assert_eq!(hit, vec!["pw".to_string()]);

        // Inside another token too — over-matching is the safe direction.
        let (text, _) = a.mask("the hunter2000 dataset");
        assert_eq!(text, format!("the {handle}000 dataset"));

        // Even a very short one, if that is what the user said.
        let mut a = Aliases::default();
        assert!(a.insert("tiny", "ab", "marked by you", true).is_ok());
    }

    /// A PLAIN alias keeps word-boundary matching: it is cosmetic, so there a
    /// false positive is the worse error and `sam` must not rewrite the
    /// middle of `samson`.
    #[test]
    fn a_plain_alias_still_matches_only_whole_words() {
        let mut a = Aliases::default();
        let handle = a.insert("me", "sam", "config", false).unwrap();
        let (text, _) = a.mask("sam and samson");
        assert_eq!(text, format!("{handle} and samson"));
    }

    /// Ranges are for styling    /// Ranges are for styling, so they name positions and never values — and
    /// they follow the same matching rule the masking does.
    #[test]
    fn ranges_locate_a_secret_for_styling() {
        let mut a = Aliases::default();
        a.insert("pw", "hunter2", "marked by you", true).unwrap();
        assert_eq!(a.secret_ranges("my password is hunter2 ok"), vec![(15, 22)]);
        assert_eq!(
            a.secret_ranges("the hunter2000 dataset"),
            vec![(4, 11)],
            "styling follows the same match rule the masking does"
        );
        // A plain alias is not a withheld secret, so it is not styled as one.
        a.insert("repo", "/tmp/probe-repo", "config", false)
            .unwrap();
        assert!(a.secret_ranges("cd /tmp/probe-repo").is_empty());
    }

    // --- detection, and not leaking -------------------------------------------

    /// Purely syntactic, for the permission engine — which holds no table and
    /// must still see that a line spends a secret. Plain aliases are excluded:
    /// they do not gate.
    #[test]
    fn only_secret_handles_are_reported_to_the_gate() {
        let found = secret_handles_in("curl {{a.1234}} {{a:repo}} x");
        assert_eq!(found, vec!["{{a.1234}}"]);
        assert!(secret_handles_in("ls {{a:repo}}").is_empty());
        assert!(secret_handles_in("ls -la").is_empty());
    }

    /// **What the gate sees must cover what execution spends.**
    ///
    /// The two walks are separate functions and they disagreed on one thing:
    /// how far to advance past a candidate that is not shaped like ours.
    /// `secret_handles_in` skipped past the closing `}}`; `expand_for` stepped
    /// past only the opening `{{` and re-scanned. So a nested opener —
    /// `{{x {{gh.1234}}` — hid a real handle from the gate and not from the
    /// shell: `decide_stage`'s promise that a stage spending a secret always
    /// faces a human was false for that spelling, and the model chooses the
    /// spelling (Reviewer A, HIGH).
    ///
    /// Stated as the containment property rather than as one example, because
    /// the failure is the *disagreement*, not the string.
    ///
    /// **Every row spends** — a premise: it skipped a row that spent nothing,
    /// so an execution that spent no handle at all passed every row (review
    /// stop 12, T8 M13: the shape check inverted, eleven tests red and this
    /// one green). Each spelling here is one that DOES spend.
    #[test]
    fn the_gate_sees_every_handle_execution_would_spend() {
        let (a, handle) = secret("gh", crate::fakes::GITHUB_TOKEN);
        for line in [
            format!("echo {handle}"),
            format!("echo \"{{{{x {handle}\""),
            format!("echo {{{{ {handle}"),
            format!("echo {{{{a:repo}} {handle}"),
            format!("echo {{{{not-shaped}} {handle}"),
        ] {
            let spent = a.expand_at_exec(&line).map(|e| e.spent).unwrap_or_default();
            assert!(!spent.is_empty(), "premise: {line} spends its handle");
            assert!(
                !secret_handles_in(&line).is_empty(),
                "execution spends {spent:?} but the gate sees no handle in: {line}"
            );
        }
    }

    /// **A core guard needs a core test.** Deleting the `sensitive &&
    /// !spendable` refusal in `expand_for` left `cargo test -p shannon-core`
    /// entirely green — it was caught only by `shannon-tui`'s
    /// `a_no_spend_secret_is_masked_but_never_expanded`. AGENTS.md recommends
    /// per-crate runs for tight iteration, so a guard covered only from the
    /// other side of the workspace is a guard that looks optional here
    /// (Reviewer A).
    #[test]
    fn a_secret_marked_no_spend_is_masked_but_never_expanded() {
        let mut a = Aliases::default();
        let handle = a
            .insert_with("gh", crate::fakes::GITHUB_TOKEN, "test", true, false)
            .expect("minted");

        // Masking is unaffected: it still never reaches the wire.
        let (masked, hit) = a.mask(&format!("token {}", crate::fakes::GITHUB_TOKEN));
        assert_eq!(masked, format!("token {handle}"));
        assert_eq!(hit.len(), 1);

        // But nothing can turn it back into the value.
        let err = a
            .expand_at_exec(&format!("echo {handle}"))
            .expect_err("must refuse");
        assert!(err.contains("not lent for use"), "{err}");

        // And the spendable listing — what the model is told it may use —
        // excludes it, while `secrets()` still reports that it exists.
        assert_eq!(a.spendable().count(), 0);
        assert_eq!(a.secrets().count(), 1);
    }

    /// **A value the shell would act on is refused, not spliced.** The
    /// expansion happens after the gate, so this span is never analysed — a
    /// value carrying `$(`/`` ` ``/`${`/`$name` would change the structure of
    /// the line the human approved, and silently (Reviewer E).
    ///
    /// The premise rows are the point: an ordinary password with punctuation,
    /// spaces, and even a trailing `$` must still work. Only what survives
    /// double quotes is refused.
    #[test]
    fn a_value_the_shell_would_act_on_is_refused_with_a_reason() {
        for ok in [
            crate::fakes::GITHUB_TOKEN,
            "correct horse battery staple",
            "p@ssw0rd!#%^&*-_=+[]|;:,.<>?/",
            "trailing-dollar$",
        ] {
            let mut a = Aliases::default();
            let handle = a.insert("pw", ok, "test", true).expect("minted");
            assert!(
                a.expand_at_exec(&format!("echo {handle}")).is_ok(),
                "premise: an ordinary value must still expand: {ok}"
            );
        }

        // **Every context-ender in the table**, driven from the table so a row
        // added later arrives with a test rather than without one.
        for (ch, needle) in STRUCTURAL {
            let bad = format!("head{ch}tail");
            let mut a = Aliases::default();
            let handle = a.insert("pw", &bad, "test", true).expect("minted");
            let err = a
                .expand_at_exec(&format!("echo {handle}"))
                .expect_err("a context-ender must be refused");
            assert!(err.contains(needle), "unhelpful refusal for {ch:?}: {err}");
        }

        // **The shape that defeated the previous check**, verbatim: a double
        // quote inside a value spliced into the double-quoted position the
        // module's own comment calls the common one. It ends the quoting, and
        // everything after it is a second command (Reviewer E).
        let mut a = Aliases::default();
        let handle = a
            .insert("gh", r#"x" ; id ; echo ""#, "test", true)
            .expect("minted");
        let err = a
            .expand_at_exec(&format!(r#"curl -H "Bearer {handle}""#))
            .expect_err("a value that ends the quoting must be refused");
        assert!(err.contains("end the quoting"), "{err}");
        assert!(
            !err.contains(" ; id ; "),
            "the refusal must not echo the secret: {err}"
        );

        for (bad, needle) in [
            ("has$(id)inside", "runs a command"),
            ("has`id`inside", "backtick"),
            ("has${HOME}inside", "expands a variable"),
            ("has$HOME inside", "expands to a variable"),
        ] {
            let mut a = Aliases::default();
            let handle = a.insert("pw", bad, "test", true).expect("minted");
            let err = a
                .expand_at_exec(&format!("echo {handle}"))
                .expect_err("must refuse");
            assert!(err.contains(needle), "unhelpful refusal for {bad}: {err}");
            assert!(
                !err.contains(bad),
                "the refusal must not echo the secret itself: {err}"
            );
        }
    }

    /// A value is never printed by `Debug` — one `tracing` call would otherwise
    /// write it to a log file. Structural, via [`Secret`]'s redacting `Debug`.
    #[test]
    fn debug_never_prints_a_value() {
        let (a, _) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let rendered = format!("{a:?} {:?}", a.entries()[0]);
        assert!(!rendered.contains("ghp_pretendvalue"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
        assert!(rendered.contains("1 secret"), "{rendered}");
    }

    /// Re-inserting a name rotates the value in place: the handle the model is
    /// already using keeps working, and resolves to the new value.
    #[test]
    fn a_rotated_value_keeps_its_handle() {
        let (mut a, h) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let again = a
            .insert("gh", crate::fakes::GITHUB_TOKEN_ROTATED, "test", true)
            .unwrap();
        assert_eq!(h, again);
        let e = a.expand_at_exec(&format!("echo {h}")).unwrap();
        assert!(e.text.contains(crate::fakes::GITHUB_TOKEN_ROTATED), "{e:?}");
        assert_eq!(a.entries().len(), 1, "rotated, not duplicated");
    }

    /// **`[secrets] env` lends each variable as a secret, and says which are
    /// not set** — through the lookup seam, so no real variable is read or
    /// set (review stop 12, T8 M13: the ingest was tested by nothing). Set
    /// but empty is not set; a lent secret is a secret, never one you
    /// marked (the next launch lends it again); and its value is masked.
    #[test]
    fn secrets_env_lends_each_variable_and_names_the_missing() {
        const VALUE: &str = crate::fakes::GITHUB_TOKEN_LENT;
        let fake = |name: &str| match name {
            "GH_TOKEN" => Some(VALUE.to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        let env = ["GH_TOKEN", "EMPTY", "UNSET"].map(String::from);
        let (table, problems) = Aliases::from_config_with(&[], &env, false, &fake);
        let lent: Vec<&Alias> = table.secrets().collect();
        assert_eq!(lent.len(), 1, "one lent");
        assert_eq!(lent[0].source, "env GH_TOKEN");
        assert!(lent[0].sensitive, "a secret");
        assert!(!lent[0].is_marked(), "lent, not marked: never kept");
        assert_eq!(
            problems,
            [
                "EMPTY: not set in this environment",
                "UNSET: not set in this environment"
            ]
        );
        let (masked, _) = table.mask(&format!("token {VALUE}"));
        assert!(!masked.contains("ghp_pretend"), "{masked}");
    }

    /// **The variables no command may be handed are a secret's, and only a
    /// secret's** — lent by `[secrets] env` or by an `[[alias]]` with `env`
    /// (a secret unless it says not); never a plain alias's variable, whose
    /// value is no one's to hide, nor a secret that came from no variable
    /// (review stop 14, Reviewer E F1).
    #[test]
    fn the_lent_variables_are_every_secrets_and_no_others() {
        let fake = |name: &str| Some(format!("value-of-{name}-0123456789"));
        let spec = |name: &str, env: Option<&str>, sensitive: Option<bool>| crate::AliasSpec {
            name: name.into(),
            value: env.is_none().then(|| format!("value-of-{name}-0123456789")),
            env: env.map(String::from),
            sensitive,
        };
        let specs = [
            spec("db", Some("DATABASE_URL"), None),
            spec("project", Some("PROJECT_DIR"), Some(false)),
            spec("pin", None, Some(true)),
        ];
        let env = ["GH_TOKEN".to_string()];
        let (mut table, problems) = Aliases::from_config_with(&specs, &env, false, &fake);
        assert!(problems.is_empty(), "{problems:?}");
        table
            .insert("typed", "value-typed-0123456789", MARKED_SOURCE, true)
            .unwrap();
        assert_eq!(table.lent_vars(), ["DATABASE_URL", "GH_TOKEN"]);
    }

    // --- $HOME, the one alias shipped on by default ---------------------------

    /// A table with `$HOME` aliased, as a default session has.
    fn with_home() -> (Aliases, String) {
        let home = std::env::var("HOME").expect("a home directory");
        let home = home.trim_end_matches('/').to_string();
        let (table, _) = Aliases::from_config(&[], &[], true);
        (table, home)
    }

    /// The wire form is `~`, not a minted handle: it is a **canonical form that
    /// already exists**, which is the whole reason it is worth shipping on.
    #[test]
    fn home_travels_as_a_portable_tilde() {
        let (table, home) = with_home();
        let (text, hit) = table.mask(&format!("error in {home}/proj/src/a.rs:12"));
        assert_eq!(text, "error in ~/proj/src/a.rs:12");
        assert_eq!(hit, vec!["home".to_string()]);
    }

    /// And it round-trips, including inside quotes — where bash would NOT have
    /// expanded it. That hazard exists only because our masking put `~` in front
    /// of the model, so repairing it is not optional.
    #[test]
    fn a_quoted_tilde_still_resolves() {
        let (table, home) = with_home();
        for (line, want) in [
            ("cat ~/notes.txt", format!("cat {home}/notes.txt")),
            (
                "cat \"~/My Documents/x\"",
                format!("cat \"{home}/My Documents/x\""),
            ),
            ("cd ~", format!("cd {home}")),
        ] {
            assert_eq!(table.expand_at_exec(line).unwrap().text, want, "{line}");
        }
    }

    /// **The cases that decide whether this is safe.** `~` is shell syntax, so
    /// the rules that matter are the shell's: another user's home, bash's
    /// regex-match operator, and an editor's backup file are all NOT `$HOME`.
    #[test]
    fn a_tilde_that_is_not_home_is_left_alone() {
        let (table, home) = with_home();
        for line in [
            "ls ~root/bin",     // a different user's home — never guess
            "[[ $x =~ ^foo ]]", // bash's regex-match operator
            "rm main.rs~",      // an editor leftover
            "echo a~b",         // mid-word
        ] {
            let got = table.expand_at_exec(line).unwrap().text;
            assert_eq!(got, line, "rewrote a non-home tilde: {line}");
            assert!(!got.contains(&home), "{got}");
        }
    }

    /// A user alias for a path *under* home wins, because masking runs longest
    /// value first. The more specific name is the one worth reading.
    #[test]
    fn a_project_alias_beats_the_home_alias() {
        let home = std::env::var("HOME").expect("a home directory");
        let home = home.trim_end_matches('/');
        let spec = crate::AliasSpec {
            name: "repo".to_string(),
            value: Some(format!("{home}/proj")),
            env: None,
            sensitive: None,
        };
        let (table, problems) = Aliases::from_config(std::slice::from_ref(&spec), &[], true);
        assert!(problems.is_empty(), "{problems:?}");
        let (text, _) = table.mask(&format!("{home}/proj/src/a.rs and {home}/other"));
        assert_eq!(text, "{{a:repo}}/src/a.rs and ~/other", "{text}");
    }

    /// Off means off.
    #[test]
    fn home_aliasing_is_switchable() {
        let (table, _) = Aliases::from_config(&[], &[], false);
        assert!(table.is_empty());
    }

    /// **The bug the elegance review found, pinned.** Expanding `~` must be
    /// silent: it is not sensitive, so it is not an event.
    ///
    /// It used to report `spent = ["home"]`, which reached the user as
    /// `secret spent: home` and the model as
    /// `[secret substituted at execution: home]` — on most commands, since
    /// `alias_home` ships on and masking teaches the model to write `~`. That is
    /// the design's own forbidden failure: the notice channel every real secret
    /// depends on, drowned by a non-event.
    #[test]
    fn expanding_a_plain_alias_is_not_an_event() {
        let (table, home) = with_home();
        let e = table.expand_at_exec("cat ~/notes.txt").expect("resolves");
        assert_eq!(e.text, format!("cat {home}/notes.txt"));
        assert!(e.spent.is_empty(), "not an event: {:?}", e.spent);

        // The same at the gate, for a minted plain alias.
        let (mut t2, _) = Aliases::from_config(&[], &[], false);
        let h = t2
            .insert("repo", "/tmp/probe-repo", "config", false)
            .unwrap();
        let e = t2.expand_at_gate(&format!("ls {h}")).unwrap();
        assert_eq!(e.text, "ls /tmp/probe-repo");
        assert!(e.spent.is_empty(), "not an event: {:?}", e.spent);
    }

    /// ...while spending a real secret still is one. The guard is only worth
    /// anything if it still fires where it should.
    #[test]
    fn spending_a_secret_is_still_an_event() {
        let (a, h) = secret("gh", crate::fakes::GITHUB_TOKEN);
        let e = a.expand_at_exec(&format!("curl -H {h} x")).unwrap();
        assert_eq!(e.spent, vec!["gh".to_string()]);
    }

    /// **Into a file, a handle is its value, and nothing else is the
    /// shell's** (S21-Q27) — spent, and said; `~` stays text where a shell
    /// line's becomes the home; a value with a quote and a newline goes in
    /// as it is, which a shell line refuses; an unknown handle still
    /// refuses, naming the write.
    #[test]
    fn into_a_file_a_handle_is_its_value_and_nothing_else_is_the_shells() {
        let (mut a, home) = with_home();
        let h = a.insert("db", "pa\"ss\nword", "test", true).expect("mint");
        let into = a.put_in_file(&format!("PASS={h}\ncd ~/x\n")).unwrap();
        assert_eq!(into.text, "PASS=pa\"ss\nword\ncd ~/x\n");
        assert_eq!(into.spent, vec!["db".to_string()]);
        let line = a.expand_at_exec("cd ~/x").unwrap();
        assert_eq!(
            line.text,
            format!("cd {home}/x"),
            "premise: a line's ~ is the home"
        );
        let why = a.expand_at_exec(&format!("echo {h}")).unwrap_err();
        assert!(
            why.contains("refusing rather than running"),
            "a line refuses it: {why}"
        );
        let unknown = h.replace("db.", "dx.");
        let why = a.put_in_file(&unknown).unwrap_err();
        assert!(why.contains("refusing to write the file"), "{why}");
    }

    /// `resolve` is its own axis: two entries agree on `sensitive` and disagree
    /// on when they resolve. Keying the code on sensitivity is what forced `~`
    /// into a bolted-on second pass.
    #[test]
    fn when_an_entry_resolves_is_not_implied_by_whether_it_is_sensitive() {
        let (mut table, _) = Aliases::from_config(&[], &[], true);
        table
            .insert("repo", "/tmp/probe-repo", "config", false)
            .unwrap();
        let home = table.entries().iter().find(|e| e.name == "home").unwrap();
        let repo = table.entries().iter().find(|e| e.name == "repo").unwrap();
        assert_eq!(home.sensitive, repo.sensitive, "both non-sensitive");
        assert_ne!(
            home.resolve, repo.resolve,
            "and they resolve at different times"
        );
        assert_eq!(home.resolve, ResolveAt::Exec);
        assert_eq!(repo.resolve, ResolveAt::Gate);
    }

    /// Each pass resolves only what is due then — the property the gate display
    /// rule rests on.
    #[test]
    fn a_pass_resolves_only_what_is_due_at_its_point() {
        let (mut table, home) = with_home();
        let s = table
            .insert("gh", crate::fakes::GITHUB_TOKEN, "t", true)
            .unwrap();
        let p = table.insert("repo", "/tmp/probe-repo", "c", false).unwrap();
        let line = format!("cd {p} && cat ~/x && curl -H {s} y");

        let gated = table.expand_at_gate(&line).expect("gate pass");
        assert!(
            gated.text.contains("/tmp/probe-repo"),
            "minted name resolved"
        );
        assert!(
            gated.text.contains("~/x"),
            "`~` still readable: {}",
            gated.text
        );
        assert!(gated.text.contains(&s), "secret still hidden");

        let run = table.expand_at_exec(&gated.text).expect("exec pass");
        assert!(run.text.contains(&format!("{home}/x")));
        assert!(run.text.contains(crate::fakes::GITHUB_TOKEN));
        assert_eq!(
            run.spent,
            vec!["gh".to_string()],
            "only the secret is an event"
        );
    }

    /// **The escape hatch's first layer.** A question about a secret is almost
    /// always a question about its properties, and those can be answered without
    /// the value ever leaving. What the model gets is a fact; what it does not
    /// get is the characters.
    #[test]
    fn a_held_value_can_be_described_without_being_sent() {
        let cfg = crate::SecretsConfig::default();
        let mut table = Aliases::default();
        table
            .insert("db", "Xk7pQ2mZ9vL4nR8tW1yB3cF6hJ0sD5gA", "t", true)
            .unwrap();
        let d = table.describe("db", &cfg).expect("described");

        // Enough to answer "is this strong enough?"...
        assert!(d.contains("32-63 characters"), "{d}");
        assert!(d.contains("lowercase + uppercase + digits"), "{d}");
        assert!(d.contains("randomness"), "{d}");
        // ...and nothing of the value itself.
        assert!(!d.contains("Xk7pQ2mZ"), "{d}");

        // A recognised format is named, which answers "is this the right kind?".
        table
            .insert("gh", "ghp_abcdefghijklmnopqrstuvwxyz0123456789", "t", true)
            .unwrap();
        let d = table.describe("gh", &cfg).expect("described");
        assert!(d.contains("github token"), "{d}");
        assert!(!d.contains("abcdefghij"), "{d}");

        assert!(table.describe("nonexistent", &cfg).is_none());
    }

    /// Length is bucketed on purpose: "about 40" answers every real question and
    /// narrows a search space less than an exact count does.
    #[test]
    fn a_description_is_deliberately_coarse() {
        let cfg = crate::SecretsConfig::default();
        let mut table = Aliases::default();
        table
            .insert("a", "abcdefghijklmnopqrst", "t", true)
            .unwrap();
        table
            .insert("b", "abcdefghijklmnopqrstu", "t", true)
            .unwrap();
        assert_eq!(
            table.describe("a", &cfg),
            table.describe("b", &cfg),
            "two values one character apart must not be distinguishable"
        );
    }
}

/// Property tests for the substitution table.
///
/// The table takes text from two directions it does not control: command output
/// on the way out, and a model- or human-written command on the way in. Both are
/// arbitrary. **"What is our unexpected input is someone else's expected"** —
/// a path with emoji in it, a shell that folds lines, a name in a script we have
/// never rendered.
#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    /// Command-shaped noise, weighted toward the constructs that decide `~`:
    /// another user's home, bash's regex operator, an editor leftover, quotes.
    fn arb_command() -> impl Strategy<Value = String> {
        let atom = prop_oneof![
            Just("~".to_string()),
            Just("~/".to_string()),
            Just("~root".to_string()),
            Just("=~".to_string()),
            Just("file~".to_string()),
            Just("{{".to_string()),
            Just("}}".to_string()),
            Just("{{a:".to_string()),
            Just("{{".to_string()),
            Just("{{a:nope}}".to_string()),
            Just("{{nope.dead}}".to_string()),
            Just("\"".to_string()),
            Just("'".to_string()),
            Just("|".to_string()),
            Just(";".to_string()),
            Just("é€🙂".to_string()),
            Just("\u{202e}".to_string()),
            "[a-zA-Z0-9_./-]{0,10}",
        ];
        prop::collection::vec(atom, 0..30).prop_map(|v| v.join(" "))
    }

    /// Surrounding text with **no handles in it**, for the round-trip property.
    /// An unminted handle anywhere in the input makes resolution refuse — which
    /// is the fail-closed rule working, not a round-trip failure — so it must be
    /// excluded rather than asserted around.
    fn arb_plain_text() -> impl Strategy<Value = String> {
        let atom = prop_oneof![
            Just("~".to_string()),
            Just("\"".to_string()),
            Just("|".to_string()),
            Just("é€🙂".to_string()),
            Just("\u{202e}".to_string()),
            "[a-zA-Z0-9_./-]{0,10}",
        ];
        prop::collection::vec(atom, 0..20).prop_map(|v| v.join(" "))
    }

    /// Names people might write, including ones that must be sanitised away.
    fn arb_name() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("repo".to_string()),
            Just("GITHUB_TOKEN".to_string()),
            Just("a,b..c".to_string()),
            Just("$(id)".to_string()),
            Just("é€🙂".to_string()),
            Just("--".to_string()),
            "[a-zA-Z0-9 ._,$-]{0,16}",
        ]
    }

    /// The scan `secret_ranges` replaced, kept verbatim as its reference: a
    /// slice compared at every char, longest needle first, a hit dropped when
    /// it overlaps an earlier one.
    fn per_char_scan(table: &Aliases, text: &str) -> Vec<(usize, usize)> {
        let mut order: Vec<&str> = table
            .entries
            .iter()
            .filter(|a| a.sensitive)
            .flat_map(|a| [a.value.expose(), a.handle.as_str()])
            .collect();
        order.sort_by_key(|needle| std::cmp::Reverse(needle.len()));
        let chars: Vec<char> = text.chars().collect();
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for needle in order {
            let needle: Vec<char> = needle.chars().collect();
            if needle.is_empty() || needle.len() > chars.len() {
                continue;
            }
            for at in 0..=chars.len() - needle.len() {
                if chars[at..at + needle.len()] != needle[..] {
                    continue;
                }
                let run = (at, at + needle.len());
                if runs.iter().any(|&(lo, hi)| run.0 < hi && lo < run.1) {
                    continue;
                }
                runs.push(run);
            }
        }
        runs.sort_unstable();
        runs
    }

    /// **A hit that overlaps a styled region steps on one char, not past
    /// itself** — so a later, overlapping hit of the same value is still
    /// found. `xxab` (longer, so first) covers `aba`'s hit at 2; its hit at
    /// 4 touches nothing and must be styled. Skipping the rejected hit's
    /// whole length (what `match_indices` does) missed it — and the property
    /// below, as first written, never generated this shape.
    #[test]
    fn a_hit_after_a_rejected_overlapping_one_is_still_styled() {
        let mut table = Aliases::default();
        table.insert("long", "xxab", MARKED_SOURCE, true).unwrap();
        table.insert("short", "aba", MARKED_SOURCE, true).unwrap();
        assert_eq!(table.secret_ranges("xxababa"), vec![(0, 4), (4, 7)]);
        assert_eq!(per_char_scan(&table, "xxababa"), vec![(0, 4), (4, 7)]);
    }

    proptest! {
        /// **The linear `secret_ranges` styles exactly what the per-char scan
        /// did** — overlapping values, periodic ones (`aba` in `ababa`, which
        /// the two-letter alphabet makes common), multibyte text, a handle in
        /// the line (Reviewer B, round 19).
        #[test]
        fn secret_ranges_match_the_per_char_scan(
            values in prop::collection::vec(prop_oneof!["[ab]{1,5}", "[abé日]{1,4}"], 1..4),
            text in prop_oneof!["[ab]{0,40}", "[abé日{}x.]{0,40}"],
            handle in prop::option::of(0usize..4),
        ) {
            let mut table = Aliases::default();
            let handles: Vec<String> = values
                .iter()
                .enumerate()
                .map(|(i, v)| table.insert(&format!("s{i}"), v, MARKED_SOURCE, true).unwrap())
                .collect();
            let text = match handle {
                Some(i) => format!("{text}{}{text}", handles[i % handles.len()]),
                None => text,
            };
            prop_assert_eq!(table.secret_ranges(&text), per_char_scan(&table, &text));
        }
    }

    proptest! {
        /// Resolution runs on text we do not control, at the last moment before
        /// the shell sees it. A panic here kills the command *and* the gate.
        #[test]
        fn resolving_never_panics(cmd in arb_command()) {
            let (table, _) = Aliases::from_config(&[], &[], true);
            let _ = table.expand_at_gate(&cmd);
            let _ = table.expand_at_exec(&cmd);
            let _ = secret_handles_in(&cmd);
        }

        /// **Fail closed, always.** After a successful pass, no handle of the
        /// namespace that pass owns is left over: it was resolved, or the whole
        /// command was refused. A handle that silently survived would reach the
        /// shell as literal text.
        #[test]
        fn a_pass_leaves_no_handle_of_its_own_namespace(cmd in arb_command()) {
            let (mut table, _) = Aliases::from_config(&[], &[], true);
            table.insert("repo", "/tmp/probe-repo", "config", false).unwrap();
            if let Ok(e) = table.expand_at_gate(&cmd) {
                // `{{a:` may remain only as an unterminated fragment, which
                // is not a handle — it has no closing delimiter to resolve to.
                for h in e.text.match_indices("{{a:") {
                    prop_assert!(
                        !e.text[h.0..].contains("}}"),
                        "an alias handle survived: {}", e.text
                    );
                }
            }
        }

        /// A name is reduced to characters that mean nothing to a shell, so a
        /// handle can never carry syntax — and never a `,` or `..`, which would
        /// wake up brace expansion and turn the handle into something else.
        #[test]
        fn a_minted_handle_is_always_shell_inert(name in arb_name()) {
            let mut table = Aliases::default();
            if let Ok(handle) = table.insert(&name, "a-value-long-enough", "t", false) {
                let inner = handle
                    .trim_start_matches("{{a:")
                    .trim_end_matches("}}");
                prop_assert!(
                    inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                    "not inert: {handle}"
                );
                prop_assert!(!inner.contains(".."), "would brace-expand: {handle}");
                prop_assert!(!inner.is_empty());
            }
        }

        /// **The round trip.** A held value that surfaces in output comes back as
        /// its handle, and that handle resolves to the value again — so the model
        /// can act on a result it never saw.
        #[test]
        fn masking_then_resolving_recovers_the_value(
            before in arb_plain_text(),
            after in arb_plain_text(),
        ) {
            let mut table = Aliases::default();
            let value = crate::fakes::GITHUB_TOKEN;
            let handle = table.insert("gh", value, "t", true).unwrap();
            let text = format!("{before} {value} {after}");

            let (masked, hit) = table.mask(&text);
            prop_assert!(!masked.contains(value), "value survived masking");
            prop_assert!(masked.contains(&handle));
            prop_assert_eq!(hit, vec!["gh".to_string()]);

            let back = table.expand_at_exec(&masked).expect("resolves");
            prop_assert_eq!(back.text, text);
        }

        /// `~` is shell syntax, so only the shell's own reading of it is home.
        /// Everything else must come through untouched — rewriting `~root` or
        /// `=~` would be a wrong answer, and for a deny rule a wrong answer is a
        /// bypass.
        #[test]
        fn only_a_shell_tilde_resolves(cmd in arb_command()) {
            let (table, _) = Aliases::from_config(&[], &[], true);
            let Ok(out) = table.expand_at_exec(&cmd) else { return Ok(()) };
            // A tilde the shell would NOT read as home must survive intact.
            // Rewriting `~root` guesses at another user's directory; rewriting
            // `=~` turns a regex test into a path — and for a deny rule, a wrong
            // answer is a bypass.
            prop_assert_eq!(
                cmd.matches("~root").count(),
                out.text.matches("~root").count(),
                "another user's home was rewritten: {}", out.text
            );
            prop_assert_eq!(
                cmd.matches("=~").count(),
                out.text.matches("=~").count(),
                "bash's regex operator was rewritten: {}", out.text
            );
        }

        /// **Masking twice is masking once** (review stop 12, T8 M13) —
        /// over values drawn from a handle's interior (names, hex, `.`) and
        /// text that also holds braces, so a value sits inside handles,
        /// beside them and in their names. Masked text is masked again on
        /// its way to the log and the store; a second pass that changed it
        /// nested a handle in itself.
        ///
        /// The residual, measured here with braces in the values too: a held
        /// value made of a handle's own delimiters (`}{`) can match across
        /// the seam of two handles a first pass put side by side, and a
        /// second pass mangles them — over-matching, never revealing.
        /// Skipping every match that touches a handle would close it and
        /// leave a value straddling a handle's edge in clear.
        #[test]
        fn masking_twice_is_masking_once(
            values in prop::collection::vec("[a-c0-9.]{1,4}", 1..4),
            text in "[a-c0-9.{} ]{0,40}",
        ) {
            let mut t = Aliases::default();
            for (i, value) in values.iter().enumerate() {
                let _ = t.insert(&format!("s{i}"), value, "marked by you", true);
            }
            let (once, _) = t.mask(&text);
            prop_assert_eq!(&t.mask(&once).0, &once, "from {:?}", text);
            // And the half that matters: every held value left in the text
            // is part of a handle, never the value standing in the clear.
            let handles: Vec<&str> = t.entries().iter().map(|e| e.handle.as_str()).collect();
            let spans = spans_of(&once, &handles);
            for entry in t.entries() {
                let value = entry.value.expose();
                for (at, _) in once.match_indices(value) {
                    let inside = spans.iter().any(|s| s.start <= at && at + value.len() <= s.end);
                    prop_assert!(inside, "{:?} left in the clear in {:?}", value, once);
                }
            }
        }
    }
}
