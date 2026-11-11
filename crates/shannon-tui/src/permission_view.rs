//! Rendering a permission [`Verdict`] as a **colored, annotated-source** report.
//!
//! "A wall of shell is unreadable, and an unreadable prompt is consent theatre"
//! (Milestone 4). Rather than a table of extracted stages, we show the command
//! **as written** (pretty-printed with control flow on its own lines) and colour
//! each command in place — a modern compiler-diagnostic / traceback shape (see
//! the design memory `[[project_shannon_analyze_diagnostics]]`). Three layers:
//!
//! 1. a verdict **headline** (ALLOW / ASK / DENY) — the *permission decision*;
//! 2. the **source**, re-sliced from the original line, with every command
//!    coloured by its **danger** (effect), a traffic light that tells the reader
//!    *where to look and what to think*:
//!    **green** (the command's name) = read-only/safe, **yellow** (fill) =
//!    mutating or *unknown* (a warning), **red** (fill + bold) =
//!    destructive/executor ("LOOK HERE"). Under the light, ordinary shell
//!    colouring in hues the light never uses (`lexeme_syntax`, `pane_look`):
//!    flags, strings, expansions; scaffolding (`while`/`do`/`done`, pipes)
//!    quiet, so the eye lands on the commands;
//! 3. the **provenance** overlay: an unresolved value flowing into a dangerous
//!    command is traced back to the command that produced it (a read-only `find`
//!    feeding `rm -r "$FOO"`), the producer raised to a yellow warning, and the
//!    carrier variable underlined at its binding + use sites (a *separate* colour
//!    channel from the fill) so the eye follows `source -> $VAR -> sink`;
//! 4. the **findings**, grouped by `(decision, effect, reason)` so a shared cause
//!    is stated once, plus a polarity-correct link note per flow, then one action
//!    line for the verdict.
//!
//! Colour rides a `Tone` scale (Safe/Warn/Danger = green/yellow/red) that is
//! deliberately its own axis: a command's **danger** (`tone_of`) and a
//! provenance source's **implication** (`Warn`) both map onto it, while the
//! **permission decision** (allow/ask/deny) is a *separate* axis driving only the
//! headline + findings. Colour is communication, not decoration
//! (`[[project_shannon_permission_failure_mode]]`). This is the preview surface
//! behind `:analyze` and the live decision pane alike. The shell's colours
//! under the light are the theme's, as ROLES the pane paints when drawn
//! (`crate::paint`, `view::painter` — `theme::Context::Decision`); the
//! light's own are the app's (`Look::Own`), decided here and not a theme's
//! to change. No theme is read in this file. Per-line traceback carets, precise
//! sub-command spans inside substitutions, and an ASCII/NO_COLOR fallback are
//! tracked refinements.

use crate::decision_page::{DecisionPage, KeyRow, KeyTable};
use crate::paint::{Look, PageLine};
use crate::style::{CellStyle, Ink};
use crossterm::event::KeyCode;
use std::ops::Range;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use shannon_core::permission::{
    ActKind, ActVerdict, By, Cited, Decision, Guard, Hard, NoProposal, NoRule, Origin, RuleSet,
    StageVerdict, Verdict,
};
use shannon_core::shell::{
    Command, Effect, Flow, Lexeme, Operator, Pipeline, Stage, lex, parse_line, provenance,
    subst_inner_span,
};

/// **The `:analyze` report as a page, for the tests** — [`report_lines`] in a
/// [`TextView`](crate::text_view::TextView), as a window on the app's one
/// report buffer holds it (`crate::page`).
#[cfg(test)]
#[derive(Debug, Clone)]
struct AnalyzeView {
    text: crate::text_view::TextView,
}

#[cfg(test)]
impl AnalyzeView {
    /// The report on `input` under `rules`, laid out by the default breaks.
    fn of(input: &str, rules: &RuleSet) -> AnalyzeView {
        AnalyzeView {
            text: crate::text_view::TextView::new(report_lines(input, rules, Breaks::default())),
        }
    }

    fn text(&self) -> &crate::text_view::TextView {
        &self.text
    }
}

/// Does this command's text denote exactly one behaviour?
///
/// A standing "always" grant is keyed on the command *string*, which is only as
/// narrow as the string is specific. `rm -rf build/*` has stable text and an
/// unstable meaning — the glob matches whatever exists at run time, and the
/// model can create files between grants. Same for `$VAR` and `$(…)`. And a line
/// the analyzer refused to read is a grant for something we never understood.
///
/// **One answer to "is there a rule to offer"**: `RuleSet::proposed_allow`,
/// computed once with the report ([`report_parts`]) and read by the legend,
/// the `A` key and the refusal alike. There were two predicates for one key:
/// a grant predicate written for the exact-text grants removed this round
/// (`may_become_a_grant` — no secret, no credential path, stable text) gated
/// the key, and the proposal could then fail after it was offered, so `A` on
/// `alias ll=ls && ls docs` proposed a rule that could never answer the call,
/// forever (Reviewer D, round 16).
///
/// Why there is none, in the pane's own words: [`explain_act`] for a call
/// that asks whatever a rule allows — never "your rule does not cover it",
/// which sent the user to widen the rule (Reviewer F, round 16).
///
/// `credential_knob` is the setting that decides a credential path for the
/// call in question — `[secrets]`'s, or `[claude_code]`'s for a call Claude
/// Code runs ([`ToolPrompt::credential_knob`]).
pub(crate) fn no_proposal_reason(why: &NoProposal, credential_knob: &str) -> String {
    match why {
        NoProposal::NothingAsks => "nothing here asks".into(),
        NoProposal::CannotDescribe(why) => {
            format!("no rule to offer: {why} — `a` runs it once")
        }
        // "No ALLOW lifts this", and what does: "no rule can stop this asking"
        // read as permanent beside a shipped ask that `:unask!` removes
        // (review 20, Reviewer F).
        NoProposal::RuleCannotAnswer(act) => cannot_answer(act, credential_knob, true),
    }
}

/// **Why no allow answers `act`**, and what does — with the act explained
/// where nothing beside it does (a notice), without where the pane's finding
/// line already says it.
fn cannot_answer(act: &ActVerdict, credential_knob: &str, explained: bool) -> String {
    // A credential named outright is lifted by a grant naming it — one `A`
    // never offers, so it is the user's to write.
    let named_guard = matches!(&act.by, By::Rules(cited)
        if cited.iter().any(|c| c.guard.is_some() && !c.reached));
    let lead = match named_guard {
        true => "`A` offers no grant to a credential",
        false => "no allow lifts this",
    };
    let why = match explained {
        true => format!(" — {}", explain_act(act)),
        false => String::new(),
    };
    // A deny has no `a`.
    let once = match act.decision {
        Decision::Deny => "",
        _ => " — `a` runs it once",
    };
    format!(
        "{lead}{why}{once}{}",
        lifted_instead_by(act, credential_knob)
    )
}

/// What DOES lift an act no allow lifts, where something does: a rule that
/// asks or denies is taken out (`:unask!`, `:undeny!`); a credential path —
/// a shipped guard read from `[secrets] paths` — is `credential_knob`'s to
/// say, or a grant naming the place lifts it.
pub(crate) fn lifted_instead_by(act: &ActVerdict, credential_knob: &str) -> String {
    let By::Rules(cited) = &act.by else {
        return String::new();
    };
    // The rule the reason names ([`first_cited`]): the first in list order
    // named a contact's ask as the way out of a deny beside it (review of
    // c9c40fe, A and C).
    let Some(rule) = first_cited(cited).filter(|c| c.action != Decision::Allow) else {
        return String::new();
    };
    if rule.guard.is_some() {
        // A grant naming the place is at least as specific as the guard, and
        // lifts it — `cat .env` denied under Claude Code said only the knob,
        // and read as having no way out (Steven, 10-11). Met from above (`.`
        // holding Shannon's state) no grant on `.` is as specific: the knob.
        let grant = match &act.act {
            ActKind::Path { op, word, .. }
                if !rule.reached
                    && !word.is_empty()
                    && !word.contains(char::is_whitespace)
                    && !word.starts_with(['-', '#']) =>
            {
                format!("`:allow :{} {word}` lifts it, or ", rule_op(*op))
            }
            _ => String::new(),
        };
        return format!(" · {grant}`{credential_knob}` decides these");
    }
    let verb = match rule.action {
        Decision::Deny => "deny",
        Decision::Ask => "ask",
        Decision::Allow => return String::new(),
    };
    let bang = if rule.origin == Origin::Session {
        ""
    } else {
        "!"
    };
    let line = rule.one_line();
    let line: &str = &line;
    let body = line.split_once(' ').map_or(line, |(_, body)| body);
    format!(" · `:un{verb}{bang} {body}` takes the rule out")
}

/// The setting that decides a credential path for a call: Claude Code's
/// for one it runs, `[secrets]`' for Shannon's own — its file tools too.
fn credential_knob(call: Option<&CallShown>) -> &'static str {
    match call.is_some_and(CallShown::claude_code) {
        true => "[claude_code] path_policy",
        false => "[secrets] path_policy",
    }
}

/// A path act's operation as a rule writes it: `:read`, …, `:*` for a word
/// at no operation (an unmodelled program's).
fn rule_op(op: Option<shannon_core::permission::model::Effect>) -> &'static str {
    use shannon_core::permission::model::Effect;
    match op {
        Some(Effect::Read) => "read",
        Some(Effect::Write) => "write",
        Some(Effect::Delete) => "delete",
        Some(Effect::Exec) => "exec",
        None => "*",
    }
}

/// How a pending tool call was resolved — the outcome of the human decision.
///
/// Returned by [`ToolPrompt::resolve`] so the *decision* is a value the caller
/// acts on, rather than the view reaching into the app. Keeps the gate testable
/// without a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolAction {
    /// Refuse this invocation. The model gets an error result and re-plans.
    Deny,
    /// Run this exact invocation, once. Standing policy is unchanged.
    AllowOnce,
    /// Offer a standing rule to EDIT: the `:` line opens holding `allow <the
    /// narrowest useful rule>`. Nothing is granted until the user reads it,
    /// edits it if they like, and presses Enter — see [`ToolPrompt`].
    WriteRule,
    /// Edit the command; the edited text is re-gated from scratch.
    Edit,
}

/// The live permission gate for one pending tool call: the analysis the human
/// reads, plus the decision they make.
///
/// Renders through the SAME producer as `:analyze` (`report_lines`), so the
/// preview and the real prompt cannot drift — only the action line differs
/// ([`ReportMode`]).
///
/// **Security shape (the least-privilege action grammar):**
/// - Dismissal (`q`, as `d`) is [`ToolAction::Deny`]; there is no way to
///   "accidentally allow". `Esc` answers nothing: a vim user presses it by
///   reflex without reading, and a reflexive deny cost them the call they
///   meant to allow (Steven, 10-11).
/// - A `Deny` verdict offers no allow key at all — a standing rule is changed
///   deliberately, not overridden in the moment.
/// - "Always" (`A`) **writes nothing by itself**. It opens the `:` line holding
///   `allow <rule>`: the rule naming this call as written
///   (`RuleSet::proposed_allow`, e.g. `rm build` for `rm -rf build`), offered
///   only when it would answer the call, and shown in the legend before the
///   key is pressed. The user reads, edits and confirms it with Enter; it lasts
///   for the session (`:allow`). Steven, 2026-09-30: the rule taught must be the
///   user's to edit.
pub struct ToolPrompt {
    /// The `tool_use` block id this decision answers.
    tool_use_id: String,
    /// The command as proposed (or as last edited).
    command: String,
    /// The command the MODEL asked for, never rewritten by an edit.
    ///
    /// Kept so the result can disclose a substitution: the model's context holds
    /// its own request, so handing back the output of a *different* command
    /// without saying so lets it conclude its request ran.
    requested: String,
    /// The engine's verdict for `command` — what it decided on its own, before
    /// the human. Drives which keys are live.
    decision: Decision,
    /// The rule `A` would offer, or why there is none — computed with the
    /// report, so the legend, the key and the refusal read one answer.
    proposal: Result<String, NoProposal>,
    /// The report body, styled once per (re-)gating, plus its scroll.
    text: crate::text_view::TextView,
    /// Which of the report's lines are the command.
    command_lines: Range<usize>,
    /// How the command is laid out — kept, so a re-gate after an edit and a
    /// refresh after a policy change lay it out as the first report did.
    /// (No theme: the report holds roles, painted when the pane is drawn.)
    breaks: Breaks,
    /// The action line — a **pinned legend**, not content. It is rendered as a
    /// fixed footer and never scrolls, because the keys that resolve a decision
    /// are exactly what must not fall off the bottom of a long report.
    /// Painted at the draw, as the body is. Written by
    /// [`relegend`](Self::relegend) alone, from what the call is now.
    action: Option<PageLine>,
    /// **The call, when it is not a command line** — one of Shannon's file
    /// tools, or any of Claude Code's: what the pane must say that the
    /// analyzer cannot know. Kept so a re-gate after an edit and a refresh
    /// after a policy change build the same kind of report, and so the
    /// legend, the `e` key and the always-ask check read ONE answer: what is
    /// struck through on screen is what the key refuses.
    call: Option<CallShown>,
    /// **How many calls wait behind this one** — its session's, answered in
    /// the order they came (vision.md's FIFO). Kept in step by the app at each
    /// event's end (`App::settle`); the title says `N queued` when any do,
    /// and the legend `n/N cycle` (Steven, 10-19/20).
    behind: usize,
    /// **Whose call it is** — its session, as notices and its transcript's
    /// border name it. A decision belongs to its session, said in words as
    /// well as by where it opens (`buffers.md` §9): with sessions running
    /// apart, position alone cannot say whose call you are allowing. `None`
    /// for a call no longer tracked — never a guess.
    whose: Option<String>,
}

/// **Who runs a call the pane shows**, which decides what it says about
/// where the call's output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    /// One of Shannon's file tools: Shannon runs it, and its output passes
    /// Shannon's redaction before the model reads it.
    Shannon,
    /// Claude Code runs it, and its model sees the output as it is.
    ClaudeCode,
}

/// **A call that is not a command line, as the decision pane must present
/// it** — one of Shannon's file tools, or any of Claude Code's: facts about
/// the call the analyzer cannot know, supplied by its caller
/// (`App::call_shown`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallShown {
    /// The tool's name (`Read`, `Edit`, Claude Code's `Bash`, `WebFetch`, …).
    pub tool: String,
    /// Who runs it.
    pub runs: Runner,
    /// The rules cannot read it, so it asks every time: no analysis, no rule
    /// to offer.
    pub unreadable: bool,
    /// Claude Code asks to run it OUTSIDE its own sandbox
    /// (`dangerouslyDisableSandbox`): analysed and shown as ever, but asked
    /// every time — a rule reads the command, not that request — and no rule
    /// offered (finding 17).
    pub unsandboxed: bool,
    /// An edit maps back onto its input: `Bash`'s command, a `Write`'s
    /// content, an `Edit`'s new text (`claude_code::edit_text`).
    pub editable: bool,
    /// The file a file tool writes and what it writes there
    /// (`claude_code::change`). Shown under the analysis and named in the
    /// title: the rules judge `cat > path`, Shannon's reading of the call,
    /// and approving that alone was approving a write unseen (Steven, 10-05).
    pub change: Option<(String, shannon_core::backend::claude_code::Change)>,
    /// The file it names, and the other file that path lands on through a
    /// link (`files::lands`, F2c) — named in the title, since what is
    /// decided is that file: a write to `notes` that is `~/.bashrc` must not
    /// read as a write to `notes`.
    pub lands: Option<(String, String)>,
    /// The file a write lands on and how many names it has, when more than
    /// one (`files::names`, review stop 16, E 3): written in place, it
    /// changes under every one, wherever they are — why a write inside the
    /// project asks, said on the pane (Steven, 11-04).
    pub names: Option<(String, u64)>,
}

impl CallShown {
    /// Whether Claude Code runs it: its output reaches its model unredacted.
    fn claude_code(&self) -> bool {
        self.runs == Runner::ClaudeCode
    }

    /// Why it asks every time, said where an analysis would be.
    pub fn unreadable_why(&self) -> Option<String> {
        self.unreadable.then(|| {
            format!(
                "Shannon's rules cannot read Claude Code's {}, so it asks every time",
                self.tool
            )
        })
    }

    /// Why a call leaving Claude Code's sandbox has no rule to offer, said
    /// after the analysis ("no rule to offer: …").
    pub fn unsandboxed_why(&self) -> Option<String> {
        self.unsandboxed.then(|| {
            "Claude Code asks to run this OUTSIDE its sandbox (`dangerouslyDisableSandbox`), \
             and a rule reads the command, not that request, so it asks every time"
                .to_string()
        })
    }

    /// **Why no rule decides it, whatever the rules say** — either reason.
    /// What a rule added while it is open must not answer
    /// (`App::revisit_call`).
    pub fn asks_always_why(&self) -> Option<String> {
        self.unreadable_why().or_else(|| self.unsandboxed_why())
    }

    /// Why `e` is refused, for a call whose edit cannot be applied — and
    /// what can: Shannon's own turns edit any call (Steven, 10-04: "it
    /// encourages people to try the full featured version").
    pub fn edit_refusal(&self) -> Option<String> {
        (!self.editable).then(|| match self.runs {
            Runner::ClaudeCode => format!(
                "Claude Code takes an edit to Bash, Write and Edit, not {} — deny it and say \
                 what you want instead (on an API model, every call can be edited)",
                self.tool
            ),
            Runner::Shannon => format!(
                "a {} writes nothing to edit — deny it and say what you want instead",
                self.tool
            ),
        })
    }
}

/// Said under every Claude Code call's report: the one way this path differs
/// from Shannon's own, and the knob that answers for credential paths.
const PROVIDER_NOTE: &str = "Claude Code runs this, and its model sees the output as it is — \
     Shannon's redaction is not between them. Credential paths: `[claude_code] path_policy`.";

impl ToolPrompt {
    /// Gate `command` (the tool call's argument) under `rules` and build the
    /// prompt the human answers, its command laid out by `breaks`.
    pub fn new(
        tool_use_id: impl Into<String>,
        command: &str,
        rules: &RuleSet,
        breaks: Breaks,
    ) -> ToolPrompt {
        Self::build(tool_use_id.into(), command, None, rules, breaks)
    }

    /// [`new`](Self::new), laid out by the default breaks — the tests'.
    #[cfg(test)]
    pub(crate) fn of(tool_use_id: impl Into<String>, command: &str, rules: &RuleSet) -> ToolPrompt {
        Self::new(tool_use_id, command, rules, Breaks::default())
    }

    /// **A call that is not a command line** — a file tool's, or any of
    /// Claude Code's — read as `line`. One the rules cannot read is put to
    /// the human as written — no analysis (the line is not shell), no rule to
    /// offer (none would answer it); one that takes no edit has its `e`
    /// struck through.
    pub fn for_call(
        tool_use_id: impl Into<String>,
        line: &str,
        call: CallShown,
        rules: &RuleSet,
        breaks: Breaks,
    ) -> ToolPrompt {
        Self::build(tool_use_id.into(), line, Some(call), rules, breaks)
    }

    fn build(
        tool_use_id: String,
        command: &str,
        call: Option<CallShown>,
        rules: &RuleSet,
        breaks: Breaks,
    ) -> ToolPrompt {
        let command = command.trim().to_string();
        let report = prompt_report(&command, call.as_ref(), rules, breaks);
        let mut prompt = ToolPrompt {
            breaks,
            tool_use_id,
            decision: report.decision,
            proposal: report.proposal,
            text: crate::text_view::TextView::new(report.lines),
            command_lines: report.command,
            action: None,
            requested: command.clone(),
            command,
            call,
            behind: 0,
            whose: None,
        };
        prompt.relegend();
        prompt
    }

    /// **The legend, from what the call is now** ([`key_table`](Self::key_table)).
    /// The one writer of `action`: construction, an edit's re-gate, a
    /// policy's refresh and a change in the count all call it, so no key is
    /// listed from a call that has since changed.
    fn relegend(&mut self) {
        self.action = Some(self.key_table().legend());
    }

    /// **Its keys, from what the call is now** — its verdict, the rule `A`
    /// offers, whether `e` applies, whether calls wait behind it — the one
    /// table its legend, its short form, its refusal and its
    /// [`resolve`](Self::resolve) all read (review stop 8, C 8).
    ///
    /// **`d`(eny) first, and what every dismissal falls to** — the safe
    /// default must also be the easy one. `q` is listed because it DENIES
    /// here: everywhere else in this app `q` closes a viewer harmlessly, so a
    /// user reaching for it to "read the transcript first" would answer the
    /// decision instead. A DENY verdict has no allow at all — the way past a
    /// standing deny is to change the rule, on purpose, not to answer a
    /// prompt — so no allow is listed, resolved, or offered in short.
    ///
    /// `j/k` is listed because this is the one pane whose hidden content is
    /// a safety question: every other scrollable pane advertises its keys on
    /// the status row, and this one's fell through to the default row
    /// (Reviewer F). `n/N` walk the calls waiting behind it, when any do —
    /// before `A`, whose rule is what a narrow pane cuts. **`A` goes last**,
    /// and only with a rule, which it shows with its scope before the key is
    /// pressed (Reviewer F, round 16): a long rule is cut at the pane's edge,
    /// and it must be the rule's tail that goes, never a key; the `:` line it
    /// opens shows the whole rule before Enter. With no rule `A` still
    /// resolves, to say why (`App::propose_rule`), unlisted.
    pub(crate) fn key_table(&self) -> KeyTable<ToolAction> {
        let allows = self.decision != Decision::Deny;
        let editable = self.call.as_ref().is_none_or(|call| call.editable);
        let deny = KeyRow::new(
            &[KeyCode::Char('d'), KeyCode::Char('q')],
            Some(ToolAction::Deny),
            "d/q deny",
            Some("d deny"),
        );
        let mut rows = vec![deny];
        if allows {
            rows.push(KeyRow::new(
                &[KeyCode::Char('a')],
                Some(ToolAction::AllowOnce),
                "a allow once",
                Some("a allow"),
            ));
        }
        let edit = match allows {
            true => "e edit",
            false => "e edit the command",
        };
        rows.push(KeyRow {
            struck: !editable,
            ..KeyRow::new(
                &[KeyCode::Char('e')],
                Some(ToolAction::Edit),
                edit,
                Some("e edit"),
            )
        });
        let scroll = &[KeyCode::Char('j'), KeyCode::Char('k')];
        rows.push(KeyRow::new(scroll, None, "j/k scroll", None));
        if self.behind > 0 {
            let cycle = &[KeyCode::Char('n'), KeyCode::Char('N')];
            rows.push(KeyRow::new(cycle, None, "n/N cycle", None));
        }
        if allows {
            let always = KeyRow::new(&[KeyCode::Char('A')], Some(ToolAction::WriteRule), "", None);
            rows.push(match &self.proposal {
                Ok(rule) => KeyRow {
                    label: format!("A always, this session: allow {rule}"),
                    ..always
                },
                Err(_) => KeyRow {
                    listed: false,
                    ..always
                },
            });
        }
        KeyTable::new(rows)
    }

    /// How many calls wait behind this one.
    pub fn behind(&self) -> usize {
        self.behind
    }

    /// Say how many calls wait behind this one.
    pub fn set_behind(&mut self, behind: usize) {
        if std::mem::replace(&mut self.behind, behind) != behind {
            self.relegend();
        }
    }

    /// **The model's own request, put back** on a call shown again after `n`
    /// parked it edited (`App::cycle_call`): the call is rebuilt from the
    /// queue's text, the edit, and without this an edit would be taken for
    /// what the model asked — and its result would not say it was replaced.
    pub fn restore_request(&mut self, requested: String) {
        self.requested = requested;
    }

    /// Whose call this is ([`set_sessions`](Self::set_sessions)).
    pub fn whose(&self) -> Option<&str> {
        self.whose.as_deref()
    }

    /// Say whose call this is. Only whose: a session's pane shows its own
    /// calls, so the next one is never another's.
    pub fn set_sessions(&mut self, whose: Option<String>) {
        self.whose = whose;
    }

    /// The call, if it is not a command line ([`CallShown`]).
    pub fn call(&self) -> Option<&CallShown> {
        self.call.as_ref()
    }

    /// **The pane's title: the question owed, and what the body does not
    /// show.** A command is "run this?" and nothing more while the body shows
    /// it: a copy in the title was the same text twice, the second flattened
    /// to one line (Steven, 10-05, image 92: "more noisy since the
    /// information is already in the pane"). Squeezed past showing it, the
    /// title carries it as it always did — the status row cannot, under a
    /// notice. One of Claude Code's other tools is named as that tool — the
    /// report shows Shannon's reading of it, never its name — and a file
    /// tool by the file it writes, which its `cat > '…'` reading looked like
    /// a command you could edit (Steven, 10-05: "edit is inconsistent") —
    /// and through a link, by where it lands as well (F2c).
    pub fn question(&self, command_shown: bool) -> (&'static str, Option<String>) {
        let unseen = (!command_shown).then(|| self.command_on_one_line());
        match &self.call {
            Some(call) if call.tool != "Bash" => {
                // Who runs it, named where it is not Shannon — the pane is
                // Shannon's, so its own tools need no name before theirs.
                let tool = match call.runs {
                    Runner::ClaudeCode => format!("Claude Code {}", call.tool),
                    Runner::Shannon => call.tool.clone(),
                };
                let question = match call.change {
                    Some(_) => "write this?",
                    None => "allow this?",
                };
                let named = match (&call.lands, &call.change, unseen) {
                    (Some((path, at)), ..) => format!("{tool} {path} → {at}"),
                    (None, Some((path, _)), _) => format!("{tool} {path}"),
                    (None, None, Some(line)) => format!("{tool}: {line}"),
                    (None, None, None) => tool,
                };
                (question, Some(named))
            }
            _ => ("run this?", unseen),
        }
    }

    /// The command on one row, its line breaks marked rather than lost and a
    /// heredoc's body said by its size: a title or the status row has no row
    /// after it (`one_row`).
    pub fn command_on_one_line(&self) -> String {
        one_row(&self.command).0
    }

    /// How this pane laid its command out, kept from when it was gated — so
    /// `e` opens the layout on screen, not one a `:set` since would give.
    pub fn breaks(&self) -> Breaks {
        self.breaks
    }

    /// **Whether the command is wholly on screen** in a body `rows` tall and
    /// `width` wide, at the report's scroll — the question whose `false`
    /// puts the command in the title ([`ToolPrompt::question`]). A command
    /// that wraps is shown when every row it wraps to is (`TextView::shows`:
    /// rows are counted by the wrap that draws them, since S4).
    pub fn shows_command(&self, rows: u16, width: u16) -> bool {
        self.text.shows(self.command_lines.clone(), rows, width)
    }

    /// Why there is no rule to offer, in this pane's words — naming the knob
    /// that answers for a credential path on THIS call.
    pub fn why_no_rule(&self, why: &NoProposal) -> String {
        no_proposal_reason(why, self.credential_knob())
    }

    /// The setting that decides a credential path for this call.
    pub fn credential_knob(&self) -> &'static str {
        credential_knob(self.call.as_ref())
    }

    /// The rule `A` would offer, or why there is none.
    pub fn proposal(&self) -> &Result<String, NoProposal> {
        &self.proposal
    }

    /// The `tool_use` id this prompt answers.
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    /// The command under review (as last edited).
    pub fn command(&self) -> &str {
        &self.command
    }

    /// What the model originally asked for, if the human changed it. `None` when
    /// the command is still the one that was requested.
    pub fn edited_from(&self) -> Option<&str> {
        (self.command != self.requested).then_some(self.requested.as_str())
    }

    /// The engine's own verdict (before the human's decision).
    pub fn decision(&self) -> Decision {
        self.decision
    }

    /// Replace the command with an edited one and re-gate it from scratch —
    /// **editing is not approval**. The new text gets the same scrutiny the
    /// original did, so an edit cannot be used to smuggle something past a
    /// verdict the user already saw.
    pub fn re_gate(&mut self, command: &str, rules: &RuleSet) {
        self.command = command.trim().to_string();
        let report = prompt_report(&self.command, self.call.as_ref(), rules, self.breaks);
        self.decision = report.decision;
        self.proposal = report.proposal;
        // A fresh TextView starts at the top, which is what a re-gated report
        // wants: the new verdict is at the head of the page.
        self.text = crate::text_view::TextView::new(report.lines);
        self.command_lines = report.command;
        self.relegend();
    }

    /// **The same call, what it writes edited** (`e` on a `Write` or an
    /// `Edit`): its `call` — what the pane shows it writing — replaced, its
    /// line the edited text's (`line`: a secret's handle put in or taken out
    /// is a spend the line says, S21-Q27), and the report re-gated from
    /// scratch as any edit's is.
    pub fn re_gate_call(&mut self, call: CallShown, line: &str, rules: &RuleSet) {
        self.call = Some(call);
        self.re_gate(line, rules);
    }

    /// **The same call, under a policy that changed without changing its
    /// verdict**: the report, the legend and the `A` proposal recomputed, the
    /// reader's place kept. All three were computed under the OLD policy and
    /// were shown on until the verdict flipped — naming a rule that was gone,
    /// withholding an `A` a rule could now answer, and, through a stored
    /// cause's index, crashing (Reviewers A, D, round 17).
    pub fn refresh(&mut self, rules: &RuleSet) {
        let report = prompt_report(&self.command, self.call.as_ref(), rules, self.breaks);
        self.decision = report.decision;
        self.proposal = report.proposal;
        if self.text.lines() != report.lines.as_slice() {
            self.text.set_lines(report.lines);
        }
        self.command_lines = report.command;
        self.relegend();
    }

    /// Map a keypress to a decision, or `None` if the key isn't an action (the
    /// caller handles scrolling and ignores the rest) — read off its
    /// `key_table`, as its legend is.
    ///
    /// Fail-closed by construction: every dismissal is a deny, and a DENY
    /// verdict has no allow row. Not `n`: it shows the next call waiting and
    /// answers nothing (`App::cycle_call`; Steven, 10-20: the legend never
    /// offered it as a deny). `A` opens the rule the legend shows, or — when
    /// there is none (a glob, a variable, an unreadable line, a call that
    /// asks whatever a rule allows) — says why: ONE door for both, the
    /// caller's `propose_rule`, reading the proposal.
    pub fn resolve(&self, key: &crossterm::event::KeyEvent) -> Option<ToolAction> {
        self.key_table().resolve(key)
    }
}

/// The report is a decision's page: its sizing is the one every kind keeps.
impl DecisionPage for ToolPrompt {
    fn text(&self) -> &crate::text_view::TextView {
        &self.text
    }

    fn text_mut(&mut self) -> &mut crate::text_view::TextView {
        &mut self.text
    }

    fn legend(&self) -> Option<&PageLine> {
        self.action.as_ref()
    }

    /// **The live gate's title — a question owed** (not a passive
    /// "analyze"), so it reads as blocking at a glance; the command only
    /// when the body cannot show it ([`question`](Self::question)).
    ///
    /// **Whose call it is** after the command and before the queue — a
    /// narrow title is cut from its end, and the command is the one thing it
    /// must keep. **How many wait behind this one** — this session's, only
    /// when any do (Steven, 10-19: "how many are in the background"; then
    /// "If there's 2 calls, 1 is hidden then we say `1 queued`", the held
    /// pane's word, so the two say one thing) — AFTER the command: a narrow
    /// title cut the command it asks about to "mkdi…" to keep what followed.
    /// How to reach the pane is its number, on its border (`{n}<C-w>w`).
    fn title(&self, rows: u16, width: u16) -> String {
        let (question, subject) = self.question(self.shows_command(rows, width));
        let subject = subject.map_or(String::new(), |s| format!(" — {s}"));
        let subject = match self.whose() {
            Some(session) => format!("{subject} · {session}"),
            None => subject,
        };
        match self.behind() {
            0 => format!(" {question}{subject} "),
            n => format!(" {question}{subject} · {n} queued "),
        }
    }

    fn subject(&self) -> String {
        self.command_on_one_line()
    }

    fn keys_in_short(&self) -> String {
        self.key_table().in_short()
    }

    fn not_a_key(&self) -> String {
        self.key_table().not_a_key()
    }

    fn answers_width(&self) -> usize {
        self.key_table().answers_width()
    }

    /// Claude Code's line: a held value in it is one its model wrote.
    fn leaked(&self) -> bool {
        self.call.as_ref().is_some_and(CallShown::claude_code)
    }
}

/// Build the `:analyze <input>` report as styled lines: a verdict headline, the
/// command **as written** with the stages that need attention highlighted in
/// situ (fill-in-colour), then the findings — grouped so a shared reason isn't
/// repeated per command. The action line for the whole verdict is drawn from
/// its decision and proposal by who shows it: `:analyze` ([`report_lines`])
/// and the pane (`ToolPrompt::relegend`), whose keys change while it waits.
///
/// Read from ONE place here, with the rule `A` would offer: the pane's
/// decision, its legend and its `A` key all read this, rather than each
/// deciding the line again and possibly differently. One place, not one
/// decision: `proposed_allow` decides the line and then decides it again
/// with its rule added, and this decides it once more — four whole-line
/// decisions per Ask, beside the gate's own (review 20, Reviewer D counted;
/// docketed, since the costly part, a `re:` credential entry against a long
/// word, is now bounded per stage).
fn report_parts(input: &str, rules: &RuleSet, mode: ReportMode, breaks: Breaks) -> Report {
    let input = input.trim();
    let mut out: Vec<PageLine> = Vec::new();
    // The rules `A` would teach, in order: it offers the first, and the rest
    // follow once it is taught (a program's form, then the places — Steven,
    // 10-10). Said up front, so one `A` is not taken for the whole answer.
    let (proposal, then) = match rules.proposed_allows(input) {
        Ok(mut all) => {
            let first = all.remove(0);
            (Ok(first), all)
        }
        Err(why) => (Err(why), Vec::new()),
    };
    // Fail-closed: a line the analyzer refuses stays Ask — still a decision
    // the user has to make, so the pane shows its keys.
    let mut decision = Decision::Ask;
    let command;

    match parse_line(input) {
        Err(why) => {
            // Fail-closed: the analyzer refused the whole line. The verdict is
            // always Ask here — show it in the ask colour, echo the command
            // plainly, and say why.
            out.push(headline_line(Decision::Ask, "not analyzed — you decide"));
            out.push(Line::from("").into());
            // Mark what was refused — `$((…))`, a subshell, the quote left open
            // — rather than the whole line: every refused line used to be one
            // yellow block, the reason about one construct and the colour about
            // all of it (demo 2026-09-25, class F). A refusal found past the
            // lexer (unbalanced control flow) has no span, and marks the line.
            let refused = shannon_core::shell::lex::refusal_span(input).unwrap_or(0..input.len());
            let from = out.len();
            out.extend(format_source(
                input,
                &[(refused, Mark::Block(Tone::Warn))],
                &[],
                breaks,
            ));
            command = from..out.len();
            out.push(Line::from("").into());
            out.push(
                Line::from(vec![
                    Span::styled("  ╰─ ", Style::default().add_modifier(Modifier::DIM)),
                    Span::raw(format!("this command {}", why.reason())),
                ])
                .into(),
            );
        }
        Ok(pipeline) => {
            // The gate's own verdict, raw-line scan and all
            // (`RuleSet::decide_parsed`): the pipeline's alone headed a
            // handle in a comment ALLOW while the gate asked.
            let verdict = rules.decide_parsed(input, &pipeline);
            decision = verdict.decision;
            out.push(headline_line(
                verdict.decision,
                decision_gloss(verdict.decision),
            ));
            out.push(Line::from("").into());

            // Trace unresolved values back to the command that produced them, so a
            // read-only producer implicated by feeding a dangerous sink can be
            // flagged (its output decides what the sink acts on).
            let flows = provenance::analyze(input, &pipeline);

            // Each command's verbs tinted by its DANGER (effect) — green =
            // safe, yellow = warning, red = "look here" — and a block on what
            // each act stands on (`stage_marks`); then a `Warn` block on each
            // flow's source (read-only, but implicated). Control-flow
            // scaffolding stays plain, so the eye lands on the commands.
            let mut fills: Vec<(Range<usize>, Mark)> = pipeline
                .stages
                .iter()
                .zip(verdict.stages())
                .flat_map(|(stage, sv)| stage_marks(stage, sv))
                .collect();
            // The producing COMMAND behind each flow, and the CARRIER at its binding
            // site, are both part of the warning chain — the value comes FROM the
            // producer and is held BY the carrier — so both go yellow (`Warn`). The
            // benign `read`/`for` that merely introduces the carrier stays plain.
            let warn = Mark::Block(Tone::Warn);
            fills.extend(flows.iter().map(|f| (f.source.clone(), warn)));
            fills.extend(
                flows
                    .iter()
                    .filter_map(|f| f.carrier.as_ref())
                    .map(|c| (c.binding_site.clone(), warn)),
            );

            // Underline each flow's carrier variable at its binding + use sites, a
            // per-variable colour on a separate channel from the fill, so the eye
            // follows `source -> $VAR -> sink`.
            let underlines = carrier_underlines(&flows);
            let from = out.len();
            out.extend(format_source(input, &fills, &underlines, breaks));
            command = from..out.len();

            let findings = collect_findings(&pipeline, &verdict, mode);
            if !findings.is_empty() || !flows.is_empty() {
                out.push(Line::from("").into());
                for f in &findings {
                    out.extend(finding_lines(f));
                }
                // A link note per flow: the producer is read-only, BUT its output
                // selects what the sink acts on (polarity-correct — the relation is
                // the subject, so it doesn't read as a mis-classified `find`).
                for f in &flows {
                    out.extend(flow_finding_lines(input, &pipeline, f));
                }
            }
        }
    }
    Report {
        lines: out,
        command,
        decision,
        proposal,
        then,
    }
}

/// What [`report_parts`] builds: the report, and the two answers its action
/// line and the keys depend on.
struct Report {
    lines: Vec<PageLine>,
    /// Which of `lines` are the command — what the pane must show, or the
    /// title must carry instead (`ToolPrompt::shows_command`).
    command: Range<usize>,
    decision: Decision,
    proposal: Result<String, NoProposal>,
    /// The rules `A` offers after `proposal`, once it is taught.
    then: Vec<String>,
}

/// **The decision pane's report** — the one door [`ToolPrompt`]'s three
/// builders (`build`, `re_gate`, `refresh`) go through, so a call Claude Code
/// runs stays that kind of call whichever of them rebuilds it: the same
/// analysis (or none, for a call the rules cannot read), the note saying who
/// runs it, and the rule `A` offers for the write it makes.
fn prompt_report(
    command: &str,
    call: Option<&CallShown>,
    rules: &RuleSet,
    breaks: Breaks,
) -> Report {
    let mut report = match call.and_then(CallShown::unreadable_why) {
        Some(why) => opaque_report(command, &why),
        None => report_parts(command, rules, ReportMode::Prompt, breaks),
    };
    // **Leaving Claude Code's sandbox asks, whatever a rule allows** (finding
    // 17). The analysis stays — what the command does is what you weigh — but
    // its headline says why it asks, and `A` offers nothing: a rule allowing
    // the command would have allowed it outside the sandbox too. A deny
    // still denies.
    if let Some(why) = call.and_then(CallShown::unsandboxed_why)
        && report.decision != Decision::Deny
    {
        report.decision = Decision::Ask;
        if let Some(headline) = report.lines.first_mut() {
            *headline = headline_line(Decision::Ask, "outside Claude Code's sandbox — you decide");
        }
        report.proposal = Err(NoProposal::CannotDescribe(why));
        report.then.clear();
    }
    if let Some((path, change)) = call.and_then(|call| call.change.as_ref()) {
        // A file tool's grant names the write — `:write <path>` — not the
        // `cat` Shannon reads it as: `allow cat <file>` read as a grant to
        // READ, and covered the write (Steven, 10-05).
        if report.decision == Decision::Ask
            && let Some(rule) = write_grant(command, path, rules)
        {
            report.proposal = Ok(rule);
            report.then.clear();
        }
        report.lines.push(Line::from("").into());
        report.lines.extend(change_lines(path, change));
    }
    // Why a write of a file in the project asks: its other names.
    if let Some((file, names)) = call.and_then(|call| call.names.as_ref()) {
        report.lines.push(Line::from("").into());
        report.lines.push(
            Line::from(format!(
                "  {file} has {names} names: written in place, it changes under every one, \
                 wherever the others are, and Shannon cannot list them"
            ))
            .into(),
        );
    }
    // The rule `A` adds, in full where it is read: the legend gives it the
    // row's last cells, and a long one was cut before its first word.
    if let Ok(rule) = &report.proposal
        && report.decision != Decision::Deny
    {
        report.lines.push(Line::from("").into());
        report.lines.push(
            Line::from(vec![
                Span::styled(
                    "  A adds, this session: ",
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::raw(format!("allow {rule}")),
            ])
            .into(),
        );
        // And what follows it: one `A` per rule, each said before the first
        // is pressed, so the call still asking after one is no surprise.
        for next in &report.then {
            report.lines.push(
                Line::from(vec![
                    Span::styled(
                        "  then A adds:          ",
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    Span::raw(format!("allow {next}")),
                ])
                .into(),
            );
        }
        // Past them all, a stage no rule answers still asks: said before
        // the first `A`, not found after the last.
        let offered: Vec<String> = std::iter::once(rule.clone())
            .chain(report.then.iter().cloned())
            .collect();
        if let Ok(taught) = rules.with_allows(&offered)
            && taught.decide_line(command).decision != Decision::Allow
        {
            report.lines.push(
                Line::from(vec![
                    Span::styled(
                        "  then:                 ",
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    Span::raw(still_asks(command, &taught)),
                ])
                .into(),
            );
        }
    }
    // **Where `A` is missing, why — on the pane.** The legend dropped the
    // key and the reason waited behind a press of it, so `git -c
    // core.pager=less log` read as arbitrary; a deny said no way out at all
    // (Steven, 10-11). A deny has no `A` to miss: what lifts it, if anything.
    if let Err(why) = &report.proposal {
        let knob = credential_knob(call);
        let said = match (report.decision, why) {
            (Decision::Allow, _) | (_, NoProposal::NothingAsks) => None,
            (Decision::Deny, NoProposal::RuleCannotAnswer(act)) => {
                let lift = lifted_instead_by(act, knob);
                lift.strip_prefix(" · ")
                    .map(|l| ("  to lift it: ", l.to_string()))
            }
            (Decision::Deny, NoProposal::CannotDescribe(_)) => None,
            // The finding above says what the act is; this line says only
            // what that means for `A`.
            (Decision::Ask, NoProposal::RuleCannotAnswer(act)) => {
                Some(("  no A: ", cannot_answer(act, knob, false)))
            }
            (Decision::Ask, why) => Some(("  no A: ", no_proposal_reason(why, knob))),
        };
        if let Some((label, text)) = said {
            report.lines.push(Line::from("").into());
            report.lines.push(
                Line::from(vec![
                    Span::styled(label, Style::default().add_modifier(Modifier::DIM)),
                    Span::raw(text),
                ])
                .into(),
            );
        }
    }
    if call.is_some_and(CallShown::claude_code) {
        report.lines.push(Line::from("").into());
        report.lines.push(
            Line::from(Span::styled(
                format!("  {PROVIDER_NOTE}"),
                Style::default().add_modifier(Modifier::DIM),
            ))
            .into(),
        );
    }
    report
}

/// **The grant `A` offers for a file tool's write**: `:write <path>`, a
/// level rule naming the write. `None` where the path cannot be a rule's word
/// (a space, a leading `-` or `#`), or where the engine, asked with the rule
/// added, says it would not answer this call — the promise
/// `RuleSet::proposed_allow` makes, kept here too.
fn write_grant(line: &str, path: &str, rules: &RuleSet) -> Option<String> {
    if path.is_empty() || path.contains(char::is_whitespace) || path.starts_with(['-', '#']) {
        return None;
    }
    let rule = format!(":write {path}");
    let mut with = rules.clone();
    with.extend_from_lines(&format!("allow {rule}")).ok()?;
    (with.decide_line(line).decision == Decision::Allow).then_some(rule)
}

/// **What a file tool writes**, as a diff — the lines the transcript's record
/// of it shows ([`Record::change`](crate::tool_record::Record::change)), so
/// what was approved and what the history says ran cannot read differently.
/// Its roles are painted with the pane's (`view::painter`): the decision
/// map, where a diff's syntax keeps off the verdict's hues.
fn change_lines(path: &str, change: &shannon_core::backend::claude_code::Change) -> Vec<PageLine> {
    let mut record = crate::tool_record::Record::default();
    record.change(path, change);
    let (lines, styles) = record.into_parts();
    lines
        .iter()
        .zip(&styles)
        .map(|(line, styles)| PageLine::of_cells(line, styles))
        .collect()
}

/// The report for a call the rules cannot read: the line as written, and why
/// it asks. Always Ask, with no rule to offer — an allow rule would not answer
/// it, so the legend must not teach one.
fn opaque_report(line: &str, why: &str) -> Report {
    let proposal = Err(NoProposal::CannotDescribe(why.to_string()));
    let mut lines: Vec<PageLine> = vec![
        headline_line(Decision::Ask, "not analyzed — you decide"),
        Line::from("").into(),
    ];
    let from = lines.len();
    lines.extend(line.lines().map(|l| Line::from(l.to_string()).into()));
    let command = from..lines.len();
    lines.push(Line::from("").into());
    lines.push(
        Line::from(vec![
            Span::styled("  ╰─ ", Style::default().add_modifier(Modifier::DIM)),
            Span::raw(why.to_string()),
        ])
        .into(),
    );
    Report {
        lines,
        command,
        decision: Decision::Ask,
        proposal,
        then: Vec::new(),
    }
}

/// The whole report as one list — the `:analyze` shape, where the action line is
/// ordinary trailing content rather than a pinned legend. Descriptive (it
/// never executes): no key to press, and no `e`. **The `:analyze` page**: the
/// lines its one buffer holds (`App::open_analyze_command`).
///
/// Parses the line itself (to show the stages) and asks `rules` for the
/// verdict, so the display and the decision come from exactly the same
/// analysis the engine uses, laid out by `breaks` as the live pane would.
/// Purely a preview — nothing is executed.
pub(crate) fn report_lines(input: &str, rules: &RuleSet, breaks: Breaks) -> Vec<PageLine> {
    let mode = ReportMode::Analyze;
    let Report {
        lines: mut out,
        decision,
        proposal,
        then,
        ..
    } = report_parts(input, rules, mode, breaks);
    if let Some(action) = analyze_line(decision, &proposal, then.len()) {
        out.push(Line::from("").into());
        out.push(action);
    }
    out
}

/// **One piece of a laid-out line**: its text, what it is to the shell
/// (`syntax` — `lexeme_syntax`, coloured through the palette), and the gate's
/// marks on it (a stage's danger `fill`, a carrier's `underline`). The layout
/// is one thing and each surface paints it its own way: the pane its danger
/// over the syntax (`pane_line`), `e` and the transcript neither (`plain`).
#[derive(Debug, Clone, PartialEq)]
struct Run {
    text: String,
    syntax: CellStyle,
    fill: Option<Mark>,
    underline: Option<Ink>,
}

impl Run {
    /// Text with no colour of its own: an indent, a separating blank.
    fn plain(text: impl Into<String>) -> Run {
        Run {
            text: text.into(),
            syntax: CellStyle::PLAIN,
            fill: None,
            underline: None,
        }
    }
}

/// A laid-out line: its runs, in order.
type Runs = Vec<Run>;

/// **A run as the decision pane paints it**: the danger first, the syntax
/// under it. A yellow or red BLOCK is the alarm, on the words an act stands
/// on — no longer the whole stage (Steven, 10-11: "highlight less"). The
/// stage's tone is a TINT on its verbs' text: a claim about the command, so
/// its flags, strings and expansions keep their syntax (Steven, 10-05: "we
/// should have syntax highlighting here too"). With neither, syntax alone —
/// a ROLE, which the pane's painter colours with the theme's decision map
/// (`view::painter`).
///
/// **The verdict's colours are roles too** (Steven, 10-14: every colour is
/// the user's). They were the app's own, `Look::Own`, so that a theme could
/// recolour the shell and never the answer — but the shipped verdict names
/// the terminal's red and green, which the terminal's own theme already
/// decides, so that guarantee was never whole. What stands in its place is
/// said, not enforced: a theme that paints two steps alike is told so at
/// load (`Theme::verdicts_alike`), and the shell's roles are kept off the
/// theme's own verdict colours (`a_hostile_theme_is_told_what_it_did`).
fn pane_look(run: &Run) -> Look {
    match run.fill {
        Some(Mark::Block(tone)) => Look::Role(tone_fill(tone)),
        Some(Mark::Tint(tone)) => Look::Role(CellStyle::ink(tone.ink()).bold()),
        None => Look::Role(run.syntax),
    }
}

/// A laid-out line as the decision pane draws it: each run's look, and its
/// underline drawn over it when painted.
fn pane_line(runs: &[Run]) -> PageLine {
    PageLine {
        runs: runs
            .iter()
            .map(|run| crate::paint::Run {
                text: run.text.clone(),
                look: pane_look(run),
                underline: run.underline,
            })
            .collect(),
        style: Style::default(),
    }
}

/// The command as written, indented, with the given byte ranges background-
/// filled in their verdict colour. Ranges are sorted and any overlap dropped
/// (defensive — stage spans don't normally overlap); embedded newlines split
/// into rows so a multi-line command still reads as written. What does not
/// lex has no syntax to show.
fn source_lines(input: &str, highlights: &[(Range<usize>, Mark)]) -> Vec<Runs> {
    // Sorted, non-overlapping fill ranges.
    let mut ranges: Vec<(Range<usize>, Mark)> = highlights.to_vec();
    ranges.sort_by_key(|(r, _)| r.start);
    let mut clean: Vec<(Range<usize>, Mark)> = Vec::new();
    for (r, t) in ranges {
        if clean.last().is_some_and(|(last, _)| r.start < last.end) {
            continue;
        }
        clean.push((r, t));
    }
    // Flatten the input into styled runs (each either a tone colour or plain).
    // `get` rather than raw indexing: a defensive miss drops a run, never panics,
    // matching `format_source`'s style (Reviewers A/B — this fallback's safety
    // shouldn't rest on the caller only ever passing char-aligned ranges).
    let mut runs: Vec<(String, Option<Mark>)> = Vec::new();
    let mut pos = 0usize;
    for (r, t) in &clean {
        if r.start > pos
            && let Some(s) = input.get(pos..r.start)
        {
            runs.push((s.to_string(), None));
        }
        if let Some(s) = input.get(r.start..r.end) {
            runs.push((s.to_string(), Some(*t)));
        }
        pos = r.end;
    }
    if let Some(s) = input.get(pos..) {
        runs.push((s.to_string(), None));
    }
    // Split runs into rows on '\n', each row indented two spaces.
    let mut lines: Vec<Runs> = Vec::new();
    let mut cur: Runs = indent_runs(0);
    for (text, fill) in runs {
        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                cur.push(Run {
                    fill,
                    ..Run::plain(part)
                });
            }
            if parts.peek().is_some() {
                // The newline closes this row and opens the next (indented).
                lines.push(std::mem::replace(&mut cur, indent_runs(0)));
            }
        }
    }
    lines.push(cur);
    lines
}

/// A control-flow keyword's role in the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kw {
    /// `do` / `then` — opens an indented body after it.
    Body,
    /// `done` / `fi` — closes a body (dedent before it).
    Close,
    /// `for` / `while` / `until` / `if` / `elif` / `else` — sits inline.
    Head,
}

fn word_kw(word: &str) -> Option<Kw> {
    match word {
        "do" | "then" => Some(Kw::Body),
        "done" | "fi" => Some(Kw::Close),
        "for" | "while" | "until" | "if" | "elif" | "else" => Some(Kw::Head),
        _ => None,
    }
}

/// **Which lexemes are reserved words, as bash reads them**: an unquoted
/// `do`, `then`, `done`, … in command position — first, or after an
/// operator or another reserved word. Anywhere else it is a word:
/// `grep -n 'then' f` and `rg do src` were laid out as a body, one command
/// split in two, and `e` opened `src/lib.rs` as a command of its own
/// (review of 53feeaa, Reviewers A and C). One per lexeme, for `layout`
/// and `long_commands` alike.
fn reserved(input: &str, lexemes: &[shannon_core::shell::Spanned<Lexeme>]) -> Vec<Option<Kw>> {
    let mut command_position = true;
    lexemes
        .iter()
        .map(|sp| {
            let keyword = match &sp.node {
                Lexeme::Word(w)
                    if command_position && input.get(sp.span.clone()) == Some(w.as_str()) =>
                {
                    word_kw(w)
                }
                _ => None,
            };
            command_position = match &sp.node {
                Lexeme::Word(_) => keyword.is_some(),
                Lexeme::Op(op) => op.is_connector(),
                Lexeme::Subst(_) => false,
                Lexeme::HereDoc(_) => command_position,
            };
            keyword
        })
        .collect()
}

/// The leading indent for a body-nesting `level` (two-space base + four per level).
fn indent_runs(level: usize) -> Runs {
    vec![Run::plain(" ".repeat(2 + level * 4))]
}

// What a command's pieces are to the shell — see `lexeme_syntax`. Roles,
// as every highlighter's are (`style::Ink`): a command is a function called,
// a flag a parameter. The role and nothing else: how a command's name
// stands out is the theme's (`function.call`), so a syntect keyword and a
// shell one read alike, and a theme can say otherwise.
const COMMAND: CellStyle = CellStyle::ink(Ink::FunctionCall);
const FLAG: CellStyle = CellStyle::ink(Ink::Parameter);
const QUOTED: CellStyle = CellStyle::ink(Ink::String);
const EXPANSION: CellStyle = CellStyle::ink(Ink::VariableSpecial);
const OPERATOR: CellStyle = CellStyle::ink(Ink::Operator);
const KEYWORD: CellStyle = CellStyle::ink(Ink::Keyword);
/// A heredoc's text: data, not commands.
const DATA: CellStyle = CellStyle::PLAIN.dim();

/// **What each lexeme is to the shell**, as the colour it starts from: a
/// reserved word (`reserved`'s); a command's name — the first word in
/// command position, past any `NAME=value`; a redirect's target, plain (a
/// file, not a command); a flag; an operator; a heredoc's text; or a plain
/// word. A piece glued to the one before is that word's (`--x=$(y)z`).
///
/// Display only — the gate decides on `parse_line`'s reading, never this.
/// One per lexeme, for the pane (`layout`) and the transcript (`shell_cells`).
fn lexeme_syntax(
    input: &str,
    lexemes: &[shannon_core::shell::Spanned<Lexeme>],
    keywords: &[Option<Kw>],
) -> Vec<CellStyle> {
    let mut command_position = true;
    let mut target = false;
    let mut prev: Option<(usize, CellStyle)> = None;
    lexemes
        .iter()
        .zip(keywords)
        .map(|(sp, keyword)| {
            let glued = prev.map(|(end, _)| end) == Some(sp.span.start);
            let text = input.get(sp.span.clone()).unwrap_or_default();
            let syntax = match &sp.node {
                Lexeme::Word(_) if keyword.is_some() => {
                    command_position = true;
                    KEYWORD
                }
                Lexeme::Word(_) if glued => prev.map_or(CellStyle::PLAIN, |(_, s)| s),
                Lexeme::Subst(_) => {
                    command_position &= glued;
                    EXPANSION
                }
                Lexeme::Word(_) if std::mem::take(&mut target) => CellStyle::PLAIN,
                Lexeme::Word(_) if command_position && is_assignment(text) => EXPANSION,
                Lexeme::Word(_) if command_position => {
                    command_position = false;
                    COMMAND
                }
                Lexeme::Word(_) if text.starts_with('-') => FLAG,
                Lexeme::Word(_) => CellStyle::PLAIN,
                Lexeme::Op(
                    Operator::Write | Operator::Append | Operator::Read | Operator::HereDoc,
                ) => {
                    target = true;
                    OPERATOR
                }
                Lexeme::Op(op) => {
                    command_position |= op.is_connector();
                    OPERATOR
                }
                Lexeme::HereDoc(_) => DATA,
            };
            // Only a word continues a word: `>out` is a redirect and its
            // target, not one piece.
            prev = matches!(sp.node, Lexeme::Word(_) | Lexeme::Subst(_))
                .then_some((sp.span.end, syntax));
            syntax
        })
        .collect()
}

/// `NAME=…` or `NAME+=…`: an assignment, where a command's name would be.
fn is_assignment(word: &str) -> bool {
    let name = word.split_once('=').map_or("", |(name, _)| name);
    let name = name.strip_suffix('+').unwrap_or(name);
    name.chars()
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// **A word's characters, coloured**: what is quoted, as a string; what
/// expands (`$NAME`, `${…}`, `$?`), as an expansion — inside double quotes
/// too, where the shell expands it; the rest as the word is (`base`). One
/// style per character. Display only.
fn word_cells(text: &str, base: CellStyle) -> Vec<CellStyle> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![base; chars.len()];
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        match (quote, chars[i]) {
            // An escaped pair is what it stands in: outside quotes the word,
            // inside double quotes the string.
            (None, '\\') => i += 2,
            (Some('"'), '\\') => {
                for cell in out.iter_mut().skip(i).take(2) {
                    *cell = QUOTED;
                }
                i += 2;
            }
            (Some('"') | None, '$') => {
                let end = expansion_end(&chars, i);
                for cell in &mut out[i..end] {
                    *cell = EXPANSION;
                }
                i = end;
            }
            (None, open @ ('\'' | '"')) => {
                quote = Some(open);
                out[i] = QUOTED;
                i += 1;
            }
            (Some(close), c) => {
                if c == close {
                    quote = None;
                }
                out[i] = QUOTED;
                i += 1;
            }
            (None, _) => i += 1,
        }
    }
    out
}

/// Where the expansion starting at `chars[at]` (a `$`) ends: `${…}` through
/// its brace, `$NAME` through its name, `$?`-style one character, a bare `$`
/// itself.
fn expansion_end(chars: &[char], at: usize) -> usize {
    match chars.get(at + 1) {
        Some('{') => chars[at..]
            .iter()
            .position(|&c| c == '}')
            .map_or(chars.len(), |close| at + close + 1),
        Some(c) if *c == '_' || c.is_ascii_alphabetic() => {
            at + 1
                + chars[at + 1..]
                    .iter()
                    .take_while(|c| **c == '_' || c.is_ascii_alphanumeric())
                    .count()
        }
        Some(c) if "?#@*$!-".contains(*c) || c.is_ascii_digit() => at + 2,
        _ => at + 1,
    }
}

/// The source-colour axis: what a token communicates, decoupled from *why*.
/// Ordered green < yellow < red so the loudest tone wins when spans overlap. This
/// is deliberately its own scale, NOT the internal [`Effect`] lattice (where
/// `Unknown` dominates as `u8::MAX` — which would paint an unknown yellow over a
/// destructive red, the wrong way round for communication). Two things map onto
/// it: a command's danger ([`tone_of`]) and a provenance source's implication
/// (always [`Tone::Warn`] — read-only, but "look here").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tone {
    Safe,
    Warn,
    Danger,
}

/// **What the pane puts on a word** (P5, Steven 10-11: "highlight less …
/// focus the highlighting on the parts we should bring attention to"): the
/// stage's tone on its verbs' TEXT, and a BLOCK on what makes an act what it
/// is — `-delete`, `-exec … \;`, a redirection's target, the path a rule
/// denied ([`stage_marks`]). Tints order below blocks, so a verb that is
/// itself the cause (`rm`) takes the block; within each, the loudest tone
/// wins, as the fill always has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    Tint(Tone),
    Block(Tone),
}

/// Every mark a stage puts on the line: its verbs tinted in its tone
/// ([`stage_tone`]), and each act's cause blocked in its own
/// ([`block_tone`]). An act that asks with no words of its own (the `.` a
/// listing reads where it runs) is the verb's to show.
fn stage_marks(stage: &Stage, sv: &StageVerdict) -> Vec<(Range<usize>, Mark)> {
    let verbs: Vec<Range<usize>> = sv
        .acts
        .iter()
        .filter(|a| matches!(a.act, ActKind::Program { .. }))
        .flat_map(|a| a.at.verb.iter().cloned())
        .collect();
    let mut out: Vec<(Range<usize>, Mark)> = Vec::new();
    if let Some(tone) = stage_tone(stage, sv) {
        out.extend(verbs.iter().map(|r| (r.clone(), Mark::Tint(tone))));
    }
    for av in &sv.acts {
        let Some(tone) = block_tone(av) else {
            continue;
        };
        let at = if av.at.cause.is_empty() && av.decision != Decision::Allow {
            &verbs
        } else {
            &av.at.cause
        };
        out.extend(at.iter().map(|r| (r.clone(), Mark::Block(tone))));
    }
    out
}

/// The block an act's cause wears: the louder of what it does and what was
/// decided (an ask is at least a warning, a deny the alarm); none for what
/// is safe. A path the program names is its program's to show while it is
/// allowed — `cp a b` is a writer, and `b` needs no block of its own — and
/// while only its operation decided it: inside, `rm x` asks because it
/// deletes, which `rm` already shows (Steven, 10-12: "expecting to deny an
/// `rm` but also denying `x`?"). A rule or the boundary decides a path on
/// its own account and blocks it — and so does a guarded place it holds
/// (`rm -r .` above a key: review of c9c40fe, A and C, the block lost when
/// that moved to `reaches`); a redirection is the shell's, with no program
/// to carry it.
fn block_tone(av: &ActVerdict) -> Option<Tone> {
    let named = matches!(
        av.act,
        ActKind::Path {
            indirect: false,
            ..
        }
    );
    let by_its_op = matches!(av.by, By::Default(NoRule::Inside(_))) && av.reaches.is_empty();
    if named && (av.decision == Decision::Allow || by_its_op) {
        return None;
    }
    let does = match act_says(av) {
        Says::Effect(e) => Some(tone_of(e)),
        Says::NoClaim | Says::Nothing => None,
    };
    let decided = match av.decision {
        Decision::Allow => None,
        Decision::Ask => Some(Tone::Warn),
        Decision::Deny => Some(Tone::Danger),
    };
    does.max(decided).filter(|t| *t > Tone::Safe)
}

/// The colour a stage should wear, or `None` for "make no claim".
///
/// Two rules, both one-directional:
///
/// 1. **Uncertainty may add alarm, never remove it.** When bindings are
///    disturbed `effect()` collapses to `Unknown` — correct, fail-closed, for
///    the DECISION — but taking that as the colour downgraded `rm -r "$FOO"`
///    from red to yellow, so a destructive command read as *less* dangerous
///    because we understood it less. The tone is the LOUDEST any of the
///    stage's acts says (`act_says`), and an act no model reads, or one with
///    a word no model knows, says unknown — so knowing less only ever adds.
///    (It said "the louder of what the stage resolves to and what its
///    command intrinsically is", a reading of the flat table it no longer
///    makes: review stop 12, T7.)
/// 2. **Green is a positive claim.** A safe command hiding an unknown that could
///    change what it does (`find . "$X"`, where `$X` could be `-delete`) is
///    raised to a warning; a safe command that merely carries an unresolved
///    value it cannot act on (`echo "$FOO"`, `find src/*.rs`), or a word the
///    gate could not place (`cat "$F"`), makes no claim at all and stays
///    plain — warning there is noise that dilutes the warnings that matter.
///
/// What a command IS is read from the stage's ACTS — the decision's own
/// reading — so the colour and the verdict cannot disagree about it. It
/// was read from the flat effect table, a second reader of the models that
/// knew no subcommand tree and no user model: `git status` and a user's
/// `lsd` painted as unknown under an ALLOW (Steven, 10-11). Whether a word
/// is still unresolved is the shell's, and stays read from the stage.
fn stage_tone(stage: &Stage, sv: &StageVerdict) -> Option<Tone> {
    let says: Vec<Says> = sv.acts.iter().map(act_says).collect();
    let loudest = says
        .iter()
        .filter_map(|s| match s {
            Says::Effect(e) => Some(tone_of(*e)),
            Says::NoClaim | Says::Nothing => None,
        })
        .max()?;
    let unplaced = says.iter().any(|s| matches!(s, Says::NoClaim));
    let claimless = unplaced || stage.command.has_unresolved_operand();
    (loudest > Tone::Safe || !claimless).then_some(loudest)
}

/// What one act says about the danger of its stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Says {
    /// What it does, as the display lattice names it.
    Effect(Effect),
    /// A word the gate could not place, or a word at no operation (an
    /// unmodelled program's): it may be anything, so the stage cannot be
    /// called safe — but it is not KNOWN danger either.
    NoClaim,
    /// Nothing of its own: a peeled wrapper (what it runs is its own act),
    /// or the file a program is run by (the program act says what runs).
    Nothing,
}

/// One act's danger. A program is what its model says its node does,
/// raised to unknown when no model reads it, when the model is a
/// completion import (it says what the words are, never what they do), or
/// when a word in it is one no model knows — whatever a rule made of it.
fn act_says(av: &ActVerdict) -> Says {
    use shannon_core::permission::model::{Effect as Op, Trust};
    let of = |op: Op| match op {
        Op::Read => Effect::ReadOnly,
        Op::Write => Effect::Mutating,
        Op::Delete => Effect::Destructive,
        Op::Exec => Effect::Executor,
    };
    match &av.act {
        ActKind::Program { wrapper: true, .. }
        | ActKind::Path {
            op: Some(Op::Exec), ..
        } => Says::Nothing,
        ActKind::Program {
            effect, unknown, ..
        } => {
            let read_in_full = av
                .model
                .as_ref()
                .is_some_and(|m| m.trust != Trust::Completion)
                && unknown.is_empty();
            match of(*effect) {
                Effect::ReadOnly if !read_in_full => Says::Effect(Effect::Unknown),
                _ if av.model.is_none() => Says::Effect(Effect::Unknown),
                e => Says::Effect(e),
            }
        }
        ActKind::Path { op: Some(op), .. } => Says::Effect(of(*op)),
        ActKind::Path { op: None, .. } | ActKind::Unplaced(_) => Says::NoClaim,
        ActKind::Host { .. }
        | ActKind::Unsound(_)
        | ActKind::Secret { .. }
        | ActKind::LentVar { .. } => Says::Effect(Effect::Unknown),
        ActKind::Code { .. } | ActKind::Privilege(_) => Says::Effect(Effect::Executor),
    }
}

/// A command's danger as a colour tone: green = read-only/safe, yellow =
/// mutating or *unknown* (warning), red = destructive/executor ("look here").
/// Unknown is a yellow warning, not red — red is reserved for KNOWN danger so it
/// keeps its meaning; the fail-closed safety lives in the decision, not the colour.
fn tone_of(effect: Effect) -> Tone {
    match effect {
        Effect::ReadOnly => Tone::Safe,
        Effect::Mutating | Effect::Unknown => Tone::Warn,
        Effect::Destructive | Effect::Executor => Tone::Danger,
    }
}

/// The tone a token's span should take — the **loudest** among the spans that
/// contain it. A token can sit inside more than one: `echo $(rm -rf /)` nests a
/// destructive `rm` inside the read-only `echo`, and a provenance source overlays
/// a `Warn` on an otherwise-green producer. We surface the worst, or the "danger
/// hidden in a substitution" case would render green ("safe").
fn fill_for(span: &Range<usize>, highlights: &[(Range<usize>, Mark)]) -> Option<Mark> {
    // Loudest tone among the containing spans (max).
    overlay_for(span, highlights).max()
}

/// The loudest tone among fills lying WITHIN `construct` — for colouring a
/// substitution's inner content as one run. The inner commands' own stage fills
/// sit inside it (as does a flow's `Warn` on the producer), so a hidden `rm`
/// still shouts red (`echo $(rm -rf /)`). The command the substitution is spliced
/// INTO is excluded: its span reaches past the construct, and its danger is not
/// the inner command's — `rm -f $(find …)` painted the harmless `find` red.
fn fill_within(construct: &Range<usize>, fills: &[(Range<usize>, Mark)]) -> Option<Mark> {
    fills
        .iter()
        .filter(|(r, _)| construct.start <= r.start && r.end <= construct.end)
        .map(|(_, t)| *t)
        .max()
}

/// The underline colour a token's span should take — the carrier-variable colour
/// of the first flow whose binding/use site contains it. `None` for a non-carrier
/// token. The underline is a SEPARATE channel from the fill, so it composes: a
/// `$FOO` inside a red sink is red-fill + its variable's underline.
fn underline_for(span: &Range<usize>, underlines: &[(Range<usize>, Ink)]) -> Option<Ink> {
    // First containing span (identity — variable occurrences don't overlap).
    overlay_for(span, underlines).next()
}

/// Split a word into `(text, syntax, underline)` pieces, wherever either
/// changes: its quoted strings and expansions (`word_cells`), and a carrier
/// sitting *inside* it (`"FOO is: $FOO"` is one word), so the trace does not
/// break at the first quoted string. One piece for a word of one colour and
/// no carrier — the common case.
fn word_pieces(
    input: &str,
    span: &Range<usize>,
    base: CellStyle,
    underlines: &[(Range<usize>, Ink)],
) -> Vec<(String, CellStyle, Option<Ink>)> {
    let word = input.get(span.clone()).unwrap_or_default();
    let mut pieces: Vec<(String, CellStyle, Option<Ink>)> = Vec::new();
    for ((offset, ch), syntax) in word.char_indices().zip(word_cells(word, base)) {
        let at = span.start + offset;
        let underline = underlines
            .iter()
            .find(|(r, _)| r.contains(&at))
            .map(|(_, c)| *c);
        match pieces.last_mut() {
            Some((text, s, u)) if *s == syntax && *u == underline => text.push(ch),
            _ => pieces.push((ch.to_string(), syntax, underline)),
        }
    }
    pieces
}

/// The values of every `(range, T)` overlay whose range *contains* `span`. The
/// one "does this range cover this token" test, shared by the fill (reduced with
/// `max`) and the underline (reduced with `first`).
fn overlay_for<'a, T: Copy>(
    span: &'a Range<usize>,
    entries: &'a [(Range<usize>, T)],
) -> impl Iterator<Item = T> + 'a {
    entries
        .iter()
        .filter(move |(r, _)| r.start <= span.start && span.end <= r.end)
        .map(|(_, t)| *t)
}

/// Append one token to the current line, inserting a separating space (unless
/// `attached`, e.g. a trailing `;`). A shared background bridges the
/// separating space so a multi-token stage reads as one continuous block.
fn push_tok(
    cur: &mut Runs,
    started: &mut bool,
    prev_fill: &mut Option<Mark>,
    token: Run,
    attached: bool,
) {
    if *started && !attached {
        // Bridge the space only when both sides share a BLOCK, so a red/yellow
        // block stays continuous (`-exec rm {} \;`); a tint is on text, and
        // needs none. The underline never bridges — variables are single
        // tokens.
        let bridge = if token.fill == *prev_fill {
            token.fill.filter(|m| matches!(m, Mark::Block(_)))
        } else {
            None
        };
        cur.push(Run {
            fill: bridge,
            ..Run::plain(" ")
        });
    }
    *started = true;
    *prev_fill = token.fill;
    cur.push(token);
}

/// A token: its text, and what the layout knows of it.
fn tok(text: &str, syntax: CellStyle, fill: Option<Mark>, underline: Option<Ink>) -> Run {
    Run {
        text: text.to_string(),
        syntax,
        fill,
        underline,
    }
}

/// The command pretty-printed as **plain text** — the layout the gate shows
/// (control flow on its own indented lines), with no colour: what the
/// layout's tests read. The transcript shows it as shell (`highlighted_shell`).
#[cfg(test)]
fn pretty_command(input: &str) -> Vec<String> {
    layout(input.trim(), &[], &[], Breaks::default())
        .lines
        .iter()
        .map(|line| plain(line))
        .collect()
}

/// **A command as the transcript shows it** (Steven, 10-05, image 93:
/// "everything is in blue … put it in a markdown code block and do syntax
/// highlighting"): the shell `e` opens — the pane's layout with its ` \`
/// continuations, where that reads back as the same command
/// (`editable_command`), as written where not, and as written with no
/// `shape` (`tui.pretty_commands = false`) — each character coloured by what
/// it is to the shell, the pane's syntax without the gate's marks. Valid
/// shell, so the code block it goes in can be copied and run.
pub(crate) fn highlighted_shell(
    input: &str,
    shape: Option<Breaks>,
) -> Vec<(String, Vec<CellStyle>)> {
    let text = match shape {
        Some(breaks) => editable_command(input, breaks),
        None => input.trim().to_string(),
    };
    let mut cells = shell_cells(&text).into_iter();
    text.split('\n')
        .map(|line| {
            let styles: Vec<CellStyle> = cells.by_ref().take(line.chars().count()).collect();
            cells.next(); // the newline
            (line.to_string(), styles)
        })
        .collect()
}

/// **A `:rules` line as a page shows it** (Steven, 10-11: "`allow` is our
/// okay color, `ask` is our warning, and `deny` is our crit … everything
/// that follows … is standard syntax highlighting"). A rule's line —
/// `  <verb> <rule>`, an annotation after three spaces — has its verb in the
/// verdict's own colour ([`decision_color`], the app's, as the pane's
/// verdict is: no theme repaints what a rule decides) and the rule after it
/// coloured as shell is; its annotation, and every other line, plain.
pub(crate) fn rule_listing_line(line: &str) -> crate::paint::PageLine {
    use crate::paint::{PageLine, Run};
    let plain = || PageLine::from(ratatui::text::Line::from(line.to_string()));
    let Some(body) = line.strip_prefix("  ") else {
        return plain();
    };
    let Some((verb, rest)) = body.split_once(' ') else {
        return plain();
    };
    let decision = match verb {
        "allow" => Decision::Allow,
        "ask" => Decision::Ask,
        "deny" => Decision::Deny,
        _ => return plain(),
    };
    let (rule, note) = match rest.find("   (") {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let mut page = PageLine {
        runs: vec![
            Run::own("  ", Style::default()),
            Run::role(verb, CellStyle::ink(decision_ink(decision))),
            Run::own(" ", Style::default()),
        ],
        style: Style::default(),
    };
    // One line of shell: a rule laid out across lines is listed a line at a
    // time, and only its first carries the verb.
    let styles: Vec<CellStyle> = highlighted_shell(rule, None)
        .into_iter()
        .flat_map(|(_, styles)| styles)
        .collect();
    page.runs.extend(PageLine::of_cells(rule, &styles).runs);
    if !note.is_empty() {
        page.runs.push(Run::own(note, Style::default()));
    }
    page
}

/// A run of a listing page's heading or prose: words, or a reference — a
/// `[table]`, a `:command` — what the reader would type, painted as the
/// manual paints a literal.
#[derive(Clone, Copy)]
pub(crate) enum Said<'a> {
    Plain(&'a str),
    Ref(&'a str),
}

/// **A `:rules` section heading** (Steven, 10-11: "section headers should
/// have some color too. And paint the bracketed index"): bold in the
/// listing's heading colour (`ui.listing.heading`), its references in the
/// literal's (`ui.help.code`). Said in parts by the page that writes it, so
/// nothing guesses a heading back from the text's shape.
pub(crate) fn listing_heading(parts: &[Said]) -> crate::paint::PageLine {
    said_line(parts, Some(Ink::ListingHeading), true)
}

/// **A `:rules` line of prose**: plain, its references painted.
pub(crate) fn listing_prose(parts: &[Said]) -> crate::paint::PageLine {
    said_line(parts, None, false)
}

fn said_line(parts: &[Said], words: Option<Ink>, bold: bool) -> crate::paint::PageLine {
    use crate::paint::{PageLine, Run};
    let cell = |ink: Option<Ink>| CellStyle {
        ink,
        bold,
        ..CellStyle::PLAIN
    };
    PageLine {
        runs: parts
            .iter()
            .map(|part| match *part {
                Said::Plain(text) => Run::role(text, cell(words)),
                Said::Ref(text) => Run::role(text, cell(Some(Ink::HelpCode))),
            })
            .collect(),
        style: Style::default(),
    }
}

/// **A command's line break, marked on one row** — where a title, the
/// status row or a call in flight has no next row. One mark for every one
/// of them, put by [`one_row`]: it was written out at each (review of
/// 716f4f9, C-L4).
pub(crate) const LINE_BREAK: &str = " ⏎ ";

/// **A command with each line continuation read as the shell reads it** — a
/// `\` ending a line, outside any word or quote, and the indent after it: one
/// blank. So a command on one row marks only the breaks the shell sees. `e`
/// writes a long command back with ` \` on each continued line, and a call
/// in flight then read `cd x \ ⏎       && find …` (Steven, 10-31: "what is
/// with all these return symbols?"). A `\` the lexer holds — in a word, in
/// quotes — is the word's; text that does not lex is kept as written.
///
/// Its reader, before it marks a break: [`one_row`].
fn continuations_joined(text: &str) -> String {
    let Ok(lexemes) = lex(text) else {
        return text.to_string();
    };
    // By start, each with the farthest end so far: "is `at` inside any
    // lexeme" in a binary search, whatever the lexemes' own order.
    let mut spans: Vec<Range<usize>> = lexemes.iter().map(|sp| sp.span.clone()).collect();
    spans.sort_by_key(|span| span.start);
    let reach: Vec<usize> = spans
        .iter()
        .scan(0, |far, span| {
            *far = (*far).max(span.end);
            Some(*far)
        })
        .collect();
    let held = |at: usize| {
        let before = spans.partition_point(|span| span.start <= at);
        before > 0 && reach[before - 1] > at
    };
    let blank = |b: u8| b == b' ' || b == b'\t';
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    // A continuation read: one blank before whatever text comes next, however
    // many continuations stand between.
    let mut gap = false;
    let put = |out: &mut String, piece: &str, gap: bool| {
        if !piece.is_empty() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            out.push_str(piece);
        }
    };
    for (at, _) in text.match_indices("\\\n") {
        if at < from || held(at) {
            continue;
        }
        // The blanks before it are the separator it stands in for — unless
        // a word holds them (`a\ `, an escaped blank).
        let mut end = at;
        while end > from && blank(bytes[end - 1]) && !held(end - 1) {
            end -= 1;
        }
        put(&mut out, &text[from..end], gap);
        gap = true;
        from = at + 2;
        while from < bytes.len() && blank(bytes[from]) {
            from += 1;
        }
    }
    put(&mut out, &text[from..], gap);
    out
}

/// **A command on one row**, coloured as the shell reads it — where a
/// title, a call in flight or a shut record has no next row: the breaks the
/// shell sees marked ([`LINE_BREAK`]), a continuation a blank
/// ([`continuations_joined`]), and a heredoc's body said by its size,
/// `<<'EOF' ⟨74 lines⟩`. The body is data a row cannot show, and spelt out
/// it spent the row before what ran after it: `ran: cat > test.py <<'EOF' ⏎
/// #!/usr/bin/env python3 ⏎ …` never reached `&& python3 test.py` (Steven,
/// 11-02, images 44 and 45). Its readers: `ToolPrompt::command_on_one_line`
/// (a title, the status row) and `tool_record`'s `Record::titled` (a call
/// in flight, a shut record).
pub(crate) fn one_row(text: &str) -> (String, Vec<CellStyle>) {
    let text = continuations_joined(text.trim());
    // Each body's span — from the row after its operator's through its
    // delimiter's — and its size.
    let bodies: Vec<(Range<usize>, usize)> = lex(&text)
        .map(|lexemes| {
            lexemes
                .into_iter()
                .filter_map(|sp| match sp.node {
                    Lexeme::HereDoc(body) => Some((sp.span, body.lines().count())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let mut cells = shell_cells(&text).into_iter();
    let (mut row, mut styles) = (String::new(), Vec::new());
    let mut put = |piece: &str, look: &[CellStyle]| {
        row.push_str(piece);
        styles.extend_from_slice(look);
    };
    let mut start = 0;
    for line in text.split('\n') {
        let look: Vec<CellStyle> = cells.by_ref().take(line.chars().count()).collect();
        cells.next(); // the newline
        let at = start;
        start += line.len() + 1;
        match bodies.iter().find(|(span, _)| span.contains(&at)) {
            Some((span, lines)) => {
                if span.start == at {
                    let said = format!(" ⟨{lines} line{}⟩", if *lines == 1 { "" } else { "s" });
                    put(&said, &vec![DATA; said.chars().count()]);
                }
            }
            None => {
                if at > 0 {
                    put(
                        LINE_BREAK,
                        &vec![CellStyle::PLAIN; LINE_BREAK.chars().count()],
                    );
                }
                put(line, &look);
            }
        }
    }
    (row, styles)
}

/// Each character of `text` coloured as the layout colours it
/// (`lexeme_syntax`, `word_cells`); what no lexeme covers — a blank, a
/// continuation's `\`, a comment — plain, and all of it plain where the
/// lexer refuses the text.
fn shell_cells(text: &str) -> Vec<CellStyle> {
    let mut by_byte = vec![CellStyle::PLAIN; text.len()];
    if let Ok(lexemes) = lex(text) {
        let keywords = reserved(text, &lexemes);
        for (sp, base) in lexemes.iter().zip(lexeme_syntax(text, &lexemes, &keywords)) {
            let Some(piece) = text.get(sp.span.clone()) else {
                continue;
            };
            let cells = match sp.node {
                Lexeme::Word(_) => word_cells(piece, base),
                _ => vec![base; piece.chars().count()],
            };
            for ((offset, _), cell) in piece.char_indices().zip(cells) {
                by_byte[sp.span.start + offset] = cell;
            }
        }
    }
    text.char_indices().map(|(at, _)| by_byte[at]).collect()
}

/// **The command laid out as the gate shows it, as shell to edit** (`e`):
/// each line a statement, or a continuation of one (` \`), so a long call
/// opens in the shape it was read in rather than as one line (Steven, 10-05:
/// "are we able to pass that pretty-printed variant to `$EDITOR`").
///
/// **Offered only when it reads back as the same command** (`same_command`,
/// the analyser's own reading of both, and the reserved words bash reads).
/// The layout is for reading, and a layout once split `rg -n do src` in two
/// and folded `for x in a b⏎do` into `for x in a b do` (review of 53feeaa,
/// Reviewers A and C). Both are fixed in the layout; the check stays, and
/// where it fails the line opens as written.
pub fn editable_command(input: &str, breaks: Breaks) -> String {
    let source = input.trim();
    match layout_as_shell(source, breaks) {
        Some(text) if same_command(source, &text) => text,
        _ => input.to_string(),
    }
}

/// [`editable_command`] by the default breaks — the tests'.
#[cfg(test)]
fn editable(input: &str) -> String {
    editable_command(input, Breaks::default())
}

/// The layout as shell, unchecked — `None` when it is one line anyway.
fn layout_as_shell(source: &str, breaks: Breaks) -> Option<String> {
    let laid = layout(source, &[], &[], breaks);
    if laid.lines.len() < 2 {
        return None;
    }
    let text = laid
        .lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line = plain(line);
            // The pane's base indent goes; a heredoc's text is as written.
            let line = match laid.verbatim.contains(&i) {
                true => line,
                false => line.strip_prefix(BASE_INDENT).unwrap_or(&line).to_string(),
            };
            match laid.continued.contains(&i) {
                true => format!("{line} \\"),
                false => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(text)
}

/// **Whether two texts are one command to the shell**, as the analyser
/// reads them (`parse_line`): the same stages, each the same command — its
/// words as the shell joins them — with the same redirects, heredoc bodies,
/// place and environment, joined by the same operators (a newline is the
/// `;` it stands for).
///
/// The analyser's reading, not the layout's: this compared lexemes, and
/// dropped a separator after any word `do` or `then`, so it passed the
/// layout that split `rg -n do src` in two, and one that split a glued
/// `$(printf x)-suffix` (review of 53feeaa, Reviewers A and C).
///
/// **And the same reserved words, in order** (`reserved`): the analyser
/// peels control flow loosely, so `for x in a b\ndo …` and `for x in a b do
/// …` (a newline before `do` laid out as a space, which makes `do` a word of
/// the list) read as one; their reserved words do not.
fn same_command(a: &str, b: &str) -> bool {
    use shannon_core::shell::parse_line;
    let keywords = |text: &str| -> Option<Vec<String>> {
        let lexemes = lex(text).ok()?;
        Some(
            reserved(text, &lexemes)
                .iter()
                .zip(&lexemes)
                .filter(|(keyword, _)| keyword.is_some())
                .filter_map(|(_, sp)| match &sp.node {
                    Lexeme::Word(w) => Some(w.clone()),
                    _ => None,
                })
                .collect(),
        )
    };
    if keywords(a).is_none() || keywords(a) != keywords(b) {
        return false;
    }
    let (Ok(a), Ok(b)) = (parse_line(a), parse_line(b)) else {
        return false;
    };
    // A newline is the `;` it stands for, and a separator before the first
    // command separates nothing.
    let joined = |op: Option<Operator>| match op {
        None | Some(Operator::Newline) => Some(Operator::Seq),
        op => op,
    };
    // A redirection is what it does to which file; where it is written moves
    // with the layout.
    let redirects = |s: &shannon_core::shell::Stage| -> Vec<_> {
        s.redirects
            .iter()
            .map(|r| (r.kind, r.target.clone()))
            .collect()
    };
    a.stages.len() == b.stages.len()
        && a.stages.iter().zip(&b.stages).all(|(x, y)| {
            x.command == y.command
                && redirects(x) == redirects(y)
                && x.data == y.data
                && x.cwd == y.cwd
                && x.bindings == y.bindings
                && x.env_set == y.env_set
                && joined(x.connector) == joined(y.connector)
        })
}

/// The indent every laid-out line starts with (`indent_runs(0)`).
const BASE_INDENT: &str = "  ";

/// A laid-out line's text, without its colours or trailing blanks — but a
/// blank a backslash escapes is a word's (`rm a\ `, a file named `a `), and
/// trimmed it lost the space and left a `\` that ate the newline (found by
/// `a_one_line_layout_reads_back_as_the_same_command`).
fn plain(line: &[Run]) -> String {
    let text: String = line.iter().map(|run| run.text.as_str()).collect();
    let mut end = text.trim_end().len();
    // Give back one blank where the text before it ends in an odd run of
    // backslashes: that blank is escaped.
    let escapes = text[..end]
        .bytes()
        .rev()
        .take_while(|b| *b == b'\\')
        .count();
    if escapes % 2 == 1 && end < text.len() {
        end += 1;
    }
    text[..end].to_string()
}

/// **How a long command is laid out** (`tui.command_width`,
/// `tui.flags_per_line`): past `width` columns a chain gets a line per
/// command, a pipeline a line per stage, and a command a line per run of
/// flags, packed to `width` — and, where `flags` is not 0, no more than
/// `flags` of them to a line (Steven, 10-05: "How are you determining how
/// many flags to use before creating a new line? Can we make that
/// configurable?"). The pane, `:analyze`, the transcript and `e` lay a
/// command out by one value, so none of them reads it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Breaks {
    width: usize,
    flags: usize,
}

impl Breaks {
    /// The configured breaks; a width of `0` never breaks for length.
    pub fn of(tui: &shannon_core::config::TuiConfig) -> Breaks {
        Breaks {
            width: match tui.command_width {
                0 => usize::MAX,
                width => usize::from(width),
            },
            flags: usize::from(tui.flags_per_line),
        }
    }

    /// **How the transcript lays a command out**: as the pane does, or as
    /// written under `tui.pretty_commands = false`. Every record's and
    /// replay's shape, so a live record and a reopened one cannot differ.
    pub fn for_records(tui: &shannon_core::config::TuiConfig) -> Option<Breaks> {
        tui.pretty_commands.then(|| Breaks::of(tui))
    }
}

impl Default for Breaks {
    /// The coded defaults' — one source, the config's own.
    fn default() -> Breaks {
        Breaks::of(&shannon_core::config::TuiConfig::default())
    }
}

/// **The pipes of every pipeline too long for one line**, by where each
/// starts. A pipeline runs between statement and chain separators (`;`, a
/// newline, `&&`, `||`), and is measured alone: a long LINE of short
/// pipelines breaks at its `&&`s and keeps each pipeline whole.
fn long_pipelines(lexemes: &[shannon_core::shell::Spanned<Lexeme>], width: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut pipes: Vec<usize> = Vec::new();
    let mut span: Option<(usize, usize)> = None;
    let mut close = |span: &mut Option<(usize, usize)>, pipes: &mut Vec<usize>| {
        if span.is_some_and(|(start, end)| end - start > width) {
            out.append(pipes);
        }
        pipes.clear();
        *span = None;
    };
    for sp in lexemes {
        match &sp.node {
            Lexeme::Op(Operator::And | Operator::Or | Operator::Seq | Operator::Newline) => {
                close(&mut span, &mut pipes);
                continue;
            }
            Lexeme::Op(Operator::Pipe) => pipes.push(sp.span.start),
            _ => {}
        }
        let start = span.map_or(sp.span.start, |(start, _)| start);
        span = Some((start, sp.span.end));
    }
    close(&mut span, &mut pipes);
    out
}

/// **Where a command too long for its line breaks before a flag or a
/// redirect**, by its start (Steven, 10-05: "calls with a lot of flags …
/// pushed everything into a single line"). A command runs between
/// separators, pipes and keywords, and is measured alone. One longer than
/// the width ([`Breaks`]) is packed greedily: a flag keeps the words after
/// it (its value, an operand) on its line, a redirect its target, and a
/// line breaks only before one that would carry it past the width — so
/// `-e 'pattern'` stays one piece, and a run of short flags shares a line —
/// or that would put more of them on it than `tui.flags_per_line` allows.
/// A redirect is a place to break too (11-03, Steven's tutor example): an
/// `awk` whose one program filled the line wrapped its `> file` to the
/// pane's first column, no break before it to take. Each break comes with
/// the start of its command, which the layout hangs the rest under.
fn long_commands(
    input: &str,
    lexemes: &[shannon_core::shell::Spanned<Lexeme>],
    breaks: Breaks,
) -> Vec<(usize, usize)> {
    // The command being measured: each word's start, its width, and
    // whether a flag starts there.
    let mut words: Vec<(usize, usize, bool)> = Vec::new();
    let mut out = Vec::new();
    let pack = |words: &mut Vec<(usize, usize, bool)>, out: &mut Vec<(usize, usize)>| {
        let width = |ws: &[(usize, usize, bool)]| -> usize {
            ws.iter().map(|w| w.1).sum::<usize>() + ws.len().saturating_sub(1)
        };
        if width(words) > breaks.width {
            let starts: Vec<usize> = (0..words.len()).filter(|&i| i == 0 || words[i].2).collect();
            // The line's width so far, and how many flags it holds.
            let (mut line, mut flags) = (0, 0);
            for (n, &from) in starts.iter().enumerate() {
                let to = starts.get(n + 1).copied().unwrap_or(words.len());
                let group = width(&words[from..to]);
                let full = breaks.flags > 0 && flags == breaks.flags;
                if n > 0 && (full || line + 1 + group > breaks.width) {
                    out.push((words[from].0, words[0].0));
                    (line, flags) = (group, 1);
                } else {
                    line += usize::from(n > 0) + group;
                    flags += usize::from(n > 0);
                }
            }
        }
        words.clear();
    };
    let mut prev_end: Option<usize> = None;
    for (sp, keyword) in lexemes.iter().zip(reserved(input, lexemes)) {
        let glued = prev_end.replace(sp.span.end) == Some(sp.span.start);
        let text = input.get(sp.span.clone()).unwrap_or_default();
        let width = text.chars().count();
        match &sp.node {
            Lexeme::Word(_) if keyword.is_some() => pack(&mut words, &mut out),
            Lexeme::Word(_) | Lexeme::Subst(_) => match words.last_mut() {
                // One word to the shell (`a$(b)c`), one word here.
                Some(last) if glued => last.1 += width,
                _ => words.push((
                    sp.span.start,
                    width,
                    !words.is_empty() && text.starts_with('-'),
                )),
            },
            Lexeme::Op(Operator::Write | Operator::Append | Operator::Read | Operator::Dup) => {
                words.push((sp.span.start, width, !words.is_empty()));
            }
            // A heredoc's body follows its line: broken before, the body
            // would read as hung off the break.
            Lexeme::Op(Operator::HereDoc) => words.push((sp.span.start, width, false)),
            Lexeme::Op(_) | Lexeme::HereDoc(_) => pack(&mut words, &mut out),
        }
    }
    pack(&mut words, &mut out);
    out
}

/// The command laid out as lines, as the decision pane draws them — see
/// [`layout`] and [`pane_look`].
///
/// **A break inside one statement is drawn as shell writes it**, ` \`, and
/// quiet: the pane shows the text `e` opens ([`editable_command`]), so what
/// looks like a command is one, copied out or read (Steven, 10-31: "the
/// pretty printing doesn't show a valid command, though it looks like it").
fn format_source(
    input: &str,
    fills: &[(Range<usize>, Mark)],
    underlines: &[(Range<usize>, Ink)],
    breaks: Breaks,
) -> Vec<PageLine> {
    let laid = layout(input, fills, underlines, breaks);
    laid.lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let mut drawn = pane_line(line);
            if laid.continued.contains(&i) {
                drawn.runs.push(crate::paint::Run {
                    text: " \\".to_string(),
                    look: Look::Role(CellStyle::ink(Ink::TextQuiet)),
                    underline: None,
                });
            }
            drawn
        })
        .collect()
}

/// A command laid out ([`layout`]): its lines, and which of them the next
/// continues (a break inside one statement, where shell needs a ` \`) and
/// which are a heredoc's text, as written.
struct Layout {
    lines: Vec<Runs>,
    continued: Vec<usize>,
    verbatim: Vec<usize>,
}

/// Pretty-print the command with control-flow structure on its own indented
/// lines, filling highlighted stage spans in situ. Re-lexes the input (cheap,
/// once) to get tokens *with* their source spans, re-slices each token from the
/// original line so quotes/spelling stay exact, and lays them out: `do`/`then`
/// opens an indented body that `done`/`fi` closes, `;`/newline break statements,
/// a long chain breaks before its `&&`/`||`, a long pipeline before its pipes,
/// a long command before its flags (`long_commands`), and short ones stay on
/// the line. If lexing fails (an unanalyzable line) it falls back to the raw
/// one-line rendering.
///
/// Also says **which lines the next continues** — a break inside one
/// statement, where shell needs a ` \` to read the two as one — and which
/// are a heredoc's text ([`editable_command`]).
fn layout(
    input: &str,
    fills: &[(Range<usize>, Mark)],
    underlines: &[(Range<usize>, Ink)],
    breaks: Breaks,
) -> Layout {
    let Ok(lexemes) = lex(input) else {
        return Layout {
            lines: source_lines(input, fills),
            continued: Vec::new(),
            verbatim: Vec::new(),
        };
    };
    let mut verbatim: Vec<usize> = Vec::new();
    let long_chain = input.len() > breaks.width;
    let breaking_pipes = long_pipelines(&lexemes, breaks.width);
    let breaking_flags = long_commands(input, &lexemes, breaks);
    // Whether the line being built continues a broken chain (`&& …`), so a
    // pipeline broken inside it sits one level deeper than its siblings.
    let mut in_chain = false;
    // The column the command being laid out began at, for its flags' hang.
    let mut command_col = 0usize;
    let mut continued: Vec<usize> = Vec::new();
    let mut lines: Vec<Runs> = Vec::new();
    let mut cur = indent_runs(0);
    let mut indent = 0usize;
    let mut started = false;
    let mut prev_fill: Option<Mark> = None;
    let mut prev_end: Option<usize> = None;

    let keywords = reserved(input, &lexemes);
    let syntax = lexeme_syntax(input, &lexemes, &keywords);
    let mut it = lexemes.iter().zip(keywords).zip(syntax).peekable();
    while let Some(((sp, keyword), syntax)) = it.next() {
        // A piece that starts where the last ended is the same WORD to the
        // shell (`a$(b)c`, `X=$(cmd)`): a space between would show two.
        let glued = prev_end.replace(sp.span.end) == Some(sp.span.start);
        let text = input.get(sp.span.clone()).unwrap_or_default().to_string();
        let fill = fill_for(&sp.span, fills);
        let ul = underline_for(&sp.span, underlines);
        let this = || tok(&text, syntax, fill, ul);
        // Does the *next* token manage its own line break (so a `;` before it
        // shouldn't)? `; do` stays together; `; done` lets the dedent handle it.
        let next_break = it
            .peek()
            .is_some_and(|((_, kw), _)| matches!(kw, Some(Kw::Body | Kw::Close)));
        let next_body = it
            .peek()
            .is_some_and(|((_, kw), _)| matches!(kw, Some(Kw::Body)));

        // A long command's next run of flags, on a line of its own, hung
        // deeper than where the command began — after `&& `, after `| `, or
        // at the statement's indent.
        if breaking_flags
            .iter()
            .any(|&(_, command)| command == sp.span.start)
        {
            let width: usize = cur.iter().map(|run| run.text.chars().count()).sum();
            command_col = width + usize::from(started);
        }
        if started
            && breaking_flags
                .iter()
                .any(|&(flag, _)| flag == sp.span.start)
        {
            lines.push(std::mem::take(&mut cur));
            continued.push(lines.len() - 1);
            cur = vec![Run::plain(" ".repeat(command_col + 4))];
            started = false;
            prev_fill = None;
        }

        match &sp.node {
            Lexeme::Word(_) => match keyword {
                Some(Kw::Body) => {
                    push_tok(&mut cur, &mut started, &mut prev_fill, this(), false);
                    lines.push(std::mem::take(&mut cur));
                    indent += 1;
                    cur = indent_runs(indent);
                    started = false;
                    prev_fill = None;
                }
                Some(Kw::Close) => {
                    if started {
                        lines.push(std::mem::take(&mut cur));
                    }
                    indent = indent.saturating_sub(1);
                    cur = indent_runs(indent);
                    started = false;
                    prev_fill = None;
                    push_tok(&mut cur, &mut started, &mut prev_fill, this(), false);
                }
                // A plain word: split where its colour changes — a string, an
                // expansion — or a carrier sits inside it (`"FOO is: $FOO"`).
                _ => {
                    let pieces = word_pieces(input, &sp.span, syntax, underlines);
                    for (i, (text, syntax, colour)) in pieces.iter().enumerate() {
                        push_tok(
                            &mut cur,
                            &mut started,
                            &mut prev_fill,
                            tok(text, *syntax, fill, *colour),
                            i > 0 || glued, // the pieces of one word are contiguous
                        );
                    }
                }
            },
            Lexeme::Op(Operator::Seq) => {
                // `;` attaches to the previous token, then breaks the statement
                // unless the next token manages its own break.
                in_chain = false;
                push_tok(
                    &mut cur,
                    &mut started,
                    &mut prev_fill,
                    tok(";", OPERATOR, None, None),
                    true,
                );
                if !next_break {
                    lines.push(std::mem::take(&mut cur));
                    cur = indent_runs(indent);
                    started = false;
                    prev_fill = None;
                }
            }
            Lexeme::Op(Operator::Newline) => {
                in_chain = false;
                if started && !next_break {
                    lines.push(std::mem::take(&mut cur));
                    cur = indent_runs(indent);
                    started = false;
                    prev_fill = None;
                } else if started && next_body {
                    // `for x in a b⏎do` keeps `do` on this line, so the newline
                    // it stood after is written as the `;` it is: laid out
                    // as a space, `do` became a word of the list.
                    push_tok(
                        &mut cur,
                        &mut started,
                        &mut prev_fill,
                        tok(";", OPERATOR, None, None),
                        true,
                    );
                }
            }
            // A substitution renders as three parts so the eye lands on the
            // producing COMMAND (`find …`), not the `$(…)`/`<(…)` plumbing that
            // captures it: the opener and closer are plain, and the inner content is
            // ONE run coloured by the WORST tone within it — a destructive command
            // hidden in a substitution (`echo $(rm -rf /)`) must still shout red.
            Lexeme::Subst(_) => {
                let inner = subst_inner_span(input, &sp.span);
                let opener = input.get(sp.span.start..inner.start).unwrap_or_default();
                let body = input.get(inner.clone()).unwrap_or_default();
                let closer = input.get(inner.end..sp.span.end).unwrap_or_default();
                let inner_fill = fill_within(&sp.span, fills);
                let inner_ul = underline_for(&inner, underlines);
                if !opener.is_empty() {
                    push_tok(
                        &mut cur,
                        &mut started,
                        &mut prev_fill,
                        tok(opener, syntax, None, None),
                        glued,
                    );
                }
                if !body.is_empty() {
                    // `attached` when an opener preceded it, so `<(find` has no gap.
                    // One run, so it is coloured as the command it is: its own
                    // green, where it earns one (`pane_look`).
                    push_tok(
                        &mut cur,
                        &mut started,
                        &mut prev_fill,
                        tok(body, COMMAND, inner_fill, inner_ul),
                        !opener.is_empty(),
                    );
                }
                if !closer.is_empty() {
                    push_tok(
                        &mut cur,
                        &mut started,
                        &mut prev_fill,
                        tok(closer, syntax, None, None),
                        !opener.is_empty() || !body.is_empty(),
                    );
                }
            }
            // A heredoc's body — and its delimiter line — as written, on lines
            // of their own: it is the text being fed (often a script being
            // written to disk), so the reader must see it, verbatim and not
            // re-laid-out. Dimmed: data, not commands.
            Lexeme::HereDoc(_) => {
                if started {
                    lines.push(std::mem::take(&mut cur));
                }
                for line in text.lines() {
                    verbatim.push(lines.len());
                    lines.push(vec![tok(line, DATA, None, None)]);
                }
                cur = indent_runs(indent);
                started = false;
                prev_fill = None;
            }
            // A LONG line breaks before each `&&`/`||`/`|`, the operator leading
            // a continuation line — one command a line, so the eye can count
            // them. The demo's four-command cleanup rendered as one wrapped
            // line (2026-09-25), and so did a four-stage pipeline, its `awk`
            // the danger and the last thing on the line (Steven, 10-04). A
            // short one (`pwd && ls`, `ls | wc -l`) stays whole.
            Lexeme::Op(Operator::And | Operator::Or) if long_chain => {
                if started {
                    lines.push(std::mem::take(&mut cur));
                    continued.push(lines.len() - 1);
                }
                cur = indent_runs(indent);
                cur.push(Run::plain("  "));
                in_chain = true;
                started = false;
                prev_fill = None;
                push_tok(&mut cur, &mut started, &mut prev_fill, this(), false);
            }
            // A pipeline too long for its line, a stage a line — but only
            // that pipeline: a short one inside a long chain stays whole, so
            // the chain's commands still read one a line.
            Lexeme::Op(Operator::Pipe) if breaking_pipes.contains(&sp.span.start) => {
                if started {
                    lines.push(std::mem::take(&mut cur));
                    continued.push(lines.len() - 1);
                }
                cur = indent_runs(indent);
                cur.push(Run::plain(if in_chain { "    " } else { "  " }));
                started = false;
                prev_fill = None;
                push_tok(&mut cur, &mut started, &mut prev_fill, this(), false);
            }
            // A short line's pipes and `&&`/`||`, and redirect operators, stay
            // on the same line.
            Lexeme::Op(_) => push_tok(&mut cur, &mut started, &mut prev_fill, this(), false),
        }
    }
    if started || lines.is_empty() {
        lines.push(cur);
    }
    Layout {
        lines,
        continued,
        verbatim,
    }
}

/// The in-situ style for a [`Tone`], escalating so the eye is drawn to what
/// matters (`[[project_shannon_analyze_diagnostics]]`):
/// - **Safe** → green *foreground* only — reassuring, low-key ("this is fine");
/// - **Warn** → yellow *background* fill — a noticeable warning;
/// - **Danger** → red *background* fill, **bold** — "LOOK HERE".
///
/// Green is fg-only so a whole safe command isn't a heavy block; the fill (a
/// coloured background) is reserved for what needs attention, and red + bold wins
/// over yellow so the truly dangerous stage dominates even amid warnings. Each
/// a role (`ui.safe`, `ui.caution.fill`, `ui.danger.fill`), so the colours are
/// the theme's and the emphasis with them.
fn tone_fill(tone: Tone) -> CellStyle {
    CellStyle::ink(match tone {
        Tone::Safe => Ink::Safe,
        Tone::Warn => Ink::CautionFill,
        Tone::Danger => Ink::DangerFill,
    })
}

impl Tone {
    /// The tone's traffic-light role (`ui.safe`, `ui.caution`, `ui.danger`) —
    /// the single source the tint and the findings' effect-label
    /// ([`effect_ink`]) draw from, so the danger scale can't drift between two
    /// hand-kept tables.
    fn ink(self) -> Ink {
        match self {
            Tone::Safe => Ink::Safe,
            Tone::Warn => Ink::Caution,
            Tone::Danger => Ink::Danger,
        }
    }
}

/// The verdict → role mapping: the safety attractor. Green means it runs
/// unprompted, yellow means you'll be asked, red means it's blocked — as
/// shipped; a theme says what each looks like.
fn decision_ink(d: Decision) -> Ink {
    match d {
        Decision::Allow => Ink::Safe,
        Decision::Ask => Ink::Caution,
        Decision::Deny => Ink::Danger,
    }
}

/// One-word gloss for the line-level verdict.
fn decision_gloss(d: Decision) -> &'static str {
    match d {
        Decision::Allow => "runs without asking",
        Decision::Ask => "you'll be prompted first",
        Decision::Deny => "this line will not run",
    }
}

/// The line-level summary row: a bold, verdict-coloured tag plus what it means.
fn headline_line(d: Decision, gloss: &str) -> PageLine {
    use crate::paint::Run;
    let tag = match d {
        Decision::Allow => "ALLOW",
        Decision::Ask => "ASK",
        Decision::Deny => "DENY",
    };
    PageLine {
        runs: vec![
            Run::own("  ", Style::default()),
            Run::role(format!("{tag:<5}"), CellStyle::ink(decision_ink(d)).bold()),
            Run::own(" ", Style::default()),
            Run::own(gloss, Style::default().add_modifier(Modifier::DIM)),
        ],
        style: Style::default(),
    }
}

/// A group of stages that share a decision, effect, and reason. One note covers
/// them all (the commands are listed) instead of repeating an identical reason
/// per command — the "don't repeat a note N times" lesson
/// (`[[project_shannon_analyze_diagnostics]]`).
struct Finding {
    says: ActSays,
    /// The programs that share this finding, in first-seen order, de-duplicated.
    commands: Vec<String>,
}

/// What one act's finding says — the key acts are grouped by.
#[derive(PartialEq)]
struct ActSays {
    decision: Decision,
    /// What the act does, when it says (`act_says`); a word not placed
    /// says no effect of its own.
    effect: Option<Effect>,
    /// The model a rule-free verdict rests on ([`model_said`]).
    model: Option<String>,
    /// The API call it makes ([`call_said`]).
    call: Option<String>,
    why: String,
}

/// Group the attention-worthy (Ask/Deny) acts of every stage into findings
/// by what they say, so the brew-loop's four "not modeled" stages become
/// one note listing four commands, not four identical notes — and a stage
/// with two acts asking (a write outside, a flag no allow named) is two
/// notes, each saying its own why.
///
/// `:analyze` explains an allow too: each program act allowed, after what
/// asks, so an ALLOW says whose model allowed it (a user's `lsd`, Steven
/// 10-11). The prompt pane is a question, and lists only what it asks.
fn collect_findings(pipeline: &Pipeline, verdict: &Verdict, mode: ReportMode) -> Vec<Finding> {
    let explained = |a: &&ActVerdict| match a.decision {
        Decision::Allow => {
            mode == ReportMode::Analyze && matches!(a.act, ActKind::Program { wrapper: false, .. })
        }
        Decision::Ask | Decision::Deny => true,
    };
    let mut findings: Vec<Finding> = Vec::new();
    for (stage, sv) in pipeline.stages.iter().zip(verdict.stages()) {
        for act in sv.acts.iter().filter(explained) {
            // A program act is named as it was called — `git push`, not
            // `git`, beside an allowed `git status`; the `rm` a `find -exec`
            // runs, not the `find` — and any other act by its stage.
            let label = match &act.act {
                ActKind::Program { name, chain, .. } => std::iter::once(name)
                    .chain(chain)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => command_label(&stage.command),
            };
            let says = ActSays {
                decision: act.decision,
                effect: match act_says(act) {
                    Says::Effect(e) => Some(e),
                    Says::NoClaim | Says::Nothing => None,
                },
                model: model_said(act),
                call: match &act.act {
                    ActKind::Program { call, .. } => call.as_ref().map(call_said),
                    _ => None,
                },
                why: explain_act(act),
            };
            let at = match findings.iter().position(|f| f.says == says) {
                Some(at) => at,
                None => {
                    findings.push(Finding {
                        says,
                        commands: Vec::new(),
                    });
                    findings.len() - 1
                }
            };
            if !findings[at].commands.contains(&label) {
                findings[at].commands.push(label.clone());
            }
        }
    }
    // What asks first: the allows explain, after.
    findings.sort_by_key(|f| f.says.decision == Decision::Allow);
    findings
}

/// The model a verdict with no rule rests on — its file and its trust
/// (`model: programs/lsd.toml, author`) — since nothing else stands behind
/// it: a user's own model allowing `lsd` is their statement, not ours. A
/// rule's verdict is the rule's, and says so in its reason.
fn model_said(act: &ActVerdict) -> Option<String> {
    let m = act.model.as_ref()?;
    matches!(act.by, By::Default(_))
        .then(|| format!("model: {}, {}", m.source.shown(), m.trust.name()))
}

/// The call as the user would write it to the API: every method given
/// (which one the program takes is its own), the host, the endpoint — and
/// what it is, when the model knows.
fn call_said(call: &shannon_core::permission::Call) -> String {
    let at = call.endpoint.trim_start_matches('/');
    let target = match &call.host {
        Some(host) => format!("{host}/{at}"),
        None => at.to_string(),
    };
    let said = format!("{} {target}", call.methods.join("/"));
    match &call.gloss {
        Some(gloss) => format!("{said} — {gloss}"),
        None => said,
    }
}

/// A short name for a stage's command — its program, or a placeholder for a bare
/// assignment (`FOO=bar` with no command).
fn command_label(command: &Command) -> String {
    if command.program().is_empty() {
        "(env assignment)".to_string()
    } else {
        command.program().to_string()
    }
}

/// Render one grouped finding: a verdict tag, the command(s), the effect, and
/// the shared reason hanging beneath.
fn finding_lines(f: &Finding) -> Vec<PageLine> {
    use crate::paint::Run;
    let says = &f.says;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let plain = |text: &str| Run::own(text, Style::default());
    let line = |runs: Vec<Run>| PageLine {
        runs,
        style: Style::default(),
    };
    let mut head = vec![
        plain("  "),
        Run::role(
            decision_tag(says.decision),
            CellStyle::ink(decision_ink(says.decision)).bold(),
        ),
        plain("  "),
        Run::own(
            f.commands.join(", "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(effect) = says.effect {
        head.push(plain("  ·  "));
        head.push(Run::role(
            effect_name(effect),
            CellStyle::ink(effect_ink(effect)),
        ));
    }
    if let Some(model) = &says.model {
        head.push(plain("  ·  "));
        head.push(Run::own(model.clone(), dim));
    }
    let mut lines = vec![line(head)];
    if let Some(call) = &says.call {
        lines.push(line(vec![Run::own("   ├─ ", dim), plain(call)]));
    }
    lines.push(line(vec![Run::own("   ╰─ ", dim), plain(&says.why)]));
    lines
}

/// The underline spans for every flow's carrier variable — its binding site and
/// each use site, a role per variable (`ui.analyze.carrier.N`, by first-seen
/// order) so multiple carriers in one line stay individually traceable. The
/// shipped carriers are muted and deliberately NOT green/yellow/red (those
/// are the danger channel), so the carrier reads as a separate signal that
/// composes with the danger fill.
fn carrier_underlines(flows: &[Flow]) -> Vec<(Range<usize>, Ink)> {
    let mut order: Vec<&str> = Vec::new();
    let mut out: Vec<(Range<usize>, Ink)> = Vec::new();
    for c in flows.iter().filter_map(|f| f.carrier.as_ref()) {
        let idx = order
            .iter()
            .position(|v| *v == c.variable)
            .unwrap_or_else(|| {
                order.push(&c.variable);
                order.len() - 1
            });
        let role = Ink::CARRIERS[idx % Ink::CARRIERS.len()];
        out.push((c.binding_site.clone(), role));
        out.extend(c.use_sites.iter().map(|u| (u.clone(), role)));
    }
    out
}

/// The link note for one provenance flow: a `warn` tag naming the producer and
/// what it feeds, then the polarity-correct explanation — the producer is
/// read-only, BUT its output decides what the sink acts on. Stated relation-first
/// so it doesn't read as "we mis-classified a harmless command."
///
/// "read-only" is said only when every stage of the producer is: a producer that
/// writes too (`X=$(rm -v a); rm -r $X`) is red in the source above, and the note
/// must not contradict it.
fn flow_finding_lines(input: &str, pipeline: &Pipeline, flow: &Flow) -> Vec<PageLine> {
    let source = unwrap_subst(input.get(flow.source.clone()).unwrap_or_default());
    let read_only = pipeline
        .stages
        .iter()
        .filter(|s| flow.source.start <= s.span.start && s.span.end <= flow.source.end)
        .all(|s| matches!(s.effect(), Effect::ReadOnly));
    let sink = input.get(flow.sink.clone()).unwrap_or_default();
    let sink_prog = pipeline
        .stages
        .iter()
        .find(|s| s.span == flow.sink)
        .map_or_else(
            || sink.split_whitespace().next().unwrap_or(sink),
            |s| s.command.program(),
        );
    let feeds = match &flow.carrier {
        Some(c) => format!("feeds ${}", c.variable),
        None => format!("feeds {sink_prog}'s arguments"),
    };
    let relation =
        format!("its output selects what `{sink_prog}` acts on — check it to know what runs");
    use crate::paint::Run;
    let line = |runs: Vec<Run>| PageLine {
        runs,
        style: Style::default(),
    };
    vec![
        line(vec![
            Run::own("  ", Style::default()),
            Run::role("[warn]", CellStyle::ink(Ink::Caution).bold()),
            Run::own("  ", Style::default()),
            Run::own(source, Style::default().add_modifier(Modifier::BOLD)),
            Run::own("  ·  ", Style::default()),
            Run::own(feeds, Style::default()),
        ]),
        line(vec![
            Run::own("   ╰─ ", Style::default().add_modifier(Modifier::DIM)),
            Run::own(
                if read_only {
                    format!("read-only, but {relation}")
                } else {
                    relation
                },
                Style::default(),
            ),
        ]),
    ]
}

/// Strip a substitution wrapper for display: `<(cmd)` / `$(cmd)` / `` `cmd` `` ->
/// `cmd`, so a link note reads `find . -type f`, not `<(find . -type f)`.
fn unwrap_subst(text: &str) -> &str {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix("<(").or_else(|| t.strip_prefix("$(")) {
        inner.strip_suffix(')').unwrap_or(inner)
    } else if let Some(inner) = t.strip_prefix('`') {
        inner.strip_suffix('`').unwrap_or(inner)
    } else {
        t
    }
}

/// Who the report is for — the one place the *same* analysis diverges.
///
/// `:analyze` never executes, so its action line is DESCRIPTIVE (it names what a
/// prompt would offer and never offers to escalate). The live gate is about to
/// run something, so its action line is the real key legend. Everything above
/// the action line — headline, coloured source, findings — is identical, which is
/// the point: the preview shows you exactly what the gate will show you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportMode {
    /// `:analyze <cmd>` — a dry run. Descriptive; nothing to act on.
    Analyze,
    /// The live permission gate, with a decision pending.
    Prompt,
}

/// **`:analyze`'s line for the whole verdict** — descriptive only: an ASK
/// names the options the prompt *would* offer and a DENY names the only real
/// paths. The live gate's legend is its key table's
/// ([`ToolPrompt::key_table`]; this held both, review stop 8 C 8).
///
/// `more` is how many rules `A` offers after the first: the prompt lists
/// them below; this preview counts them here.
///
/// **No arrow before it.** The lead-in was a cyan `→`, and an arrow at the
/// head of a list of options *is* a selection cursor in every TUI that has
/// one — so it read as "deny is currently selected, move me". Nothing moved,
/// because nothing was ever selected: each option has its own key and
/// pressing it commits. A false affordance is worse than a bare list, and
/// making it real would be worse still — a cursor means `Enter` commits
/// whatever it happens to be resting on, which is exactly the accident the
/// gate exists to prevent. So the live legend NAMES its row (`answer:`), and
/// this preview, offering nothing, is indented.
fn analyze_line(
    d: Decision,
    proposal: &Result<String, NoProposal>,
    more: usize,
) -> Option<PageLine> {
    let always = proposal.as_ref().ok();
    let line = match d {
        Decision::Allow => return None,
        // No "explain" — that action is deferred and unbound, and a preview that
        // teaches a key which does not exist is worse than one that says less.
        // "always" IS listed when it exists: it is the only key that mutates
        // session policy, so omitting it hid the consequential one. (Reviewer F.)
        Decision::Ask => match always {
            Some(rule) => {
                let more = match more {
                    0 => String::new(),
                    n => format!(" + {n} more"),
                };
                format!(
                    "at the prompt: allow once · always (`allow {rule}`{more}, this session) \
                     · edit · deny"
                )
            }
            None => "at the prompt: allow once · edit · deny".into(),
        },
        // "the rule" sent a user denied by `[secrets] path_policy` to look for
        // a rule `:rules` does not have (review 20, Reviewer F).
        Decision::Deny => "blocked — edit the command, or change what denies it (a rule, or \
                           [secrets]) yourself"
            .into(),
    };
    let mut runs = vec![crate::paint::Run::role("  ", CellStyle::ink(Ink::Tool))];
    runs.extend(PageLine::from(Line::from(vec![Span::raw(line)])).runs);
    Some(PageLine {
        runs,
        style: Style::default(),
    })
}

/// Why a stage got its decision: its first act that is not allowed, in
/// plain language — or, allowed, its first act.
pub(crate) fn explain_stage(stage: &StageVerdict) -> String {
    // The act that decided the stage — a deny over an ask, and among the
    // asks a HARD one first, since no rule lifts it and the sentence must
    // not send the user to write one (review 2026-10-10, E2: `A` named the
    // secret handle while this named the host) — else the first.
    let deciding = stage.acts.iter().filter(|a| a.decision == stage.decision);
    deciding
        .clone()
        .find(|a| matches!(a.by, By::Hard(_)))
        .or_else(|| deciding.clone().next())
        .or(stage.acts.first())
        .map(explain_act)
        .unwrap_or_else(|| "nothing to decide".into())
}

/// **Why `command` still asks** under `rules`: the first stage that asks,
/// named when the line has several, and the pane's own reason
/// ([`explain_stage`]). Said twice, one sentence: by the prompt before `A`
/// is pressed, past every rule it will offer ([`prompt_report`]), and by the
/// app after a rule was taught with the decision open
/// (`App::revisit_open_decision`).
///
/// It said "it does not cover the open call" whatever held the call, so a
/// credential path or `sudo` — which ask whatever a rule allows — read as "your
/// rule is too narrow", and the next move was a wider rule that could never
/// help (Reviewer F, round 16).
pub(crate) fn still_asks(command: &str, rules: &RuleSet) -> String {
    let Ok(pipeline) = parse_line(command) else {
        return "the open call still asks — it could not be analyzed".into();
    };
    let verdict = rules.decide_parsed(command, &pipeline);
    let Some((stage, held)) = pipeline
        .stages
        .iter()
        .zip(verdict.stages())
        .find(|(_, v)| v.decision != Decision::Allow)
    else {
        return "the open call still asks".into();
    };
    let why = explain_stage(held);
    // A call a rule denies is "still denied", not "still asks": a deny pane
    // revisited stays open, and says which it is.
    let stands = match held.decision {
        Decision::Deny => "is still denied",
        _ => "still asks",
    };
    if pipeline.stages.len() > 1 {
        let part = command.get(stage.span.clone()).unwrap_or(command).trim();
        format!("`{part}` {stands} — {why}")
    } else {
        format!("the open call {stands} — {why}")
    }
}

/// Plain-language "why this act got this decision": what it is, what
/// decided it — the rule cited, the default named, the hard ask — and what
/// an allow left uncovered, so "denied by rule <X>" and "asking for the
/// write outside" are actionable.
///
/// From the verdict alone: it took the `RuleSet` to look the rule up by
/// index, and a cause kept past a policy change crashed or named another
/// rule (Reviewers A, D, round 17).
///
/// The place's own account comes first; what it holds that a rule guards,
/// met from above, follows it (`ActVerdict::reaches`): "`/` holds Shannon's
/// own state" was said of `rm -r /` as its reason, where Steven expected
/// "outside where Shannon started" (10-13).
pub(crate) fn explain_act(act: &ActVerdict) -> String {
    if act.reaches.is_empty() {
        return explain_account(act, true);
    }
    // What a guarded place's output costs is said once, at the end (review
    // of e10b6d5, B: a guard named outright and one held below each said it).
    format!(
        "{}; it also {}{}",
        explain_account(act, false),
        reach_clause(&act.reaches),
        sent(act)
    )
}

/// "holds X, which this may reach (the rule and N more)" — what a place
/// holds that rules met softly keep: said as the reason where it is one,
/// and after the place's own account where that decides.
fn reach_clause(cited: &[Cited]) -> String {
    format!(
        "holds {}, which this may reach ({})",
        held_below(cited),
        citation(cited)
    )
}

/// The rule a verdict cites first: the strictest, then by what it keeps
/// ([`kept`]) — and the first of what is left, so the same line says the
/// same rule (the last of ten equal guards was `~/.docker/config.json`). A
/// path act's rules are all named or all met softly (`decide_path`), so
/// which it was met by needs no place in the order.
fn first_cited(cited: &[Cited]) -> Option<&Cited> {
    cited
        .iter()
        .min_by_key(|c| (std::cmp::Reverse(c.action), kept(c.guard).0))
}

/// What a rule keeps, and where a phrase names it: Shannon's state, then
/// credentials, then any other rule's place. A `match`, so a new kind of
/// guard does not compile until it is said — it was a table the type did
/// not check (review of c9c40fe, A and C).
fn kept(guard: Option<Guard>) -> (u8, &'static str) {
    match guard {
        Some(Guard::OwnState) => (0, "Shannon's own state"),
        Some(Guard::Credential) => (1, "paths that commonly hold credentials"),
        None => (2, "places a rule guards"),
    }
}

/// The rule cited, and how many stand beside it: `ask :* \c~/.ssh and 9
/// more` — `:rules` lists them.
fn citation(cited: &[Cited]) -> String {
    let Some(first) = first_cited(cited) else {
        return String::new();
    };
    match cited.len() - 1 {
        0 => first.one_line().into_owned(),
        more => format!("{} and {more} more", first.one_line()),
    }
}

/// What the rules met softly keep, in a phrase: "Shannon's own state and
/// paths that commonly hold credentials".
fn held_below(cited: &[Cited]) -> String {
    let mut kinds: Vec<(u8, &str)> = cited.iter().map(|c| kept(c.guard)).collect();
    kinds.sort();
    kinds.dedup();
    kinds
        .iter()
        .map(|(_, said)| *said)
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Why a guarded place matters to a command that may read it: what it
/// reads reaches the model. A write or a delete sends nothing of what is
/// there — said of `rm`, the clause was false.
fn sent(act: &ActVerdict) -> &'static str {
    use shannon_core::permission::model::Effect;
    match &act.act {
        ActKind::Path {
            op: Some(Effect::Write | Effect::Delete),
            ..
        } => "",
        _ => " — and command output is sent to the model and written to this session's log",
    }
}

/// [`explain_act`]'s first part: what decided the act — with what a
/// guarded place's output costs ([`sent`]) when `tail`, as the last thing
/// said.
fn explain_account(act: &ActVerdict, tail: bool) -> String {
    let tail = match tail {
        true => sent(act),
        false => "",
    };
    // The program act's subject is the command the finding line names, so
    // it says nothing of its own — and two commands held for one reason
    // share one finding (`collect_findings`).
    let subject = match &act.act {
        ActKind::Program { .. } => String::new(),
        ActKind::Path { word, .. } => format!("`{word}`"),
        ActKind::Host { dir, host } => match dir {
            shannon_core::permission::acts::Dir::Fetch => format!("a fetch from {host}"),
            shannon_core::permission::acts::Dir::Send => format!("a send to {host}"),
        },
        ActKind::Code { program, what } => format!("`{program}`: {what}"),
        ActKind::Privilege(w) => format!("`{w}`"),
        ActKind::Unsound(why) => why.clone(),
        ActKind::Unplaced(w) => format!("`{w}`"),
        ActKind::Secret { handle, .. } => handle.clone(),
        ActKind::LentVar { name, .. } => format!("`{name}`"),
    };
    match &act.by {
        By::Rules(cited) => {
            let verb = match act.decision {
                Decision::Allow => "allowed",
                Decision::Ask => "flagged (asks)",
                Decision::Deny => "denied",
            };
            // The rule that decided: the strictest among those cited. Where it
            // came from says what to do about it: edit your config, `remove`
            // a shipped rule, or `:undeny` it for this session.
            let rule = first_cited(cited).expect("a verdict by rules cites at least one");
            if act.decision == Decision::Allow || act.uncovered.is_empty() {
                if let Some(guard) = rule.guard {
                    // Say what makes it sensitive AND why that matters here:
                    // this command may be harmless to the machine and hand a
                    // secret to a third party. Met from above, the place only
                    // HOLDS the guarded one: "`.` matches a pattern that
                    // commonly holds credentials", said of the start with
                    // Shannon's state below it, was false twice (Steven, 10-11).
                    if rule.reached {
                        // All met softly, as the first is: a path act's
                        // rules are named or met softly, never both
                        // (`decide_path`).
                        return format!("{subject} {}{tail}", reach_clause(cited));
                    }
                    let what = match guard {
                        Guard::Credential => "matches a pattern that commonly holds credentials",
                        Guard::OwnState => "is Shannon's own state",
                    };
                    return format!("{subject} {what} ({}){tail}", citation(cited));
                }
                format!(
                    "{}{verb} by rule: {} ({})",
                    lead(&subject),
                    rule.one_line(),
                    rule.origin.describe()
                )
            } else {
                // A rule names exactly what it names; `*` is what covers the
                // rest. Said, since `allow lsd -a` read as covering `lsd -a .`
                // (Steven, 10-11: kept exact, the `*` form named).
                let line = rule.one_line();
                let wider = match line.trim_end().ends_with('*') {
                    true => String::new(),
                    false => format!(" · `{} *` covers any other word or flag", line.trim_end()),
                };
                format!(
                    "{}allowed by {} ({}), which does not cover: {} — asking for that{wider}",
                    lead(&subject),
                    line,
                    rule.origin.describe(),
                    act.uncovered.join(", ")
                )
            }
        }
        By::Default(why) => match why {
            NoRule::ModelledRead => {
                "read-only, and no rule says otherwise — allowed by default".into()
            }
            NoRule::Wrapper => "runs the command inside it, which is judged on its own".into(),
            // Short enough for a row: the finding line names the command and
            // its effect, so the reason says only the place and the grant.
            NoRule::Inside(op) => match act.decision {
                Decision::Allow => format!("{subject} is inside where Shannon started — allowed"),
                _ => format!(
                    "{} of {subject}, which no rule grants — asking",
                    op_name(*op)
                ),
            },
            NoRule::Outside => {
                format!("{subject} is outside where Shannon started — asking")
            }
            NoRule::Unmodelled => "not a program Shannon models, so what it does is unknown — \
                                   asking; a rule naming it runs it"
                .into(),
            NoRule::Completion => "its model is imported from shell completions — it says \
                                   what the words are, not that a call is safe — asking; a \
                                   rule naming it runs it"
                .into(),
            NoRule::Effect(effect) => format!(
                "it {}, and no rule covers it — asking by default",
                match effect {
                    shannon_core::permission::model::Effect::Read => "reads",
                    shannon_core::permission::model::Effect::Write => "writes",
                    shannon_core::permission::model::Effect::Delete => "deletes",
                    shannon_core::permission::model::Effect::Exec => "runs programs",
                }
            ),
            NoRule::Unknown(words) => format!(
                "it carries {}, which its model does not know — asking",
                words
                    .iter()
                    .map(|w| format!("`{w}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            NoRule::Guarded(whys) => format!(
                "{} — a rule naming the flag, or `*`, runs it",
                whys.join("; ")
            ),
            NoRule::NeedsNamed(whys) => format!(
                "it {} — only a rule naming it exactly lifts that",
                whys.join("; ")
            ),
            NoRule::Host => {
                // The direction's own verb: a `:fetch` rule grants no send
                // (review 2026-10-10, E2).
                let verb = match &act.act {
                    ActKind::Host {
                        dir: shannon_core::permission::acts::Dir::Send,
                        ..
                    } => "send",
                    _ => "fetch",
                };
                format!(
                    "{subject}, which no rule names — asking (`allow :{verb} <host>` grants it)"
                )
            }
        },
        By::Hard(why) => match why {
            // Two very different situations behind one shape, and the
            // difference is the whole message. A handle we minted is the
            // feature working: the user decides whether *this* command should
            // spend it. A handle we did not mint could not have been written
            // from inside this session, so it is either a mistake or an
            // injected instruction — say so, and that it will not run.
            Hard::Secret => match &act.act {
                ActKind::Secret {
                    handle,
                    known: true,
                } => format!(
                    "spends {handle} — the real value is substituted at execution and is \
                     never shown to the model, but it IS sent wherever this command sends it"
                ),
                ActKind::Secret { handle, .. } => format!(
                    "names {handle}, which this session does not hold — nothing could have \
                     written that handle from here, so it is a mistake or an injected \
                     instruction; it will refuse to run"
                ),
                // Reaching for the variable rather than spending the handle:
                // nothing is handed it, so this reads a value only where a
                // shell set it again — which is why it is asked, not refused.
                ActKind::LentVar { name, handle } => format!(
                    "names `{name}`, the variable lent as {handle} — no command is handed it, \
                     but a shell that sets it again would print the value; a command uses the \
                     secret by spending {handle}"
                ),
                _ => "spends a secret".into(),
            },
            // Privilege is a different question from effect: the command's
            // own effect is whatever it is, and it applies to every file on
            // the machine.
            Hard::Privilege => format!(
                "runs under {subject}, so it acts as another user — whatever this \
                 command does, it can do to any file on the machine; this asks \
                 whatever a rule allows"
            ),
            Hard::Unsound => format!("{subject} — this asks whatever a rule allows"),
            Hard::Unplaced => format!(
                "{subject} could not be placed — it still expands, or a `cd` could not be \
                 followed — so where it reaches is not known; the gate never guesses"
            ),
            // A hard ask, said as its siblings are: "code no rule covers"
            // read as "write the rule", and none answers (Steven, 10-11). The
            // subject already says what the code is ("code the model does not
            // read", "`core.pager` names a program it runs"): only the
            // consequence follows it — said twice, it read as a stutter.
            Hard::Code => format!("{subject} — this asks whatever a rule allows"),
        },
    }
}

/// A subject with the space after it, or nothing: the program act has none.
fn lead(subject: &str) -> String {
    if subject.is_empty() {
        String::new()
    } else {
        format!("{subject} ")
    }
}

fn op_name(op: Option<shannon_core::permission::model::Effect>) -> &'static str {
    use shannon_core::permission::model::Effect;
    match op {
        Some(Effect::Read) => "a read",
        Some(Effect::Write) => "a write",
        Some(Effect::Delete) => "a delete",
        Some(Effect::Exec) => "a run",
        None => "a use",
    }
}

fn decision_tag(d: Decision) -> &'static str {
    match d {
        Decision::Allow => "[allow]",
        Decision::Ask => "[ask]  ",
        Decision::Deny => "[deny] ",
    }
}

/// An effect in PROSE, for a finding a person reads ("read-only"). Not the
/// rule syntax: a rule's class is `:readonly`, which the core names itself
/// (`permission::rule::effect_name`) for a trigger that quotes the rule.
fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::ReadOnly => "read-only",
        Effect::Mutating => "mutating",
        Effect::Destructive => "destructive",
        Effect::Executor => "executor",
        Effect::Unknown => "unknown",
    }
}

/// Effect severity → the findings' effect-label role. Routed through [`Tone`]
/// (`tone_of` + [`Tone::ink`]) so the danger traffic light is defined ONCE:
/// green = read-only/safe, yellow = warning/*unknown*, red = destructive/executor.
/// (Unknown is yellow here, not the fail-closed-lattice red — red stays reserved
/// for KNOWN danger; enforcement lives in the decision, colour is communication.)
fn effect_ink(effect: Effect) -> Ink {
    tone_of(effect).ink()
}

#[cfg(test)]
mod tests {
    use super::AnalyzeView;
    use crate::decision_page::DecisionPage;
    use crate::style::Ink;
    use crate::theme::{Context, Theme};
    use shannon_core::permission::RuleSet;

    /// **A page as the decision pane paints it** (`TextView::painted_as`,
    /// `Analyze` — the decision map, as `Tool`'s), so every colour these
    /// tests read was chosen where the app chooses it.
    ///
    /// Leaked, so a test may hold a span it found past the statement that
    /// found it — test memory, a page at a time, for chains written when the
    /// lines were the view's own.
    pub(super) trait Painted {
        fn painted(&self) -> &'static [ratatui::text::Line<'static>];
    }

    impl Painted for crate::text_view::TextView {
        fn painted(&self) -> &'static [ratatui::text::Line<'static>] {
            Box::leak(
                self.painted_as(crate::workspace::PaneKind::Page(
                    crate::workspace::PageKind::Analyze,
                ))
                .into_boxed_slice(),
            )
        }
    }

    /// One line's text, spans concatenated.
    fn line_text(line: &ratatui::text::Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Flatten a view's styled lines to plain text, so a test can assert on
    /// content without reaching into spans.
    fn flatten(view: &AnalyzeView) -> String {
        view.text()
            .painted()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **A word that may become a flag is said on the pane** — the bare "an
    /// argument still expands" read as a broken command (review 20,
    /// Reviewer F). `$F` could be `--force`, so the deny on it may hold once
    /// the shell expands the word: the stage cannot be read soundly, and
    /// says so, whatever `allow git *` says. The rendered report, not the
    /// verdict enum.
    #[test]
    fn a_word_that_may_become_a_flag_is_said() {
        let mut rs = RuleSet::default();
        rs.extend_from_lines("allow git:*\ndeny re:^git push.*(-f|--force)")
            .unwrap();
        let shown = flatten(&AnalyzeView::of("git push $F origin main", &rs));
        assert!(shown.contains("ASK"), "{shown}");
        assert!(shown.contains("`$F` may expand into a flag"), "{shown}");
    }

    /// **Every headline says what its verdict means** — the tag AND the words
    /// beside it, for a line of each verdict, through the pane's own report.
    /// Tests read the tag alone, so an ASK headed "runs without asking"
    /// passed the crate (review stop 12, T7 3). The match fails to compile on
    /// a new verdict until its headline is said here.
    #[test]
    fn every_headline_says_what_its_verdict_means() {
        use shannon_core::permission::Decision;
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        for (line, decision) in [
            ("cat notes.txt", Decision::Allow),
            ("rm /etc/x", Decision::Ask),
            ("rm --no-preserve-root -rf /x", Decision::Deny),
        ] {
            let want = match decision {
                Decision::Allow => "  ALLOW runs without asking",
                Decision::Ask => "  ASK   you'll be prompted first",
                Decision::Deny => "  DENY  this line will not run",
            };
            let shown = flatten(&AnalyzeView::of(line, &rs));
            assert_eq!(shown.lines().next(), Some(want), "`{line}`:\n{shown}");
        }
    }

    /// The `$BAR` probe: the loop binds `FOO`, but the sink deletes `$BAR` — an
    /// UNRELATED, untraceable variable. So there is no flow: `read` is benign
    /// (green, not a yellow warning), and `find` stays green (not implicated). The
    /// colour must never imply a trace that isn't there.
    #[test]
    fn benign_read_and_untraced_sink_stay_calm() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while read -r FOO; do rm -r "$BAR"; done < <(find . -type f)"#,
            &rs,
        );
        let read = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "read")
            .expect("read token")
            .style;
        // `read` is a benign builtin: green foreground, NOT a yellow warning fill.
        assert_eq!(read.fg, Some(Color::Green), "read is benign/green");
        assert_eq!(read.bg, None, "read is not a yellow warning fill");
        // `$BAR` is untraceable, so `find` is not implicated — no yellow, no note.
        let find = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("find . -type f"))
            .expect("find token")
            .style;
        assert_eq!(find.bg, None, "find stays green — no flow was invented");
        assert!(
            !flatten(&view).contains("its output selects"),
            "no invented link note"
        );
    }

    /// The canonical provenance case: a read-only `find` feeding a destructive
    /// `rm` through a loop variable. `find` must render as a WARNING (yellow),
    /// not green — it is the implicated source (Steven's report).
    #[test]
    fn implicated_source_renders_yellow_not_green() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)"#,
            &rs,
        );
        // The `<(find …)` producer token carries a yellow FILL, not the green fg
        // its intrinsic read-only effect would give.
        let find = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("find . -type f"))
            .expect("the find producer is rendered");
        assert_eq!(
            find.style.bg,
            Some(Color::Yellow),
            "implicated source is yellow"
        );
    }

    /// **Uncertainty may only add alarm, never remove it.** When an earlier
    /// stage disturbs the bindings (here `read IFS=…`, which can poison word
    /// splitting), every later stage's effect collapses to `Unknown` — correct,
    /// fail-closed, for the *decision*. But taking that as the colour downgraded
    /// `rm -r "$FOO"` from red to yellow: the command read as LESS dangerous
    /// exactly because we understood it less, and the whole line went one flat
    /// shade of yellow. (Steven's report.)
    #[test]
    fn disturbed_bindings_never_downgrade_a_dangerous_command() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while read IFS= FOO; do echo "hi"; rm -r "$FOO"; done < <(find . -type f)"#,
            &rs,
        );
        let spans: Vec<_> = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .collect();

        let rm = spans
            .iter()
            .find(|s| s.content == "rm")
            .expect("the rm is rendered");
        assert_eq!(
            rm.style.bg,
            Some(Color::Red),
            "a destructive command stays RED even when we can't vouch for its name"
        );

        // The counterpart: a benign command we can no longer identify is a
        // genuine warning — yellow, not green, and not red either.
        let echo = spans
            .iter()
            .find(|s| s.content.contains("echo"))
            .expect("the echo is rendered");
        assert_eq!(
            echo.style.bg,
            Some(Color::Yellow),
            "an unidentifiable echo warns, but isn't 'LOOK HERE'"
        );
    }

    /// **What a command is comes from the decision's own reading.** The
    /// colour was read from a flat table that knew no subcommand tree and no
    /// user model, so an allowed `git status` and a user's `lsd` painted as
    /// unknown (amber), and an asking `git push` was labelled `unknown`
    /// (Steven, 10-11). Asserted on the rendered cells.
    #[test]
    fn a_tree_and_a_users_model_are_painted_as_their_acts_read_them() {
        use ratatui::style::Color;
        use shannon_core::permission::model::{Models, Source, load};
        let lsd = load::parse(
            "name = \"lsd\"\neffect = \"read\"\nreads_here = \"list\"\n\
             positional = [{ at = \"rest\", role = \"list\" }]\n",
            Source::User("lsd.toml".into()),
        )
        .expect("the model parses");
        let imported = load::parse(
            "name = \"lsc\"\neffect = \"read\"\ntrust = \"completion\"\n\
             positional = [{ at = \"rest\", role = \"list\" }]\n",
            Source::User("lsc.toml".into()),
        )
        .expect("the model parses");
        let mut models = Models::shipped().clone();
        models.insert(lsd);
        models.insert(imported);
        let rs = RuleSet::defaults()
            .with_models(std::sync::Arc::new(models))
            .with_start("/home/me/proj");
        let verb = |line: &str, name: &str| {
            let view = AnalyzeView::of(line, &rs);
            let style = view
                .text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content == name)
                .unwrap_or_else(|| panic!("`{name}` is rendered in `{line}`"))
                .style;
            (style.fg, style.bg)
        };
        use shannon_core::permission::Decision;
        assert_eq!(
            rs.decide_line("git status").decision,
            Decision::Allow,
            "premise"
        );
        assert_eq!(verb("git status", "git"), (Some(Color::Green), None));
        assert_eq!(
            rs.decide_line("lsd src").decision,
            Decision::Allow,
            "premise"
        );
        assert_eq!(verb("lsd src", "lsd"), (Some(Color::Green), None));
        // Unmodelled, it is still a warning — the colour reads the model; so
        // is a word the model does not know, and a model that says only
        // what the words are (a completion import).
        // (An unknown program is itself the cause, and wears the block; a
        // known one with a word it does not know is tinted, and the word
        // wears it.)
        assert_eq!(verb("lsdx src", "lsdx").1, Some(Color::Yellow));
        assert_eq!(verb("lsd --frob src", "lsd"), (Some(Color::Yellow), None));
        assert_eq!(verb("lsd --frob src", "--frob").1, Some(Color::Yellow));
        assert_eq!(verb("lsc src", "lsc").1, Some(Color::Yellow));

        let said = flatten(&AnalyzeView::of("git push origin main", &rs));
        assert!(said.contains("git push  ·  mutating"), "{said}");
        assert!(!said.contains("unknown"), "{said}");
    }

    /// **A finding says what its verdict rests on** (P5). With no rule, the
    /// model — whose file, what trust — dim on its head line; for a call to
    /// an API, the method, host and endpoint, and the model's gloss; each
    /// program act by its own name and chain. `:analyze` explains an allow
    /// too, after what asks; the prompt lists only what it asks.
    #[test]
    fn a_finding_says_what_its_verdict_rests_on() {
        use ratatui::style::Modifier;
        use shannon_core::permission::model::{Models, Source, load};
        let lsd = load::parse(
            "name = \"lsd\"\neffect = \"read\"\ntrust = \"author\"\n",
            Source::User("lsd.toml".into()),
        )
        .expect("the model parses");
        let mut models = Models::shipped().clone();
        models.insert(lsd);
        let rs = RuleSet::defaults()
            .with_models(std::sync::Arc::new(models))
            .with_start("/home/me/proj");
        // The finding line holding `head` in `line`'s report, and the next.
        let finding = |rs: &RuleSet, line: &str, head: &str| {
            let lines = AnalyzeView::of(line, rs).text().painted();
            let at = lines
                .iter()
                .position(|l| line_text(l).contains(head))
                .unwrap_or_else(|| panic!("`{head}` in:\n{}", flatten(&AnalyzeView::of(line, rs))));
            (lines[at].clone(), line_text(&lines[at + 1]))
        };

        let (head, _) = finding(&rs, "lsd", "[allow]  lsd");
        let model = head
            .spans
            .iter()
            .find(|s| s.content == "model: programs/lsd.toml, author")
            .expect("the user's model, by its file and trust");
        assert!(model.style.add_modifier.contains(Modifier::DIM));
        // `ls` is a generated model, and says so (review stop 12, Q13);
        // `gh`, written by hand, says `verified`.
        let (head, _) = finding(&rs, "ls -la", "[allow]  ls");
        assert!(line_text(&head).ends_with("model: shipped, generated"));
        let (head, _) = finding(&rs, "gh pr list", "[allow]  gh pr list");
        assert!(line_text(&head).ends_with("model: shipped, verified"));

        let (_, call) = finding(&rs, "gh api user", "[allow]  gh api");
        assert_eq!(
            call,
            "   ├─ GET api.github.com/user — the authenticated user's profile"
        );
        let (_, call) = finding(&rs, "gh api repos/o/r/issues -f t=x", "[ask]    gh api");
        assert_eq!(call, "   ├─ POST api.github.com/repos/o/r/issues");

        let (head, why) = finding(&rs, "uniq notes.txt out.txt", "[ask]    uniq");
        assert!(line_text(&head).contains("model: shipped"));
        assert_eq!(
            why,
            "   ╰─ it writes, and no rule covers it — asking by default"
        );
        // A rule's verdict is the rule's.
        let mut ruled = RuleSet::defaults().with_start("/home/me/proj");
        ruled
            .extend_from_lines("allow uniq *")
            .expect("the rule parses");
        let (head, why) = finding(&ruled, "uniq notes.txt out.txt", "[allow]  uniq");
        assert!(!line_text(&head).contains("model:"), "{}", line_text(&head));
        assert!(why.contains("allowed by rule"), "{why}");

        let said = flatten(&AnalyzeView::of("git status && git push", &rs));
        let push = said.find("[ask]    git push").expect("the push, by name");
        let status = said
            .find("[allow]  git status")
            .expect("the status, by name");
        assert!(push < status, "what asks first:\n{said}");
        let asked: String = super::ToolPrompt::of("t", "git status && git push", &rs)
            .text()
            .painted()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(asked.contains("[ask]    git push"), "{asked}");
        assert!(!asked.contains("[allow]"), "the prompt asks:\n{asked}");
    }

    /// **Every `A` is said before the first** (P5, "and N more"): a later
    /// stage's rule follows the first stage's on the prompt, and a stage no
    /// rule answers is said as still asking — past every `A`, not found
    /// after the last. `:analyze`, a preview, counts them.
    #[test]
    fn every_a_is_said_before_the_first() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        let prompt = |line: &str| {
            super::ToolPrompt::of("t", line, &rs)
                .text()
                .painted()
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let two = prompt("git push && cat /etc/hosts");
        assert!(
            two.contains("A adds, this session: allow git push"),
            "{two}"
        );
        assert!(
            two.contains("then A adds:          allow :read /etc/hosts"),
            "{two}"
        );
        assert!(!two.contains("still asks"), "they answer it:\n{two}");

        let held = prompt("git push && sudo ls");
        assert!(
            held.contains("then:                 `sudo ls` still asks — runs under `sudo`"),
            "{held}"
        );

        let previewed = flatten(&AnalyzeView::of("git push && cat /etc/hosts", &rs));
        assert!(
            previewed.contains("always (`allow git push` + 1 more, this session)"),
            "{previewed}"
        );
    }

    /// **The block goes on what the act stands on; the verb's text carries
    /// the stage's tone** (Steven, 10-11, choosing among mocks: "Verb tinted
    /// + block on cause"). Read off the rendered cells, the four he chose by.
    #[test]
    fn the_block_goes_on_the_cause_and_the_verb_is_tinted() {
        use ratatui::style::Color;
        use shannon_core::permission::Decision;
        let mut rs = RuleSet::defaults().with_start("/home/me/proj");
        rs.extend_from_lines("deny :read delete_this_dir")
            .expect("the rule parses");
        // (fg, bg) of the first span that is `word`, in `line`'s report.
        let look = |line: &str, word: &str| {
            let view = AnalyzeView::of(line, &rs);
            let style = view
                .text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content == word)
                .unwrap_or_else(|| panic!("`{word}` is rendered in `{line}`"))
                .style;
            (style.fg, style.bg)
        };
        let bg = |line: &str, word: &str| look(line, word).1;

        let delete = "find . -name '*.o' -delete";
        assert_eq!(
            bg(delete, "-delete"),
            Some(Color::Red),
            "the flag that deletes"
        );
        assert_eq!(
            look(delete, "find"),
            (Some(Color::Red), None),
            "the verb, tinted"
        );
        assert_eq!(bg(delete, "'*.o'"), None, "the rest, its syntax");

        let exec = "find . -exec rm {} \\;";
        for word in ["-exec", "rm", "{}", "\\;"] {
            assert_eq!(
                bg(exec, word),
                Some(Color::Red),
                "`{word}`: -exec through its end"
            );
        }
        let spans: Vec<_> = AnalyzeView::of(exec, &rs)
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .cloned()
            .collect();
        let after = spans
            .iter()
            .position(|s| s.content == "-exec")
            .and_then(|i| spans.get(i + 1))
            .expect("a blank after `-exec`");
        assert_eq!(
            (after.content.as_ref(), after.style.bg),
            (" ", Some(Color::Red)),
            "one block, bridged"
        );

        let redirect = "echo hi > notes.txt";
        assert_eq!(bg(redirect, "notes.txt"), Some(Color::Yellow), "the write");
        assert_eq!(bg(redirect, ">"), Some(Color::Yellow), "with its operator");
        assert_eq!(bg(redirect, "hi"), None);

        let denied = "ls delete_this_dir";
        assert_eq!(rs.decide_line(denied).decision, Decision::Deny, "premise");
        assert_eq!(
            bg(denied, "delete_this_dir"),
            Some(Color::Red),
            "the path it denied"
        );
        assert_eq!(bg(denied, "ls"), None, "not the program that reads");

        let rm = "rm notes.txt";
        assert_eq!(
            bg(rm, "rm"),
            Some(Color::Red),
            "the verb that is the danger"
        );
        // Its file asks only because it is deleted — `rm` says so — while
        // a file outside asks on its own account (Steven, 10-12).
        assert_eq!(rs.decide_line(rm).decision, Decision::Ask, "premise");
        assert_eq!(bg(rm, "notes.txt"), None, "what it deletes, inside");
        assert_eq!(
            bg("rm /tmp/a", "/tmp/a"),
            Some(Color::Red),
            "what it deletes, outside"
        );

        // A file a writer names, allowed, is the writer's to show. (`uniq`
        // is modelled, its second operand written: a write at an operation,
        // which an unmodelled `cp`'s words are not.)
        let uniq = "uniq notes.txt out.txt";
        assert!(
            shannon_core::permission::model::Models::shipped()
                .get("uniq")
                .is_some(),
            "premise: uniq is modelled"
        );
        assert_eq!(
            bg(uniq, "uniq"),
            Some(Color::Yellow),
            "a writer with no rule"
        );
        assert_eq!(bg(uniq, "out.txt"), None, "its allowed write");
    }

    /// The traffic light must not cry wolf, and green must mean something.
    ///
    /// `IFS= read -r FOO` is the single most-recommended safe-bash idiom; it was
    /// rendering RED because `IFS` was modelled like `LD_PRELOAD`. And a green
    /// `echo "$FOO"` over-claimed ("fully accounted for") while a yellow one
    /// over-warned (echo cannot become dangerous). (Steven's report.)
    #[test]
    fn the_traffic_light_reserves_red_and_green_for_real_claims() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while IFS= read -r FOO; do echo "FOO is: $FOO"; rm -r "$FOO"; echo "hi"; done < <(find . -type f)"#,
            &rs,
        );
        let spans: Vec<_> = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .collect();
        let bg_of = |needle: &str| {
            spans
                .iter()
                .find(|s| s.content.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is rendered"))
                .style
        };

        // `IFS=` warns; it does not shout. Red is for code injection.
        assert_eq!(bg_of("IFS=").bg, Some(Color::Yellow));
        // A safe command carrying an unresolved value makes NO claim: plain.
        assert_eq!(bg_of("FOO is:").bg, None, "no green, no warning");
        // A fully-known safe command still earns its green — on its name,
        // the claim's subject; its string keeps its syntax (Steven, 10-05).
        let verb_before = |needle: &str| {
            let at = spans
                .iter()
                .position(|s| s.content.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is rendered"));
            spans[..at]
                .iter()
                .rev()
                .find(|s| s.content == "echo")
                .expect("an echo before it")
                .style
        };
        assert_eq!(verb_before(r#""hi""#).fg, Some(Color::Green));
        // …in the decision pane's string, which is not the claim's green.
        let string = Theme::terminal().paint(Context::Decision, Ink::String).fg;
        assert_eq!(bg_of(r#""hi""#).fg, string);
        assert_ne!(string, Some(Color::Green));
        assert_ne!(
            verb_before("FOO is:").fg,
            Some(Color::Green),
            "the claim-less echo is not green"
        );
        // The destructive one keeps the loudest colour.
        assert_eq!(bg_of("rm").bg, Some(Color::Red));

        // The carrier is traceable even INSIDE a bigger token.
        let embedded = spans
            .iter()
            .find(|s| s.content == "$FOO")
            .expect("the embedded reference is its own run");
        assert!(
            embedded
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED),
            "the trace shouldn't break at a quoted string"
        );
    }

    /// **A refused line marks what was refused, not the whole line** (demo
    /// 2026-09-25, class F): the `$((…))` is yellow, the `ls` beside it plain,
    /// and the reason names arithmetic — it said "command substitution".
    #[test]
    fn a_refused_line_marks_the_construct() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("echo $((1+2)) && ls -la", &rs);
        let spans: Vec<_> = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .collect();
        let bg = |needle: &str| {
            spans
                .iter()
                .find(|s| s.content.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is rendered"))
                .style
                .bg
        };
        assert_eq!(bg("$((1+2))"), Some(Color::Yellow));
        assert_eq!(bg("-la"), None, "the rest of the line is not unread");
        assert!(spans.iter().any(|s| s.content.contains("arithmetic")));
    }

    /// **A long chain gets a line per command**, the `&&`/`||` leading each
    /// continuation (the demo's cleanup line was one wrapped block); a short
    /// chain stays on one line.
    #[test]
    fn a_long_chain_breaks_before_its_operators() {
        let demo = "cd delete_this_dir && find . -type f -name \"*.txt\" | head -20 \
                    && rm -f $(find . -name \"*.tmp\") && echo \"Cleanup complete\"";
        let lines = super::pretty_command(demo);
        assert_eq!(lines.len(), 4, "{lines:#?}");
        assert_eq!(lines[0].trim(), "cd delete_this_dir");
        assert!(lines[1].trim_start().starts_with("&& find"), "{lines:#?}");
        assert!(lines[1].contains("| head -20"), "a pipe stays on its line");
        assert_eq!(super::pretty_command("pwd && ls -la"), ["  pwd && ls -la"]);
    }

    /// **A pipeline too long for a line gets a line per stage**, the `|`
    /// leading each — the one in Steven's screenshot (10-04) put its
    /// dangerous `awk` last on one wrapped line. Inside a broken chain it
    /// sits a level deeper than the chain's commands; a short one stays whole.
    #[test]
    fn a_long_pipeline_breaks_before_its_pipes() {
        let piped = "find . -type f -name \"*.js\" -o -name \"*.py\" -o -name \"*.md\" | xargs \
                     wc -l 2>/dev/null | tail -1 | awk '{print \"Total: \" $1}'";
        let lines = super::pretty_command(piped);
        assert_eq!(lines.len(), 4, "{lines:#?}");
        assert!(lines[0].trim_start().starts_with("find"), "{lines:#?}");
        for (line, stage) in lines[1..].iter().zip(["| xargs", "| tail", "| awk"]) {
            assert!(line.trim_start().starts_with(stage), "{lines:#?}");
        }
        assert_eq!(super::pretty_command("ls | wc -l"), ["  ls | wc -l"]);

        let chained = format!("cd somewhere && {piped}");
        let lines = super::pretty_command(&chained);
        let lead = |l: &str| l.len() - l.trim_start().len();
        let and = lines
            .iter()
            .find(|l| l.trim_start().starts_with("&&"))
            .expect("&&");
        let pipe = lines
            .iter()
            .find(|l| l.trim_start().starts_with("|"))
            .expect("|");
        assert!(lead(pipe) > lead(and), "{lines:#?}");
    }

    /// The `rg` of image 91 (10-05), one line of flags in the pane.
    const MANY_FLAGS: &str = "rg --line-number --fixed-strings -e 'let sealed = \
        seal.unwrap_or_else(|| config.authored());' -e 'let preference = \
        graph.episode(&base).map_or(config, |e| e.config.clone());' -e 'ep.config.authored()' \
        crates/shannon-tui/src/app/session_ops.rs";

    /// **A command too long for a line breaks before its flags** (Steven,
    /// 10-05: "calls with a lot of flags … pushed everything into a single
    /// line"): a flag keeps its value, a run of short flags shares a line,
    /// and the runs hang deeper than the command — after `&& ` too. A short
    /// command stays whole.
    #[test]
    fn a_long_command_breaks_before_its_flags() {
        let lines = super::pretty_command(MANY_FLAGS);
        assert_eq!(
            lines.iter().map(|l| l.trim()).collect::<Vec<_>>(),
            [
                "rg --line-number --fixed-strings",
                "-e 'let sealed = seal.unwrap_or_else(|| config.authored());'",
                "-e 'let preference = graph.episode(&base).map_or(config, |e| e.config.clone());'",
                "-e 'ep.config.authored()' crates/shannon-tui/src/app/session_ops.rs",
            ]
        );
        let lead = |l: &str| l.len() - l.trim_start().len();
        assert_eq!(lead(&lines[1]), lead(&lines[0]) + 4, "{lines:#?}");

        let chained = super::pretty_command(&format!("cd here && {MANY_FLAGS}"));
        let rg = chained
            .iter()
            .position(|l| l.contains("&& rg"))
            .expect("&& rg");
        let command = chained[rg].find("rg").unwrap();
        assert_eq!(lead(&chained[rg + 1]), command + 4, "{chained:#?}");

        let short = "cargo test -p shannon-tui --lib -- --nocapture";
        assert_eq!(super::pretty_command(short), [format!("  {short}")]);

        // A piece glued to a substitution is part of its word, not a flag to
        // break before (review of 53feeaa, Reviewer A: the merge was unheld).
        let glued = "rg --line-number --fixed-strings -e 'first pattern, long' \
                     $(printf x)-suffix-not-a-flag -e 'second pattern, longer still' src";
        let lines = super::pretty_command(glued);
        assert!(
            lines.iter().all(|l| !l.trim_start().starts_with("-suffix")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("$(printf x)-suffix-not-a-flag")),
            "{lines:#?}"
        );
        // A reserved word only where bash reads one: quoted, `'then'` is a
        // program, and laid out as a body it would read as structure.
        assert_eq!(
            super::pretty_command("echo a; 'then' b"),
            ["  echo a;", "  'then' b"]
        );
    }

    /// **A long command breaks before its redirect too** (11-03, the
    /// tutor's example): one `awk` program filling the line wrapped its
    /// `> file` to the pane's first column, with no break to take. Its
    /// target stays with it; a short one stays whole.
    #[test]
    fn a_long_command_breaks_before_its_redirect() {
        let awk = "awk 'BEGIN { for (i = 1; i <= 5; i++) printf \"%d,%d,item-%02d\\n\", i, \
                   i*i, i }' > /tmp/scratchpad/permtest/data.csv";
        let lines = super::pretty_command(awk);
        let trimmed: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
        assert_eq!(trimmed.len(), 2, "{lines:#?}");
        // (The ` \` is the drawing's, not the layout's.)
        let first = trimmed[0].starts_with("awk 'BEGIN") && trimmed[0].ends_with("i }'");
        assert!(first, "{lines:#?}");
        assert_eq!(trimmed[1], "> /tmp/scratchpad/permtest/data.csv");
        let lead = |l: &str| l.len() - l.trim_start().len();
        assert_eq!(
            lead(&lines[1]),
            lead(&lines[0]) + 4,
            "hung under it: {lines:#?}"
        );
        let short = "sort data.csv > sorted.csv 2>&1";
        assert_eq!(super::pretty_command(short), [format!("  {short}")]);
    }

    /// **The width and the flags a line holds are the user's**
    /// (`tui.command_width`, `tui.flags_per_line`; Steven, 10-05: "How are
    /// you determining how many flags to use before creating a new line? Can
    /// we make that configurable?").
    #[test]
    fn a_long_commands_breaks_are_configured() {
        use super::{Breaks, plain};
        use shannon_core::config::TuiConfig;
        let laid = |width: u16, flags: u16, input: &str| -> Vec<String> {
            let tui = TuiConfig {
                command_width: width,
                flags_per_line: flags,
                ..TuiConfig::default()
            };
            super::layout(input, &[], &[], Breaks::of(&tui))
                .lines
                .iter()
                .map(|line| plain(line).trim().to_string())
                .collect()
        };
        let flags = "rg --count --ignore-case --type=rust --glob='!target/**' --regexp='TODO' .";
        assert_eq!(laid(72, 0, flags).len(), 2, "packed to 72");
        assert_eq!(
            laid(120, 0, flags),
            [flags],
            "wider: it fits, it stays whole"
        );
        assert_eq!(laid(0, 0, flags), [flags], "0 never breaks for length");
        assert_eq!(
            laid(72, 1, flags),
            [
                "rg --count",
                "--ignore-case",
                "--type=rust",
                "--glob='!target/**'",
                "--regexp='TODO' .",
            ],
            "one flag a line, each with its value and what follows it"
        );
        assert_eq!(
            laid(72, 2, flags),
            [
                "rg --count --ignore-case",
                "--type=rust --glob='!target/**'",
                "--regexp='TODO' ."
            ]
        );
        // The cap shapes a command already too long; a short one stays one.
        assert_eq!(laid(72, 1, "ls -l -a src"), ["ls -l -a src"]);
        // And the default is the config's own.
        assert_eq!(Breaks::default(), Breaks::of(&TuiConfig::default()));
    }

    /// **A continuation on one row is one blank, and nothing else moves** —
    /// each joined text the same command to the analyser as its source; a
    /// `\` in quotes is the quote's; what does not lex is kept as written
    /// (Steven, 10-31: "what is with all these return symbols?").
    #[test]
    fn a_continuation_on_one_row_is_one_blank() {
        for (source, joined) in [
            ("cd x \\\n  && find .", "cd x && find ."),
            ("find . \\\n\t-name a\necho b", "find . -name a\necho b"),
            ("a \\\n \\\n b", "a b"),
            ("echo 'a\\\nb'", "echo 'a\\\nb'"),
            ("echo x\necho y", "echo x\necho y"),
        ] {
            assert_eq!(super::continuations_joined(source), joined, "{source:?}");
            assert!(
                super::same_command(source, joined),
                "{source:?} → {joined:?}"
            );
        }
        let unclosed = "echo 'a \\\n b";
        assert_eq!(
            super::continuations_joined(unclosed),
            unclosed,
            "does not lex"
        );
    }

    /// **A title's one row marks only the breaks the shell sees** — the
    /// command `e` wrote back, continued, reads as one statement there.
    #[test]
    fn a_title_marks_no_continuation() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        let edited = "cd src \\\n  && ls -la \\\n     -R\necho done";
        let prompt = super::ToolPrompt::of("t", edited, &rs);
        assert_eq!(
            prompt.command_on_one_line(),
            "cd src && ls -la -R ⏎ echo done"
        );
    }

    /// **A heredoc on one row is said by its size** — its body is data the
    /// row cannot show, and spelt out it pushed what ran after it off the
    /// row (Steven, 11-02, image 45). Two bodies on a line are each said;
    /// a text the lexer refuses keeps every line, marked.
    #[test]
    fn a_heredoc_on_one_row_is_said_by_its_size() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        let script =
            "cat > t.py <<'EOF'\n#!/usr/bin/env python3\nprint(1)\nEOF\ncd /tmp && python3 t.py";
        let prompt = super::ToolPrompt::of("t", script, &rs);
        assert_eq!(
            prompt.command_on_one_line(),
            "cat > t.py <<'EOF' ⟨2 lines⟩ ⏎ cd /tmp && python3 t.py"
        );
        let (row, styles) = super::one_row(script);
        let at =
            |needle: &str| row.chars().count() - row[row.find(needle).unwrap()..].chars().count();
        assert_eq!(styles.len(), row.chars().count(), "a style per char");
        assert_eq!(styles[at("⟨2")], super::DATA, "the size, as data");
        assert_eq!(
            styles[at("python3")].ink,
            Some(Ink::FunctionCall),
            "what ran, as shell"
        );
        assert_eq!(
            super::one_row("paste <<'A' <<'B'\none\nA\ntwo\nthree\nB\necho x").0,
            "paste <<'A' <<'B' ⟨1 line⟩ ⟨2 lines⟩ ⏎ echo x"
        );
        assert_eq!(
            super::one_row("cat <<EOF\n$HOME\nEOF").0,
            "cat <<EOF ⏎ $HOME ⏎ EOF",
            "an expanding body does not lex: every line, as written"
        );
    }

    /// **The pane draws the command as shell** — each line inside one
    /// statement ends ` \`, quiet, so the lines read together are the
    /// command the gate judged, and copy out as it (Steven, 10-31: "doesn't
    /// show a valid command, though it looks like it").
    #[test]
    fn the_pane_draws_a_long_command_as_shell() {
        let source = "cd ~/src && find . -type f -name '*.md' -not -path '*/target/*' \
                      -not -path '*/.git/*' | wc -l";
        let lines = super::format_source(source, &[], &[], super::Breaks::default());
        assert!(lines.len() > 2, "premise: laid out over lines");
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.runs.iter().map(|r| r.text.as_str()).collect())
            .collect();
        let continued: Vec<&crate::paint::Run> = lines
            .iter()
            .filter_map(|l| l.runs.last().filter(|r| r.text == " \\"))
            .collect();
        assert_eq!(
            continued.len(),
            lines.len() - 1,
            "every break inside it: {text:?}"
        );
        let quiet = super::CellStyle::ink(Ink::TextQuiet);
        for run in continued {
            assert_eq!(run.look, super::Look::Role(quiet));
        }
        // And painted so: the decision pane's colour for it is the quiet one.
        let theme = crate::theme::Theme::terminal();
        assert_eq!(
            theme.style(crate::theme::Context::Decision, quiet).fg,
            Some(theme.fg(Ink::TextQuiet))
        );
        assert!(
            super::same_command(source, &text.join("\n")),
            "read together, the same command: {text:?}"
        );
    }

    /// **`e` opens the layout as shell**: a line a statement, or a ` \`
    /// continuation of one, at column 0 — and only where it reads back as
    /// the same command. The layout folds a newline before `do` (a reading
    /// for the eye), which as shell would make `do` an argument: that opens
    /// as written.
    #[test]
    fn the_editor_gets_the_layout_as_shell() {
        let edited = super::editable(MANY_FLAGS);
        assert_eq!(
            edited.lines().collect::<Vec<_>>(),
            [
                "rg --line-number --fixed-strings \\",
                "    -e 'let sealed = seal.unwrap_or_else(|| config.authored());' \\",
                "    -e 'let preference = graph.episode(&base).map_or(config, |e| e.config.clone());' \\",
                "    -e 'ep.config.authored()' crates/shannon-tui/src/app/session_ops.rs",
            ]
        );
        let words = |s: &str| -> Vec<super::Lexeme> {
            super::lex(s)
                .unwrap()
                .into_iter()
                .map(|sp| sp.node)
                .collect()
        };
        assert_eq!(words(&edited), words(MANY_FLAGS));

        let chain = "cd delete_this_dir && find . -type f -name \"*.txt\" | head -20 \
                     && rm -f $(find . -name \"*.tmp\") && echo \"Cleanup complete\"";
        let edited = super::editable(chain);
        assert!(edited.contains(" \\\n  && find"), "{edited}");
        assert_eq!(words(&edited), words(chain));

        let looped = "for f in *.rs; do rustfmt --check \"$f\"; done";
        assert_eq!(
            super::editable(looped),
            "for f in *.rs; do\n    rustfmt --check \"$f\";\ndone",
            "a body's lines, and nothing the shell would read differently"
        );

        // A newline before `do` is written as the `;` it is — laid out as a
        // space it made `do` a word of the list.
        assert_eq!(
            super::editable("for x in a b\ndo echo $x; done"),
            "for x in a b; do\n    echo $x;\ndone"
        );
        assert_eq!(super::editable("ls -la"), "ls -la");

        // A word `do` or `then` is a word (review of 53feeaa, Reviewers A
        // and C: these split one command in two).
        for line in [
            "grep -n 'then' src/lib.rs",
            "rg -n do src",
            "echo do; rm -rf --one-file-system --preserve-root=all /tmp/shannon-test-absent/a /tmp/b",
        ] {
            let edited = super::editable(line);
            assert!(
                super::same_command(line, &edited),
                "{line:?} opened as {edited:?}"
            );
            let stages = |text: &str| {
                shannon_core::shell::parse_line(text)
                    .map(|p| p.stages.len())
                    .ok()
            };
            assert_eq!(
                stages(&edited),
                stages(line),
                "{line:?} opened as {edited:?}"
            );
        }

        // And the check refuses what they would have opened: a word made a
        // reserved one, a reserved one made a word, a glued word split.
        for (line, layout) in [
            ("rg -n do src", "rg -n do\n    src"),
            ("grep -n 'then' f", "grep -n 'then'\n    f"),
            (
                "for x in a b\ndo echo $x; done",
                "for x in a b do\n    echo $x;\ndone",
            ),
            (
                "echo $(printf x)-suffix",
                "echo $(printf x) \\\n    -suffix",
            ),
        ] {
            assert!(!super::same_command(line, layout), "{line:?} vs {layout:?}");
        }
    }

    /// **A blank a backslash escapes ends a laid-out line as the word's** —
    /// `rm a\ ` names a file `a `, and a line cut after it kept the `\` and
    /// lost the blank, a `\` that then ate the newline: the case the
    /// round-trip proptest found, pinned (its give-back was mutated away
    /// green in four runs of four; review stop 12, T7).
    #[test]
    fn an_escaped_blank_at_a_line_end_stays_the_words() {
        // A statement's line, not continued: a continued one ends ` \`,
        // which reads the same either way (measured: `rm a\  -r`).
        let source = "rm a\\ \nx";
        let text = super::layout_as_shell(source, super::Breaks::default())
            .expect("premise: laid out on lines");
        assert!(
            text.lines().next().is_some_and(|l| l.ends_with("a\\ ")),
            "premise: the first line ends on the escaped blank: {text:?}"
        );
        assert!(
            super::same_command(source, &text),
            "{source:?} laid out as {text:?}"
        );
    }

    /// **A heredoc's body is shown as written**, on lines of its own — it is
    /// the text being fed, often a script being written to disk, and the
    /// reader deciding must see it.
    #[test]
    fn a_heredoc_body_is_shown_as_written() {
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            "cat > s.sh <<'EOF'\n  rm -rf build\nEOF\nchmod +x s.sh",
            &rs,
        );
        let lines: Vec<String> = view
            .text()
            .painted()
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(lines.iter().any(|l| l == "  rm -rf build"), "{lines:#?}");
        assert!(lines.iter().any(|l| l == "EOF"), "{lines:#?}");
    }

    /// **The warning asks the gate's question.** `find . $X` may hide a
    /// `-delete` — a warning; `find src/*.rs` can only become paths — no claim.
    /// The colour used to ask whether ANY operand still expanded, and painted
    /// the second a warning over a command the gate allows.
    #[test]
    fn a_warning_means_a_word_could_become_a_flag() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let bg = |cmd: &str| {
            let view = AnalyzeView::of(cmd, &rs);
            let find = view
                .text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content.contains("find"))
                .map(|s| s.style.bg);
            find.unwrap_or_else(|| panic!("{cmd}: `find` is rendered"))
        };
        assert_eq!(bg("find . $X"), Some(Color::Yellow));
        assert_eq!(bg("find src/*.rs -type f"), None);
    }

    /// "Always" has a rule only when the command's TEXT denotes one behaviour.
    /// A glob keeps the same text and matches whatever exists at run time, and
    /// an unanalyzable line is a standing grant for something we refused to
    /// read — both get allow-once instead, and the legend offers no `A`.
    /// (Reviewer E.)
    #[test]
    fn always_is_withheld_when_the_text_does_not_pin_the_behaviour() {
        use super::{Decision, ToolAction, ToolPrompt};
        let rs = RuleSet::defaults();
        let legend = |p: &ToolPrompt| -> String {
            p.legend()
                .map(crate::paint::PageLine::text)
                .unwrap_or_default()
        };
        let a = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        );
        let stable = ToolPrompt::of("t", "rm -r /tmp/scratch", &rs);
        assert_eq!(stable.proposal().as_deref(), Ok("rm -r"));
        assert!(legend(&stable).contains("A always"));
        assert_eq!(stable.resolve(&a), Some(ToolAction::AllowOnce));

        for shifty in ["rm -rf build/*", r#"rm -r "$TARGET""#, "echo $(whoami"] {
            let p = ToolPrompt::of("t", shifty, &rs);
            assert!(p.proposal().is_err(), "no standing rule for: {shifty}");
            assert!(!legend(&p).contains("A always"), "{shifty}: {}", legend(&p));
            // Allow-once is still available for anything not denied.
            if p.decision() != Decision::Deny {
                assert_eq!(p.resolve(&a), Some(ToolAction::AllowOnce), "{shifty}");
            }
        }
    }

    /// **`n/N cycle` is in the legend exactly while calls wait** (Steven,
    /// 10-20: "allow `n` for next, to cycle through the queue", "`N` cycle in
    /// reverse") — on an ask and on a deny alike, before `A`, whose rule is
    /// what a narrow pane cuts — and leaves with the last of them. The legend
    /// is re-derived from what the call is now, so the count reaching it is a
    /// change of legend; and an `e` struck through stays struck through.
    #[test]
    fn n_and_shift_n_are_listed_while_calls_wait() {
        use super::{Breaks, Decision, ToolPrompt};
        use ratatui::style::Modifier;
        let rs = RuleSet::defaults();
        let text = |p: &ToolPrompt| p.legend().expect("a legend").text();
        let keys = "n/N cycle";
        let mut ask = ToolPrompt::of("t", "touch /tmp/x", &rs);
        assert!(!text(&ask).contains(keys), "alone: {}", text(&ask));
        ask.set_behind(2);
        let legend = text(&ask);
        assert!(legend.contains(&format!("j/k scroll · {keys}")), "{legend}");
        let always = legend.find("A always").expect("premise: `A` offers a rule");
        assert!(legend.find(keys) < Some(always), "before `A`: {legend}");
        ask.set_behind(0);
        assert!(!text(&ask).contains(keys), "gone with the last");

        let mut denying = RuleSet::defaults();
        denying.extend_from_lines("deny touch").expect("parses");
        let mut deny = ToolPrompt::of("t", "touch /tmp/x", &denying);
        assert_eq!(deny.decision(), Decision::Deny, "premise: a deny");
        deny.set_behind(1);
        assert!(
            text(&deny).ends_with(&format!(" · {keys}")),
            "{}",
            text(&deny)
        );

        let call = super::CallShown {
            tool: "Bash".into(),
            runs: super::Runner::ClaudeCode,
            unreadable: false,
            unsandboxed: false,
            editable: false,
            change: None,
            lands: None,
            names: None,
        };
        let mut fixed = ToolPrompt::for_call("t", "ls /", call, &rs, Breaks::default());
        fixed.set_behind(1);
        let struck = (fixed.legend().unwrap().runs.iter())
            .find(|r| r.text == "e edit")
            .is_some_and(|r| {
                matches!(r.look, crate::paint::Look::Own(s)
                    if s.add_modifier.contains(Modifier::CROSSED_OUT))
            });
        assert!(
            struck,
            "`e` still struck through once the count changed the legend"
        );
    }

    /// **A call's keys across its verdicts** (review stop 8, C 8 / D 4): `d`,
    /// `q` and `e` answer every call; `a` and `A` only one the rules do not
    /// deny, and no other plain letter answers anything. A deny offers no
    /// allow — in its legend, its refusal or its short form, which offered
    /// `a allow`, a dead key on the row. `A` with no rule answers (to say
    /// why) and is not offered; with one, it is.
    #[test]
    fn a_calls_keys_answer_what_its_verdict_allows_and_nothing_else() {
        use super::{Breaks, Decision, ToolAction, ToolPrompt};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let rs = RuleSet::defaults();
        let mut denying = RuleSet::defaults();
        denying.extend_from_lines("deny touch").expect("parses");
        let fixed = super::CallShown {
            tool: "Bash".into(),
            runs: super::Runner::ClaudeCode,
            unreadable: false,
            unsandboxed: false,
            editable: false,
            change: None,
            lands: None,
            names: None,
        };
        let ruleless = ToolPrompt::of("t", "touch \"$SHANNON_TEST_TARGET\"", &rs);
        assert!(ruleless.proposal().is_err(), "premise: no rule to offer");
        let cases = [
            (
                "an ask with a rule",
                ToolPrompt::of("t", "touch /tmp/x", &rs),
            ),
            ("an ask with none", ruleless),
            ("a deny", ToolPrompt::of("t", "touch /tmp/x", &denying)),
            (
                "a deny with no edit",
                ToolPrompt::for_call("t", "touch /tmp/x", fixed, &denying, Breaks::default()),
            ),
        ];
        for (what, mut prompt) in cases {
            for behind in [0, 2] {
                prompt.set_behind(behind);
                let allows = prompt.decision() != Decision::Deny;
                for c in ('a'..='z').chain('A'..='Z') {
                    let mods = match c.is_ascii_uppercase() {
                        true => KeyModifiers::SHIFT,
                        false => KeyModifiers::NONE,
                    };
                    let answer = prompt.resolve(&KeyEvent::new(KeyCode::Char(c), mods));
                    let expected = match c {
                        'd' | 'q' => Some(ToolAction::Deny),
                        'e' => Some(ToolAction::Edit),
                        'a' if allows => Some(ToolAction::AllowOnce),
                        'A' if allows => Some(ToolAction::WriteRule),
                        _ => None,
                    };
                    assert_eq!(answer, expected, "{what}, {behind} behind: `{c}`");
                }
                let legend = prompt.legend().expect("a legend").text();
                let refusal = prompt.not_a_key();
                let short = prompt.keys_in_short();
                assert_eq!(
                    refusal.strip_prefix("not a decision key — "),
                    legend.strip_prefix("  answer: "),
                    "{what}: the refusal lists what the legend does"
                );
                for said in [&legend, &refusal, &short] {
                    assert_eq!(said.contains("allow"), allows, "{what}: {said}");
                }
                assert_eq!(
                    legend.contains("A always"),
                    allows && prompt.proposal().is_ok(),
                    "{what}: `A` offered only with a rule — {legend}"
                );
                assert_eq!(legend.contains("n/N cycle"), behind > 0, "{what}: {legend}");
                let editable = prompt.call().is_none_or(|call| call.editable);
                assert_eq!(short.contains("e edit"), editable, "{what}: {short}");
            }
        }
    }

    /// **The legend follows the call through an edit and a policy change** —
    /// re-derived from the verdict each time (`ToolPrompt::relegend`), so a
    /// call that a re-gate or a refresh made a deny no longer offers `a`:
    /// the allow keys are inert there, and a legend listing them is a dead
    /// key on the one pane that must not have one.
    #[test]
    fn the_legend_follows_a_re_gate_and_a_refresh() {
        use super::{Decision, ToolPrompt};
        let rs = RuleSet::defaults();
        let mut denying = RuleSet::defaults();
        denying.extend_from_lines("deny touch").expect("parses");
        let offers_allow =
            |p: &ToolPrompt| p.legend().expect("a legend").text().contains("a allow");

        let mut edited = ToolPrompt::of("t", "rm -r /tmp/scratch", &rs);
        assert!(offers_allow(&edited), "premise: an ask offers `a`");
        edited.re_gate("touch /tmp/x", &denying);
        assert_eq!(edited.decision(), Decision::Deny, "premise: now a deny");
        assert!(!offers_allow(&edited), "the edit's legend, not the ask's");

        let mut refreshed = ToolPrompt::of("t", "touch /tmp/x", &rs);
        assert!(offers_allow(&refreshed), "premise: an ask offers `a`");
        refreshed.refresh(&denying);
        assert_eq!(refreshed.decision(), Decision::Deny, "premise: now a deny");
        assert!(!offers_allow(&refreshed), "the new policy's legend");
    }

    /// Colour follows the DATA FLOW, not the syntax: the yellow lands on the
    /// producing COMMAND (`find …`), while the `<(` / `)` plumbing that captures it
    /// stays plain. (Steven's report — don't highlight the process-sub operator.)
    #[test]
    fn substitution_delimiters_are_not_filled() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)"#,
            &rs,
        );
        let spans: Vec<_> = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .collect();
        let body = spans
            .iter()
            .find(|s| s.content.contains("find . -type f"))
            .expect("the inner command is rendered");
        assert_eq!(body.style.bg, Some(Color::Yellow), "the producer is yellow");
        let opener = spans
            .iter()
            .find(|s| s.content.contains("<("))
            .expect("the `<(` opener is rendered");
        assert_eq!(opener.style.bg, None, "`<(` is plumbing, not filled");
        let closer = spans
            .iter()
            .find(|s| s.content == ")")
            .expect("the `)` closer is rendered");
        assert_eq!(closer.style.bg, None, "`)` is plumbing, not filled");
    }

    /// The carrier at its BINDING site holds the untrusted value, so it joins the
    /// warning chain — yellow, not the plain/green of the benign `read` that merely
    /// introduces it. (Steven — the carrier is the pivot from producer to sink.)
    #[test]
    fn carrier_binding_site_is_yellow() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)"#,
            &rs,
        );
        let foo = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "FOO")
            .expect("the carrier at its binding site");
        assert_eq!(
            foo.style.bg,
            Some(Color::Yellow),
            "the carrier is warned, not painted safe"
        );
    }

    /// The carrier variable is underlined at BOTH its binding site and its use
    /// site, in the same colour — so the eye traces `find -> FOO -> $FOO`.
    #[test]
    fn carrier_variable_is_underlined_at_binding_and_use() {
        use ratatui::style::Modifier;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)"#,
            &rs,
        );
        let underlined: Vec<_> = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .collect();
        // `FOO` (binding) and `"$FOO"` (use) are both underlined, same colour.
        let binding = underlined.iter().find(|s| s.content.as_ref() == "FOO");
        let use_site = underlined.iter().find(|s| s.content.contains("$FOO"));
        assert!(
            binding.is_some(),
            "binding site underlined:\n{underlined:#?}"
        );
        assert!(use_site.is_some(), "use site underlined");
        assert_eq!(
            binding.unwrap().style.underline_color,
            use_site.unwrap().style.underline_color,
            "same carrier colour links binding and use"
        );
    }

    /// The polarity-correct link note: relation-first, so it doesn't read as a
    /// mis-classified `find`.
    #[test]
    fn flow_link_note_is_polarity_correct() {
        let rs = RuleSet::defaults();
        let out = flatten(&AnalyzeView::of(
            r#"while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)"#,
            &rs,
        ));
        assert!(out.contains("feeds $FOO"), "names the carrier:\n{out}");
        assert!(
            out.contains("read-only, but its output selects what `rm` acts on"),
            "relation-first note:\n{out}"
        );
    }

    /// The self-sanitising negative carries through to the UI: a substitution
    /// feeding a HARMLESS command yields no provenance flow — no link note and no
    /// underlined carrier. (The commands may still be yellow for being *unmodeled*
    /// — that's the effect axis, orthogonal to provenance; what matters here is
    /// that no source→sink link was invented.)
    #[test]
    fn harmless_substitution_gets_no_flow_note() {
        use ratatui::style::Modifier;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("diff <(sort a) <(sort b)", &rs);
        assert!(
            !flatten(&view).contains("its output selects"),
            "no link note invented"
        );
        assert!(
            !view
                .text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.style.add_modifier.contains(Modifier::UNDERLINED)),
            "no carrier underlined — there is no traced flow"
        );
    }

    /// **The pane heads a line as the gate decides it** — a handle only the
    /// raw line holds (a comment) asks at the gate, and the report read the
    /// pipeline alone: it headed the line ALLOW, "runs without asking", over
    /// a call that was asking (review stop 12, T8 M8). One verdict,
    /// `RuleSet::decide_parsed`.
    #[test]
    fn the_pane_heads_a_raw_line_handle_as_the_gate_does() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        let line = "ls # {{tok.1a2b}}";
        assert_eq!(
            rs.decide_line(line).decision,
            shannon_core::permission::Decision::Ask,
            "premise"
        );
        let rows: Vec<String> = (AnalyzeView::of(line, &rs).text().painted().iter())
            .map(line_text)
            .collect();
        let head = rows.iter().find(|r| r.contains("ASK")).cloned();
        assert!(
            head.is_some_and(|h| h.contains("you'll be prompted first")),
            "{rows:#?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("runs without asking")),
            "{rows:#?}"
        );
    }

    /// Control flow is pretty-printed: `do` opens an indented body that `done`
    /// closes on its own line — so the command reads as written and each part is
    /// visible (not clipped off one long line).
    #[test]
    fn control_flow_is_formatted_onto_indented_lines() {
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(r#"for i in {1..9}; do rm "$i"; done"#, &rs);
        let rows: Vec<String> = view.text().painted().iter().map(line_text).collect();
        assert!(
            rows.iter().any(|r| r.trim_end() == "  for i in {1..9}; do"),
            "head + `do` on one line:\n{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.starts_with("      ") && r.contains(r#"rm "$i""#)),
            "body is indented under the loop:\n{rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.trim() == "done"),
            "`done` dedents to its own line:\n{rows:#?}"
        );
    }

    /// Colour communicates DANGER (effect), not the permission decision: green =
    /// read-only/safe (fg only), yellow = mutating/unknown (fill), red =
    /// destructive/executor "look here" (fill + bold). So a destructive `rm` is
    /// red even though its *decision* is only "ask".
    #[test]
    fn colour_communicates_danger_not_decision() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("rm $FOO; echo hi; find . -name x; frobnicate y", &rs);
        let style_of = |needle: &str| {
            view.text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content.as_ref() == needle)
                .unwrap_or_else(|| panic!("no span {needle:?}"))
                .style
        };
        // destructive → red BACKGROUND (the "look here"), not the ask-yellow of
        // its decision.
        assert_eq!(
            style_of("rm").bg,
            Some(Color::Red),
            "rm is red (destructive)"
        );
        // read-only → green FOREGROUND, no fill.
        assert_eq!(
            style_of("echo").fg,
            Some(Color::Green),
            "echo is green (safe)"
        );
        assert_eq!(style_of("echo").bg, None, "green is fg-only, not a fill");
        assert_eq!(
            style_of("find").fg,
            Some(Color::Green),
            "find is green (safe)"
        );
        // unmodeled → unknown → yellow fill (warning), NOT red.
        assert_eq!(
            style_of("frobnicate").bg,
            Some(Color::Yellow),
            "unknown is a yellow warning, not red"
        );
    }

    /// Danger hidden in a substitution must surface. In `echo $(rm -rf /)` the
    /// `$(rm -rf /)` token sits inside BOTH the read-only `echo` stage (which
    /// encloses it) and the destructive `rm` stage — the colour must be the worst
    /// (red), never the enclosing green. This is the exact case the view exists to
    /// expose, so painting it "safe" would be the worst possible failure.
    #[test]
    fn danger_hidden_in_a_substitution_is_not_painted_safe() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("echo $(rm -rf /)", &rs);
        let subst = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("rm -rf"))
            .expect("the substitution token is rendered");
        assert_eq!(
            subst.style.bg,
            Some(Color::Red),
            "hidden destructive substitution is red, not the enclosing green"
        );
    }

    /// The converse: a substitution takes ITS OWN tone, not the tone of the
    /// command it is spliced into. `rm -f $(find …)` painted the harmless `find`
    /// red; it is a producer whose output picks what `rm` deletes, so it is the
    /// flow's yellow, with a note saying so — and `rm` itself stays red.
    #[test]
    fn a_substitution_is_not_painted_with_the_command_around_it() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("rm -f $(find . -name '*.o')", &rs);
        let bg = |needle: &str| {
            view.text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is rendered"))
                .style
                .bg
        };
        assert_eq!(bg("find"), Some(Color::Yellow), "the producer, implicated");
        assert_eq!(bg("rm"), Some(Color::Red), "the sink keeps its own danger");
        let out = flatten(&view);
        assert!(out.contains("feeds rm's arguments"), "{out}");
        assert!(
            out.contains("read-only, but its output selects what `rm` acts on"),
            "{out}"
        );

        // With no flow to implicate it, the inner command is just itself: a
        // redirect target's `echo` is green, not the `rm` around it.
        let view = AnalyzeView::of("rm -f x > $(echo out)", &rs);
        let echo = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("echo out"))
            .expect("the substitution is rendered")
            .style;
        assert_eq!(echo.bg, None, "no warning fill borrowed from `rm`");
        assert_eq!(echo.fg, Some(Color::Green), "the inner command's own tone");
    }

    /// A word the lexer hands over in pieces is shown as the one word it is: a
    /// space in `a $(echo b) c` would read as three arguments, a different
    /// command from the one that runs.
    #[test]
    fn a_word_glued_to_a_substitution_renders_as_one() {
        let rs = RuleSet::defaults();
        for cmd in [
            "rm -f a$(echo b)c",
            "X=$(find .) rm -f y",
            "echo x >/tmp/a$(echo b)",
        ] {
            let out = flatten(&AnalyzeView::of(cmd, &rs));
            assert!(out.contains(cmd), "{cmd} rendered as:\n{out}");
        }
    }

    /// The note says "read-only" only when it is: a producer that deletes too is
    /// red above, and the note must not call it read-only below.
    #[test]
    fn the_flow_note_does_not_call_a_writing_producer_read_only() {
        let rs = RuleSet::defaults();
        let out = flatten(&AnalyzeView::of("X=$(rm -v a); rm -r $X", &rs));
        assert!(
            out.contains("its output selects what `rm` acts on"),
            "{out}"
        );
        assert!(!out.contains("read-only, but"), "{out}");
        // The sink is named by its program, not by the line's first word.
        let out = flatten(&AnalyzeView::of("LC_ALL=C rm -f $(find .)", &rs));
        assert!(out.contains("what `rm` acts on"), "{out}");
    }

    /// The reported bug: a multi-stage command highlighted several parts, but on
    /// one long line they clipped off-screen. Formatting puts each stage on its
    /// own line, so the fills land on multiple visible rows.
    #[test]
    fn multiple_stages_highlight_across_formatted_lines() {
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of(
            r#"while IFS= read -r P; do brew cat "$P" | grep x; done < <(brew list)"#,
            &rs,
        );
        let filled_rows = view
            .text()
            .painted()
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.style.bg.is_some()))
            .count();
        assert!(
            filled_rows >= 3,
            "highlights spread over multiple formatted rows, not one clipped line \
             (got {filled_rows})"
        );
    }

    #[test]
    fn denied_command_reads_clearly() {
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("rm --no-preserve-root -rf /tmp/x", &rs);
        let out = flatten(&view);
        // The verdict, the command AS WRITTEN, the effect, and the naming of the
        // rule — everything the approval prompt needs. (The one shipped deny;
        // `find`'s carvings ask since 2026-10-02.)
        assert!(out.contains("DENY"), "shows the line verdict");
        assert!(
            out.contains("rm --no-preserve-root -rf /tmp/x"),
            "command shown as written"
        );
        assert!(out.contains("destructive"), "effect class shown");
        assert!(out.contains("deny rm --no-preserve-root"), "names the rule");
    }

    #[test]
    fn unanalyzable_command_explains_itself() {
        let rs = RuleSet::defaults();
        // An unbalanced substitution is still refused, and the report says why.
        let view = AnalyzeView::of("echo $(whoami", &rs);
        let out = flatten(&view);
        assert!(out.contains("ASK"));
        assert!(out.contains("not analyzed"));
    }

    #[test]
    fn denied_stage_is_filled_in_situ() {
        use ratatui::style::Color;
        let rs = RuleSet::defaults();
        let view = AnalyzeView::of("rm --no-preserve-root -rf /tmp/x", &rs);
        // What decided it is highlighted where it sits: the flag the rule keys
        // on, and the program that deletes, carry a RED BACKGROUND — and only
        // they (Steven, 10-11: "highlight less"): `-rf` is neither.
        let spans: Vec<_> = view
            .text()
            .painted()
            .iter()
            .flat_map(|l| l.spans.iter())
            .cloned()
            .collect();
        let bg = |word: &str| {
            spans
                .iter()
                .find(|s| s.content == word)
                .unwrap_or_else(|| panic!("`{word}` is rendered"))
                .style
                .bg
        };
        assert_eq!(
            bg("--no-preserve-root"),
            Some(Color::Red),
            "the rule's flag"
        );
        assert_eq!(bg("rm"), Some(Color::Red), "the program that deletes");
        assert_ne!(bg("-rf"), Some(Color::Red), "a flag no rule named");
    }

    /// The brew-loop lesson: several stages that share a reason collapse into a
    /// single grouped finding listing the commands, not one note per command.
    #[test]
    fn stages_sharing_a_reason_group_into_one_finding() {
        let rs = RuleSet::defaults();
        // Two unmodeled programs piped together — same (decision, effect, reason).
        let view = AnalyzeView::of("frobnicate | quuxify", &rs);
        let out = flatten(&view);
        // One finding lists both commands together…
        assert!(
            out.contains("frobnicate, quuxify"),
            "both commands share one finding line:\n{out}"
        );
        // …and there is exactly one hanging reason (╰─), not one per command.
        assert_eq!(
            out.matches("╰─").count(),
            1,
            "the shared reason appears once, grouped:\n{out}"
        );
    }

    /// **A place is explained by its own account first** (Steven, 10-13:
    /// "`/` holds Shannon's own state" was the reason given for `rm -r /` —
    /// "shouldn't this be about how `/` is outside the directory?"). What it
    /// holds below follows, citing a guard before a rule that keeps no kind
    /// of its own; and only a read is said to send what it reads.
    #[test]
    fn a_place_is_explained_by_its_own_account_first() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        let said = flatten(&AnalyzeView::of("rm -r /", &rs));
        assert!(
            said.contains(
                "`/` is outside where Shannon started — asking; it also holds paths that \
                 commonly hold credentials and places a rule guards, which this may reach \
                 (ask :* \\c~/.ssh and "
            ),
            "{said}"
        );
        assert!(
            !said.contains("sent to the model"),
            "a delete sends nothing:\n{said}"
        );
        let said = flatten(&AnalyzeView::of("rg foo ~", &rs));
        assert!(
            said.contains(
                "`~` is outside where Shannon started — asking; it also holds paths that \
                 commonly hold credentials, which this may reach (ask :* \\c~/.ssh and "
            ),
            "{said}"
        );
        assert!(
            said.contains("— and command output is sent to the model"),
            "{said}"
        );
    }

    /// **The reason names what decided, and nothing beside it** (review of
    /// c9c40fe and e10b6d5, A, B and C). The way out of a deny is the deny,
    /// whichever order the rules were written in — not an ask met from above
    /// beside it; a user's own allow is no guarded place `~` holds; what a
    /// guarded place's output costs is said once; and the place a guard
    /// lies below is marked, as one a rule decides is.
    #[test]
    fn the_reason_names_what_decided_it() {
        use ratatui::style::Color;
        let with = |rules: &str| {
            let mut rs = RuleSet::defaults().with_start("/home/me/proj");
            rs.extend_from_lines(rules).expect("the rules parse");
            rs
        };
        let prompt = |rs: &RuleSet, line: &str| {
            super::ToolPrompt::of("t", line, rs)
                .text()
                .painted()
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        for rules in [
            "ask :* /srv/keep\ndeny :delete /srv",
            "deny :delete /srv\nask :* /srv/keep",
        ] {
            let said = prompt(&with(rules), "rm -r /srv");
            assert!(
                said.contains("to lift it: `:undeny :delete /srv` takes the rule out"),
                "{rules:?}:\n{said}"
            );
            assert!(!said.contains(":unask"), "{rules:?}:\n{said}");
        }

        let said = flatten(&AnalyzeView::of("rg foo ~", &with("allow :read ~")));
        assert!(
            said.contains("`~` holds paths that commonly hold credentials, which this may reach"),
            "{said}"
        );
        assert!(
            !said.contains("places a rule guards"),
            "your allow:\n{said}"
        );

        let said = flatten(&AnalyzeView::of(
            "rg foo ~/.ssh",
            &with("ask :* ~/.ssh/config"),
        ));
        assert!(said.contains("; it also holds"), "{said}");
        assert_eq!(said.matches("sent to the model").count(), 1, "{said}");

        let mut denied = RuleSet::defaults()
            .with_start("/home/me/proj")
            .with_secrets(shannon_core::SecretsConfig {
                path_policy: shannon_core::PathPolicy::Deny,
                ..Default::default()
            });
        denied
            .extend_from_lines("deny :* ~")
            .expect("the rule parses");
        let said = flatten(&AnalyzeView::of("rg foo ~", &denied));
        assert!(said.contains("denied by rule: deny :* ~"), "{said}");

        let rs = with("ask :* keys");
        let bg = |line: &str, word: &str| {
            AnalyzeView::of(line, &rs)
                .text()
                .painted()
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content == word)
                .unwrap_or_else(|| panic!("`{word}` is rendered in `{line}`"))
                .style
                .bg
        };
        assert_eq!(bg("rm -r .", "."), Some(Color::Red), "a guard lies below");
        assert_eq!(bg("rm -r src", "src"), None, "nothing guarded below");
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;
    use shannon_core::permission::RuleSet;
    use shannon_core::shell::{Effect, parse_line};

    fn arb_command() -> impl Strategy<Value = String> {
        let atom = prop_oneof![
            Just("rm".to_string()),
            Just("-rf".to_string()),
            Just("sudo".to_string()),
            Just("eval".to_string()),
            Just("find".to_string()),
            Just("echo".to_string()),
            Just("git".to_string()),
            Just("status".to_string()),
            Just("ls".to_string()),
            Just("-delete".to_string()),
            Just("IFS=".to_string()),
            Just("PATH=/x".to_string()),
            Just("$FOO".to_string()),
            Just("\"$FOO\"".to_string()),
            Just("$(rm -rf /)".to_string()),
            Just("<(find .)".to_string()),
            Just("`".to_string()),
            Just("\"".to_string()),
            Just("\\".to_string()),
            Just("|".to_string()),
            Just("&&".to_string()),
            Just(";".to_string()),
            Just(">".to_string()),
            Just("while".to_string()),
            Just("do".to_string()),
            Just("done".to_string()),
            Just("é€🙂".to_string()),
            Just("\u{202e}".to_string()),
            Just("\u{200b}".to_string()),
            "[a-zA-Z0-9_./ -]{0,10}",
        ];
        prop::collection::vec(atom, 0..20).prop_map(|v| v.join(" "))
    }

    proptest! {
        /// Rendering slices UNTRUSTED text by byte ranges. It must never panic —
        /// the report is the safety surface, and a panic while drawing it is the
        /// gate dying instead of warning.
        #[test]
        fn rendering_never_panics(cmd in arb_command()) {
            let rs = RuleSet::defaults();
            let view = AnalyzeView::of(&cmd, &rs);
            prop_assert!(!view.text().lines().is_empty());
            let prompt = ToolPrompt::of("t",&cmd, &rs);
            prop_assert!(prompt.content_rows(80) >= 1);
            // Narrow widths are where the wrap arithmetic bends; 1 is the
            // degenerate case `rows_at` guards with `max(1)`.
            prop_assert!(prompt.content_rows(1) >= 1);
            let _ = pretty_command(&cmd);
            let _ = editable_command(&cmd, Breaks::default());
        }

        /// **The layout, as shell, is the same command** — unguarded: a
        /// one-line command's flags, pipes, chains and bodies broken where
        /// the pane breaks them, read back by the analyser (`same_command`).
        /// The guard in `editable_command` keeps a wrong layout out of
        /// `$EDITOR`; this says the layout is not wrong. It used to assert
        /// the guard's own answer, and could not fail (review of 53feeaa,
        /// Reviewers A and C).
        #[test]
        fn a_one_line_layout_reads_back_as_the_same_command(cmd in arb_command()) {
            let source = cmd.trim();
            let Ok(lexemes) = lex(source) else {
                return Ok(());
            };
            // Shell bash would refuse — `a | do b`, `while done`, `do | b`,
            // `do` first — which the analyser reads loosely; how it is laid
            // out does not matter. In bash a `do`, `then`, `done` or `fi`
            // follows a `;` or a newline, and a `do` or `then` is followed by
            // a command.
            let refused = reserved(source, &lexemes).iter().enumerate().any(|(i, kw)| {
                let after_separator = i > 0
                    && matches!(
                        lexemes[i - 1].node,
                        Lexeme::Op(Operator::Seq | Operator::Newline)
                    );
                let before_a_command =
                    matches!(lexemes.get(i + 1).map(|n| &n.node), Some(Lexeme::Word(_)));
                match kw {
                    Some(Kw::Body) => !after_separator || !before_a_command,
                    Some(Kw::Close) => !after_separator,
                    _ => false,
                }
            });
            if refused || parse_line(source).is_err() {
                return Ok(());
            }
            if let Some(text) = layout_as_shell(source, Breaks::default()) {
                prop_assert!(same_command(source, &text), "{source:?} laid out as {text:?}");
            }
        }

        /// **The security invariant, as a property.** A stage that is destructive
        /// or an executor is never given the "safe" colour — not by an
        /// unresolved operand, not by disturbed bindings, not by anything.
        /// Uncertainty may add alarm; it may never remove it.
        ///
        /// Asserted on the tone RULE rather than on rendered text: matching
        /// span contents is too loose to be trusted (`find do | sudo` made an
        /// earlier version match the green `do` inside `sudo`). The tone is
        /// read from the acts; the flat effect table is the second reader
        /// here, an oracle the acts must never be calmer than.
        #[test]
        fn a_dangerous_stage_is_never_painted_safe(cmd in arb_command()) {
            let Ok(pipeline) = parse_line(&cmd) else { return Ok(()); };
            let verdict = RuleSet::defaults().decide_parsed(&cmd, &pipeline);
            let dangerous = |e| matches!(e, Effect::Destructive | Effect::Executor);
            for (stage, sv) in pipeline.stages.iter().zip(verdict.stages()) {
                if dangerous(stage.effect()) || dangerous(stage.command.effect) {
                    prop_assert_ne!(
                        stage_tone(stage, sv), Some(Tone::Safe),
                        "a dangerous stage would render green in {:?}", cmd
                    );
                }
            }
        }

        /// The counterpart: a stage we are sure about is never left ambiguous.
        /// What the gate runs unasked as a modelled read, its words all
        /// resolved, is green — the colour never doubts what the decision
        /// knows (`git status` painted amber under an ALLOW, Steven 10-11).
        /// (The flat table cannot be the oracle this way round: the acts
        /// know more danger than it does — `IFS=` is a hazard to them.)
        #[test]
        fn a_fully_known_safe_stage_is_green(cmd in arb_command()) {
            let Ok(pipeline) = parse_line(&cmd) else { return Ok(()); };
            let verdict = RuleSet::defaults()
                .with_start("/")
                .decide_parsed(&cmd, &pipeline);
            let read_unasked = |a: &ActVerdict| {
                a.decision == Decision::Allow
                    && matches!(
                        a.by,
                        By::Default(
                            NoRule::ModelledRead
                                | NoRule::Wrapper
                                | NoRule::Inside(Some(shannon_core::permission::model::Effect::Read))
                        )
                    )
            };
            for (stage, sv) in pipeline.stages.iter().zip(verdict.stages()) {
                if !stage.command.has_unresolved_operand()
                    && sv.acts.iter().any(|a| a.by == By::Default(NoRule::ModelledRead))
                    && sv.acts.iter().all(read_unasked)
                {
                    prop_assert_eq!(stage_tone(stage, sv), Some(Tone::Safe), "{:?}", cmd);
                }
            }
        }
    }
}

#[cfg(test)]
mod knob_tests {
    use super::*;
    use shannon_core::config::Config;

    /// **The setting a pane tells the user to change is one that exists** —
    /// spelled as shown, set to `allow`, it changes the config. A misspelt
    /// key parses to the default, which is what this catches (review 21,
    /// Reviewer C: the strings were checked by nothing).
    #[test]
    fn the_credential_knob_a_pane_names_is_a_real_setting() {
        let rs = RuleSet::defaults();
        let read = |runs| CallShown {
            tool: "Read".into(),
            runs,
            unreadable: false,
            unsandboxed: false,
            editable: false,
            change: None,
            lands: None,
            names: None,
        };
        let shannon = ToolPrompt::of("t", "cat x", &rs);
        let claude_code = ToolPrompt::for_call(
            "t",
            "cat x",
            read(Runner::ClaudeCode),
            &rs,
            Breaks::default(),
        );
        // Shannon's own Read passes `[secrets]`, as its `bash` does: its
        // output is Shannon's to redact.
        let own = ToolPrompt::for_call("t", "cat x", read(Runner::Shannon), &rs, Breaks::default());
        assert_eq!(own.credential_knob(), shannon.credential_knob());
        let default = format!("{:?}", Config::parse("").expect("empty parses"));
        for knob in [shannon.credential_knob(), claude_code.credential_knob()] {
            let (section, key) = knob
                .strip_prefix('[')
                .and_then(|k| k.split_once("] "))
                .unwrap_or_else(|| panic!("`{knob}` is not `[section] key`"));
            let set = Config::parse(&format!("[{section}]\n{key} = \"allow\"\n"))
                .unwrap_or_else(|e| panic!("`{knob}` does not parse: {e}"));
            assert_ne!(format!("{set:?}"), default, "`{knob}` changed nothing");
        }
        assert_ne!(shannon.credential_knob(), claude_code.credential_knob());
    }

    /// **A completion model's ask says why, in its own words** (review
    /// 2026-10-11, C: emptying the sentence left the suite green).
    #[test]
    fn a_completion_models_ask_says_why() {
        let act = ActVerdict {
            act: ActKind::Program {
                name: "bat".into(),
                chain: Vec::new(),
                effect: shannon_core::permission::model::Effect::Read,
                call: None,
                wrapper: false,
                unknown: Vec::new(),
            },
            decision: Decision::Ask,
            by: By::Default(NoRule::Completion),
            uncovered: Vec::new(),
            model: None,
            at: Default::default(),
            reaches: Vec::new(),
        };
        let said = explain_act(&act);
        assert!(
            said.contains("shell completions") && said.contains("asking"),
            "{said}"
        );
    }

    /// An asking verdict of `act`, by `by` — a sentence's input.
    fn asking(act: ActKind, by: By) -> ActVerdict {
        ActVerdict {
            act,
            decision: Decision::Ask,
            by,
            uncovered: Vec::new(),
            model: None,
            at: Default::default(),
            reaches: Vec::new(),
        }
    }

    /// **Each ask no rule lifts says so, and what it means** — one verdict
    /// per hard ask and per rule that must name its flag, each sentence held
    /// to its claim. None appeared in any test: explaining every handle as a
    /// legitimate spend, an unminted (possibly injected) one too, passed the
    /// crate (review stop 12, T7 4).
    #[test]
    fn every_ask_no_rule_lifts_says_what_it_means() {
        let program = || ActKind::Program {
            name: "sort".into(),
            chain: Vec::new(),
            effect: shannon_core::permission::model::Effect::Read,
            call: None,
            wrapper: false,
            unknown: Vec::new(),
        };
        let secret = |known| ActKind::Secret {
            handle: "{{gh.8d31}}".into(),
            known,
        };
        let cases: [(ActVerdict, &[&str], &[&str]); 8] = [
            (
                asking(secret(true), By::Hard(Hard::Secret)),
                &["spends {{gh.8d31}}", "IS sent"],
                &["injected"],
            ),
            (
                asking(secret(false), By::Hard(Hard::Secret)),
                &["does not hold", "injected instruction", "refuse to run"],
                &["spends"],
            ),
            (
                asking(ActKind::Privilege("sudo".into()), By::Hard(Hard::Privilege)),
                &["`sudo`", "another user", "any file"],
                &[],
            ),
            (
                asking(ActKind::Unplaced("$F".into()), By::Hard(Hard::Unplaced)),
                &["`$F` could not be placed", "never guesses"],
                &[],
            ),
            (
                asking(
                    ActKind::Unsound("the name is not what runs".into()),
                    By::Hard(Hard::Unsound),
                ),
                &["the name is not what runs", "whatever a rule allows"],
                &[],
            ),
            (
                asking(
                    ActKind::Code {
                        program: "awk".into(),
                        what: "code the model does not read".into(),
                    },
                    By::Hard(Hard::Code),
                ),
                &["`awk`: code", "whatever a rule allows"],
                &[],
            ),
            (
                asking(
                    program(),
                    By::Default(NoRule::Guarded(vec![
                        "--compress-program runs a program".into(),
                    ])),
                ),
                &[
                    "--compress-program runs a program",
                    "a rule naming the flag, or `*`",
                ],
                &["exactly"],
            ),
            (
                asking(
                    program(),
                    By::Default(NoRule::NeedsNamed(vec!["names a program".into()])),
                ),
                &["names a program", "only a rule naming it exactly"],
                &["`*`"],
            ),
        ];
        for (act, says, never) in &cases {
            let said = explain_act(act);
            for words in *says {
                assert!(said.contains(words), "{:?}: no `{words}` in {said}", act.by);
            }
            for words in *never {
                assert!(!said.contains(words), "{:?}: `{words}` in {said}", act.by);
            }
        }

        // The widening hint, from a real verdict: an allow that names its
        // form exactly says which `*` would cover a word it does not.
        let mut rs = RuleSet::defaults();
        rs.extend_from_lines("ask gh *\nallow gh pr view").unwrap();
        let said = still_asks("gh pr view 1 --no-such-flag", &rs);
        assert!(
            said.contains("`allow gh pr view *` covers any other word or flag"),
            "{said}"
        );
    }

    /// **The ask no rule lifts is the one named** — of `cat /etc/hosts
    /// $F`'s two asks, the unplaced `$F`, which no rule lifts, before the
    /// outside path, which an allow would: naming the first sent the user to
    /// write a rule that can never answer the call (review 2026-10-10, E2).
    /// Held by nothing: taking the first deciding act passed the crate
    /// (review stop 12, T7 2).
    #[test]
    fn the_ask_no_rule_lifts_is_the_one_named() {
        let line = "cat /etc/hosts $F";
        let rs = RuleSet::defaults();
        let pipeline = parse_line(line).expect("a line");
        let verdict = rs.decide_parsed(line, &pipeline);
        let first_ask = (verdict.stages()[0].acts.iter())
            .find(|a| a.decision == Decision::Ask)
            .expect("premise: it asks");
        assert!(
            !matches!(first_ask.by, By::Hard(_)),
            "premise: the first asking act is not the hard one: {:?}",
            first_ask.by
        );
        let said = still_asks(line, &rs);
        assert!(said.contains("`$F` could not be placed"), "{said}");
        assert!(!said.contains("outside"), "{said}");
    }
}

/// What a Claude Code write shows, apart from the screen.
#[cfg(test)]
mod change_tests {
    use super::*;
    use shannon_core::backend::claude_code::Change;

    /// What `change_lines` shows of a change, as text.
    fn text(path: &str, change: &Change) -> Vec<String> {
        change_lines(path, change)
            .iter()
            .map(PageLine::text)
            .collect()
    }

    /// **A Write's diff is painted as the pane paints** — its file's syntax
    /// off the verdict's hues: a type in the shipped code map is yellow,
    /// which beside a verdict reads as a warning (review of f50fda5,
    /// Reviewers A and C — `change_lines` painted as code passed the suite).
    /// Painted by the production painter for the decision pane, so the map
    /// that pane is given is what is checked.
    #[test]
    fn a_writes_diff_is_painted_off_the_verdicts_hues() {
        use crate::theme::{Context, Theme};
        let theme = Theme::terminal();
        let paint = crate::view::painter(crate::workspace::PaneKind::Tool, &theme);
        let lines: Vec<Line> = change_lines("a.rs", &Change::Content("struct Foo;\n".into()))
            .iter()
            .map(|line| paint.line(line))
            .collect();
        let span = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("Foo"))
            .expect("the type is drawn");
        let pane = theme.paint(Context::Decision, Ink::Type).fg;
        assert_eq!(span.style.fg, pane, "{lines:?}");
        assert_ne!(
            pane,
            theme.paint(Context::Code, Ink::Type).fg,
            "premise: the maps differ"
        );
        assert_ne!(pane, Some(ratatui::style::Color::Yellow));
    }

    /// **Bounded**: a ten-thousand-line Write is a pane, not a hang, and says
    /// how much it did not show.
    #[test]
    fn a_long_write_is_cut_and_says_so() {
        let content: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let shown = text("notes.txt", &Change::Content(content));
        let most = crate::tool_record::MOST_SHOWN;
        assert_eq!(shown.len(), 1 + most + 1, "heading, lines, the count");
        // The count from the cut, not typed (review stop 12, T7).
        let unshown = format!("  … {} more lines", 1000 - most);
        assert_eq!(shown.last(), Some(&unshown));
    }

    /// An empty Write says so rather than showing nothing under a heading;
    /// several edits say how many, in the order they apply.
    #[test]
    fn an_empty_write_and_several_edits_are_named() {
        let shown = text("a", &Change::Content(String::new()));
        assert_eq!(shown, ["  what it writes: an empty file"]);
        let edits = Change::Edits(vec![("a".into(), "b".into()), ("c".into(), "d".into())]);
        let shown = text("a", &edits);
        assert_eq!(
            shown,
            [
                "  the 2 edits, in order:",
                "  - a",
                "  + b",
                "  - c",
                "  + d"
            ]
        );
    }

    /// **`A` offers `:write` only where it would answer the call**, and never
    /// for a path a rule cannot spell. A credential path's guard is lifted by
    /// a grant naming the place (Q9), so the grant is offered there too.
    #[test]
    fn the_write_grant_is_offered_only_where_it_answers() {
        let rs = RuleSet::defaults();
        assert_eq!(
            write_grant("cat > '/tmp/x/a.rs'", "/tmp/x/a.rs", &rs),
            Some(":write /tmp/x/a.rs".into())
        );
        assert_eq!(write_grant("cat > '/tmp/a b'", "/tmp/a b", &rs), None);
        // A credential path's guard is lifted by a grant naming the place.
        assert_eq!(
            write_grant("cat > '~/.ssh/config'", "~/.ssh/config", &rs),
            Some(":write ~/.ssh/config".into())
        );
        // …and none where it would not answer: a rule asking of the program,
        // or of the place as specifically, outlasts the grant. Only the
        // spelling half was tested, so offering a grant without asking the
        // engine passed the crate (review stop 12, T7 1).
        for asking in ["ask cat", "ask :write /tmp/x/a.rs"] {
            let mut rs = RuleSet::defaults();
            rs.extend_from_lines(asking).expect("a rule");
            assert_eq!(
                write_grant("cat > '/tmp/x/a.rs'", "/tmp/x/a.rs", &rs),
                None,
                "under `{asking}`"
            );
        }
    }
}

/// The box must fit what it renders, not what it was handed.
#[cfg(test)]
mod sizing_tests {
    use super::*;

    /// **A long command cannot push the danger report out of its own box.**
    ///
    /// `content_rows` counted logical lines while the pane renders with
    /// `Wrap { trim: false }` — the same number only while nothing wraps.
    /// The MODEL supplies the command, so the model controls the wrap ratio,
    /// so it controlled how many rows of its own findings sat below the fold
    /// of a box that drew its border as though complete. A realistic
    /// 386-cell `grep` line needed 13 rendered rows in a 9-row body, hiding
    /// `[ask] rm · destructive` and the line explaining why (Reviewer E).
    ///
    /// The verdict headline survives at row 0 either way — and `ASK` is the
    /// verdict for a huge class of ordinary commands, so what falls off is
    /// precisely *which stage is dangerous*. An unreadable prompt is consent
    /// theatre, arriving through the sizing rather than the text.
    #[test]
    fn a_wrapping_command_still_sizes_its_own_box() {
        let rs = RuleSet::defaults();
        // Padding the model gets for free: a long pattern, then something
        // destructive after it.
        let padding = "x".repeat(360);
        let cmd = format!("grep -rn {padding} . ; rm -rf /tmp/shannon-test-absent");
        let prompt = ToolPrompt::of("t", &cmd, &rs);

        let width = 80u16;
        let rows = prompt.content_rows(width);

        // **The assertion that used to be here was `x <= x`.** `content_rows`
        // IS `rows_at + footer`, so `rows_at <= content_rows - footer` could
        // not fail for any input, and the box-sizing rule it claimed to check
        // was held by nothing (Reviewer C).
        //
        // What is real is that the box RESPONDS to the width: a narrower pane
        // wraps more and must be sized taller. That is the property the model
        // can attack — it supplies the command, so it controls the wrap ratio.
        assert!(
            prompt.content_rows(40) > rows,
            "a narrower box wraps more and must be sized taller: 40 -> {}, 80 -> {rows}",
            prompt.content_rows(40)
        );
        assert!(
            rows as usize > prompt.text().lines().len(),
            "premise: this command really does wrap, so logical lines and \
                 rendered rows disagree — otherwise the test proves nothing"
        );
        // **The residual, named rather than implied.** Both sides of every
        // assertion here are `rows_at`, so this cannot see a report whose
        // DRAWN height exceeds the box. Until S4 it could — rows counted by
        // cells, drawn by ratatui's word wrap, so a model-supplied command
        // pushed its own verdicts below a fold the border drew as complete.
        // One ruler closed it; `the_rows_counted_are_the_rows_drawn`
        // (`text_view`) compares the count with the drawing.
    }
}
