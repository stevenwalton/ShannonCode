//! The permission engine: decide whether a proposed command line may run.
//!
//! This sits on top of [`shell`](crate::shell). The analyzer answers *what is
//! this command* (a [`Pipeline`] of structural [`Stage`]s); the models
//! ([`model`]) say what a program does with its words; this module answers
//! *may it run* by reading each stage as ACTS on targets ([`acts`]) — a
//! program at a node of its tree, a path at an operation, a host at a
//! direction — and deciding each act by the rules that apply to it
//! ([`decide`]). The split is deliberate: the analyzer and the models are
//! spelling- and order-invariant fact, and stay pure; policy — which the
//! user owns and edits — lives here.
//!
//! # The decision, in one paragraph
//!
//! For each act of each stage, the rules that apply are resolved by TRUST
//! then SPECIFICITY: the user's own rules decide, the project's and the
//! shipped ones only narrow them (an ask or a deny of theirs stays unless a
//! rule of the user's is at least as specific), and among the deciding rules
//! the most specific — by inclusion: a longer chain, more flags, a path
//! below — displace the broader, the strictest of what is left deciding. An
//! allow on a program covers the act only if every node it touched is
//! covered (containment). With no rule, the defaults (design §8.1, Q7): a
//! path inside where Shannon started allows at read and write and asks at
//! delete and exec, outside it asks; a modelled program at a read node
//! allows and past a read asks; an unmodelled program asks; a host asks. A
//! secret handle, a privilege wrapper, a stage the gate cannot read soundly,
//! a word it cannot place and code it does not read ask whatever a rule
//! allows. The line's verdict is its strictest stage — a line is only as
//! safe as its worst part.
//!
//! # Why the default is safe without a giant deny list
//!
//! Because the fallthrough already **asks** for anything that isn't a
//! modelled read inside the project, an empty rule set never silently runs a
//! mutation, and a program Shannon does not model never runs unasked. The
//! shipped rules (`defaults`) are GUARDS beneath a broad allow of yours —
//! the credential paths, the harness's own surfaces, `rm --no-preserve-root`
//! — lifted only by a rule naming the place. A flag that makes a program
//! dangerous (`find -exec`) needs no shipped rule: the model marks it, and
//! an allow that does not name it does not cover it.

pub mod acts;
pub mod decide;
mod defaults;
pub mod line;
pub mod model;
mod rule;

pub use decide::{ActKind, ActVerdict, By, Call, Hard, ModelRef, NoRule, StageVerdict};
pub(crate) use rule::one_line;
pub use rule::{Cited, Decision, Guard, Origin, Rule};

use std::sync::Arc;

use crate::shell::{Pipeline, Stage, Unanalyzable, parse_line};
use acts::{Context, Dir, PathAct};
use decide::{Gate, Policy};
use line::Tier;
use model::{Effect, Models};

/// A `~` at the front of a path as the home it means — the home Shannon
/// runs under. What a pattern compares a path by (`pattern::PathRe`).
pub(crate) fn expand_home(path: &str) -> String {
    match crate::secrets::home_dir() {
        Some(home) => acts::expand_home(path, home),
        None => path.to_string(),
    }
}

/// Where a relative path is placed when Shannon could not say where it
/// started: a directory that cannot exist, so a relative word stays "inside"
/// (as a relative `cd` does) and nothing absolute does — the fail-closed
/// reading, kept from the old engine.
const NO_START: &str = "/\u{1}no-start";

/// A policy: an ordered list of rules. Order is irrelevant to the *outcome*
/// (specificity and trust decide, not position) but is preserved for display
/// and editing.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
    /// Names `[exec] env` sets for the command. The gate's own table is not
    /// the command's for them, so a word naming one stays unplaced (review
    /// 2026-10-10, A and B: `$TMPDIR/x` placed under the gate's TMPDIR
    /// while the command wrote under the configured one).
    exec_env: Vec<String>,
    /// **The variables of Shannon's environment whose value is held as a
    /// secret** — `(name, handle)` ([`Aliases::held_vars`](crate::Aliases::held_vars)):
    /// a line naming one faces a human, as a lent one's does (the close of
    /// review stop 14, V 3). The process's environment, which the engine
    /// does not read itself: the app hands it over.
    held_vars: Vec<(String, String)>,
    /// Credential policy. Lives here because "does this command touch something
    /// that holds secrets" is a *permission* question — and once command output
    /// leaves the machine, a read of `~/.ssh/id_ed25519` is the dangerous case
    /// that the effect lattice alone calls harmless (`docs/DesignDocs/secrets.md`).
    /// Every entry is a shipped guard ([`defaults::credential_lines`]).
    secrets: crate::SecretsConfig,
    /// **Shannon's own state** — literal paths launch registered from each
    /// resolver — guarded beside `[secrets]` but read as literals, never as
    /// that list's globs ([`with_own_state`](Self::with_own_state)).
    own_state: Vec<String>,
    /// **Claude Code's home**, in each spelling launch resolved — what
    /// `{claude-home}` names in a `[secrets]` entry
    /// ([`with_claude_home`](Self::with_claude_home)).
    claude_home: Vec<String>,
    /// The session's substitution table, consulted **only** to tell a handle we
    /// minted from one we did not — a distinction the prompt must make, because
    /// an unminted handle is the shape of an injected instruction.
    ///
    /// The table itself rather than a copied list of handle strings: two lists
    /// drift silently, and the drift would land on the side that says "this is
    /// fine". No value is reachable from here — [`Aliases`](crate::Aliases)
    /// exposes only names and handles.
    aliases: Arc<crate::Aliases>,
    /// The shipped rules `[permissions] remove` took out ([`configured_by`]).
    ///
    /// [`configured_by`]: Self::configured_by
    removed_shipped: Vec<Rule>,
    /// What `[permissions]` asked for and this policy could not honour, as
    /// sentences ([`configured_by`]) — kept, so what `:rules` shows is read
    /// off the policy the gate holds, never re-derived from the config's
    /// strings.
    ///
    /// [`configured_by`]: Self::configured_by
    unhonoured: Vec<String>,
    /// Where Shannon started — absolute — or `None` when it could not say.
    /// What every relative path is placed against, and what "inside" means
    /// ([`with_start`](Self::with_start)).
    start: Option<String>,
    /// The scratch directory, in each spelling a command may use — a place
    /// granted at every operation ([`with_scratch`](Self::with_scratch)).
    scratch: Vec<String>,
    /// The models the acts are read by: the shipped set unless a caller
    /// supplies one ([`with_models`](Self::with_models) — the acceptance
    /// suite's held-out models; a user's own directory in P6b).
    models: Option<Arc<Models>>,
}

impl RuleSet {
    /// An empty policy — every command falls to the defaults (a modelled read
    /// inside the project runs, everything else asks). Useful as a base to
    /// add to.
    pub fn empty() -> RuleSet {
        RuleSet::default()
    }

    /// The shipped default policy: the guards and the whole-command allows
    /// (see the `defaults` module).
    pub fn defaults() -> RuleSet {
        RuleSet {
            rules: defaults::fixed(),
            ..RuleSet::default()
        }
    }

    /// As [`configured_by`](Self::configured_by), by the shipped set — for
    /// tests: code says which models (review 2026-10-11, C).
    #[cfg(test)]
    pub fn configured(
        permissions: &crate::config::PermissionsConfig,
        session: &[Rule],
        removed: &[Rule],
    ) -> (RuleSet, Vec<String>) {
        Self::configured_by(permissions, session, removed, None)
    }

    /// **The policy a `[permissions]` section asks for**, every rule read by
    /// `models` — the shipped set with a user's directory over it
    /// ([`Models::with_user_dir`]); `None`, the shipped set: the shipped
    /// defaults minus what `remove` names, then its `rules`, then `session`
    /// (the rules above the config's — memory's, skills', those typed at
    /// `:allow` / `:deny` / `:ask` — read again from their text, so one read
    /// before a model changed reads as the command will), less any rule
    /// `removed` this session (`:unallow` / `:unask` / `:undeny`, whatever
    /// its origin, read by the same models).
    ///
    /// Returns each entry it could not honour, as a sentence. A rule that does
    /// not parse is skipped, and so is a session rule that no longer reads. So
    /// is a `remove` that names no shipped rule: the rule stays, which is the
    /// safe side, and saying so is what keeps a typo from reading as a
    /// decision that took effect. The app says the sentences where it builds
    /// the gate (`rebuild_rules`); the policy keeps them too
    /// ([`unhonoured`](Self::unhonoured)), with the shipped rules `remove`
    /// took out ([`removed_shipped`](Self::removed_shipped)).
    pub fn configured_by(
        permissions: &crate::config::PermissionsConfig,
        session: &[Rule],
        removed: &[Rule],
        models: Option<Arc<Models>>,
    ) -> (RuleSet, Vec<String>) {
        let reader = models.as_deref().unwrap_or_else(|| Models::shipped());
        let (mut rules, removed_shipped, mut problems) =
            Self::shipped_after_by(&permissions.remove, reader);
        for line in &permissions.rules {
            match Rule::read_by(Origin::Config, line, reader) {
                None => {}
                // (A rule naming a flag only as written is in force, and
                // says so where it is listed — `Rule::as_written_note` — not
                // here: it is honoured, narrower than it reads.)
                Some(Ok(rule)) => rules.push(rule),
                Some(Err(why)) => problems.push(format!(
                    "permissions.rules `{}`: {why}",
                    rule::one_line(line)
                )),
            }
        }
        let (session, refused) = Self::reread_all(session.iter().cloned(), reader);
        rules.extend(session);
        problems.extend(refused);
        // What this session removed, read by the same models as what it is
        // compared with: compared as read, a rule removed under other models
        // matched nothing and came back (review 2026-10-11, A). (One that no
        // longer reads keeps its reading: a rule of the same text no longer
        // reads either, and is not here to match.)
        let removed: Vec<Rule> = removed
            .iter()
            .map(|gone| gone.reread(reader).unwrap_or_else(|_| gone.clone()))
            .collect();
        rules.retain(|rule| !removed.iter().any(|gone| gone.same(rule)));
        let set = RuleSet {
            rules,
            removed_shipped,
            unhonoured: problems.clone(),
            models,
            ..RuleSet::default()
        };
        (set, problems)
    }

    /// **The rules the project's file holds, named as its**
    /// (`Origin::Project`). One tier with your config's (Steven, 10-13: a
    /// project file's rules are the user's own), so this changes what
    /// `:rules` and a pane say — which file to edit — never a decision.
    /// `lines` are the project layers' own (`ConfigStack::project_rules`).
    pub fn with_project_rules(mut self, lines: &[String]) -> RuleSet {
        // As the one reader keeps a line (`Rule::read_by`): its `#` comment
        // off, trimmed — a commented project rule was named yours, and
        // `:unallow` said "(your config)" (review 2026-10-13, A).
        let held: Vec<String> = lines
            .iter()
            .map(|l| rule::strip_comment(l).trim().to_string())
            .collect();
        for rule in self.rules.iter_mut().filter(|r| r.origin == Origin::Config) {
            if held.iter().any(|l| *l == rule.source.trim()) {
                rule.origin = Origin::Project;
            }
        }
        self
    }

    /// The shipped rules `[permissions] remove` took out of this policy.
    pub fn removed_shipped(&self) -> &[Rule] {
        &self.removed_shipped
    }

    /// What `[permissions]` asked for and this policy could not honour — a
    /// rule that did not parse, a `remove` that named no shipped rule — as
    /// the sentences [`configured_by`](Self::configured_by) returned.
    pub fn unhonoured(&self) -> &[String] {
        &self.unhonoured
    }

    /// **Rules read again by `models`**, each from its text, keeping where
    /// it came from — the one policy for every rule read before the models
    /// in force were known (the session's, the shipped set's): a shipped
    /// rule that no longer reads keeps its shipped reading (a model never
    /// removes a shipped rule, `remove` does; defensive — no shipped rule
    /// gives a flag a value, the one reading a model can refuse); any other
    /// is out of force, and said.
    fn reread_all(
        rules: impl IntoIterator<Item = Rule>,
        models: &Models,
    ) -> (Vec<Rule>, Vec<String>) {
        let (mut kept, mut said) = (Vec::new(), Vec::new());
        for rule in rules {
            match rule.reread(models) {
                Ok(read) => kept.push(read),
                Err(_) if rule.origin == Origin::Shipped => kept.push(rule),
                Err(why) => said.push(match rule.origin {
                    Origin::Session => format!("the session's rule `{}`: {why}", rule.one_line()),
                    _ => format!("`{}`: {why}", rule.one_line()),
                }),
            }
        }
        (kept, said)
    }

    /// The shipped rules after `[permissions] remove`, read by `models`:
    /// those kept, those removed, and each name that removed nothing or could
    /// not be read.
    ///
    /// **The one reading of `remove`**, for the gate
    /// ([`configured_by`](Self::configured_by)); `:rules` lists what it
    /// removed ([`removed_shipped`](Self::removed_shipped)), so the listing
    /// cannot describe a removal the gate did not make. Compared as parsed
    /// rules, so a flag may be named by any of its spellings. A former
    /// carving (`ask find:-exec`) is no shipped rule any more — the model
    /// carries it — and the sentence says what lifts it instead.
    pub fn shipped_after_by(
        remove: &[String],
        models: &Models,
    ) -> (Vec<Rule>, Vec<Rule>, Vec<String>) {
        let (fixed, _) = Self::reread_all(defaults::fixed(), models);
        let (mut kept, mut removed, mut problems) = (fixed, Vec::new(), Vec::new());
        for named in remove {
            match Rule::read_by(Origin::Config, named, models) {
                None => {}
                Some(Err(why)) => problems.push(format!(
                    "permissions.remove `{}`: {why}",
                    rule::one_line(named)
                )),
                Some(Ok(gone)) => {
                    let (out, stay): (Vec<Rule>, Vec<Rule>) =
                        kept.into_iter().partition(|r| r.same(&gone));
                    kept = stay;
                    if out.is_empty() && !removed.iter().any(|r: &Rule| r.same(&gone)) {
                        let carving = gone.action == Decision::Ask
                            && gone.program().is_some_and(|p| p != "*");
                        problems.push(if carving {
                            format!(
                                "permissions.remove `{}` is not a shipped rule — a flag that \
                                 makes a program dangerous asks beneath any allow that does \
                                 not name it; `allow {} *`, or an allow naming the flag, \
                                 lifts it, and the places it deletes or writes are the path \
                                 rules' (`allow :delete <dir>`)",
                                rule::one_line(named),
                                gone.program().unwrap_or("<program>")
                            )
                        } else {
                            format!(
                                "permissions.remove `{}` is not a shipped rule — nothing was \
                                 removed (`:rules` lists them)",
                                rule::one_line(named)
                            )
                        });
                    }
                    removed.extend(out);
                }
            }
        }
        (kept, removed, problems)
    }

    /// Install the user's credential policy (sensitive paths and what to do
    /// about them).
    pub fn with_secrets(mut self, secrets: crate::SecretsConfig) -> RuleSet {
        self.secrets = secrets;
        self
    }

    /// **Shannon's own state**: the paths launch registered (its config and
    /// models, its key, log and store), each guarded as a credential is —
    /// asked, or refused under `path_policy = "deny"`, and a write asked
    /// even under `"allow"` — and each read as the literal path it is, where
    /// `[secrets] paths` are globs (a space or a `[` in it once made the
    /// guard protect nothing; review 2026-10-11, E2).
    pub fn with_own_state(mut self, paths: Vec<String>) -> RuleSet {
        self.own_state = paths;
        self
    }

    /// **Claude Code's home**, as launch resolved it once
    /// (`claude_import::claude_home`: `$CLAUDE_CONFIG_DIR`, else
    /// `~/.claude`), with its real path where a link leads elsewhere — each
    /// a literal. What `{claude-home}` fills in a `[secrets]` entry
    /// (`secrets::CLAUDE_HOME`); none known, such an entry holds nothing.
    pub fn with_claude_home(mut self, spellings: Vec<String>) -> RuleSet {
        self.claude_home = spellings;
        self
    }

    /// **What the guards could not hold**, as sentences: a `[secrets]`
    /// entry the rule line cannot read, a `{claude-home}` misplaced or with
    /// no home to name. They were read and dropped — an entry with a space
    /// protected nothing, and nothing said so.
    pub fn guard_notes(&self) -> Vec<String> {
        defaults::credential_bodies(&self.secrets, &self.own_state, &self.claude_home).1
    }

    /// The same policy with `policy` at a credential path — for a call judged
    /// under a different credential policy than the session's own (one Claude
    /// Code runs: `[claude_code] path_policy`).
    pub fn with_path_policy(mut self, policy: crate::PathPolicy) -> RuleSet {
        self.secrets.path_policy = policy;
        self
    }

    /// Install the session's substitution table, so the prompt can say whether a
    /// handle in a command is one we actually hold.
    pub fn with_aliases(mut self, aliases: Arc<crate::Aliases>) -> RuleSet {
        self.aliases = aliases;
        self
    }

    /// **Where Shannon started** — an absolute path (`current_dir`): the
    /// directory every command's shell begins in, where every relative path
    /// is placed, and what "inside" means. Without it, nothing absolute can
    /// be said to be inside (it asks; never the reverse).
    pub fn with_start(mut self, dir: &str) -> RuleSet {
        self.start = Some(crate::shell::pipeline::normalize_path(dir));
        self
    }

    /// **The scratch directory** (`crate::scratch`): a place granted at
    /// every operation, as where Shannon started is — the agent's own
    /// (Steven, 10-06). Only one `scratch::prepare` vouched for. Its
    /// spellings are one place to the gate (`/tmp/…` and `/private/tmp/…`
    /// on macOS), granted once.
    pub fn with_scratch(mut self, spellings: Vec<String>) -> RuleSet {
        self.scratch = spellings
            .iter()
            .map(|dir| crate::shell::pipeline::normalize_path(dir))
            .collect();
        self.scratch.sort();
        self.scratch.dedup();
        self
    }

    /// The names `[exec] env` sets for the command: the gate's own values
    /// for them are not the command's, so they are not substituted.
    pub fn with_exec_env(mut self, names: Vec<String>) -> RuleSet {
        self.exec_env = names;
        self
    }

    /// The variables of Shannon's environment whose value is held as a
    /// secret, with its handle ([`Aliases::held_vars`](crate::Aliases::held_vars)).
    pub fn with_held_vars(mut self, vars: Vec<(String, String)>) -> RuleSet {
        self.held_vars = vars;
        self
    }

    /// The gate's reading of the command's environment: the executor's
    /// table, less every name `[exec] env` overrides.
    fn env_table(&self) -> std::collections::BTreeMap<String, String> {
        let mut env = decide::env_table(&[]);
        for name in &self.exec_env {
            env.remove(name);
        }
        env
    }

    /// The models the acts are read by, in place of the shipped set — and
    /// the rules, read again by them. A rule they cannot read is out of
    /// force and said ([`unhonoured`](Self::unhonoured)); a shipped one keeps
    /// its shipped reading. Production builds through
    /// [`configured_by`](Self::configured_by), which reads each rule once.
    pub fn with_models(mut self, models: Arc<Models>) -> RuleSet {
        let (rules, refused) = Self::reread_all(std::mem::take(&mut self.rules), &models);
        self.rules = rules;
        self.unhonoured.extend(refused);
        self.models = Some(models);
        self
    }

    /// The models the rules and the commands are read by: the shipped set,
    /// unless [`configured_by`](Self::configured_by) or
    /// [`with_models`](Self::with_models) gave others.
    pub fn models(&self) -> &Models {
        self.models.as_deref().unwrap_or_else(|| Models::shipped())
    }

    /// Read and append rules from lines, through [`Rule::read_by`] — the one
    /// reader every source uses — by the models this policy reads commands
    /// by ([`models`](Self::models)), returning a human-facing error on the
    /// first malformed line. Tests build policy this way (after
    /// [`with_models`](Self::with_models), which a harness of fixtures sets
    /// first); production reads config and session rules through
    /// [`configured_by`](Self::configured_by).
    pub fn extend_from_lines(&mut self, text: &str) -> Result<(), String> {
        for line in text.lines() {
            if let Some(rule) = Rule::read_by(Origin::Session, line, self.models()) {
                self.rules.push(rule?);
            }
        }
        Ok(())
    }

    /// The rules, in declaration order (for inspection / the in-app editor).
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// **The guards held beyond the rules**: each `[secrets] paths` entry
    /// and each place of Shannon's own state, as the line the decision
    /// reads it as — for `:rules`, which listed only the rules, so the
    /// guard that held `cat ~/.ssh/config` was nowhere on the page. One
    /// reading with the decision's (`credential_bodies`); an entry the line
    /// cannot read is said at the config load, not here.
    pub fn guard_lines(&self) -> Vec<(String, Guard)> {
        defaults::credential_bodies(&self.secrets, &self.own_state, &self.claude_home)
            .0
            .into_iter()
            .map(|(line, _, guard)| (line, guard))
            .collect()
    }

    /// The places `[secrets] paths_except` carves out of every guard.
    pub fn guard_exceptions(&self) -> &[String] {
        &self.secrets.paths_except
    }

    /// **The places granted beyond the rules**, as rule lines: the scratch
    /// directory — read as a place, and anything to what is in it, not its
    /// own removal (`rm -rf <scratch>` was not inside it; review of f50fda5,
    /// Reviewers A, B). The decision reads these same lines.
    pub fn grant_lines(&self) -> Vec<String> {
        self.scratch
            .iter()
            .flat_map(|dir| [format!("allow :read {dir}"), format!("allow :* {dir}/*")])
            .collect()
    }

    /// **The policy in force**, as the decision reads it: every rule at its
    /// tier, the credential guards the `[secrets]` configuration asks for,
    /// and the scratch directory as a place granted. Built per decision —
    /// a few dozen lines read by the models, well under the pane's budget —
    /// so no copy of it can go stale behind a builder.
    fn policy(&self) -> Policy {
        let mut policy = Policy::default();
        let shipped = |source: &str| Cited {
            action: Decision::Allow,
            source: source.to_string(),
            origin: Origin::Shipped,
            guard: None,
            reached: false,
        };
        for rule in &self.rules {
            let parsed = line::Parsed {
                action: rule.action,
                bodies: rule.bodies().to_vec(),
            };
            policy.push(parsed, rule.tier(), rule.cited());
        }
        // An entry the line cannot read is in the notes, said where the app
        // builds the gate (`guard_notes`); here the guards in force.
        let (guards, _notes) =
            defaults::credential_bodies(&self.secrets, &self.own_state, &self.claude_home);
        for (text, parsed, guard) in guards {
            let mut cited = shipped(&text);
            cited.action = parsed.action;
            cited.guard = Some(guard);
            policy.push(parsed, Tier::Shipped, cited);
        }
        for text in self.grant_lines() {
            if let Ok(parsed) = line::parse(&text, Models::shipped()) {
                policy.push(parsed, Tier::Shipped, shipped(&text));
            }
        }
        policy
    }

    /// Decide a whole command line from its text. Fails **closed**: a line the
    /// analyzer refuses ([`Unanalyzable`] — substitution, control flow, an
    /// unterminated quote) becomes an [`Decision::Ask`] carrying the reason, so
    /// the human sees the command and the *why* rather than it running or
    /// vanishing.
    pub fn decide_line(&self, input: &str) -> Verdict {
        match parse_line(input) {
            Ok(pipeline) => self.decide_parsed(input, &pipeline),
            Err(why) => Verdict {
                decision: Decision::Ask,
                outcome: Err(why),
            },
        }
    }

    /// **The verdict of a line already parsed** — `input` is the text
    /// `pipeline` was parsed from. The one door for every reader of a
    /// line's verdict: the gate (`decide_line`), the proposals
    /// (`stage_proposals`), the decision pane's report and its "still asks"
    /// (shannon-tui `permission_view`). They called the pipeline's own
    /// `decide`, blind to the raw scan below, so a handle in a comment
    /// asked at the gate while the pane headed it ALLOW, "runs without
    /// asking", and `A` said nothing asks (review stop 12, T8 M8).
    pub fn decide_parsed(&self, input: &str, pipeline: &Pipeline) -> Verdict {
        let mut verdict = self.decide(pipeline);
        // The executor expands the RAW line. A handle the parser's fields
        // dropped — a `for` list, `{"{{tok.X}}"` with its quotes removed, a
        // comment — is spent all the same, so the raw text is scanned too
        // and faces a human (review 2026-10-10, E2). The stage reader's own
        // scan stays: it names the stage.
        if let Some(handle) = crate::secrets::secret_handles_in(input).first()
            && let Ok(stages) = &mut verdict.outcome
            && !stages.iter().any(|s| {
                s.acts
                    .iter()
                    .any(|a| matches!(a.act, ActKind::Secret { .. }))
            })
            && let Some(first) = stages.first_mut()
        {
            first.acts.push(ActVerdict {
                act: ActKind::Secret {
                    handle: handle.to_string(),
                    known: self.aliases.knows(handle),
                },
                decision: Decision::Ask,
                by: By::Hard(Hard::Secret),
                uncovered: Vec::new(),
                model: None,
                at: decide::Written {
                    verb: Vec::new(),
                    cause: input
                        .find(handle)
                        .map(|at| at..at + handle.len())
                        .into_iter()
                        .collect(),
                },
                reaches: Vec::new(),
            });
            first.decision = first.decision.max(Decision::Ask);
            verdict.decision = verdict.decision.max(Decision::Ask);
        }
        // **A line that names a variable a secret was lent from faces a
        // human**, whatever a rule allows (Steven, 10-31, S18-Q1): the raw
        // line, as the handle's scan reads it, so a comment, a quote or
        // another program's code (`ENVIRON["…"]`) is read too.
        // And one that holds a held value, however it came to (V 3).
        let mut lent: Vec<(String, String)> = self
            .aliases
            .entries()
            .iter()
            .filter_map(|a| a.lent_from().map(|v| (v.to_string(), a.handle.clone())))
            .collect();
        for (name, handle) in &self.held_vars {
            if !lent.iter().any(|(n, _)| n == name) {
                lent.push((name.clone(), handle.clone()));
            }
        }
        if let Ok(stages) = &mut verdict.outcome
            && let Some(first) = stages.first_mut()
        {
            for (name, handle) in lent {
                let Some(at) = names_variable(input, &name) else {
                    continue;
                };
                first.acts.push(ActVerdict {
                    act: ActKind::LentVar { name, handle },
                    decision: Decision::Ask,
                    by: By::Hard(Hard::Secret),
                    uncovered: Vec::new(),
                    model: None,
                    at: decide::Written {
                        verb: Vec::new(),
                        cause: vec![at],
                    },
                    reaches: Vec::new(),
                });
                first.decision = first.decision.max(Decision::Ask);
                verdict.decision = verdict.decision.max(Decision::Ask);
            }
        }
        verdict
    }

    /// Decide an already-parsed [`Pipeline`]: every stage by its acts —
    /// blind to the raw line, which only [`decide_parsed`](Self::decide_parsed)
    /// reads, so not offered outside this crate.
    pub(crate) fn decide(&self, pipeline: &Pipeline) -> Verdict {
        let policy = self.policy();
        let env = self.env_table();
        let stages: Vec<StageVerdict> = self.with_gate(&policy, &env, |gate| {
            pipeline
                .stages
                .iter()
                .map(|s| decide::decide_stage(gate, s))
                .collect()
        });
        Verdict {
            decision: decide::decision_of(&stages),
            outcome: Ok(stages),
        }
    }

    /// Run `f` with a gate over `policy` — the context built on the stack,
    /// no leak.
    fn with_gate<T>(
        &self,
        policy: &Policy,
        env: &std::collections::BTreeMap<String, String>,
        f: impl FnOnce(&Gate) -> T,
    ) -> T {
        let cx = Context {
            start: self.start.as_deref().unwrap_or(NO_START),
            home: crate::secrets::home_dir().unwrap_or("/"),
            models: self.models(),
            env,
        };
        let knows = |handle: &str| self.aliases.knows(handle);
        let gate = Gate {
            policy,
            cx: &cx,
            knows: &knows,
        };
        f(&gate)
    }

    /// **Is `path` a credential path to this policy** — `[secrets]` and
    /// Shannon's own state, as every command's words are judged — and by
    /// which entry? `None` under `path_policy = "allow"`, the user's "treat
    /// these as any path", and where a rule of the user's at least as specific
    /// lifts the guard. What memory and skills ask before reading a file
    /// (`knowledge::find`), so a location can never send one to the model.
    pub fn credential_path(&self, path: &str) -> Option<String> {
        let policy = self.policy();
        let env = self.env_table();
        self.with_gate(&policy, &env, |gate| {
            let absolute = acts::absolute(gate.cx.start, path, gate.cx.home);
            let act = PathAct {
                op: Some(Effect::Read),
                paths: vec![absolute.clone()],
                word: path.to_string(),
                program: String::new(),
                redirect: false,
                // Memory and skills READ the file: what it holds is sent.
                listing: false,
                at: Vec::new(),
            };
            let verdict = decide::decide_path_alone(gate, &act);
            // Held by a rule that is not an allow: the shipped entry when
            // one is cited — else, when a stricter rule of the user's
            // displaced the guard (`deny :* ~/.ssh`), the shipped entry the
            // path still falls under: displacement by a deny or an ask
            // never loosens this answer (review 2026-10-10, E1). A user's
            // own deny on a path that is no credential (a disabled skill's
            // file) is not one, and stays None. A guard met from above beside
            // the place's own account (`reaches`) holds it as ever.
            let held: Vec<&Cited> = match &verdict.by {
                By::Rules(cited) => cited.iter().collect(),
                _ => Vec::new(),
            };
            let held: Vec<&Cited> = held.into_iter().chain(&verdict.reaches).collect();
            match verdict.decision {
                d if d != Decision::Allow && !held.is_empty() => held
                    .iter()
                    .find(|c| c.guard.is_some())
                    .map(|c| defaults::credential_entry(&c.source))
                    .or_else(|| {
                        policy.lines.iter().find_map(|line| match &line.body {
                            line::Body::Path { pat, except, .. }
                                if line.cited.guard.is_some()
                                    && pat
                                        .resolve(gate.cx.start, gate.cx.home)
                                        .matches(&absolute)
                                    && !except.iter().any(|e| {
                                        e.resolve(gate.cx.start, gate.cx.home).matches(&absolute)
                                    }) =>
                            {
                                Some(defaults::credential_entry(&line.cited.source))
                            }
                            _ => None,
                        })
                    }),
                _ => None,
            }
        })
    }

    /// **The rule `A` offers for `line`**: the first of
    /// [`proposed_allows`](Self::proposed_allows). Once it is taught, the
    /// line is proposed again, and the next one is offered.
    pub fn proposed_allow(&self, line: &str) -> Result<String, NoProposal> {
        self.proposed_allows(line).map(|mut rules| rules.remove(0))
    }

    /// **Every rule `A` will offer for `line`, in order**: the first asking
    /// stage's ([`stage_proposals`](Self::stage_proposals)) — or why there
    /// are none — then each later stage's, as it will be offered once those
    /// before it are taught. The list ends where nothing asks, or at a stage
    /// no rule answers: the call still asks there, which the pane says. A
    /// second stage asking after the first's rules were taught was a
    /// surprise the pane did not warn of (P5, "and N more").
    pub fn proposed_allows(&self, line: &str) -> Result<Vec<String>, NoProposal> {
        let mut all = self.stage_proposals(line)?;
        let mut taught = self.with_allows(&all)?;
        // Each answer allows its stage, so the next one asking is a later
        // stage: one round per stage at most.
        let stages = parse_line(line).map_or(0, |p| p.stages.len());
        for _ in 1..stages {
            let Ok(next) = taught.stage_proposals(line) else {
                break;
            };
            taught = taught.with_allows(&next)?;
            all.extend(next);
        }
        Ok(all)
    }

    /// This policy with `texts` taught as `:allow` will read them, by the
    /// models in force.
    pub fn with_allows(&self, texts: &[String]) -> Result<RuleSet, NoProposal> {
        let mut with = self.clone();
        for text in texts {
            match Rule::parse_by(&format!("allow {text}"), self.models()) {
                Ok(rule) => with.rules.push(rule),
                Err(why) => return Err(NoProposal::CannotDescribe(why)),
            }
        }
        Ok(with)
    }

    /// **The narrowest allow rules that answer `line`**, as rule text
    /// without the verb, for the first stage this policy does not already
    /// allow — or why there are none.
    ///
    /// They name what asked, and only that: the program's form when its act
    /// asks — its chain, the flags an allow left uncovered or a writer used
    /// (`rm -r -f`), its data operands as written; for a program Shannon
    /// does not model, its bare words and a `*` for its flags (`make test
    /// CC=cc *`), never the program alone — then each place that asks, as a
    /// path rule at its operation (`:delete build`, `:write /tmp/out.json`);
    /// or a host at its direction (`:fetch example.org`). A program's form
    /// never carries a place (Steven, 10-10). They are offered only if THIS
    /// policy plus all of them decides the stage Allow, so they never answer
    /// less than the call they were proposed for.
    ///
    /// A hard ask — a secret handle, a privilege wrapper, a stage the gate
    /// cannot read, a word it cannot place, code the model does not read —
    /// and a rule that refuses are said up front
    /// ([`NoProposal::RuleCannotAnswer`]): no allow lifts them, and a wider
    /// rule would not help, which is the thing to say.
    ///
    /// **A credential is never a rule's word, nor quoted in a reason.** A
    /// rule keeps its words as written, and `:config save` writes them into
    /// the project's file, beside its code — the shape Apiiro measured in
    /// Claude Code's allowlists, 1,122 public files holding a real secret
    /// (11-08). Asked here, of what the answer says, at the only way out: at
    /// each word it was asked of, a word nobody remembered went through (a
    /// flag written with its value, review stop 17, D M1). The reason names
    /// where the value sat, never the value: a refusal reaches the screen
    /// and the log.
    pub fn stage_proposals(&self, line: &str) -> Result<Vec<String>, NoProposal> {
        let answer = self.stage_proposals_as_written(line);
        let Ok(pipeline) = parse_line(line) else {
            return answer;
        };
        let credentials: Vec<(&str, String)> = (pipeline.stages.iter())
            .flat_map(|stage| credential_words(stage, &self.secrets, &self.aliases))
            .collect();
        let says = |value: &str| match &answer {
            Ok(rules) => (rules.iter().flat_map(|rule| rule.split_whitespace())).any(|word| {
                word == value
                    || (word.strip_suffix(value))
                        .is_some_and(|head| head.ends_with('=') || head.ends_with('/'))
            }),
            Err(NoProposal::CannotDescribe(why)) => why.contains(value),
            Err(_) => false,
        };
        // A word the answer rewrote (a path rule keeps a path's tail) is no
        // longer the token it came from, so what reads as a credential is
        // asked of the word itself (review stop 17, close verifier M1).
        let shaped = (answer.iter().flatten())
            .flat_map(|rule| rule.split_whitespace())
            .find_map(|word| reads_as_a_credential(word, &self.secrets, &self.aliases));
        let named = (credentials.iter().find(|(value, _)| says(value))).map(|(_, what)| what);
        match named.or(shaped.as_ref()) {
            Some(what) => Err(NoProposal::CannotDescribe(credential_reason(what))),
            None => answer,
        }
    }

    /// The proposal as its words are read, before the one question of them
    /// ([`stage_proposals`](Self::stage_proposals)).
    fn stage_proposals_as_written(&self, line: &str) -> Result<Vec<String>, NoProposal> {
        let cannot = |why: String| Err(NoProposal::CannotDescribe(why));
        let Ok(pipeline) = parse_line(line) else {
            return cannot(
                "it could not be analyzed, so a rule could not say what it allows".into(),
            );
        };
        let verdict = self.decide_parsed(line, &pipeline);
        let Some((at, stage, held)) = pipeline
            .stages
            .iter()
            .zip(verdict.stages())
            .enumerate()
            .find(|(_, (_, v))| v.decision != Decision::Allow)
            .map(|(i, (s, v))| (i, s, v))
        else {
            return Err(NoProposal::NothingAsks);
        };
        let asking: Vec<&ActVerdict> = held
            .acts
            .iter()
            .filter(|a| a.decision != Decision::Allow)
            .collect();
        let Some(first) = asking.first().copied() else {
            return Err(NoProposal::NothingAsks);
        };
        if !pipeline.cwd_fully_known() {
            return cannot(
                "an earlier `cd` could not be followed, so the paths it names are unknown".into(),
            );
        }
        // A hard ask, or a rule that refuses: said up front, before any other
        // reason not to propose — once `find -exec` became a shipped ask its
        // payload hit some other early exit, and `A` offered an `:allow` that
        // could not stop the asking (Reviewer F, round 18).
        if let Some(act) = asking
            .iter()
            .find(|a| a.decision == Decision::Deny || matches!(a.by, By::Hard(_)))
        {
            return Err(NoProposal::RuleCannotAnswer(Box::new((*act).clone())));
        }
        // Why a word cannot stand for itself in a rule, if it cannot.
        let unwritable = |word: &str| {
            if word.contains(char::is_whitespace) {
                Some("holds a space, and a rule's words are split on spaces")
            } else if word.starts_with('-') {
                Some("begins with `-`, so a rule would read it as a flag")
            } else if word.starts_with('#') {
                Some("begins with `#`, which a config file reads as a comment")
            } else if crate::shell::expands(word) && !word.starts_with("~/") {
                Some("still expands, so its text is not what runs")
            } else {
                None
            }
        };
        // `checked` is the word a rule would hold; `shown`, how a reason
        // quotes it.
        let refuse = |shown: &str, checked: &str| -> Option<String> {
            unwritable(checked).map(|why| format!("`{shown}` {why}"))
        };
        // A host asks: its own rule, which no program rule can carry.
        if let ActKind::Host { dir, host } = &first.act {
            let dir = match dir {
                Dir::Fetch => "fetch",
                Dir::Send => "send",
            };
            return self.proposal_covers(&pipeline, at, vec![format!(":{dir} {host}")]);
        }
        // A file run (`./run.sh`): the exec grant, which also covers the
        // program it runs as. A file a modelled program runs (`bash x.sh`,
        // `awk -f x.awk`) is judged by that program's model too, so where
        // the grant alone does not answer, its form joins it (below): `A`
        // offered `:exec x.sh` alone, which could never answer (10-13).
        if let Some(ActKind::Path {
            op: Some(Effect::Exec),
            word,
            ..
        }) = asking.iter().map(|a| &a.act).find(|a| {
            matches!(
                a,
                ActKind::Path {
                    op: Some(Effect::Exec),
                    ..
                }
            )
        }) {
            if let Some(why) = refuse(word, word) {
                return cannot(why);
            }
            if let Ok(alone) = self.proposal_covers(&pipeline, at, vec![format!(":exec {word}")]) {
                return Ok(alone);
            }
        }
        // The stage's own program act. (A peeled wrapper — `timeout 5 CMD` —
        // is an act of its own that runs by default; the proposal is for
        // what it runs.)
        let program = held.acts.iter().find_map(|a| match &a.act {
            ActKind::Program { name, chain, .. }
                if !name.is_empty() && !matches!(a.by, By::Default(NoRule::Wrapper)) =>
            {
                Some((a, name, chain))
            }
            _ => None,
        });
        let Some((program_act, name, chain)) = program else {
            return Err(NoProposal::RuleCannotAnswer(Box::new(first.clone())));
        };
        if name.contains([':', '*']) {
            return cannot(format!("`{name}` cannot be written as a rule's program"));
        }
        let unmodelled = matches!(program_act.by, By::Default(NoRule::Unmodelled));
        let mut rules: Vec<String> = Vec::new();
        // The program's form, when the program act asks: its chain, the
        // flags it used that no form carries unnamed, its data operands as
        // written — never its places, which the path rules below grant
        // (Steven, 10-10: program rules are about arguments and flags, path
        // rules about access).
        if program_act.decision != Decision::Allow {
            let mut parts: Vec<String> = vec![name.clone()];
            parts.extend(chain.iter().cloned());
            match &program_act.by {
                By::Default(NoRule::Unknown(words)) => {
                    // Named for the user to write, never offered: `allow rm
                    // build *` reaches `rm -rf build /`.
                    let mut wide: Vec<String> = vec![name.clone()];
                    wide.extend(chain.iter().cloned());
                    wide.extend(
                        stage.command.words[1..]
                            .iter()
                            .filter(|w| !w.starts_with('-') || *w == "-")
                            .cloned(),
                    );
                    wide.push("*".into());
                    return cannot(format!(
                        "it carries {}, which `{name}`'s model does not know — a word that may \
                     name files or code no narrow rule can see; `:allow {}` covers it, with \
                     any other arguments",
                        words
                            .iter()
                            .map(|w| format!("`{w}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        wide.join(" ")
                    ));
                }
                // Unmodelled: its bare words, in order, and `*` for its flags —
                // never the program alone: `allow uv *` is the blanket grant this
                // design exists to avoid (Steven, 09-30).
                // Read as the decision reads them: a flag a rule declares
                // valued is named with its value (`-l=rust`).
                By::Default(NoRule::Unmodelled) => {
                    let (words, flags) =
                        decide::unmodelled_words(&self.policy(), name, &stage.command.words[1..]);
                    if words.is_empty() {
                        return cannot(format!(
                            "`{name}` is not modelled, and with no other word to name only \
                         `allow {name} *` covers it — any `{name}` call; write that with \
                         `:allow` if you mean it"
                        ));
                    }
                    if let Some(why) = words.iter().find_map(|w| {
                        // A declared flag is written as one: its value is
                        // the word to check.
                        let checked = match w.split_once('=') {
                            Some((_, v)) if w.starts_with('-') => v,
                            _ => w.as_str(),
                        };
                        refuse(w, checked)
                    }) {
                        return cannot(why);
                    }
                    parts.extend(words.iter().map(|w| w.to_string()));
                    if flags {
                        parts.push("*".into());
                    }
                }
                // A form an allow left uncovered: name what it did not cover. A
                // modelled program past a read with no rule: the flags no form
                // carries unnamed (`find -delete`, `tar -x`), as the reader read
                // them — a cluster is its letters.
                // (A completion model's call asks with no rule as a writer's
                // does: its flags are named, none rides.)
                By::Rules(_) | By::Default(NoRule::Effect(_) | NoRule::Completion) => {
                    for u in &program_act.uncovered {
                        if let Some(flag) = u.strip_prefix("flag ") {
                            parts.push(flag.to_string());
                        } else if let Some(word) = u.strip_prefix("word ") {
                            if let Some(why) = refuse(word, word) {
                                return cannot(why);
                            }
                            parts.push(word.to_string());
                        } else if let Some(sub) = u.strip_prefix("subcommand ") {
                            if !parts.iter().any(|p| p == sub) {
                                parts.push(sub.to_string());
                            }
                        } else {
                            return Err(NoProposal::RuleCannotAnswer(Box::new(
                                program_act.clone(),
                            )));
                        }
                    }
                }
                _ => {}
            }
            // The operands that are no path act — a pattern, a remote, a word
            // the model reads as data — named as written, so the rule means this
            // call and not every call of the form (`rg TODO /srv`).
            if !unmodelled {
                let placed: Vec<&str> = held
                    .acts
                    .iter()
                    .filter_map(|a| match &a.act {
                        ActKind::Path { word, .. } => Some(word.as_str()),
                        _ => None,
                    })
                    .collect();
                for word in &stage.command.operands {
                    // (A dashed operand is an inner program's flag — `uv run
                    // pytest -x` — named by the `*` below, or nothing's.)
                    if placed.contains(&word.as_str())
                        || parts.contains(word)
                        || (word.starts_with('-') && word != "-")
                    {
                        continue;
                    }
                    if let Some(why) = refuse(word, word) {
                        return cannot(why);
                    }
                    parts.push(word.clone());
                }
                // An inner program the model does not know (`uv run ruff check
                // src/`): its bare words, in order, and `*` for its flags — the
                // prefix an unmodelled allow is.
                for inner in held.acts.iter().filter_map(|a| match &a.act {
                    ActKind::Program { name: inner, .. }
                        if inner != name && matches!(a.by, By::Default(NoRule::Unmodelled)) =>
                    {
                        Some(inner)
                    }
                    _ => None,
                }) {
                    let words = &stage.command.words;
                    let Some(at) = words.iter().position(|w| w == inner) else {
                        continue;
                    };
                    let rest = &words[at + 1..];
                    for w in rest.iter().filter(|w| !w.starts_with('-') || *w == "-") {
                        if let Some(why) = refuse(w, w) {
                            return cannot(why);
                        }
                        if !parts.contains(w) {
                            parts.push(w.clone());
                        }
                    }
                    if rest.iter().any(|w| w.starts_with('-') && w != "-")
                        && !parts.iter().any(|p| p == "*")
                    {
                        parts.push("*".into());
                    }
                }
            }
            rules.push(parts.join(" "));
        }
        // Every place that asks: a path rule at its operation, written as the
        // line named it where the line did not move, else where it was
        // placed — that is where a path rule's path is read. A redirection's
        // target and a wrapper's argument are places like any other: the
        // program form above never carries them (review 2026-10-10, E2).
        for act in &asking {
            let ActKind::Path {
                op, paths, word, ..
            } = &act.act
            else {
                continue;
            };
            if let Some(why) = refuse(word, word) {
                return cannot(why);
            }
            let [place] = &paths[..] else {
                return cannot(format!(
                    "an earlier `cd` may have failed, so `{word}` names a file in one of \
                     several places, and a rule names one"
                ));
            };
            let moved = !matches!(stage.cwd, crate::shell::Cwd::Start);
            let path = if moved || word.starts_with('/') {
                // A place under the start is written relative to it, as a
                // rule's path is read (`cd build && rm x`: `build/x`); under
                // the stand-in for a start Shannon could not say, relative as
                // it was read.
                let start = self.start.as_deref().unwrap_or(NO_START);
                match place.strip_prefix(start).filter(|rel| rel.starts_with('/')) {
                    Some(rel) => rel.trim_start_matches('/').to_string(),
                    // Elsewhere: as it was typed, if the line did not move
                    // it (`place` is the gate's spelling, `/private/tmp/x`
                    // for a `/tmp/x` you wrote; the rule reads either).
                    None if !moved => word.clone(),
                    None => place.clone(),
                }
            } else {
                word.clone()
            };
            let op = match op {
                Some(Effect::Read) => "read",
                Some(Effect::Write) => "write",
                Some(Effect::Delete) => "delete",
                Some(Effect::Exec) => "exec",
                None => "*",
            };
            let rule = format!(":{op} {path}");
            if !rules.contains(&rule) {
                rules.push(rule);
            }
            if rules.len() > MOST_RULES_PROPOSED {
                return cannot(format!(
                    "it would take more than {MOST_RULES_PROPOSED} rules, one for each place"
                ));
            }
        }
        self.proposal_covers(&pipeline, at, rules)
    }

    /// The rules `A` would add, if THIS policy plus ALL of them decides stage
    /// `at` Allow — coverage by the engine itself, not by an argument about
    /// it. The first is the one offered; the rest follow once it is taught.
    fn proposal_covers(
        &self,
        pipeline: &Pipeline,
        at: usize,
        texts: Vec<String>,
    ) -> Result<Vec<String>, NoProposal> {
        let cannot = |why: String| Err(NoProposal::CannotDescribe(why));
        if texts.is_empty() {
            return Err(NoProposal::NothingAsks);
        }
        let with = self.with_allows(&texts)?;
        let text = texts.join(" · ");
        let decided = with.decide(pipeline);
        match decided.stages()[at].decision {
            Decision::Allow => Ok(texts),
            _ => {
                // A rule of the user's own that asks, which the narrowest
                // allow does not displace (it names the same form): say what
                // takes the rule out, not that the call cannot be described.
                let held = decided.stages()[at]
                    .acts
                    .iter()
                    .find(|a| a.decision != Decision::Allow && matches!(a.by, By::Rules(_)));
                match held {
                    Some(act) => Err(NoProposal::RuleCannotAnswer(Box::new(act.clone()))),
                    None => cannot(format!(
                        "`allow {text}` would not cover everything it touches"
                    )),
                }
            }
        }
    }
}

/// Every token of a stage a secret handle may be written in: the program,
/// the operands, each flag's value, each assignment's value, the unknown
/// words, a wrapper's own argv, each redirection's target, and the heredoc
/// bodies the stage feeds (`Stage::data`) — a handle is expanded wherever it
/// is written. Missing one field here is a silent authorisation to spend,
/// so the rule is: scan everything.
fn stage_tokens(stage: &Stage) -> impl Iterator<Item = &str> {
    let command = &stage.command;
    std::iter::once(command.program.as_str())
        .chain(command.operands.iter().map(String::as_str))
        .chain(command.flags.iter().filter_map(|f| f.value.as_deref()))
        .chain(command.env.values().map(String::as_str))
        .chain(command.unknown.iter().map(String::as_str))
        .chain(command.wrapper_args.iter().map(String::as_str))
        .chain(stage.redirects.iter().map(|r| r.target.as_str()))
        .chain(stage.data.iter().map(String::as_str))
}

/// **The words of `stage` a rule must never carry, each with where it sat**
/// — a credential:
/// - by POSITION: the value of a flag whose name announces one (`--password
///   x`, `--token=x`, `--api-key x`; a modelled flag by its model's name, an
///   unmodelled one by its spelling), at any length — the position is the
///   evidence here, where in free text the value floor has to be
///   ([`crate::secrets::names_a_credential`]);
/// - by SHAPE: any token the redactor takes out — a known format, a URL's
///   userinfo, a high-entropy run — in every field ([`stage_tokens`]).
///
/// The value of an unnamed short flag (`sshpass -p x`, `redis-cli -a x`) is
/// neither, until a model says the flag takes a credential: not guessed
/// (Steven, 11-08: "what matters is when something is *obviously* a
/// password").
fn credential_words<'a>(
    stage: &'a Stage,
    cfg: &crate::SecretsConfig,
    aliases: &crate::Aliases,
) -> Vec<(&'a str, String)> {
    let shaped = stage_tokens(stage)
        .filter_map(|token| Some((token, reads_as_a_credential(token, cfg, aliases)?)));
    named_credentials(stage, cfg)
        .into_iter()
        .chain(shaped)
        .collect()
}

/// **The values `stage` writes where a credential's name says one goes** —
/// a flag's (`--password x`, `--token=x`; a modelled flag by its model's
/// name, an unmodelled one by its spelling) and a variable's (`db_password=x
/// psql`), each with where it sat; never a word that writes none down
/// ([`writes_no_value`]). The position is the evidence, so at any length.
/// The one reading for what `A` may propose, what a rule may keep
/// ([`credential_words`]) and what a kept call may show
/// ([`named_credential_spans`]).
fn named_credentials<'a>(stage: &'a Stage, cfg: &crate::SecretsConfig) -> Vec<(&'a str, String)> {
    let command = &stage.command;
    let flags = (command.flags.iter().chain(&command.unmodeled_flags))
        .filter(|flag| crate::secrets::names_a_credential(&flag.name, cfg))
        .filter_map(|flag| {
            let value = flag.value.as_deref().filter(|v| !writes_no_value(v))?;
            Some((value, format!("the value of `{}`", flag.name)))
        });
    let env = (command.env.iter())
        .filter(|(name, value)| {
            crate::secrets::names_a_credential(name, cfg) && !writes_no_value(value)
        })
        .map(|(name, value)| (value.as_str(), format!("the value of `{name}`")));
    flags.chain(env).collect()
}

/// **Where `line` writes a value a credential's name announces**, as byte
/// ranges in order and apart: by the shell's reading of each stage
/// ([`named_spans`]), else, for a line it cannot read, by its words
/// ([`named_in_words`]). What `A` and a rule's door refuse to keep, found
/// where a kept call is scrubbed (`secrets::redact_command`): the text rule
/// reads a value bound to its name (`--password=x`) and keeps a length floor
/// for the rest, having no position to go on, so `--password x` reached the
/// transcript, the log and every later request in clear (review stop 17, E1
/// M1; Steven, 11-09: the gate's reading, "(b)").
pub(crate) fn named_credential_spans(
    line: &str,
    cfg: &crate::SecretsConfig,
) -> Vec<std::ops::Range<usize>> {
    let mut spans: Vec<std::ops::Range<usize>> = match parse_line(line) {
        Ok(pipeline) => (pipeline.stages.iter())
            .flat_map(|stage| named_spans(line, stage, cfg))
            .collect(),
        Err(_) => (named_in_words(line, cfg).into_iter())
            .map(|(span, _)| span)
            .collect(),
    };
    spans.retain(|span| span.start < span.end && line.get(span.clone()).is_some());
    spans.sort_by_key(|span| span.start);
    let mut apart: Vec<std::ops::Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        match apart.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => apart.push(span),
        }
    }
    apart
}

/// Where `stage` writes each of its [`named_credentials`] in `line`: a word
/// that is the value (`--password hunter2`); else, in a word that could
/// carry one attached — a flag, an assignment — its tail (`--password=x`,
/// `-px`, `db_password=x`), from its `=` where quotes stand between
/// (`--password="a b"`). Matched by each word as the shell passes it, so a
/// quoted value is found and its quotes go with it.
fn named_spans(
    line: &str,
    stage: &Stage,
    cfg: &crate::SecretsConfig,
) -> Vec<std::ops::Range<usize>> {
    let values: Vec<&str> = (named_credentials(stage, cfg).into_iter())
        .map(|(value, _)| value)
        .collect();
    if values.is_empty() {
        return Vec::new();
    }
    let words = stage.command.expansion.iter().zip(&stage.word_spans);
    words
        .filter_map(|((word, _), span)| {
            if values.contains(&word.as_str()) {
                return Some(span.clone());
            }
            let value = values.iter().find(|value| {
                word.strip_suffix(**value)
                    .is_some_and(|head| head.starts_with('-') || head.ends_with('='))
            })?;
            let written = line.get(span.clone())?;
            match (written.ends_with(*value), written.find('=')) {
                (true, _) => Some(span.end - value.len()..span.end),
                (false, Some(eq)) => Some(span.start + eq + 1..span.end),
                (false, None) => Some(span.clone()),
            }
        })
        .collect()
}

/// [`named_spans`] for a line the shell cannot read, by its words alone — a
/// `-`-led name and the word after it, a `name=value` word's value — each
/// with where it sat: a rule's grammar takes a stray `'` the shell's does
/// not, and "could not read it" is not "nothing there" (review stop 17, E1
/// M3).
fn named_in_words(line: &str, cfg: &crate::SecretsConfig) -> Vec<(std::ops::Range<usize>, String)> {
    // Each word is a slice of `line`, so its offset is where it starts.
    let words: Vec<(std::ops::Range<usize>, &str)> = (line.split_whitespace())
        .map(|word| {
            let at = word.as_ptr() as usize - line.as_ptr() as usize;
            (at..at + word.len(), word)
        })
        .collect();
    let named = (words.windows(2))
        .filter(|pair| {
            let (name, value) = (pair[0].1, pair[1].1);
            name.starts_with('-')
                && crate::secrets::names_a_credential(name, cfg)
                && !writes_no_value(value)
        })
        .map(|pair| (pair[1].0.clone(), format!("the value of `{}`", pair[0].1)));
    // `db_password=x`, `--token=x`: a variable's or a flag's, as a line that
    // parses reads them (close verifier M3).
    let bound = words.iter().filter_map(|(span, word)| {
        let (name, value) = word.split_once('=')?;
        let named = crate::secrets::names_a_credential(name, cfg) && !writes_no_value(value);
        named.then(|| {
            (
                span.start + name.len() + 1..span.end,
                format!("the value of `{name}`"),
            )
        })
    });
    named.chain(bound).collect()
}

/// **What `token` holds, by its own text**, where a credential's name is not
/// there to say so: a known shape, or a held secret's value. A SECRET's: never
/// a plain alias's, which is privacy or token economy, not a credential (`deny
/// cat /etc/*` under an alias for `/etc` is an ordinary rule); and a handle
/// is no value at all. The shape is read [`for_rules`], whatever `redact`
/// says of egress: every rule reaches the redactor here.
///
/// [`for_rules`]: crate::SecretsConfig::for_rules
fn reads_as_a_credential(
    token: &str,
    cfg: &crate::SecretsConfig,
    aliases: &crate::Aliases,
) -> Option<String> {
    let seen = crate::secrets::redact(token, &cfg.for_rules(), aliases);
    if let Some(kind) = seen.kinds.first() {
        return Some(format!("what reads as a {kind}"));
    }
    holds_a_value(token, aliases).then(|| HELD.to_string())
}

/// What a held value's refusal says of where it sat.
const HELD: &str = "a secret Shannon holds";

/// Does `token` hold a secret's value, a handle aside? In CHARS, as
/// `secret_ranges` gives them: taken for bytes, a multibyte character ahead
/// of the value panicked, its message holding the value (review stop 17,
/// B F1).
fn holds_a_value(token: &str, aliases: &crate::Aliases) -> bool {
    (aliases.secret_ranges(token).iter())
        .any(|&(from, _)| !token.chars().skip(from).take(2).eq("{{".chars()))
}

/// **The credentials in a line the shell cannot read**, by its words alone.
/// A rule's grammar takes what the shell's does not (a stray `'`, `$(`), and
/// "could not read it" is not "nothing there" (review stop 17, E1 M3).
fn credentials_in_words(
    line: &str,
    cfg: &crate::SecretsConfig,
    aliases: &crate::Aliases,
) -> Vec<(String, String)> {
    let named =
        (named_in_words(line, cfg).into_iter()).map(|(span, what)| (line[span].to_string(), what));
    let held = (line.split_whitespace())
        .filter(|word| holds_a_value(word, aliases))
        .map(|word| (word.to_string(), HELD.to_string()));
    named.chain(held).collect()
}

/// **The most rules one `A` offers.** One per place that asks, each then
/// decided with all the others in force: a model's `rm f0 … f999` took
/// seconds to build the pane, 5,000 operands 90 (review stop 17, B F4). No
/// list that long is a rule anyone means to keep.
const MOST_RULES_PROPOSED: usize = 32;

/// **A word in a credential's place that writes none down**: a rule's
/// wildcard (`deny mysql --password *`, a rule protecting one), a handle
/// (`{{secret.ab12}}`, what `:secret` said to use), a variable the shell
/// fills (`--api-key=$TOKEN`), nothing at all (`--password=`), a switch's own
/// word (`--auth=none`, the redactor's `is_a_setting`). A rule keeps these as
/// written and holds no credential (review stop 17, A M1; close verifier
/// LOW 1, 2).
fn writes_no_value(value: &str) -> bool {
    if value.is_empty() || crate::secrets::is_a_setting(value) {
        return true;
    }
    let wildcard = !value.is_empty() && value.chars().all(|c| matches!(c, '*' | '?'));
    let handle = crate::secrets::secret_handles_in(value) == [value];
    let variable = value.strip_prefix('$').is_some_and(|name| {
        let name = (name.strip_prefix('{').and_then(|n| n.strip_suffix('}'))).unwrap_or(name);
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    wildcard || handle || variable
}

/// **The credentials a line holds, each with where it sat** — `line` read as
/// a command line and asked of `credential_words` stage by stage: the one
/// definition `A`'s proposals and a typed rule's door both use, so the two
/// cannot disagree about what a credential is. A rule's text reads as its
/// command would (`docker login --password x *`); the value is returned only
/// for the caller to keep out of what it says.
pub fn credentials_in(
    line: &str,
    cfg: &crate::SecretsConfig,
    aliases: &crate::Aliases,
) -> Vec<(String, String)> {
    let Ok(pipeline) = parse_line(line) else {
        return credentials_in_words(line, cfg, aliases);
    };
    pipeline
        .stages
        .iter()
        .flat_map(|stage| credential_words(stage, cfg, aliases))
        .map(|(value, what)| (value.to_string(), what))
        .collect()
}

/// **Where a rule's text (`args`, without its verb) holds a credential**, if
/// it does — never the value: what a rule may keep, asked of a typed rule at
/// its door and of each rule again as `:config save` writes it, since a value
/// can be marked after the rule that holds it (review stop 17, E1 M2). Its
/// words as [`credentials_in`] reads them, then the whole text as one word,
/// for what no command reading sees: a `#` comment, a held value the
/// grammar split (E1 M3). The TUI's door asked this itself, with a second
/// spelling of the shape and held-value checks.
pub fn credential_in_rule(
    args: &str,
    cfg: &crate::SecretsConfig,
    aliases: &crate::Aliases,
) -> Option<String> {
    let found = credentials_in(args, cfg, aliases).into_iter().next();
    (found.map(|(_, what)| what)).or_else(|| reads_as_a_credential(args, cfg, aliases))
}

/// Why `A` offers no rule for a line holding a credential: where the value
/// sat, never the value ([`credential_words`]).
fn credential_reason(what: &str) -> String {
    format!("it carries a credential ({what}) — {RULE_KEEPS_ITS_WORDS}")
}

/// **Why a rule may not hold a credential**, for each refusal to say: `A`'s
/// and a typed rule's. The way forward leads, since a narrow row keeps only
/// its start: a `:secret` handle is no way forward, as a call holding one
/// asks whatever a rule allows (review stop 17, F M2: this told you to mark
/// the value, and the rule you could then write let nothing run unasked).
pub const RULE_KEEPS_ITS_WORDS: &str = "write `*` in its place to cover any value (a `:secret` \
     handle always asks); a rule is kept as written, and `:config save` would publish it";

/// **Where `line` first names the variable `name`**, as a whole name —
/// `$GITHUB_TOKEN`, `${GITHUB_TOKEN:0:4}`, `printenv GITHUB_TOKEN`,
/// `ENVIRON["GITHUB_TOKEN"]` — and never inside a longer one
/// (`MY_GITHUB_TOKEN`, `GITHUB_TOKENS`). Case as written: a variable's name
/// is case-sensitive.
fn names_variable(line: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let part = |c: char| c.is_ascii_alphanumeric() || c == '_';
    line.match_indices(name)
        .map(|(at, _)| at..at + name.len())
        .find(|r| {
            !line[..r.start].chars().next_back().is_some_and(part)
                && !line[r.end..].chars().next().is_some_and(part)
        })
}

/// A verdict for a whole command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The line-level outcome (its strictest stage).
    pub decision: Decision,
    /// Per-stage detail in order (for the formatted, highlighted prompt), **or**
    /// the reason the line could not be analyzed at all.
    ///
    /// One field, because it was two that had to disagree: `stages` was
    /// documented "empty when `blocked` is set" and `blocked` "implies
    /// `decision == Ask`", so three of the four combinations were illegal and
    /// nothing stopped anyone writing one — a hand-built `Verdict` in the tests
    /// did (Reviewer C).
    pub outcome: Result<Vec<StageVerdict>, Unanalyzable>,
}

impl Verdict {
    /// Per-stage detail, in order — empty for a line we could not analyze.
    pub fn stages(&self) -> &[StageVerdict] {
        self.outcome.as_deref().unwrap_or(&[])
    }

    /// Why the line could not be analyzed at all, if it could not. Implies
    /// `decision == Ask` (fail closed) and no stages.
    pub fn blocked(&self) -> Option<&Unanalyzable> {
        self.outcome.as_ref().err()
    }

    /// **May BACKGROUND work run this with nobody watching?**
    ///
    /// Stricter than `decision == Allow`, and deliberately so. A job — a
    /// `:child` briefing, a `:Send` reply — never manufactures a question: it
    /// either runs something or stops and waits for the human to come to it.
    /// That makes "allowed" the wrong bar, because one of the two ways a line
    /// can be allowed is consent given for a *different* situation: a **rule**
    /// the user wrote (`allow git *`, or one `A` taught against a decision they
    /// read on screen, once) was written thinking about their own foreground
    /// work, not about an agent they spawned and walked away from.
    ///
    /// What is left is the allowance that was never anybody's decision — a
    /// property of the command: every act of every stage is a modelled read,
    /// a read of a path inside where Shannon started, or a wrapper around
    /// such, by default and by no rule. A credential guard holds a read
    /// (`cat ~/.ssh/id_rsa` asks by a shipped rule, so it is not by default),
    /// and so does a word the gate could not place: an eager optimizer that
    /// wants a credential has to wait for a human either way
    /// (`[[project_shannon_security_posture]]`).
    ///
    /// Empty stages are `false`: nothing parsed is not permission.
    pub fn safe_unattended(&self) -> bool {
        self.blocked().is_none()
            && !self.stages().is_empty()
            && self.stages().iter().all(|s| {
                s.decision == Decision::Allow
                    && s.acts.iter().all(|a| {
                        // A path placed under the stand-in for a start
                        // Shannon could not say is nowhere known: it runs in
                        // the foreground, where a person can see, not alone.
                        let placed = match &a.act {
                            ActKind::Path { paths, .. } => {
                                paths.iter().all(|p| !p.starts_with(NO_START))
                            }
                            _ => true,
                        };
                        // `Inside(None)` is a place at ANY operation (a
                        // wrapper's argument, an unmodelled program's word):
                        // not a read a job may do alone (review 2026-10-10,
                        // E1: `flock build/lock …` creates the lock).
                        placed
                            && matches!(
                                a.by,
                                By::Default(
                                    NoRule::ModelledRead
                                        | NoRule::Wrapper
                                        | NoRule::Inside(Some(Effect::Read))
                                )
                            )
                    })
            })
    }
}

/// Why the decision pane's `A` has no rule to offer
/// ([`RuleSet::proposed_allow`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoProposal {
    /// Every stage is already allowed: nothing asks.
    NothingAsks,
    /// Its words cannot be written as a rule that means only this call — the
    /// reason, as a sentence.
    CannotDescribe(String),
    /// A rule naming this call exactly would match it and still not let it
    /// run: the act that holds it — a hard ask, or a rule that refuses — for
    /// the pane to explain in its own words. A WIDER rule cannot help either,
    /// which is the thing to say: telling the user "your rule does not cover
    /// it" sent them to widen it (Reviewer F, round 16). (Boxed: the
    /// verdict is many words wide, and the `Ok` is one.)
    RuleCannotAnswer(Box<ActVerdict>),
}

#[cfg(test)]
mod acceptance;
#[cfg(test)]
mod credential_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod user_model_tests;
