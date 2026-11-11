//! The `:` ex-command vocabulary.
//!
//! One table, three consumers: dispatch ([`App::execute_command`]), Tab
//! completion, and `:help` all read command names and aliases from here.
//! Declaring a command once — as a [`Command`] variant plus a [`Spec`] row —
//! means the compiler keeps everything honest: [`Command`] is `match`ed
//! exhaustively in dispatch, so you can't add a verb the parser forgets to
//! handle, and a completeness test keeps the table from silently dropping one.
//! That test walks `Command::next` (test-only, so not linkable from here), an
//! exhaustive `match` over the variants — it used to walk a
//! hand-typed `ALL` array, which is a list checking that it lists everything,
//! and deleting a verb from BOTH it and the table left the suite green with the
//! verb unreachable.
//!
//! Resolution is by **declared forms only**: the full name or an explicit
//! alias (`:q`/`:quit`, `:g`/`:graph`) — no prefix abbreviation in between
//! (a user ruling: `:gr`/`:gra`/`:grap` silently meaning `:graph` was surprise,
//! not convenience). See [`resolve`].
//!
//! [`App::execute_command`]: crate::app::App

/// A `:` command, independent of its arguments.
///
/// Matched exhaustively in `App::execute_command`, so adding a variant is a
/// compile error until dispatch handles it — the point of routing every verb
/// through one enum instead of scattered string arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `:quit` (`:q`) — close the focused pane; quit when it is the last one.
    Quit,
    /// `:qall` (`:qa`) — quit Shannon Code whatever is open.
    QuitAll,
    /// `:wq` (`:x`, `:xit`) — save this session, then close the pane.
    ///
    /// Vim's `:x` writes only when the buffer is modified and `:wq` writes
    /// unconditionally; here `:write` is already idempotent (it reports
    /// "already on disk" and does nothing), so the distinction has nothing to
    /// attach to and both spellings are one command. Steven asked for them by
    /// name after `:x` cancelled a squash he meant to keep (2026-08-25).
    WriteQuit,
    /// `:Edit` — open the transcript in `$EDITOR`, read-only.
    EditTranscript,
    /// `:config dump|edit` — view the default template, or edit your config.
    Config,
    /// `:source {file}` (`:so`) — apply a config file over the running one,
    /// additively, as vim's `:source` runs a script against the current state.
    /// A bare `:source` is refused: vim's bare form sources the current
    /// buffer, which we do not have, and `:reload` is the reload.
    Source,
    /// `:reload` (`:rel`) — source your config file again (vim's
    /// `:so $MYVIMRC`). Additive, like any `:source`.
    Reload,
    /// `:unsource {file}` (`:uns`) — take a sourced file back OUT: every layer
    /// it added leaves the stack, and its keys fall back to what the rest say.
    /// Vim has no such verb; the stack is what makes it possible.
    Unsource,
    /// `:scriptnames` (`:scr`) — what is loaded, in the order it applies (vim's
    /// list of sourced scripts).
    Scriptnames,
    /// `:set` (`:se`) — vim's option command over the config's dotted keys:
    /// `:set tui.prompt.height=15` applies at once, `x?` shows, `x&` resets.
    /// A layer on the stack, so a file sourced afterwards wins.
    Set,
    /// `:verbose` (`:verb`) — vim's modifier; here it prefixes `:set`, whose
    /// `x?` then says which file (or the command line) last set `x`.
    Verbose,
    /// `:themes` — every theme there is, beside a preview of the one under
    /// the cursor, drawn in it; `<CR>` wears it.
    Themes,
    /// `:graph` — show the conversation-graph viewer in the focused pane.
    Graph,
    /// `:split [what]` (`:sp`) — split the focused pane, stacked.
    Split,
    /// `:vsplit [what]` (`:vs`) — split the focused pane, side by side.
    VSplit,
    /// `:fork` — split the pane and clone its conversation into the new
    /// one: a side conversation the original never sees (Steven, 10-14:
    /// "the equivalent of `/btw`, but extendable").
    Fork,
    /// `:vfork` — `:fork`, side by side.
    VFork,
    /// `:model [name]` — show, or set, the model this session talks to.
    Model,
    /// `:models` — the model, effort and context budget, on one screen.
    Models,
    /// `:effort [level]` — set how hard this session's model works, or
    /// choose in a panel.
    Effort,
    /// `:memories` — the instruction files and memories this session is
    /// told, and where; `<Enter>` edits one.
    Memories,
    /// `:skills` — the skills found; `<Enter>` edits one.
    Skills,
    /// `:skill <name> [args]` — run a skill now, on the session in front.
    Skill,
    /// `:setup` — choose how Shannon reaches a model: the API, Claude Code,
    /// or the demo.
    Setup,
    /// `:trust [dir]` — say which directories are trusted (their `.shannon/`
    /// read, Claude Code run there), or trust one (`.` for here).
    Trust,
    /// `:untrust [dir]` — stop trusting a directory (here, with none).
    Untrust,
    /// `:import [--all] <id|path>` — bring a Claude Code conversation in as a
    /// session.
    Import,
    /// `:unread [all]` — take back Shannon's leave to read Claude Code's
    /// conversations: this directory's, or every one's.
    Unread,
    /// `:Send [1,3|all|ball|b2|<id>]` — send the prompt to every marked pane,
    /// to those numbers, or to sessions.
    Send,
    /// `:reply` — start a thread from the turn under the cursor.
    Reply,
    /// `:sgraph` (`:sg`) — open the graph in a stacked split.
    SGraph,
    /// `:vsgraph` (`:vsg`) — open the graph in a side-by-side split.
    VSGraph,
    /// `:close` (`:clo`) — close the focused pane.
    Close,
    /// `:hide` (`:hid`) — **stop showing this view here**, vim's verb.
    ///
    /// In vim `:hide` closes the window and leaves its buffer LOADED, as
    /// against `:bdelete`, which unloads it. We have no unload — sessions live
    /// in `buffers` for the life of the run, and viewer content (the manual, an
    /// analysis) is regenerable — so the second half of vim's distinction has
    /// nothing to attach to yet. This is the half that exists, spelled the way
    /// vim spells it, so the reflex works now and the meaning does not have to
    /// change when unloading lands.
    ///
    /// A viewer BORROWS a pane rather than owning a window, so closing the
    /// window would take a transcript with it — surprising when what you asked
    /// to hide was the manual. `close_pane_command` draws the line (a lent pane
    /// goes back to what it showed; a pane opened *for* the view closes), and
    /// `:q`, `:close` and `:hide` all reach it today: `:hide` is the
    /// narrower-intent spelling, kept for when unloading gives it a meaning of
    /// its own.
    Hide,
    /// `:only` (`:on`) — close every pane but the focused one.
    Only,
    /// `:new` — start a fresh session.
    New,
    /// `:snew` — `:split`, then `:new` in the new pane.
    SNew,
    /// `:vnew` (`:vne`) — `:vsplit`, then `:new` in the new pane.
    VNew,
    /// `:clone` — fork the current session into a new one sharing its history.
    Clone,
    /// `:merge [<hash>]` — splice a branch's divergent turns back into the
    /// session it forked from, composing a new session (both originals kept).
    ///
    /// This is `:graph`'s old `m` key. It moved to a colon command to give the
    /// `m` **prefix** back to netrw's mark vocabulary (`mf`, `mu`), which is
    /// the model `:graph` follows: a file browser, where `m` marks and the
    /// verbs act on what is marked.
    Merge,
    /// `:squash` — replace turns with a summary you write in `$EDITOR`. The
    /// marks pick the target and their *shape* picks the semantics; with none,
    /// the selected branch folds into its base.
    ///
    /// `:graph`'s old `s` key, moved alongside `:merge` so the composing verbs
    /// share one grammar instead of splitting across keys and commands — and
    /// so `d`'s refusal ("`:squash` to fold turns away") names something the
    /// user can actually type.
    Squash,
    /// `:distill` — `:squash`, with the model writing the summary: `:graph`'s
    /// `S` key, by name. Reviewed as `[merge] distill_review` says; `!` does
    /// the other for once, as `:compact!` does.
    Distill,
    /// `:compact [N]` — this session's history but its last N turns into one
    /// summary the model writes; `!` skips the review.
    Compact,
    /// `:pick` — compose the rows marked in `:graph` into a new session:
    /// its `p` key, by name.
    Pick,
    /// `:return [session]` — return a session's result into its parent, as
    /// one turn. It was `:graph`'s `r` key, which is now rename.
    Return,
    /// `:delete [all|<hash>]` — delete the current session, every session, or
    /// one by id.
    Delete,
    /// `:open <hash>` — jump to an episode or turn by id.
    Open,
    /// `:resume [session]` — bare, the sessions to choose from (`:graph`);
    /// with one, open it. Claude Code's `/resume`, for the fingers that
    /// know it (Steven, 10-14).
    Resume,
    /// `:rename [session] <name>` — name a session; `:rename` shows its
    /// name, `:rename!` takes it off. Display metadata only: the id, and
    /// everything addressed by it, is untouched (Steven, 10-14).
    Rename,
    /// `:description [text]` — say in a line what a session is; `:description`
    /// shows it, `:description!` takes it off. The inspector shows it.
    Description,
    /// `:child [-m <model>] [prompt]` — spawn a background child session
    /// (`$EDITOR` for the briefing when no prompt); `--stop <id>` cancels one.
    Child,
    /// `:continue` — resume the last turn where it stopped (truncated or
    /// interrupted), via the configured `[continue]` strategy.
    Continue,
    /// `:help` (`:h`) — show the command reference.
    Help,
    /// `:messages` (`:mes`) — the notice history, for the message the status
    /// row was too narrow to finish.
    Messages,
    /// `:write` (`:w`) — flush this session to the store now.
    Write,
    /// `:wall` (`:wa`) — flush every open session.
    WriteAll,
    /// `:analyze <command>` — preview how the permission engine would classify a
    /// shell command (its stages, effect, and allow/ask/deny verdict) without
    /// running it.
    Analyze,
    /// `:tool <command>` — route `<command>` through the permission gate exactly
    /// as an incoming model tool call would, so the decision surface can be
    /// exercised without spending a turn on the API.
    ///
    /// **It runs the command** when policy already allows it — the gate's
    /// auto-allow path is part of what is being exercised. `:analyze` is the
    /// non-executing preview; the one-word difference in the names is not a
    /// reliable signal, so the help text leads with this.
    Tool,
    /// `:secret <text>` — mark `<text>` in the prompt as a secret: it is lifted
    /// into the session's table and replaced by a handle, so it never leaves.
    Secret,
    /// `:nosecret <handle|value>` — take a secret back out: off this session's
    /// table and out of the vault. Steven named it (vim's `no…` prefix).
    NoSecret,
    /// `:alias` — list the session's substitution table: each handle beside
    /// the value it stands for, marking which are secret. The values are the
    /// user's to see — hidden from the agent, not from them.
    Secrets,
    /// `:keymap [context]` — the keys in force and the **action names** a
    /// `[keymap.*]` config table binds them to.
    ///
    /// The vocabulary of the app's largest extension point had no spelling
    /// inside the app: the config template pointed at this command before it
    /// existed, and `:help` named the contexts but not one action.
    Keymap,
    /// `:allow <rule>` — add an allow rule for this session (`:set`-like: the
    /// config file is untouched).
    Allow,
    /// `:deny <rule>` — add a deny rule for this session.
    Deny,
    /// `:ask <rule>` — add an ask rule for this session.
    Ask,
    /// `:unallow <rule>` — take an allow rule out for this session, whatever
    /// its source; the config file is untouched. vim's `:un…` pairing
    /// (`:map`/`:unmap`).
    Unallow,
    /// `:unask <rule>` — take an ask rule out for this session.
    Unask,
    /// `:undeny[!] <rule>` — take a deny rule out for this session. One this
    /// session did not add (shipped, your config) takes `!`, force: Steven,
    /// "a more dangerous event".
    Undeny,
    /// `:rules` — the policy in force, by where each rule came from.
    Rules,
    /// `:ls` (`:buffers`, `:files`) — every session this run has open, shown
    /// in a pane or not, with what needs you.
    Buffers,
    /// `:[N]b[uffer] N` — show session N (as `:ls` numbers it) in this pane.
    Buffer,
    /// `:decide [N]` — go to a decision waiting on you: session N's, else
    /// this session's, else the first; its session shown if no window shows
    /// it (`decision-panes-and-alerts.md` §3.5).
    Decide,
    /// `:aim [N|id|name]` — aim the prompt you write in at session N, shown
    /// or not; bare, say where it goes (Steven, 10-26: "a way to manually
    /// change the prompt's aim as a fallback").
    Aim,
    /// `:tabnew [N|id]` — a new tab after this one, on a fresh session or the
    /// one named: vim's `:tabnew`, with Shannon's `:new` (`tabs.md` §4).
    TabNew,
    /// `:tabclose [N]` (`:tabc`) — close this tab, showing the one to its
    /// right (vim's), or close tab N; refused on the only tab.
    TabClose,
    /// `:tabnext [N|+N|-N|$]` (`:tabn`) — the next tab, or the one named
    /// (vim's `gt`).
    TabNext,
    /// `:tabprevious [N]` (`:tabp`, `:tabN`) — the previous tab, or N back
    /// (vim's `gT`).
    TabPrevious,
    /// `:tabmove [N|+N|-N|$]` (`:tabm`) — move this tab: after tab N, N
    /// places over, or to the end (vim's).
    TabMove,
    /// `:tabonly [N]` (`:tabo`) — close every tab but this one, or but tab N
    /// (vim's).
    TabOnly,
    /// `:tabrename [name]` — name this tab; bare says it, `!` takes it off
    /// (`tabs.md` Q3).
    TabRename,
    /// `:tabs` — the tab pages and the windows in each, a live page (vim's
    /// `:tabs`, which prints them).
    Tabs,
    /// `:marks` — every mark, yours then the agent's, a live page (vim's
    /// `:marks`, which prints them; `navigation.md` §7).
    Marks,
    /// `:tab {cmd}` — run `cmd`, the first window it opens in a new tab
    /// (vim's `:tab`).
    Tab,
    /// `:[N]ball [N]` (`:ba`, `:sball`) — a window for each session; under
    /// `:tab`, a tab for each (vim's `:ball`, `:tab ball`).
    Ball,
}

impl Command {
    /// The rule verb this command adds (`false`) or takes out (`true`) —
    /// `:allow` and `:unallow` both name `allow` rules. One match, so the
    /// dispatch does not re-spell the verbs with a wildcard (Reviewer C).
    pub fn rule_verb(self) -> Option<(&'static str, bool)> {
        Some(match self {
            Command::Allow => ("allow", false),
            Command::Ask => ("ask", false),
            Command::Deny => ("deny", false),
            Command::Unallow => ("allow", true),
            Command::Unask => ("ask", true),
            Command::Undeny => ("deny", true),
            _ => return None,
        })
    }

    /// Takes vim's leading count — `:2b` is `:b 2`, and the tab commands'
    /// (vim's `:[count]tabnext`, `:[count]tabclose`, …: `:2tabnext` is
    /// `:tabnext 2`); every other command refuses one ([`Resolved::NoCount`])
    /// rather than ignore a number the user meant.
    pub fn takes_count(self) -> bool {
        matches!(
            self,
            Command::Buffer
                | Command::TabNew
                | Command::TabNext
                | Command::TabPrevious
                | Command::TabClose
                | Command::TabOnly
                | Command::TabMove
                | Command::Ball
        )
    }

    /// Every command, for the tests that must consider all of them —
    /// **complete by the compiler** (`every_variant!`: the list is an
    /// exhaustive match, so a verb left out does not compile).
    ///
    /// The third shape of this list. A hand-typed `const ALL` first, which the
    /// module doc called a completeness test: deleting `Command::Reply` from
    /// it and its `TABLE` row left all 917 tests green, with `:reply`
    /// unreachable by any spelling (Reviewer C, round 3). Then a `next()`
    /// chain said to be complete "by the compiler" — but its exhaustive match
    /// gave each verb a successor, never a predecessor, and the same deletion
    /// (Reply unchained) went red only because the manual happens to name
    /// `:reply` (review stop 12, T6 10;
    /// `[[feedback_enumeration_means_unmodeled]]`).
    #[cfg(test)]
    pub(crate) const ALL: &'static [Command] = every_variant!(
        Command: Quit,
        QuitAll,
        WriteQuit,
        EditTranscript,
        Config,
        Source,
        Reload,
        Unsource,
        Scriptnames,
        Set,
        Verbose,
        Themes,
        Graph,
        Split,
        VSplit,
        Fork,
        VFork,
        Model,
        Models,
        Effort,
        Memories,
        Skills,
        Skill,
        Setup,
        Trust,
        Untrust,
        Import,
        Unread,
        Send,
        Reply,
        SGraph,
        VSGraph,
        Close,
        Hide,
        Only,
        New,
        SNew,
        VNew,
        Clone,
        Merge,
        Squash,
        Distill,
        Compact,
        Pick,
        Return,
        Delete,
        Open,
        Resume,
        Rename,
        Description,
        Child,
        Continue,
        Help,
        Messages,
        Write,
        WriteAll,
        Analyze,
        Tool,
        Secret,
        NoSecret,
        Secrets,
        Keymap,
        Allow,
        Deny,
        Ask,
        Unallow,
        Unask,
        Undeny,
        Rules,
        Buffers,
        Buffer,
        Decide,
        Aim,
        TabNew,
        TabClose,
        TabNext,
        TabPrevious,
        TabMove,
        TabOnly,
        TabRename,
        Tabs,
        Marks,
        Tab,
        Ball,
    );
}

/// One row of the command table: a [`Command`] and the words that name it.
pub struct Spec {
    /// The command this row names.
    pub cmd: Command,
    /// Canonical, fully-spelled name — the completion target and the string a
    /// prefix must match (e.g. `"quit"`).
    pub name: &'static str,
    /// Explicit short forms resolved *before* prefix search (e.g. `["q"]`).
    /// Empty when the command has none.
    pub aliases: &'static [&'static str],
    /// Argument synopsis for `:help` and completion, vim-style: `<value>` for a
    /// required value, `{a|b}` for a required choice, `[…]` for optional. Empty
    /// when the command takes no arguments.
    pub usage: &'static str,
    /// One-line description shown in `:help`.
    pub help: &'static str,
    /// What Tab completes for this command's argument.
    pub arg: ArgKind,
    /// The command accepts a trailing `!` (`:q!`), vim's `-bang`: "do it
    /// anyway". On any other command the `!` is refused, as vim's E477 does,
    /// rather than ignored: a force that silently means nothing is worse
    /// than a refusal.
    pub bang: bool,
}

/// What Tab completes for a command's argument: a fixed set of words, a live-id
/// space, or both. Declaring it here — not in a hand-written `match` in the app
/// — is what keeps the "one table" promise for completion: a new id-taking
/// command gets completion by filling in the row it must write anyway, instead
/// of silently getting none.
#[derive(Clone, Copy)]
pub struct ArgKind {
    /// Fixed literal argument words (e.g. `config` → `dump`, `edit`).
    pub words: &'static [&'static str],
    /// Words that exist only at runtime — `:help`'s every tag and command,
    /// listed from the composed manual. A function rather than a slice because
    /// this table is a `const` and those words are computed; the words and the
    /// function are both offered.
    pub generated: Option<fn() -> Vec<String>>,
    /// The live-id space this argument draws from; the app (which owns the
    /// graph) supplies the actual ids.
    pub ids: IdSpace,
    /// **The argument is a command line of its own** — vim's modifiers
    /// (`:verbose set x?`) — so completion goes on as if the rest had been
    /// typed alone. The other fields are unused when this is set.
    pub modifies: bool,
    /// Words only the app can supply — it holds the prompt, the chat and the
    /// secret table; the row names which ([`Supplied`]).
    pub supplied: Supplied,
}

/// Which app-held words complete an argument. One axis, so a new source is a
/// new value rather than a second flag beside the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supplied {
    /// None.
    None,
    /// Text already typed — the prompt's, then the chat's, latest first
    /// (`:secret`).
    Typed,
    /// The marked secrets, by handle and by value (`:nosecret`).
    Secrets,
    /// The skills found for the session in front, by name (`:skill`).
    Skills,
    /// This directory's Claude Code conversations, by short id (`:import`).
    Conversations,
}

impl ArgKind {
    /// No completable argument (no words, no ids).
    pub const NONE: Self = Self {
        words: &[],
        generated: None,
        ids: IdSpace::None,
        modifies: false,
        supplied: Supplied::None,
    };
    /// A command line: the rest completes as one ([`ArgKind::modifies`]).
    pub const COMMAND: Self = Self {
        modifies: true,
        ..Self::NONE
    };
    /// Text already typed in the prompt or the chat ([`Supplied::Typed`]).
    pub const TYPED: Self = Self {
        supplied: Supplied::Typed,
        ..Self::NONE
    };
    /// The secrets this session holds ([`Supplied::Secrets`]).
    pub const SECRETS: Self = Self {
        supplied: Supplied::Secrets,
        ..Self::NONE
    };
    /// The skills found ([`Supplied::Skills`]).
    pub const SKILLS: Self = Self {
        supplied: Supplied::Skills,
        ..Self::NONE
    };
    /// Fixed argument words only, no live ids.
    pub const fn words(words: &'static [&'static str]) -> Self {
        Self {
            words,
            ..Self::NONE
        }
    }
    /// Words produced at runtime by `source`, no live ids.
    pub const fn generated(source: fn() -> Vec<String>) -> Self {
        Self {
            generated: Some(source),
            ..Self::NONE
        }
    }
    /// Ids from `ids`, and the fixed `words` beside them.
    pub const fn ids(ids: IdSpace, words: &'static [&'static str]) -> Self {
        Self {
            words,
            ids,
            ..Self::NONE
        }
    }
}

/// Which live-id space a command's argument draws from — the table names the
/// space, the app resolves it against the graph ([`App::matching_short_ids`]).
///
/// [`App::matching_short_ids`]: crate::app::App
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdSpace {
    /// No live ids (a fixed-word or no-argument command).
    None,
    /// Episode (session) ids only — e.g. `:delete`, which is session-scoped.
    Episodes,
    /// Episodes **and** turns — e.g. `:open`, which checks out either.
    Nodes,
}

/// **The words a session-taking command reads before any session** —
/// `:delete all`, `:Send ball` — read off the table (an argument's `words`
/// beside a live-id space), so a new keyword is refused as a session name
/// the day it is declared (`name_syntax`), rather than named `all` and then
/// deleting everything (review of 34033e8, Reviewers A and B).
pub fn session_keywords() -> impl Iterator<Item = &'static str> {
    TABLE
        .iter()
        .filter(|spec| spec.arg.ids != IdSpace::None)
        .flat_map(|spec| spec.arg.words.iter().copied())
}

/// What `:split`/`:vsplit` accept as a pane name — the `<Tab>` completion
/// source. Kept in step with `workspace::PaneKind::{from_name, OPENABLE}` by a
/// test, since a name that completes but does not resolve is worse than one
/// that does neither.
pub const PANE_NAMES: &[&str] = &[
    "output",
    "prompt",
    "graph",
    "analyze",
    "help",
    "messages",
    "ls",
    "alias",
    "rules",
    "scriptnames",
    "themes",
    "tabs",
    "marks",
];

/// What `:model` Tab-completes: the short names from
/// [`shannon_core::backend::models::ALIASES`]. A full model id is still accepted
/// — completion offers the ones worth having in the fingers, it does not gate
/// what is typeable. Kept in step with the table by
/// `model_names_match_the_alias_table` — a name that completes but does not
/// resolve is worse than one that does neither. That test did not exist when
/// this comment first claimed it did (Reviewers A and C, independently).
pub const MODEL_NAMES: &[&str] = &[
    "stub",
    "haiku",
    "sonnet",
    "opus",
    "fable",
    "sonnet-5",
    "opus-5",
    "fable-5",
    "opus-4-8",
    "opus-4-7",
    "opus-4-6",
    "sonnet-4-6",
    "haiku-4-5",
];

/// What `:effort` Tab-completes: `default`, then every level, lowest first.
/// Kept in step with `shannon_core::Effort::ALL` by
/// `effort_names_match_the_levels`.
pub const EFFORT_NAMES: &[&str] = &["default", "low", "medium", "high", "xhigh", "max"];

/// What `:set` Tab-completes: `all`, and every option it would accept —
/// read from the config's own shape (`set::option_names`), so a new setting
/// completes the day it exists and a refused one never does.
fn set_words() -> Vec<String> {
    std::iter::once("all".to_string())
        .chain(shannon_core::config::set::option_names())
        .collect()
}

/// The command table. Order here is the `:help` display order.
pub const TABLE: &[Spec] = &[
    Spec {
        cmd: Command::Quit,
        name: "quit",
        aliases: &["q"],
        usage: "",
        help: "Close this pane; quit Shannon Code from the last one -- the pane beside the prompt bar, or the bar alone (vim).",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::QuitAll,
        name: "qall",
        aliases: &["qa", "quitall"],
        usage: "",
        help: "Quit Shannon Code, however many panes are open.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::WriteQuit,
        name: "wq",
        aliases: &["x", "xit"],
        usage: "[name]",
        help: "Save this session (naming it, as `:w <name>`), then close the pane (vim's `:wq`/`:x`).",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::EditTranscript,
        name: "Edit",
        aliases: &[],
        usage: "",
        help: "Open the transcript in $EDITOR (read-only).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Config,
        name: "config",
        aliases: &[],
        usage: "{dump|edit|save [file]}",
        help: "Dump the default template, edit your config (`:reload` applies it), or save \
               this session's settings into the project's (`!`: where it is not trusted) or a \
               file you name.",
        arg: ArgKind::words(&["dump", "edit", "save"]),
        bang: true,
    },
    Spec {
        cmd: Command::Source,
        name: "source",
        aliases: &["so"],
        usage: "<file>",
        help: "Apply a config file over the running one — additive, like vim's.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Reload,
        name: "reload",
        aliases: &["rel"],
        usage: "",
        help: "Source your config file again (vim's `:so $MYVIMRC`).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Unsource,
        name: "unsource",
        aliases: &["uns"],
        usage: "<file>",
        help: "Take a sourced file's settings back out; what it set falls back.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Scriptnames,
        name: "scriptnames",
        aliases: &["scr"],
        usage: "",
        help: "List the config files in effect, in the order they apply.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Set,
        name: "set",
        aliases: &["se"],
        usage: "[all|<option>[=<value>|?|&|!]]",
        help: "Show or change an option at once: `:set tui.prompt.height=15`.",
        arg: ArgKind::generated(set_words),
        bang: false,
    },
    Spec {
        cmd: Command::Verbose,
        name: "verbose",
        aliases: &["verb"],
        usage: "set <option>?",
        help: "`:verbose set x?` — the value, and every layer that sets it.",
        arg: ArgKind::COMMAND,
        bang: false,
    },
    Spec {
        cmd: Command::Themes,
        name: "themes",
        aliases: &[],
        usage: "",
        help: "Every theme, beside a preview drawn in the one under the cursor; `<CR>` wears it.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Graph,
        name: "graph",
        aliases: &["g"],
        usage: "",
        help: "Show the conversation graph in this pane (`:q` gives the pane back).",
        arg: ArgKind::NONE,
        bang: false,
    },
    // The window family, named vim's way. Vim resolves `:sp`/`:spl`/`:spli`/
    // `:split` by mandatory-prefix; we resolve by declared forms only (a user
    // ruling — see the module doc), so the forms vim documents as the short
    // ones are spelled out here.
    Spec {
        cmd: Command::Split,
        name: "split",
        aliases: &["sp", "spl"],
        usage: "[what]",
        help: "Split the focused pane, stacked. `what` names the new pane's content.",
        arg: ArgKind::words(PANE_NAMES),
        bang: false,
    },
    Spec {
        cmd: Command::VSplit,
        name: "vsplit",
        aliases: &["vs", "vsp"],
        usage: "[what]",
        help: "Split the focused pane, side by side.",
        arg: ArgKind::words(PANE_NAMES),
        bang: false,
    },
    Spec {
        cmd: Command::Fork,
        name: "fork",
        aliases: &[],
        usage: "",
        help: "Split the pane and clone its conversation into the new one — a side \
               conversation the original never sees (`/btw`, but you can keep going).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::VFork,
        name: "vfork",
        aliases: &[],
        usage: "",
        help: "`:fork`, side by side.",
        arg: ArgKind::NONE,
        bang: false,
    },
    // Vim's "split and open X" family — `:sb`(uffer), `:sf`(ind), `:sv`(iew).
    // `:sg`/`:vsg` follow it exactly, and are what you reach for rather than
    // `:split graph`.
    Spec {
        cmd: Command::SGraph,
        name: "sgraph",
        aliases: &["sg"],
        usage: "",
        help: "Open the graph viewer in a stacked split.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::VSGraph,
        name: "vsgraph",
        aliases: &["vsg"],
        usage: "",
        help: "Open the graph viewer in a side-by-side split.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Close,
        name: "close",
        aliases: &["clo"],
        usage: "",
        help: "Close the focused pane (never the last one) — in a decision pane, denying its \
               call; not a chat a request's pane sits under (`:hide` closes it, the request \
               waiting), nor a prompt holding a draft (`:q!` drops it).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Hide,
        name: "hide",
        aliases: &["hid"],
        usage: "",
        help: "Stop showing this view here — a borrowed pane goes back, a pane opened for it \
               closes; a decision waits, hidden, for `:decide` (`:q` denies it), as does a \
               request whose chat you hide; not a prompt holding a draft.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Only,
        name: "only",
        aliases: &["on", "onl"],
        usage: "",
        help: "Close every pane but the focused one (and the prompt you write in) — not over a \
               request or a draft in another; :only! closes anyway, a request waiting, hidden, \
               a draft dropped.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::New,
        name: "new",
        aliases: &[],
        usage: "",
        help: "Show a fresh session in this pane (stays put on a blank one no other pane shows).",
        arg: ArgKind::NONE,
        bang: false,
    },
    // Vim's `:new`/`:vnew` split and then edit a new buffer; Shannon's `:new`
    // is vim's `:enew`, so the stacked one is `:snew`, as `:sg` is to
    // `:graph` (Steven, 11-10).
    Spec {
        cmd: Command::SNew,
        name: "snew",
        aliases: &[],
        usage: "",
        help: "A fresh session in a new pane, stacked with the window you are in (from the \
               prompt, the one it aims at).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::VNew,
        name: "vnew",
        aliases: &["vne"],
        usage: "",
        help: "A fresh session in a new pane beside the window you are in (from the prompt, \
               the one it aims at).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Model,
        name: "model",
        aliases: &[],
        usage: "[name]",
        help: "Set this session's model (stub, haiku, sonnet, opus, or a full id); bare, :models.",
        arg: ArgKind::words(MODEL_NAMES),
        bang: false,
    },
    Spec {
        cmd: Command::Models,
        name: "models",
        aliases: &[],
        usage: "",
        help: "Choose this session's model, and the effort and context budget, on one screen.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Effort,
        name: "effort",
        aliases: &[],
        usage: "[level]",
        help: "Set how hard this session's model works (low … max, or default) — bare, choose \
               in a panel.",
        arg: ArgKind::words(EFFORT_NAMES),
        bang: false,
    },
    Spec {
        cmd: Command::Memories,
        name: "memories",
        aliases: &[],
        usage: "",
        help: "The instruction files and memories this session is told — <Enter> edits one, \
               Tab turns to the skills.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Skills,
        name: "skills",
        aliases: &[],
        usage: "",
        help: "The skills found — <Enter> edits one, Tab turns to the memories.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Skill,
        name: "skill",
        aliases: &[],
        usage: "<name> [args]",
        help: "Run a skill now, on the session in front: its text goes with the turn.",
        arg: ArgKind::SKILLS,
        bang: false,
    },
    Spec {
        cmd: Command::Setup,
        name: "setup",
        aliases: &[],
        usage: "",
        help: "Choose how Shannon reaches a model: the API (an API key), Claude Code (your \
               `claude` login), or the demo.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Trust,
        name: "trust",
        aliases: &[],
        usage: "[dir]",
        help: "Say which directories are trusted, or trust one — `.` for this one: Shannon \
               reads a trusted project's .shannon/ (config, models, skills), and Claude Code \
               runs there with the project's own settings.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Untrust,
        name: "untrust",
        aliases: &[],
        usage: "[dir]",
        help: "Stop trusting a directory — this one, with none: its .shannon/ is no longer \
               read, nor Claude Code run there unasked.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Import,
        name: "import",
        aliases: &[],
        usage: "[--all] <id|file>",
        help: "Bring a Claude Code conversation in as a session in this pane — from its latest \
               compaction, or with --all from its first prompt. An id is its first 8 \
               characters or more; Tab lists this directory's.",
        arg: ArgKind {
            words: &["--all"],
            supplied: Supplied::Conversations,
            ..ArgKind::NONE
        },
        bang: false,
    },
    Spec {
        cmd: Command::Unread,
        name: "unread",
        aliases: &[],
        usage: "[all]",
        help: "Stop reading Claude Code's conversations — this directory's, or with all every \
               one's; `always` is taken back, and Shannon asks again.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Send,
        name: "Send",
        aliases: &[],
        usage: "[1,3|all|ball|b2|<id>]",
        help: "Send the prompt to every marked pane (<C-w><Space>), to these pane numbers, \
               to all transcript panes, to every session in :ls (ball), or to sessions by \
               :ls number (b2) or id, shown or not.",
        arg: ArgKind::ids(IdSpace::Episodes, &["all", "ball"]),
        bang: false,
    },
    Spec {
        cmd: Command::Reply,
        name: "reply",
        aliases: &[],
        usage: "",
        help: "Reply to the turn under the cursor — a thread branching from that point.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Clone,
        name: "clone",
        aliases: &[],
        usage: "[b2|<id>|<name>]",
        help: "Fork a session (the selected/current one, or one by id or name) into a new one sharing its history.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Merge,
        name: "merge",
        aliases: &[],
        usage: "[b2|<id>|<name>]",
        help: "Splice a branch (the selected/current one, or one by id or name) back into the session it forked from.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Squash,
        name: "squash",
        aliases: &[],
        usage: "",
        help: "Replace the marked turns (or the selected branch) with a summary you write in $EDITOR.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Distill,
        name: "distill",
        aliases: &[],
        usage: "",
        help: "`:squash`, the model writing the summary (`:graph`'s `S`) — reviewed in $EDITOR \
               as `[merge] distill_review` says; `!` does the other for once.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::Compact,
        name: "compact",
        aliases: &[],
        usage: "[--keep N] [instructions]",
        help: "This session's history into one summary the model writes, as Claude Code's \
               /compact — the words after it say what to keep; `!` reviews it in $EDITOR first.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::Pick,
        name: "pick",
        aliases: &[],
        usage: "",
        help: "Compose the rows marked in `:graph` into a new session (its `p`); originals kept.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Return,
        name: "return",
        aliases: &[],
        usage: "[b2|<id>|<name>]",
        help: "Return a session's result (the selected/current one, or one named) into its \
               parent, as one turn; originals kept.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Delete,
        name: "delete",
        aliases: &[],
        usage: "[all|b2|<id>|<name>]",
        help: "Delete a session: current, all, or one by :ls number (b2), id or name.",
        arg: ArgKind::ids(IdSpace::Episodes, &["all"]),
        bang: false,
    },
    Spec {
        cmd: Command::Open,
        name: "open",
        aliases: &[],
        usage: "<b2|id|name|%>",
        help: "Jump to a session (b2, its id, its name, or % for this one) or to a turn by id.",
        arg: ArgKind::ids(IdSpace::Nodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Resume,
        name: "resume",
        aliases: &[],
        usage: "[b2|<id>|<name>]",
        help: "Bare: the sessions to choose from (`:graph`); with one, open it — Claude \
               Code's `/resume`.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Rename,
        name: "rename",
        aliases: &[],
        usage: "[session] <name>",
        help: "Name this session (or the one `:graph` has selected, or one named first); \
               bare shows its name, `!` takes it off. The id never changes.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: true,
    },
    Spec {
        cmd: Command::Description,
        name: "description",
        aliases: &[],
        usage: "[text]",
        help: "Say in a line what this session is (the inspector shows it); bare shows it, \
               `!` takes it off.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::Child,
        name: "child",
        aliases: &[],
        // `--stop <id>` is deliberately not in the synopsis — it would widen
        // the whole COMMANDS gutter past usefulness; the CHILD SESSIONS help
        // section carries the full flag set (vim's index/topic split).
        usage: "[-m <model>] [prompt]",
        help: "Spawn a background child session briefed with <prompt> ($EDITOR if omitted); --stop <id> cancels one.",
        // The prompt is free text; ids only matter to `--stop`, whose targets
        // (running children) aren't a completable IdSpace yet.
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Continue,
        name: "continue",
        aliases: &[],
        usage: "",
        help: "Resume the last turn where it stopped (max_tokens / interrupted); [continue] picks the strategy.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Help,
        name: "help",
        aliases: &["h"],
        usage: "[topic]",
        help: "Show the manual in this pane; :help <topic> opens it at a chapter, a tag, a \
               :command or an option (as :set takes it).",
        // EVERY name `:help` accepts — chapters, every `*tag*` with or without
        // its prefix (an option's included), every `:command` — listed from
        // the manual itself. It
        // offered only the chapter names while accepting all of these, so
        // `:help sh<Tab>` did nothing (Steven, 2026-09-27).
        arg: ArgKind::generated(crate::help::targets),
        bang: false,
    },
    Spec {
        cmd: Command::Messages,
        name: "messages",
        aliases: &["mes"],
        usage: "",
        help: "Show every notice this session, newest last — where a status-row message that was cut off finishes.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Write,
        name: "write",
        aliases: &["w"],
        usage: "[name]",
        help: "Save this session to the store now (turns save as they commit; this is the retry when \
               one didn't); with a name, name it as vim names a buffer.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::WriteAll,
        name: "wall",
        aliases: &["wa"],
        usage: "",
        help: "Save every open session to the store.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Analyze,
        name: "analyze",
        aliases: &[],
        usage: "<command>",
        help: "Preview how a shell command would be classified (stages, effect, allow/ask/deny) — does not run it.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Tool,
        name: "tool",
        aliases: &[],
        usage: "<command>",
        help: "Route <command> through the permission gate as a model tool call would -- INCLUDING running it if policy already allows. (:analyze previews without running.)",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Secret,
        name: "secret",
        aliases: &[],
        usage: "[--no-spend] <text>",
        help: "Mark <text> as a secret: it leaves as a handle, never as itself. --no-spend also forbids commands expanding it. <Tab> completes from the prompt, then the chat, latest first.",
        arg: ArgKind::TYPED,
        bang: false,
    },
    Spec {
        cmd: Command::NoSecret,
        name: "nosecret",
        aliases: &[],
        usage: "<handle|value>",
        help: "Take a secret back out: off this session's table and out of the vault. History keeps its handle, which no longer expands, and the value is no longer masked. <Tab> completes handles and values.",
        arg: ArgKind::SECRETS,
        bang: false,
    },
    Spec {
        cmd: Command::Secrets,
        name: "alias",
        aliases: &["secrets"],
        usage: "",
        help: "Show this session's aliases and secrets ([[alias]], [secrets] env, :secret) in a live pane: each handle beside its value, kind, spendable, source. The values are yours to see; the agent never sees them.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Keymap,
        name: "keymap",
        aliases: &["km"],
        usage: "[context]",
        help: "Show the keys in force and the action names a [keymap.*] config binds them to.",
        arg: ArgKind::words(KEYMAP_CONTEXTS),
        bang: false,
    },
    Spec {
        cmd: Command::Allow,
        name: "allow",
        aliases: &[],
        usage: "<rule>",
        help: "Allow commands matching <rule> for this session (e.g. :allow uv run pytest *). Your config is untouched.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Deny,
        name: "deny",
        aliases: &[],
        usage: "<rule>",
        help: "Deny commands matching <rule> for this session. deny beats ask beats allow.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Ask,
        name: "ask",
        aliases: &[],
        usage: "<rule>",
        help: "Always ask before commands matching <rule>, for this session, whatever else allows them.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Unallow,
        name: "unallow",
        aliases: &[],
        usage: "<rule>",
        help: "Take an allow rule out for this session — shipped, your config's, or this session's. The file is untouched.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Unask,
        name: "unask",
        aliases: &[],
        usage: "<rule>",
        help: "Take an ask rule out for this session; one shipped or in your config needs ! (a call it asks about could run unasked).",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::Undeny,
        name: "undeny",
        aliases: &[],
        usage: "<rule>",
        help: "Take a deny rule out for this session; one shipped or in your config needs ! (it loosens the policy you did not write this session).",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::Rules,
        name: "rules",
        aliases: &[],
        usage: "",
        help: "Show the permission policy in force, live, in a pane: shipped rules, the ones your config removes, yours, and this session's.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Buffers,
        name: "ls",
        aliases: &["buffers", "files"],
        usage: "",
        help: "List the sessions open this run, shown in a pane or not, marking the ones that need you; with ! the pages read beside them too (the manual).",
        arg: ArgKind::NONE,
        bang: true,
    },
    // vim's `:b[uffer]`: every form from `:b` up, declared, so the synopsis
    // can say it vim's way (`synopsis`) and still list only what resolves.
    Spec {
        cmd: Command::Buffer,
        name: "buffer",
        aliases: &["b", "bu", "buf", "buff", "buffe"],
        usage: "<N|id|name>",
        help: "Show session N (as :ls numbers it; :b2 and :2b too), or one by id or name, or \
               a page :ls! numbers, in this pane; the one it replaces keeps running.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Decide,
        name: "decide",
        aliases: &[],
        usage: "[N|id|name]",
        help: "Go to a decision waiting on you: session N's, else this session's, else the \
               first; its session shown here if no window shows it, as :b N would.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::Aim,
        name: "aim",
        aliases: &[],
        usage: "[N|id|name]",
        help: "Aim the prompt you write in at session N (as :ls numbers it), or one by id or \
               name, without showing it; bare, say where it goes.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    // vim's tab pages (`tabs.md`): a named layout of panes, the buffers
    // shared. `:tabnew` is Shannon's `:new` in a new tab, as vim's is `:new`'s
    // empty buffer in one.
    Spec {
        cmd: Command::TabNew,
        name: "tabnew",
        aliases: &[],
        usage: "[N|id|name]",
        help: "Open a new tab after this one, on a fresh session, or on session N (as :ls \
               numbers it), or one by id or name.",
        arg: ArgKind::ids(IdSpace::Episodes, &[]),
        bang: false,
    },
    Spec {
        cmd: Command::TabClose,
        name: "tabclose",
        aliases: &["tabc"],
        usage: "[N|+N|-N|$]",
        help: "Close this tab and show the one to its right (the left, from the last) — or \
               close tab N and stay. Its sessions stay open (:ls). Not over a request or a \
               draft in it; ! closes anyway, a request waiting, hidden, a draft dropped.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::TabNext,
        name: "tabnext",
        aliases: &["tabn"],
        usage: "[N|+N|-N|$]",
        help: "Go to the next tab (gt), to tab N ({N}gt), N tabs over, or to the last.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::TabPrevious,
        name: "tabprevious",
        aliases: &["tabp", "tabNext", "tabN"],
        usage: "[N]",
        help: "Go to the previous tab, or N tabs back (gT, {N}gT).",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::TabMove,
        name: "tabmove",
        aliases: &["tabm"],
        usage: "[N|+N|-N|$]",
        help: "Move this tab after tab N (0: to the front), N places right or left, or (bare, \
               $) to the end.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::TabOnly,
        name: "tabonly",
        aliases: &["tabo"],
        usage: "[N|+N|-N|$]",
        help: "Close every tab but this one (or but tab N). Their sessions stay open (:ls). Not \
               over a request or a draft in one; ! closes anyway, a request waiting, hidden, a \
               draft dropped.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::TabRename,
        name: "tabrename",
        aliases: &[],
        usage: "[name]",
        help: "Name this tab — the tab line shows it instead of what its pane shows; bare \
               says it, `!` takes it off.",
        arg: ArgKind::NONE,
        bang: true,
    },
    Spec {
        cmd: Command::Tabs,
        name: "tabs",
        aliases: &[],
        usage: "",
        help: "List the tab pages and the windows in each (vim's :tabs), live — `>` the window \
               you are in, `%` the prompt a tab writes in, `+` a prompt holding a draft; <CR> \
               goes to a tab, or stands you in a window.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Marks,
        name: "marks",
        aliases: &[],
        usage: "",
        help: "List the marks, live — each session's, yours ('a) then the agent's (`a), each in \
               turn order, with its turn, the line it marks and the agent's note; <CR> goes to \
               one.",
        arg: ArgKind::NONE,
        bang: false,
    },
    Spec {
        cmd: Command::Tab,
        name: "tab",
        aliases: &[],
        usage: "{cmd}",
        help: "Run a command with the window it opens in a new tab: :tab help, :tab b 2, :tab \
               sp. One that opens none opens no tab.",
        arg: ArgKind::COMMAND,
        bang: false,
    },
    Spec {
        cmd: Command::Ball,
        name: "ball",
        aliases: &["ba", "sball", "sba"],
        usage: "[N]",
        help: "A window for each session (:ls) no window here shows, at most N; :tab ball, a \
               tab for each.",
        arg: ArgKind::NONE,
        bang: false,
    },
];

/// What `:keymap` Tab-completes: the `[keymap.<name>]` context names.
///
/// Hand-written because [`TABLE`] is a `const` and the real source
/// (`Context::config_name`) is only reachable at runtime — so it is **asserted
/// equal to it** by `keymap_context_names_match_the_engine`, the arrangement
/// `PANE_NAMES` and `MODEL_NAMES` use for the same reason. A name that
/// completes but does not resolve is worse than one that does neither.
pub const KEYMAP_CONTEXTS: &[&str] = &[
    "motion",
    "goto",
    "open-bracket",
    "close-bracket",
    "normal",
    "visual",
    "insert",
    "window",
    "z",
    "text-object",
];

/// The outcome of resolving the command *word* (the first token of the line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved<'a> {
    /// A single command matched; `args` is the rest of the line, trimmed,
    /// `bang` is whether the name was followed by `!` (only where
    /// [`Spec::bang`]), and `count` is vim's leading `[N]` (only where
    /// [`Command::takes_count`]).
    Command {
        cmd: Command,
        args: &'a str,
        bang: bool,
        count: Option<&'a str>,
    },
    /// Blank line — nothing to run.
    Empty,
    /// The word matched no command name or alias.
    Unknown,
    /// A `!` on a command that takes none (vim's E477, "No ! allowed").
    NoBang(Command),
    /// A count on a command that takes none (vim's E481, "No range allowed")
    /// — refused rather than ignored, since it is a number the user meant.
    NoCount(Command),
}

/// A command line as vim's ex parser takes it apart: `[N]name[!] args`.
///
/// **The name is a run of letters**, so `:b1` is `:b` with `1` and `:2b` is
/// `:b` with a count — vim's rule, and every name in [`TABLE`] is letters
/// (a test holds that, since a name with a digit could never resolve). It
/// split on whitespace, so `:b1` was an unknown command while `:help` said
/// `:buffer N` (Steven, 2026-10-01).
struct Parts<'a> {
    /// vim's leading count (`:2b`) or range (`:+tabmove`, `:$tabnext`,
    /// `:.tabclose`, `:-2tabnew`), as typed: digits, or `$`, `.`, `+N`,
    /// `-N` (`+` / `-` alone). The commands that take one say what it means
    /// ([`Command::takes_count`]); the rest refuse it.
    count: Option<&'a str>,
    /// The command name: the run of ASCII letters after the count.
    name: &'a str,
    /// A `!` directly after the name (`:q!`).
    bang: bool,
    /// Everything after the name and its `!`, untrimmed at the end — a
    /// trailing space is "the next word" to completion.
    rest: &'a str,
}

/// The one splitter: [`resolve`], `modified` and [`typed_name`] all read a
/// line through it, so none can disagree about where the name ends.
fn parts(line: &str) -> Parts<'_> {
    let line = line.trim_start();
    let digits = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    // vim's ranges on the tab commands, measured (Neovim 0.12.6): `:+tabmove`
    // one right, `:$tabnext` the last, `:.tabclose` this one, `:-tabnew`
    // before this one. They read as "not a command" (Steven, 10-26: `:+tabm`).
    let range = match line.as_bytes().first() {
        Some(b'$' | b'.') => 1,
        Some(b'+' | b'-') => 1 + digits(&line[1..]),
        _ => digits(line),
    };
    let (count, line) = line.split_at(range);
    let line = line.trim_start();
    let letters = line.len()
        - line
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .len();
    let (name, rest) = line.split_at(letters);
    let (bang, rest) = match rest.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    Parts {
        count: (!count.is_empty()).then_some(count),
        name,
        bang,
        rest,
    }
}

/// **The command as typed — its count and name, never what follows**, for a
/// refusal to quote. What follows a mistyped command is an argument, and an
/// argument can be a secret (`:sett secrets.extra=pw`, Reviewer B). The
/// refusal quoted the first whitespace word, which a glued argument is part
/// of: `:sett=pw` put the password on screen and in the log.
pub fn typed_name(line: &str) -> String {
    let p = parts(line);
    if p.count.is_none() && p.name.is_empty() {
        // Not a name at all (`:!ls`, `:é`): its first character, which a
        // secret cannot hide in.
        return line.trim_start().chars().take(1).collect();
    }
    format!("{}{}", p.count.unwrap_or(""), p.name)
}

/// Resolve a colon-less command line (`"config edit"`, `"q"`, `"open 2442"`)
/// into a [`Command`] plus its argument string.
///
/// Matching is case-sensitive (vim: `:Edit` is not `:edit`): a command answers
/// to its full name and its declared aliases (`spec_named`).
pub fn resolve(line: &str) -> Resolved<'_> {
    if line.trim().is_empty() {
        return Resolved::Empty;
    }
    let p = parts(line);
    let Some(spec) = spec_named(p.name) else {
        return Resolved::Unknown;
    };
    if p.bang && !spec.bang {
        return Resolved::NoBang(spec.cmd);
    }
    if p.count.is_some() && !spec.cmd.takes_count() {
        return Resolved::NoCount(spec.cmd);
    }
    Resolved::Command {
        cmd: spec.cmd,
        args: p.rest.trim(),
        bang: p.bang,
        count: p.count,
    }
}

/// **The table row a command word names** — its full name or a declared
/// alias, nothing in between. The one matching rule: [`resolve`] and
/// `modified` both ask it, so a form one accepts the other cannot refuse.
///
/// No prefix fallback. (It used to abbreviate to any unambiguous prefix,
/// vim-style — the user ruled that out after `:gr`/`:gra`/`:grap` all
/// silently meant `:graph`: keep the deliberate forms, drop the accidental
/// ones. Its `Resolved::Ambiguous` went 10-27, never produced since: two
/// rows sharing a name fail `every_declared_form_is_letters_and_resolves_to_its_row`,
/// each form held to its own row.)
fn spec_named(word: &str) -> Option<&'static Spec> {
    TABLE
        .iter()
        .find(|s| s.name == word || s.aliases.contains(&word))
}

/// `:q, :quit` — the command's declared forms: each alias, then the full name.
/// The explicit-forms rule made vim's `:q[uit]` bracket notation a lie, so the
/// synopsis now lists exactly what resolves.
/// A command that takes a `!` shows it as vim does, optional: `:q[!]`; one
/// that takes a count, vim's `:[N]b`.
///
/// **vim's notation where it is true**: when the forms are every prefix of
/// the name from the shortest up, `:b[uffer]` says exactly what resolves, in
/// one token instead of six — the reading Steven quoted from vim's manual.
fn synopsis(spec: &Spec) -> String {
    let bang = if spec.bang { "[!]" } else { "" };
    let count = if spec.cmd.takes_count() { "[N]" } else { "" };
    if let Some(short) = vim_abbreviation(spec) {
        return format!(":{count}{short}[{}]{bang}", &spec.name[short.len()..]);
    }
    let mut forms: Vec<String> = spec
        .aliases
        .iter()
        .map(|a| format!(":{count}{a}{bang}"))
        .collect();
    forms.push(format!(":{count}{}{bang}", spec.name));
    forms.join(", ")
}

/// The synopsis `cmd`'s row opens with in the commands chapter — for `:help
/// :cmd` to find that row by asking, rather than by parsing the forms back
/// out of the rendered text.
pub fn synopsis_of(cmd: Command) -> Option<String> {
    TABLE.iter().find(|s| s.cmd == cmd).map(synopsis)
}

/// The shortest form, when a command's declared forms are **exactly** the
/// prefixes of its name from that one up — so vim's bracket notation states
/// them all. `None` for any other set: `:sp, :spl, :split` lacks `:spli`, and
/// bracketing it would claim a form that does not resolve.
fn vim_abbreviation(spec: &Spec) -> Option<&'static str> {
    let shortest = *spec.aliases.iter().min_by_key(|a| a.len())?;
    let lengths = shortest.len()..spec.name.len();
    // The count is half of "exactly" (no alias outside the run) AND what
    // keeps the caller's slice `name[shortest.len()..]` in bounds: an alias
    // LONGER than its name (`:alias` is also `:secrets`) gives an empty range,
    // where `all` alone is vacuously true — deleting this panicked the whole
    // manual (mutation R6, 2026-10-01). An empty range has length 0 and a row
    // with an alias has at least 1.
    let exactly = spec.aliases.len() == lengths.len()
        && lengths
            .clone()
            .all(|n| spec.aliases.contains(&&spec.name[..n]));
    exactly.then_some(shortest)
}

/// The column every `:help` line must fit in — vim wraps its help files at 78
/// (`textwidth` in help buffers), and the page is read beside a live prompt at
/// half-screen width, so the bound is a usability contract, not cosmetics
/// (user-requested 2026-07-12). A test gates every generated line — this
/// chapter's and the OPTIONS chapter's (`options_chapter`).
pub(crate) const HELP_WIDTH: usize = 78;

/// Greedy word-wrap to `width` columns (char-counted — the help page is
/// effectively single-width text). A single word longer than `width` gets its
/// own overlong line rather than being split; the width test would flag it.
pub(crate) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Append `text` re-flowed to [`HELP_WIDTH`]: paragraphs (blank-line-separated)
/// wrap independently, so the source literals can be written for the *editor's*
/// line width while the page always renders within the help width.
fn push_prose(out: &mut String, text: &str) {
    for (i, para) in text
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .enumerate()
    {
        if i > 0 {
            out.push('\n');
        }
        for line in wrap_words(para, HELP_WIDTH) {
            out.push_str(&line);
            out.push('\n');
        }
    }
}

/// Render the command reference as chapter 3 of the `:help` manual — the one
/// chapter generated from data rather than hand-written, so a new command
/// documents itself through the table row it must add anyway.
/// [`compose`](crate::help::compose) injects this at its table-of-contents
/// position; the hand-written chapters (`help.rs`) cover :graph, child
/// sessions, lifecycle, and config. The `:q, :quit` synopsis carries the
/// declared-forms rule, and the intro prose covers the command line's other
/// affordances. Every line fits `HELP_WIDTH` (vim's help textwidth) — the
/// manual-wide contract, gated over the whole page in `help.rs`.
pub fn commands_chapter() -> String {
    use std::fmt::Write as _;

    // Left column = synopsis + usage; pad every description to a shared gutter.
    let invocations: Vec<String> = TABLE
        .iter()
        .map(|s| {
            if s.usage.is_empty() {
                synopsis(s)
            } else {
                format!("{} {}", synopsis(s), s.usage)
            }
        })
        .collect();
    let gutter = invocations.iter().map(String::len).max().unwrap_or(0);
    // The description column: what's left of the width after the indent (2),
    // the invocation gutter, and the separator (2). Wrapped descriptions
    // continue on their own lines, aligned to the column (vim's index style).
    let desc_col = gutter + 4;
    let desc_width = HELP_WIDTH.saturating_sub(desc_col).max(20);

    let mut out = String::new();
    // Chapter heading, in the manual's hand-written style (help.rs) so the
    // generated chapter is indistinguishable from the rest.
    out.push_str(
        "==============================================================================\n\
         3. COMMANDS — the : reference                               *commands*\n\n",
    );
    push_prose(
        &mut out,
        "Press `:` to open the command line, type a command, then <CR>. A command \
         answers to its full name and its listed aliases (`:q`/`:quit`, \
         `:g`/`:graph`) — nothing in between. Where every form between is listed, \
         the row says so vim's way: `:b[uffer]` is `:b` through `:buffer`. As in \
         vim, a name is letters, so `:b2` is `:b 2`, and `[N]` is a count before \
         the name (`:2b`). <Tab>/<S-Tab> complete and cycle \
         commands, arguments, and ids; <Up>/<Down> recall history \
         (prefix-filtered); <Esc>, or <BS> on the empty line, leaves the command \
         line.",
    );
    out.push('\n');
    for (inv, spec) in invocations.iter().zip(TABLE) {
        // First wrapped chunk beside the invocation; continuations aligned to
        // the description column.
        let desc = wrap_words(spec.help, desc_width);
        let _ = writeln!(
            out,
            "  {inv:<gutter$}  {}",
            desc.first().map(String::as_str).unwrap_or(""),
            gutter = gutter
        );
        for cont in desc.iter().skip(1) {
            let _ = writeln!(out, "{:desc_col$}{cont}", "", desc_col = desc_col);
        }
    }
    out
}

/// A resolved Tab-completion: the fixed left part of the line, the partial word,
/// the matching static candidate words, and the live-id space (if any) the
/// argument draws from. A candidate is applied as `format!("{stem}{candidate}")`;
/// the caller folds in ids matching `word` when `ids` isn't [`IdSpace::None`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// Everything up to and including the whitespace before the completed word.
    pub stem: String,
    /// The partial word under completion — the id-completion caller filters
    /// live ids by this prefix.
    pub word: String,
    /// Matching static completions in table order (possibly empty).
    pub candidates: Vec<String>,
    /// The live-id space to fold in for this argument, or [`IdSpace::None`]
    /// while still completing the command name (or past an unresolved verb).
    pub ids: IdSpace,
    /// Which app-held words to fold in — [`ArgKind::supplied`].
    pub supplied: Supplied,
}

/// Complete the last word of a colon-less command line. With no whitespace yet,
/// the word is the command name and candidates are matching names; past the
/// first token, the word is the argument and candidates are the resolved
/// command's fixed [`ArgKind::words`] (plus its [`Completion::ids`] space, which
/// the caller resolves against the graph). Completion always acts at end-of-line
/// — the `:` line only ever appends.
pub fn complete(line: &str) -> Completion {
    // A modifier's argument is a command line of its own: complete the rest
    // as if typed alone, and keep what came before it in the stem. (Steven:
    // `:verbose set tui.pro<Tab>` completed nothing while `:set` did.)
    if let Some(rest) = modified(line) {
        let mut inner = complete(rest);
        inner.stem = format!("{}{}", &line[..line.len() - rest.len()], inner.stem);
        return inner;
    }
    let (stem, word) = split_last_word(line);
    // A blank stem means we're still on the command name — the SAME trim rule
    // `resolve` applies (it trims before splitting), so the two functions can't
    // disagree about where the verb ends (a stray leading space no longer
    // suppresses name completion).
    if stem.trim().is_empty() {
        return Completion {
            stem: stem.to_string(),
            word: word.to_string(),
            candidates: TABLE
                .iter()
                .map(|s| s.name)
                .filter(|n| n.starts_with(word))
                .map(str::to_string)
                .collect(),
            ids: IdSpace::None,
            supplied: Supplied::None,
        };
    }
    // Past the verb → the resolved command's argument row drives both the fixed
    // words and the id space. The stem resolves to the command it belongs to
    // (`resolve` also handles a multi-word stem like `"delete all "`).
    let arg = match resolve(stem) {
        Resolved::Command { cmd, .. } => TABLE.iter().find(|s| s.cmd == cmd).map(|s| s.arg),
        _ => None,
    };
    let fixed = arg
        .map(|a| a.words)
        .unwrap_or(&[])
        .iter()
        .map(|w| w.to_string());
    let generated = arg
        .and_then(|a| a.generated)
        .map(|f| f())
        .unwrap_or_default();
    let candidates = fixed
        .chain(generated)
        .filter(|w| w.starts_with(word))
        .collect();
    Completion {
        stem: stem.to_string(),
        word: word.to_string(),
        candidates,
        ids: arg.map_or(IdSpace::None, |a| a.ids),
        supplied: arg.map_or(Supplied::None, |a| a.supplied),
    }
}

/// The command line a modifier carries, exactly as typed (a trailing space is
/// "the next word", so it must survive) — `None` unless the line's name is a
/// command whose argument [`modifies`](ArgKind::modifies), followed by a
/// space. Read through [`parts`] and [`spec_named`], as [`resolve`] reads it.
fn modified(line: &str) -> Option<&str> {
    let p = parts(line);
    if p.bang || p.count.is_some() {
        return None;
    }
    let rest = p.rest.strip_prefix(char::is_whitespace)?;
    spec_named(p.name)?.arg.modifies.then(|| rest.trim_start())
}

/// Split a line at its last whitespace into `(stem, word)` with the invariant
/// `stem + word == line` — the stem keeps its trailing separator, `word` is the
/// trailing run of non-whitespace. `word` is a **suffix** of `line`, so the
/// split offset (`line.len() - word.len()`) is always a char boundary: no byte
/// arithmetic around the separator, and no "whitespace is ASCII" assumption
/// (the `:` line accepts arbitrary Unicode, incl. multibyte whitespace like
/// U+00A0 from Option+Space).
fn split_last_word(line: &str) -> (&str, &str) {
    let word = line
        .rsplit_once(char::is_whitespace)
        .map_or(line, |(_, w)| w);
    (&line[..line.len() - word.len()], word)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **What `:keymap` completes is what `:keymap` accepts.**
    ///
    /// [`KEYMAP_CONTEXTS`] is hand-written (the table is a `const`), so it is
    /// the shape AGENTS.md says must be *asserted equal* to the thing it
    /// mirrors. Compared as ordered lists: the completion order is the listing
    /// order, and a set comparison would let them drift apart in presentation
    /// while passing.
    #[test]
    fn keymap_context_names_match_the_engine() {
        let engine: Vec<&str> = crate::bindings::CONTEXTS
            .iter()
            .map(|c| c.config_name())
            .collect();
        assert_eq!(
            KEYMAP_CONTEXTS,
            &engine[..],
            "the completion list and the engine's context names have drifted"
        );
    }

    /// Every `Command` variant has exactly one table row — the table can't
    /// silently drop or duplicate a verb.
    #[test]
    fn every_command_has_one_table_row() {
        let all = Command::ALL;
        for &cmd in all {
            let rows = TABLE.iter().filter(|s| s.cmd == cmd).count();
            assert_eq!(rows, 1, "{cmd:?} should have exactly one row");
        }
        assert_eq!(TABLE.len(), all.len(), "table/enum out of sync");
    }

    #[test]
    fn only_declared_forms_resolve() {
        // The user's ruling after `:gr`/`:gra`/`:grap` all silently meant
        // `:graph`: a command answers to its full name and declared aliases,
        // and NOTHING in between. (Consolidated in the M3 test review: this
        // one table covers exact names, aliases-beat-prefixes, and the
        // rejected in-betweens that used to be three tests.)
        for (line, want) in [
            ("q", Command::Quit),
            ("quit", Command::Quit),
            ("g", Command::Graph),
            ("graph", Command::Graph),
            ("h", Command::Help),
            ("help", Command::Help),
        ] {
            assert_eq!(
                resolve(line),
                Resolved::Command {
                    cmd: want,
                    args: "",
                    bang: false,
                    count: None,
                },
                "{line:?} should resolve to {want:?}"
            );
        }
        // Every in-between, from the table: each proper prefix of every name
        // that no command declares (a name or an alias). This sampled nine
        // (review stop 12, T6 13).
        let declared: std::collections::HashSet<&str> = (TABLE.iter())
            .flat_map(|s| std::iter::once(s.name).chain(s.aliases.iter().copied()))
            .collect();
        let mut asked = 0;
        for spec in TABLE {
            assert!(spec.name.is_ascii(), "premise: a name slices by byte");
            for end in 1..spec.name.len() {
                let line = &spec.name[..end];
                if declared.contains(line) {
                    continue;
                }
                asked += 1;
                assert_eq!(
                    resolve(line),
                    Resolved::Unknown,
                    "{line:?} (of `:{}`) must NOT resolve",
                    spec.name
                );
            }
        }
        assert!(
            asked > 9,
            "premise: more than the nine it sampled ({asked})"
        );
    }

    #[test]
    fn arguments_are_split_and_trimmed() {
        assert_eq!(
            resolve("open   2442678a"),
            Resolved::Command {
                cmd: Command::Open,
                args: "2442678a",
                bang: false,
                count: None,
            }
        );
        // Only declared forms resolve — a prefixed verb does not.
        assert_eq!(resolve("conf edit"), Resolved::Unknown);
        assert_eq!(
            resolve("config edit"),
            Resolved::Command {
                cmd: Command::Config,
                args: "edit",
                bang: false,
                count: None,
            }
        );
    }

    /// vim's `!`: part of the command word, on the commands that declare it,
    /// and REFUSED elsewhere rather than dropped, since a force that silently
    /// means nothing is worse than a refusal.
    #[test]
    fn a_bang_belongs_to_the_word_and_only_where_declared() {
        for (line, cmd) in [
            ("q!", Command::Quit),
            ("quit!", Command::Quit),
            ("qa!", Command::QuitAll),
            ("wq!", Command::WriteQuit),
            ("x!", Command::WriteQuit),
        ] {
            assert_eq!(
                resolve(line),
                Resolved::Command {
                    cmd,
                    args: "",
                    bang: true,
                    count: None,
                },
                "{line:?}"
            );
        }
        assert_eq!(resolve("graph!"), Resolved::NoBang(Command::Graph));
        assert_eq!(resolve("open! 2442"), Resolved::NoBang(Command::Open));
        // `!` alone names nothing; a bang on an unknown word is still unknown.
        assert_eq!(resolve("!"), Resolved::Unknown);
        assert_eq!(resolve("nope!"), Resolved::Unknown);
        // The `!` is the word's, never an argument's.
        assert_eq!(
            resolve("open 2442!"),
            Resolved::Command {
                cmd: Command::Open,
                args: "2442!",
                bang: false,
                count: None,
            }
        );
    }

    #[test]
    fn child_resolves_with_args_passed_verbatim() {
        // The flag grammar is the ARGUMENT parser's business (flags.rs); the
        // command table hands `:child` its whole tail untouched.
        assert_eq!(
            resolve("child -m haiku fix the tests"),
            Resolved::Command {
                cmd: Command::Child,
                args: "-m haiku fix the tests",
                bang: false,
                count: None,
            }
        );
    }

    #[test]
    fn case_is_significant() {
        // `Edit` is capital-E on purpose; lowercase matches nothing.
        assert_eq!(resolve("edit"), Resolved::Unknown);
    }

    #[test]
    fn empty_and_unknown_are_distinct() {
        assert_eq!(resolve("   "), Resolved::Empty);
        assert_eq!(resolve("nope"), Resolved::Unknown);
    }

    #[test]
    fn synopsis_lists_the_declared_forms() {
        let quit = TABLE.iter().find(|s| s.cmd == Command::Quit).unwrap();
        assert_eq!(synopsis(quit), ":q[!], :quit[!]", "vim's optional bang");
        let edit = TABLE
            .iter()
            .find(|s| s.cmd == Command::EditTranscript)
            .unwrap();
        assert_eq!(synopsis(edit), ":Edit", "no alias — just the name");
    }

    #[test]
    fn complete_offers_command_names_by_prefix() {
        // `c` is ambiguous — config, close, clone, compact, child, and
        // continue all start with it (table order).
        let c = complete("c");
        assert_eq!(c.stem, "");
        assert_eq!(
            c.candidates,
            vec![
                "config".to_string(),
                "close".to_string(),
                "clone".to_string(),
                "compact".to_string(),
                "child".to_string(),
                "continue".to_string()
            ]
        );
        // Bare `:` (empty) offers everything, in table order.
        assert_eq!(complete("").candidates.len(), TABLE.len());
        assert_eq!(complete("").candidates[0], "quit");
    }

    #[test]
    fn complete_offers_argument_words_past_the_verb() {
        let c = complete("config ");
        assert_eq!(c.stem, "config ");
        assert_eq!(
            c.candidates,
            vec!["dump".to_string(), "edit".to_string(), "save".to_string()]
        );
        // A partial argument narrows (the verb must be a declared form).
        assert_eq!(complete("config d").candidates, vec!["dump".to_string()]);
        assert_eq!(complete("config e").candidates, vec!["edit".to_string()]);
        assert_eq!(complete("delete ").candidates, vec!["all".to_string()]);
    }

    #[test]
    fn complete_stem_preserves_the_line_up_to_the_word() {
        // Applying `stem + candidate` must reconstruct the intended line, even
        // with extra spaces.
        let c = complete("config   du");
        assert_eq!(c.stem, "config   ");
        assert_eq!(format!("{}{}", c.stem, c.candidates[0]), "config   dump");
    }

    /// **Which half of "`:merge` completes and `:squash` doesn't" is true.**
    ///
    /// Reported from a drive, and worth pinning as a fact rather than a memory:
    /// NAME completion is table-driven and works for both. The difference is
    /// past the verb — `:merge` takes a session id and `:squash` takes no
    /// argument at all, so `<Tab>` there has nothing to offer.
    ///
    /// That asymmetry is deliberate but *load-bearing*: if `:squash` ever grows
    /// a `[<hash>]`, this test is the one that should fail, because the marks
    /// already name its target and the two must not silently disagree.
    #[test]
    fn squash_and_merge_complete_by_name_and_differ_only_past_the_verb() {
        assert_eq!(complete("sq").candidates, vec!["squash".to_string()]);
        assert!(
            complete("me").candidates.contains(&"merge".to_string()),
            "`me` is a prefix of both `merge` and `messages`: {:?}",
            complete("me").candidates
        );
        assert_eq!(complete("merg").candidates, vec!["merge".to_string()]);
        // Past the verb is where they differ, and this is the half to justify.
        assert_eq!(complete("merge ").ids, IdSpace::Episodes);
        assert_eq!(complete("squash ").ids, IdSpace::None);
    }

    #[test]
    fn complete_yields_nothing_for_free_form_or_unknown_args() {
        assert!(complete("open ").candidates.is_empty()); // hash: no static words
        assert!(complete("nope ").candidates.is_empty()); // unresolved verb
    }

    #[test]
    fn complete_is_char_safe_on_multibyte_whitespace() {
        // U+00A0 (Option+Space on macOS) is Unicode whitespace but 2 bytes — the
        // old `rfind`+`i+1` byte slice panicked here. Must not panic, and NBSP
        // still delimits the verb from its argument.
        assert!(complete("open\u{a0}12").candidates.is_empty()); // :open: no fixed words
        assert_eq!(
            complete("config\u{a0}d").candidates,
            vec!["dump".to_string()]
        );
        // U+3000 (CJK ideographic space, 3 bytes) too.
        assert_eq!(
            complete("config\u{3000}e").candidates,
            vec!["edit".to_string()]
        );
    }

    #[test]
    fn complete_ignores_leading_whitespace_when_naming() {
        // A stray leading space must not suppress command-name completion —
        // `resolve` trims, so `complete` must agree (else `:  q<Tab>` no-ops).
        assert_eq!(
            complete(" q").candidates,
            vec!["quit".to_string(), "qall".to_string()]
        );
        assert_eq!(
            complete("   co").candidates,
            vec![
                "config".to_string(),
                "compact".to_string(),
                "continue".to_string()
            ]
        );
    }

    #[test]
    fn commands_chapter_lists_every_command_with_its_description() {
        // Descriptions may wrap across lines (the width contract), so compare
        // with whitespace normalized — wrapping only ever changes whitespace.
        let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let page = commands_chapter();
        let flat = collapse(&page);
        for spec in TABLE {
            assert!(
                page.contains(&synopsis(spec)),
                "commands chapter missing synopsis for {:?}",
                spec.cmd
            );
            assert!(
                flat.contains(&collapse(spec.help)),
                "commands chapter missing description for {:?}",
                spec.cmd
            );
        }
        // The chapter tag anchors it in the manual's contents (vim-help style).
        assert!(page.contains("*commands*"));
    }

    #[test]
    fn commands_chapter_lines_fit_half_a_screen() {
        // The width contract (user-requested 2026-07-12): the chapter is read
        // beside a live prompt at half-screen width, so EVERY line — prose,
        // table rows, tagged headers — must render within 80 columns
        // (generated at vim's help textwidth, 78). Tabs expand at 8 like the
        // pager renders them, so a tag-aligned header can't cheat the count.
        // (`help.rs` gates the whole assembled manual; this localizes the
        // generated chapter's own failures.)
        let rendered_width = |line: &str| -> usize {
            line.chars().fold(0, |w, c| match c {
                '\t' => (w / 8 + 1) * 8,
                _ => w + 1,
            })
        };
        for line in commands_chapter().lines() {
            let w = rendered_width(line);
            assert!(w <= 80, "commands line renders {w} cols (max 80): {line:?}");
        }
    }
}

#[cfg(test)]
mod model_name_tests {
    use super::MODEL_NAMES;
    use shannon_core::backend::models::ALIASES;

    /// Completion offers exactly the names that resolve. Starts from the
    /// TABLE, not from `MODEL_NAMES` — a list cannot check that it lists
    /// everything.
    #[test]
    fn model_names_match_the_alias_table() {
        let from_table: Vec<&str> = ALIASES.iter().map(|m| m.alias).collect();
        assert_eq!(
            MODEL_NAMES,
            from_table.as_slice(),
            "`:model` completes names that do not resolve, or omits ones that do"
        );
    }

    /// `:effort` completes `default` and every level — from the TYPE's list,
    /// so a level added to `Effort` and not here fails.
    #[test]
    fn effort_names_match_the_levels() {
        let levels: Vec<&str> = shannon_core::Effort::ALL
            .iter()
            .map(|e| e.as_str())
            .collect();
        assert_eq!(super::EFFORT_NAMES[0], "default");
        assert_eq!(&super::EFFORT_NAMES[1..], levels.as_slice());
    }
}

/// How a line is taken apart — `parts`, and what it reaches.
#[cfg(test)]
mod parse_tests {
    use super::*;

    /// **vim's ex parser**: the name is a run of letters, so `:b1` is `:b`
    /// with `1`; a leading count is the `[N]` of `:[N]b[uffer]`; and every
    /// form of `b[uffer]` resolves. `:b1` was "not a command" while `:help`
    /// said `:buffer N` (Steven, 2026-10-01).
    #[test]
    fn the_name_is_letters_and_a_count_comes_first() {
        let buffer = |args, count| Resolved::Command {
            cmd: Command::Buffer,
            args,
            bang: false,
            count,
        };
        assert_eq!(resolve("b1"), buffer("1", None));
        assert_eq!(resolve("b 1"), buffer("1", None));
        assert_eq!(resolve("2b"), buffer("", Some("2")));
        assert_eq!(resolve("12 buffer"), buffer("", Some("12")));
        for form in ["b", "bu", "buf", "buff", "buffe", "buffer"] {
            assert_eq!(resolve(form), buffer("", None), "{form}");
        }
        assert_eq!(
            resolve("bufferx"),
            Resolved::Unknown,
            "no form past the name"
        );
        assert_eq!(
            resolve("Send1,3"),
            Resolved::Command {
                cmd: Command::Send,
                args: "1,3",
                bang: false,
                count: None,
            }
        );
        // A count where vim takes none is refused, never dropped.
        assert_eq!(resolve("3q"), Resolved::NoCount(Command::Quit));
        assert_eq!(resolve("3"), Resolved::Unknown, "a count is not a command");
    }

    /// **Every form a row declares is letters**, or the splitter above could
    /// never hand it to `spec_named` — a name with a digit or a dash would be
    /// declared and unreachable. Each form resolves to its own row.
    #[test]
    fn every_declared_form_is_letters_and_resolves_to_its_row() {
        for spec in TABLE {
            for form in std::iter::once(&spec.name).chain(spec.aliases) {
                assert!(
                    !form.is_empty() && form.chars().all(|c| c.is_ascii_alphabetic()),
                    "`:{form}` cannot be typed as a command name"
                );
                assert!(
                    matches!(resolve(form), Resolved::Command { cmd, .. } if cmd == spec.cmd),
                    "`:{form}` does not resolve to {:?}",
                    spec.cmd
                );
            }
        }
    }

    /// **A refusal quotes the name, never an argument** — glued or spaced.
    /// The first whitespace word was quoted, and `:sett=pw` is one word.
    #[test]
    fn a_typed_name_never_carries_an_argument() {
        assert_eq!(typed_name("sett secrets.extra=pw"), "sett");
        assert_eq!(typed_name("sett=pw"), "sett");
        assert_eq!(typed_name("3xyz hunter2"), "3xyz");
        assert_eq!(typed_name("!rm -rf"), "!");
    }

    /// **vim's bracket notation exactly where it is true**: every prefix
    /// from the shortest declared, and nowhere a form is missing.
    #[test]
    fn synopsis_brackets_only_a_complete_run_of_prefixes() {
        let of = |cmd| synopsis(TABLE.iter().find(|s| s.cmd == cmd).unwrap());
        assert_eq!(of(Command::Buffer), ":[N]b[uffer]");
        assert_eq!(of(Command::Only), ":on[ly][!]");
        assert_eq!(
            of(Command::Quit),
            ":q[!], :quit[!]",
            "`:qu` does not resolve"
        );
        assert_eq!(of(Command::Split), ":sp, :spl, :split", "`:spli` does not");
        // EXACTLY the prefixes: one more alias, and the brackets would hide it.
        let extra = Spec {
            aliases: &["b", "bu", "buf", "buff", "buffe", "x"],
            ..*TABLE.iter().find(|s| s.cmd == Command::Buffer).unwrap()
        };
        assert!(synopsis(&extra).contains(":[N]x"), "{}", synopsis(&extra));
    }

    /// `:verbose` is a modifier only once a space says its command line has
    /// begun — without one, `<Tab>` is still completing the name `verbose`.
    #[test]
    fn a_modifier_needs_a_space_before_its_command_line() {
        assert_eq!(complete("verbose").candidates, vec!["verbose".to_string()]);
        assert_eq!(modified("verbose se"), Some("se"));
    }
}
