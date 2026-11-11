//! The workspace: **however many panes exist, in whatever arrangement they are
//! currently in**, over the window-manager tree ([`wm`]).
//!
//! # Why this is not a list of panes
//!
//! Every previous version of this module described a *specific* arrangement and
//! projected it into a [`wm::Layout`] each frame — first
//! `{ orientation, order, per-axis span }` for two panes, then the same plus a
//! `tool_rows: Option<u16>` for three. Both were correct, and both were the same
//! mistake at different sizes: the arrangement lived in fields, so the *number*
//! and the *order* of panes were baked into the type. The general layer sitting
//! underneath ([`wm`]) has always supported an arbitrary tree; this module threw
//! that away and rebuilt a fixed shape sixty times a second.
//!
//! The cost was not theoretical. Deriving one pane's state by negating
//! another's (`!prompt_focused`) and answering "which pane is here?" from a
//! hand-written `if`/`else if` are both *correct at n = 2* — which is exactly
//! what makes them so hard to see. They are a different rule that happens to
//! agree at two, and when the third pane arrived they broke together: two panes
//! drew as focused, the third was unreachable by mouse, page keys skipped it,
//! and `<C-w>+` aimed at it resized something else.
//!
//! # The model
//!
//! Three things, kept apart:
//!
//! - **Where panes are** — [`wm::Layout`], *retained*, arbitrary shape and
//!   order. Splitting, removing, reorienting and swapping are its operations,
//!   and they are already n-agnostic.
//! - **What each pane shows** — the [`Slot`] registry. [`PaneKind`] is a closed
//!   set of *content kinds*; it is emphatically **not** a pane count. Two panes
//!   may show the same kind, which is the property the whole design is aimed at
//!   ("one buffer, many viewports").
//! - **Which pane has focus** — one [`wm::Focus`], a [`PaneId`]. Not a parallel
//!   enum, not a bool, and not something a pane can infer about itself by
//!   asking about a different pane.
//!
//! Everything else is a **query over those three**: [`Workspace::rects`],
//! [`Workspace::pane_at`], [`Workspace::focused`]. Add a pane and every one of
//! them keeps working, because none of them counts.

use ratatui::layout::{Position, Rect};

use crate::keymap::Direction;
use crate::layout::{BORDER_SHAPE, FrameLayout, PaneRect, content_size};
use crate::wm::{self, AxisSizing, PaneId, Sizing, SplitDir};

/// What a pane shows.
///
/// A closed set of **content kinds**, deliberately not a pane count: nothing
/// here says how many panes exist, how they are ordered, or that a kind appears
/// at most once. Adding a kind is a rendering and routing change; adding a
/// *pane* is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneKind {
    /// The conversation transcript.
    Output,
    /// The input buffer.
    Prompt,
    /// A pending decision (today: a tool call awaiting approval).
    Tool,
    /// The `:graph` session tree.
    Graph,
    /// The internals of whatever the graph pane has selected.
    Inspector,
    /// What the theme under `:themes`' cursor looks like, drawn in it — its
    /// companion, as the inspector is the graph's.
    ThemePreview,
    /// **A page** — a buffer of text a window reads (`View::Page`): the
    /// manual, a report, a listing. One variant for the eight, so every
    /// answer that is the same for all of them is one arm, and a ninth (a
    /// user's own listing, `:tabs`) is a [`PageKind`] variant and nothing
    /// else (Steven, 10-26, D2: "Approved"). Fifteen `match` lists spelled
    /// the eight out, each to say the same thing of all of them.
    Page(PageKind),
}

/// **Which page** a [`PaneKind::Page`] is — what tells one page from
/// another: its title, its manual section, the name `:split` takes, and
/// where each comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageKind {
    /// An `:analyze` command-effect report.
    Analyze,
    /// The `:help` manual.
    Help,
    /// The `:messages` notice history.
    Messages,
    /// `:ls` — every session open this run, live: what each is doing, and
    /// which needs you.
    Buffers,
    /// `:alias` (`:secrets`) — the session's substitution table, live: each
    /// handle, whether it is a secret, whether a command may spend it, and
    /// where it came from. Never a value.
    Aliases,
    /// `:rules` — the permission policy in force, live: each rule under where
    /// it came from, what was removed, and what the config could not read.
    Rules,
    /// `:scriptnames` — the config files in effect, in the order they apply,
    /// live: a window, as vim's `q?` is, not a line (Steven, 10-13).
    Scripts,
    /// `:themes` — every theme there is, shipped, yours and imported; `<CR>`
    /// wears the one under the cursor (`crate::theme_picker`).
    Themes,
    /// `:tabs` — the tab pages and the windows in each, live (vim's
    /// `:tabs`); `<CR>` goes to the tab under the cursor.
    Tabs,
    /// `:marks` — every mark, yours then the agent's, live (vim's `:marks`);
    /// `<CR>` goes to the one under the cursor.
    Marks,
    /// `shannon tutor`'s lessons — the one page you may change (vimtutor's
    /// copy: edited in place, never written; `page::Recipe::Tutor`).
    Tutor,
}

impl PageKind {
    /// The page's title.
    pub(crate) fn title(self) -> &'static str {
        match self {
            PageKind::Analyze => "analyze",
            PageKind::Help => "help",
            PageKind::Messages => "messages",
            PageKind::Buffers => "ls",
            PageKind::Aliases => "alias",
            PageKind::Rules => "rules",
            PageKind::Scripts => "scriptnames",
            PageKind::Themes => "themes",
            PageKind::Tabs => "tabs",
            PageKind::Marks => "marks",
            PageKind::Tutor => "tutor",
        }
    }

    /// **Is its text yours to change** — vim's `'modifiable'`: it takes the
    /// prompt's keymap (Insert, the operators, undo) and its edits are its
    /// buffer's, in every window on it (`PageBuffer::edit`). Every other page
    /// is read, and `y` copies from it.
    pub(crate) fn modifiable(self) -> bool {
        match self {
            PageKind::Tutor => true,
            PageKind::Analyze
            | PageKind::Help
            | PageKind::Messages
            | PageKind::Buffers
            | PageKind::Aliases
            | PageKind::Rules
            | PageKind::Scripts
            | PageKind::Themes
            | PageKind::Tabs
            | PageKind::Marks => false,
        }
    }

    /// The manual's section for this page (`PaneKind::help_topic`).
    fn help_topic(self) -> &'static str {
        match self {
            PageKind::Analyze => "analyze",
            PageKind::Help => "help",
            PageKind::Messages => "messages",
            PageKind::Buffers => "buffers",
            PageKind::Aliases => "secrets",
            PageKind::Rules => "rules",
            PageKind::Scripts => "scriptnames",
            PageKind::Themes => "themes",
            PageKind::Tabs => "tabs",
            PageKind::Marks => "mark-motions",
            PageKind::Tutor => "tutor",
        }
    }

    /// The name `:split` takes for this page (`PaneKind::from_name`). The
    /// tutor has none: it is `shannon tutor`'s, where the stub answers, and
    /// `:split tutor` in a session of yours would put its "send this" in
    /// front of a real model.
    fn from_name(name: &str) -> Option<PageKind> {
        Some(match name {
            "analyze" => PageKind::Analyze,
            "help" => PageKind::Help,
            "messages" | "mes" => PageKind::Messages,
            "buffers" | "ls" => PageKind::Buffers,
            "alias" | "secrets" => PageKind::Aliases,
            "rules" => PageKind::Rules,
            "scriptnames" | "scripts" => PageKind::Scripts,
            "themes" => PageKind::Themes,
            "tabs" => PageKind::Tabs,
            "marks" => PageKind::Marks,
            _ => return None,
        })
    }

    /// Every page, in order — the pages' part of [`PaneKind::all`], complete
    /// by the compiler (`every_variant!`). It was a `next()` chain, which let a
    /// page wired in as a dead end truncate the walk (review stop 12, Tier 3).
    pub(crate) const ALL: &'static [PageKind] = every_variant!(
        PageKind: Analyze,
        Help,
        Messages,
        Buffers,
        Aliases,
        Rules,
        Scripts,
        Themes,
        Tabs,
        Marks,
        Tutor,
    );
}

impl PaneKind {
    /// The pane title.
    pub(crate) fn title(self) -> &'static str {
        match self {
            PaneKind::Output => "output",
            PaneKind::Prompt => "prompt",
            PaneKind::Tool => "decision",
            PaneKind::Graph => "graph",
            PaneKind::Inspector => "inspector",
            PaneKind::ThemePreview => "preview",
            PaneKind::Page(page) => page.title(),
        }
    }

    /// Does this pane show a **static text page** — a
    /// [`TextView`](crate::text_view::TextView) that scrolls and navigates
    /// through the shared keymap, claiming only the keys that close it?
    ///
    /// **One answer**, and its consultations are checkable: `rg
    /// 'is_text_page'` — one in `app.rs` and `is_page`, through which the
    /// rest now ask (counted at review stop 16, D 5; it said four). The
    /// question was being re-typed as a list of kinds
    /// at every site that needed it — which keys the pane claims, where they
    /// are routed, what its legend says, and what a page key does. The
    /// inspector became a text page and got `j`/`k` from the one site that
    /// asked a predicate, while the four hand-lists left `<C-f>` dead under a
    /// legend advertising it. That is `[[feedback_rules_rot_by_retyping]]`
    /// exactly: none of the four failed to compile.
    ///
    /// The decision pane is excluded **deliberately**, not by omission: it is a
    /// `TextView` too, but it is *answered* rather than merely read, so its keys
    /// resolve before the keymap's ever run.
    pub(crate) fn is_text_page(self) -> bool {
        match self {
            PaneKind::Inspector | PaneKind::ThemePreview | PaneKind::Page(_) => true,
            PaneKind::Output | PaneKind::Prompt | PaneKind::Graph | PaneKind::Tool => false,
        }
    }

    /// **A window that shows only what it was made for** — Emacs's
    /// `window-dedicated-p`: it hosts no view, however it is sized. The
    /// decision pane belongs to its conversation and must stay answerable;
    /// a prompt window IS its draft and aim (`App`'s `PromptView`, each
    /// window's own since 10-26), never set aside — a transcript shown over
    /// one counted as a session pane while `pane_session` answered the aim
    /// (V5.3 design review, finding 1, probed), and set aside now it would
    /// put a draft out of reach. A split prompt is a flexible pane, which
    /// `is_flexible` alone let a view take.
    pub(crate) fn dedicated(self) -> bool {
        match self {
            PaneKind::Prompt | PaneKind::Tool => true,
            PaneKind::Output
            | PaneKind::Graph
            | PaneKind::Inspector
            | PaneKind::ThemePreview
            | PaneKind::Page(_) => false,
        }
    }

    /// **A pane whose body is a page you read and select in** — every text
    /// page, and the decision's report, which is one too (a `TextView`) with
    /// answer keys of its own. The one answer for what the keymap's motions,
    /// `/`, the operators, a click and a drag's copy reach: three sites spelled
    /// `is_text_page() || kind == PaneKind::Tool` and two left it off, so a drag
    /// in the decision entered Visual, its cursor did not follow, and the
    /// release copied nothing and said nothing (review stop 5, E2 #4).
    pub(crate) fn is_page(self) -> bool {
        self.is_text_page() || self == PaneKind::Tool
    }

    /// **Is this pane's text closed to typing** — vim's `'nomodifiable'`, where
    /// keys may still act? (The word, `read-only`, is borrowed: vim's `[RO]`
    /// is `'readonly'`, which lets you type.) Every pane but the prompt and a
    /// page you may change (the tutor): those drive the prompt's keymap
    /// (`App::drives_prompt_keymap`), so in every other a writer is refused —
    /// `:graph` too, though its `d` deletes
    /// and `p` picks, and the decision pane, whose own legend answers a
    /// Normal-mode writer ("not a decision key — …") and the keymap a Visual
    /// one ("read-only").
    ///
    /// **Said when you try to type, as vim's E21 is** — not on the row
    /// (Steven, 10-27, review stop 13, S13-Q1: "that's the only time vim
    /// tells me"). Its one consumer is the status row's `$readonly`, which a
    /// format may carry: the shipped `output` and `page` rows do, `graph`
    /// and `decision` do not. Held to the refusal, both ways, in every kind
    /// and in Visual too: `readonly_is_said_where_no_text_can_be_typed`.
    pub(crate) fn is_read_only(self) -> bool {
        match self {
            PaneKind::Prompt => false,
            // The tutor's text is yours to change (`PageKind::modifiable`).
            PaneKind::Page(page) => !page.modifiable(),
            PaneKind::Output
            | PaneKind::Tool
            | PaneKind::Graph
            | PaneKind::Inspector
            | PaneKind::ThemePreview => true,
        }
    }

    /// **The manual's section for this pane**, as `:help` takes it — the
    /// status row's `$help`, so `help `:help $help`` points at the page
    /// that lists what this pane's keys and commands do.
    /// `every_pane_names_a_help_topic_that_opens` holds each to the manual.
    pub(crate) fn help_topic(self) -> &'static str {
        match self {
            PaneKind::Output => "chat",
            PaneKind::Prompt => "editing",
            PaneKind::Tool => "decision",
            PaneKind::Graph => "graph-keys",
            PaneKind::Inspector => "graph-inspector",
            PaneKind::ThemePreview => "themes",
            PaneKind::Page(page) => page.help_topic(),
        }
    }

    /// The name `:split`, `:vsplit` and `:close` accept for this kind.
    pub(crate) fn from_name(name: &str) -> Option<PaneKind> {
        match name {
            "output" | "transcript" | "chat" => Some(PaneKind::Output),
            "prompt" => Some(PaneKind::Prompt),
            "graph" => Some(PaneKind::Graph),
            name => PageKind::from_name(name).map(PaneKind::Page),
        }
    }

    /// **May a user open this kind by name?** Deliberately not every variant.
    ///
    /// Exhaustive, so a new `PaneKind` is a compile error here and has to
    /// answer the question. This was a hand-written `[PaneKind; 6]` mirroring
    /// `from_name`, which is the `PaneKind::ALL` defect the comment twenty
    /// lines below celebrates removing — still live, in the same file.
    /// Reviewer D mutation-proved it: deleting `Messages` from the array left
    /// 1017 tests passing, because both of its tests assert only that the array
    /// is a *subset* of the enum. A list cannot check that it lists everything;
    /// it has to start from the type.
    pub(crate) const fn openable(self) -> bool {
        match self {
            // Opened by `shannon tutor`, where the stub answers its lessons
            // (`PageKind::from_name`).
            PaneKind::Page(PageKind::Tutor) => false,
            PaneKind::Output | PaneKind::Prompt | PaneKind::Graph | PaneKind::Page(_) => true,
            // Opened by a tool call arriving, never by hand.
            PaneKind::Tool => false,
            // Follows the graph's selection and is opened alongside it.
            PaneKind::Inspector => false,
            // Follows `:themes`' cursor and is opened alongside it.
            PaneKind::ThemePreview => false,
        }
    }

    /// **The pane opened beside this kind to describe it** — the graph's
    /// inspector (what it has selected), `:themes`' preview (the theme under
    /// its cursor).
    ///
    /// The one statement of the pairing, read at two sites (`rg
    /// 'companion\(\)|PaneKind::companion'`, 10-26): the host brings its
    /// companion wherever it opens or comes back — as a viewer, given back
    /// from under another, or a split (`App::pair_companion`, the one door a
    /// companion is born by); and the pairing read backwards
    /// ([`host_kind`](Self::host_kind)), for what a companion's host must
    /// show to hold it (`App::host_of`). (A third,
    /// `App::companions_follow`, went with plural trees.) Which
    /// companion is a host's own is the companion's record since V5.3 (3e)
    /// (`App::companions_of`), read by `release_companion`, `:only` and the
    /// reconcile; that a companion is never set aside under a view is
    /// `Window::kept`'s. It was read at five sites, ring proximity
    /// answering "whose". Each named the graph and the inspector itself
    /// until a second pair arrived.
    pub(crate) fn companion(self) -> Option<PaneKind> {
        match self {
            PaneKind::Graph => Some(PaneKind::Inspector),
            PaneKind::Page(PageKind::Themes) => Some(PaneKind::ThemePreview),
            PaneKind::Output
            | PaneKind::Prompt
            | PaneKind::Tool
            | PaneKind::Inspector
            | PaneKind::ThemePreview
            | PaneKind::Page(_) => None,
        }
    }

    /// **The kind this companion describes** — [`companion`](Self::companion)
    /// read backwards, derived from it so the pair is stated once: a
    /// companion pane names its host pane, and holds only while that pane
    /// shows this kind (`App::host_of`).
    pub(crate) fn host_kind(self) -> Option<PaneKind> {
        Self::all()
            .into_iter()
            .find(|host| host.companion() == Some(self))
    }

    /// The kinds a user may open by name — the `<Tab>` completion source.
    /// Derived from [`all`](Self::all), which the compiler keeps complete.
    pub(crate) fn openable_kinds() -> Vec<PaneKind> {
        Self::all().into_iter().filter(|k| k.openable()).collect()
    }

    /// Every kind: the unit ones, then each page ([`PageKind::ALL`]) —
    /// **complete by the compiler** (`every_variant!`: the list is an
    /// exhaustive match, `Page(_)` included, so a kind left out does not
    /// compile).
    ///
    /// The third shape of this list. A hand-written `const ALL: [PaneKind; 7]`
    /// first, whose "completeness" test iterated **over the array** and
    /// compared `ALL.len()` to itself — deleting a variant changed nothing
    /// (Reviewers, mutation-verified, the day it was written). Then a
    /// `next_kind` chain, whose exhaustive match gave each kind a successor
    /// but let one wired in as a dead end (`Foo => return None`) truncate the
    /// walk, which its test could catch only for the kinds it named (review
    /// stop 12, Tier 3; `[[feedback_enumeration_means_unmodeled]]`).
    ///
    /// **Not `#[cfg(test)]`.** It was, which meant the completeness was only
    /// ever available to the tests — production had a hand-written `OPENABLE`
    /// array instead, and that is precisely where the missing member turned
    /// up (Reviewer D). [`openable_kinds`](Self::openable_kinds) is its
    /// earned production consumer.
    pub(crate) fn all() -> Vec<PaneKind> {
        every_variant!(
            PaneKind: Output,
            Prompt,
            Tool,
            Graph,
            Inspector,
            ThemePreview;
            Page(PageKind)
        )
    }
}

/// How a pane's size is decided — a **per-pane policy**, so any number of panes
/// may use any of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extent {
    /// Takes whatever the tree gives it, and is what other panes borrow from.
    /// Guaranteed at least [`MIN_FLEXIBLE_ROWS`] of content, or something
    /// transient yields (see [`Workspace::fit`]).
    Flexible,
    /// A fixed bar the user sizes. `preferred` is what `<C-w>=` restores — the
    /// generalisation of the old `prompt_rows`/`prompt_cols` pair, now owned by
    /// whichever pane is a bar rather than by the workspace.
    Bar { preferred: AxisSizing },
    /// Sized to its **content**, and it *yields*: when the frame cannot spare
    /// the rows it shrinks, and when nothing is spare it leaves the layout for
    /// that frame rather than squeezing a flexible pane into uselessness.
    Transient { rows: u16 },
}

/// Which side of the divider a freshly split pane lands on.
///
/// Named rather than a `bool` because both readings are plausible at the call
/// site: vim's default is `Before` (`:split` opens above, `:vsplit` opens
/// left), and `[tui] splitbelow` / `splitright` select `After`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewPane {
    /// Above (horizontal) or left (vertical) — vim's default.
    Before,
    /// Below or right — `splitbelow` / `splitright`.
    After,
}

/// **What the layout asks of a window: its kind** — the policy a pane's kind
/// decides (a prompt is a bar, a decision a band; which panes `panes_of`
/// names), read from what the window SHOWS (V5.3 (3d)), so a pane's kind
/// and its content cannot be set apart: they were set by two calls, the
/// workspace's `show_in` and a field of the app's, and `:open` restored
/// the kind and left the help page sitting there.
pub(crate) trait Pane {
    fn kind(&self) -> PaneKind;
}

/// The layout's own tests' window: a kind and nothing else, so the geometry
/// is asked of the layout alone (proptest included), never of the app.
impl Pane for PaneKind {
    fn kind(&self) -> PaneKind {
        *self
    }
}

/// One pane: its identity in the tree, what it shows, how it is sized — and
/// its WINDOW, `W` (the app's `Window`; a bare [`PaneKind`] where only the
/// layout is asked): what it shows and where it is in it, held here so a pane and its
/// window are born together and go together (V5.3 (4)). They were two homes,
/// the window an app map made on demand and pruned by hand, its pane's kind
/// and binding here. **Which session a pane shows is its window's** (V5.3
/// (3c), the app's `SessionView`): the slot held it, kept under a view so
/// a covered transcript knew its session, beside the window's own cursor
/// and history — and before that a `lent` bit here said a transcript was
/// under the viewer (V4b). What a window shows, and what lies beneath it,
/// is not the workspace's to remember — nor, since (3d), its kind: asked
/// of the window ([`Slot::kind`]).
#[derive(Debug, Clone)]
pub(crate) struct Slot<W = PaneKind> {
    pub(crate) id: PaneId,
    pub(crate) extent: Extent,
    /// **Marked for a multi-send.** `<C-w><Space>` toggles it; `:Send` and a
    /// submitted prompt reach every marked transcript pane.
    ///
    /// On the slot rather than in a side table so it cannot outlive the pane:
    /// a `HashSet<PaneId>` beside the workspace would need its own line in
    /// `reconcile_pane_state`, and "the set of things that must be torn down
    /// together" is exactly what this project keeps getting wrong when it is
    /// spread across two homes (`[[feedback_teardown_doors]]`).
    pub(crate) marked: bool,
    /// **The pane's window** — born with it, removed with it into
    /// [`Workspace::take_closed`].
    pub(crate) window: W,
}

impl<W: Pane> Slot<W> {
    /// What the pane shows — its window's kind.
    pub(crate) fn kind(&self) -> PaneKind {
        self.window.kind()
    }

    /// The rows a transient pane is asking for, if it is one.
    fn transient_rows(&self) -> Option<u16> {
        match self.extent {
            Extent::Transient { rows } => Some(rows),
            _ => None,
        }
    }
}

/// Floor on a flexible pane's content rows.
///
/// The conversation must stay *readable* behind a decision — that is the whole
/// point of a pane instead of a takeover — so transient panes give rows back
/// rather than squeezing it to nothing.
const MIN_FLEXIBLE_ROWS: u16 = 4;

/// Floor on a **bar's** content rows when the frame is too tight for everything.
///
/// A bar is a size the user chose, so it is not touched while there is any
/// other way to balance the frame. But "the user chose ten rows of prompt" must
/// not outrank "there is a decision on screen that every session is blocked
/// on": a prompt shrunk to one row is still a prompt, and a decision pane
/// dropped to zero rows is a program that stops accepting input everywhere with
/// nothing on screen to answer. That is what happened — the transcripts were
/// squeezed to their titles, the decision was dropped, and the prompt kept all
/// eleven rows it had asked for.
const MIN_BAR_ROWS: u16 = 1;

/// Floor on a **transient** pane's content rows before it leaves the layout.
///
/// One row is not much of a decision pane, but it is a decision pane: it draws,
/// it can be focused, and its keys answer. Zero rows is a program that stops
/// accepting input in every session with nothing on screen to unblock it. The
/// order of sacrifice is now the order of how recoverable each loss is —
/// transient shrinks, then bars shrink, and only a frame that cannot hold all
/// three at their floors drops anything. When even that fails, the status line
/// carries the decision's answer keys — see `view::draw_status`.
const MIN_TRANSIENT_ROWS: u16 = 1;

/// The panes, their arrangement, which one has focus — and each pane's
/// window, `W` (see [`Slot`]).
#[derive(Debug, Clone)]
pub(crate) struct Workspace<W = PaneKind> {
    /// The **retained** arrangement. Not rebuilt per frame, not derived from
    /// fields describing one shape.
    layout: wm::Layout,
    /// PaneId → what it shows and how it is sized. Iterating this is how
    /// anything enumerates panes; nothing may hand-write the list.
    slots: Vec<Slot<W>>,
    focus: wm::Focus,
    /// **Where the user put the decision pane, once they did** — so the next
    /// decision opens there, at that size, for the rest of the session
    /// (Steven, 10-04: "We shouldn't restrict this"). `None`: content-fit, a
    /// band under the session that asked. `<C-w>=` forgets it.
    decision_place: Option<Place>,
    /// **The windows of the panes removed**, with what each showed, until
    /// the owner takes them ([`take_closed`](Self::take_closed)) — every
    /// removal, `close`, `only` and `hide` alike, puts its window here, so
    /// what must happen when a window goes (a decision closed is DENIED)
    /// has one door no removal can pass by. A closed pane's view used to
    /// outlive it in the app's map for the deny to read (V5.3 design
    /// review, finding 2).
    closed: Vec<(PaneId, W)>,
    /// **Rows kept above the panes** — the tab line's, while there is more
    /// than one tab (`App::sync_tabline`, vim's `showtabline=1`). Read by
    /// every frame-to-body mapping here ([`body`](Self::body)) and by the
    /// clamp's reserve, so the panes, the clicks and the sizes cannot
    /// disagree about where the panes start (`tabs.md` §5: "both take the
    /// tabline row, or one of them lies").
    top: u16,
    /// **The prompt you stood in last** ([`note_prompt`](Self::note_prompt)),
    /// asked by [`prompt_pane`](Self::prompt_pane), which checks it is still
    /// a prompt: a pane closed leaves it naming nothing, read as no note.
    last_prompt: Option<PaneId>,
}

/// A pane's place relative to its neighbour, and the size the user gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    /// The axis of the split it sits in: a band (`Horizontal`) or a column.
    dir: SplitDir,
    /// Above, or left of, its neighbour.
    first: bool,
    /// Its size wish, in a bar's storage convention (`Extent::Bar`).
    preferred: AxisSizing,
}

impl<W: Pane> Workspace<W> {
    /// Seed the default arrangement: a window — the launch's and `:tabnew`'s
    /// a transcript on its session, `:tab {cmd}`'s what the command opens,
    /// `<C-w>T`'s the window moved — with the prompt a fixed bar of
    /// `prompt_height` rows below it, each pane born holding its window (V5.3
    /// (2); the app's to say which since (3c)). This is a *starting
    /// position*, not the shape of the type — `<C-w>` moves it, and
    /// [`open`](Self::open) adds to it.
    pub(crate) fn new(prompt_height: u16, window: W, prompt_window: W) -> Self {
        debug_assert_eq!(
            prompt_window.kind(),
            PaneKind::Prompt,
            "an arrangement is born with its prompt"
        );
        debug_assert_ne!(window.kind(), PaneKind::Prompt, "and a window over it");
        let output = PaneId::fresh();
        let prompt = PaneId::fresh();
        // The prompt's two axes are remembered separately, because "12" is a
        // usable prompt in rows and a sliver in columns. Only the stacked axis
        // has a declared size; the side-by-side one is **proportional until the
        // user sizes it**, which is what "half the frame the first time you
        // `<C-w>L`" means without a reseed hook to remember to call. A ratio is
        // the honest representation of "no size has been asked for yet".
        let bar = prompt_bar(prompt_height);
        let mut layout = wm::Layout::single(output);
        layout.split_sized(output, prompt, SplitDir::Horizontal, bar);
        Workspace {
            layout,
            slots: vec![
                Slot {
                    id: output,
                    extent: Extent::Flexible,
                    marked: false,
                    window,
                },
                Slot {
                    id: prompt,
                    extent: Extent::Bar { preferred: bar },
                    marked: false,
                    window: prompt_window,
                },
            ],
            focus: wm::Focus::new(prompt),
            decision_place: None,
            closed: Vec::new(),
            top: 0,
            last_prompt: None,
        }
    }

    // --- the windows ---------------------------------------------------------

    /// `pane`'s window, if `pane` exists.
    pub(crate) fn window(&self, pane: PaneId) -> Option<&W> {
        self.slot(pane).map(|s| &s.window)
    }

    /// `pane`'s window, to change, if `pane` exists.
    pub(crate) fn window_mut(&mut self, pane: PaneId) -> Option<&mut W> {
        (self.slots.iter_mut())
            .find(|s| s.id == pane)
            .map(|s| &mut s.window)
    }

    /// Every pane's window, with its pane — in creation order: for a walk
    /// of every window, not for "which is first" (that is
    /// [`ring_order`](Self::ring_order)'s).
    pub(crate) fn windows(&self) -> impl Iterator<Item = (PaneId, &W)> {
        self.slots.iter().map(|s| (s.id, &s.window))
    }

    /// [`windows`](Self::windows), to change.
    pub(crate) fn windows_mut(&mut self) -> impl Iterator<Item = (PaneId, &mut W)> {
        self.slots.iter_mut().map(|s| (s.id, &mut s.window))
    }

    /// **The windows of the panes removed since last asked**, with what each
    /// showed — taken, so each is answered once: the door what must happen
    /// when a window goes (a decision window closed is DENIED) runs at
    /// (`App::reconcile_pane_state`), whichever removal it was.
    pub(crate) fn take_closed(&mut self) -> Vec<(PaneId, W)> {
        std::mem::take(&mut self.closed)
    }

    /// **The windows removed since the owner last took them**, read and left
    /// for [`take_closed`](Self::take_closed): what the owner's reconcile
    /// asks of this arrangement's closes before its one take
    /// (`App::reconcile_pane_state`'s re-aim — did the pane it went to close).
    pub(crate) fn closed_windows(&self) -> impl Iterator<Item = &W> {
        self.closed.iter().map(|(_, window)| window)
    }

    /// **Every window, the workspace gone** — each pane's, and those removed
    /// and not yet taken: what a tab closed leaves for the app's door
    /// [`take_closed`](Self::take_closed) feeds (`App::reconcile_pane_state`).
    pub(crate) fn into_windows(self) -> Vec<(PaneId, W)> {
        let mut windows: Vec<(PaneId, W)> = (self.slots.into_iter())
            .map(|slot| (slot.id, slot.window))
            .collect();
        windows.extend(self.closed);
        windows
    }

    /// **`pane`'s window, removed, claimed by the door that removed it** —
    /// a close made for a reason of its own (a decision pane closed by
    /// placement, by an answer or by `:hide`; a companion leaving with its
    /// host), which must not read as one the user made at the door
    /// `take_closed` feeds, nor wait there for it.
    pub(crate) fn claim_closed(&mut self, pane: PaneId) -> Option<W> {
        let at = self.closed.iter().position(|(id, _)| *id == pane)?;
        Some(self.closed.remove(at).1)
    }

    /// Remove the slots `gone` says, their windows kept for
    /// [`take_closed`](Self::take_closed) — the one way a slot leaves.
    fn remove_slots(&mut self, gone: impl Fn(&Slot<W>) -> bool) {
        let (out, keep): (Vec<Slot<W>>, Vec<Slot<W>>) = std::mem::take(&mut self.slots)
            .into_iter()
            .partition(|s| gone(s));
        self.slots = keep;
        (self.closed).extend(out.into_iter().map(|s| (s.id, s.window)));
    }
}

/// The prompt bar's declared size: its configured height on the vertical axis,
/// and "no width asked for yet" across. One function because this was written
/// out three times — at construction, at `<C-w>=`, and now at recovery — and a
/// non-trivial rule spelled three ways is the shape that rots
/// (`[[feedback_rules_rot_by_retyping]]`).
fn prompt_bar(prompt_height: u16) -> AxisSizing {
    AxisSizing {
        horizontal: Sizing::SecondCells(prompt_height.saturating_add(BORDER_SHAPE.0)),
        vertical: Sizing::Ratio(50),
    }
}

/// The frame minus `top` rows above (the tab line) and the status row below
/// — the area the pane tree tiles. A free function of the rows kept: the
/// geometry is the same whatever a slot holds ([`Workspace::body`] passes
/// its own).
fn body_of(frame: Rect, top: u16) -> Rect {
    let top = top.min(frame.height);
    Rect {
        y: frame.y.saturating_add(top),
        height: frame.height.saturating_sub(top).saturating_sub(1),
        ..frame
    }
}

impl<W: Pane> Workspace<W> {
    // --- the registry: every "which pane" question walks this ---------------

    /// The ids of every pane that exists, as a set — for tests that compare
    /// the panes before and after an action. Production keyed nothing else by
    /// `PaneId` once each window moved into its slot: a window cannot outlive
    /// its pane, so nothing needs pruning against this.
    #[cfg(test)]
    pub(crate) fn pane_ids(&self) -> std::collections::HashSet<PaneId> {
        self.slots.iter().map(|s| s.id).collect()
    }

    /// How many panes exist — the question `:q` asks to decide whether closing
    /// this one means quitting.
    pub(crate) fn pane_count(&self) -> usize {
        self.slots.len()
    }

    /// Every pane, in ring order.
    ///
    /// Production code reaches the panes through [`rects`](Self::rects) (it
    /// wants geometry too), so today only the tests walk the bare registry —
    /// and they walk it precisely so they stop hardcoding a pane count.
    #[cfg(test)]
    pub(crate) fn slots(&self) -> &[Slot<W>] {
        &self.slots
    }

    /// The slot for a pane id.
    pub(crate) fn slot(&self, id: PaneId) -> Option<&Slot<W>> {
        self.slots.iter().find(|s| s.id == id)
    }

    /// Every pane showing `kind`, in ring order.
    ///
    /// The plural of [`first_of`](Self::first_of), for the callers that need to
    /// know there is more than one — a second prompt window changes what the
    /// prompt's title has to say.
    pub(crate) fn panes_of(&self, kind: PaneKind) -> impl Iterator<Item = PaneId> + '_ {
        self.ring_order()
            .into_iter()
            .filter(move |&id| self.kind_of(id) == Some(kind))
    }

    /// The **first** pane showing `kind`, in ring order.
    ///
    /// "First" and not "the": a kind may legitimately appear on several panes,
    /// and callers that want a specific one must hold its [`PaneId`]. This is
    /// for the callers that genuinely mean "wherever the transcript is".
    /// [`panes_of`](Self::panes_of)'s first: it walked the slots, the order
    /// panes were MADE, under this doc saying ring order — so with two
    /// prompts, `set_focus(Prompt)` went to the one made first, wherever it
    /// was on screen (review stop 10, C 6: either order passed every test).
    pub(crate) fn first_of(&self, kind: PaneKind) -> Option<PaneId> {
        self.panes_of(kind).next()
    }

    /// Is any pane showing `kind`? (Order is no part of the answer.)
    pub(crate) fn has(&self, kind: PaneKind) -> bool {
        self.slots.iter().any(|s| s.kind() == kind)
    }

    /// Every pane, in the order `<C-w>w` cycles and the corner numbers show —
    /// the order anything "nearest" is measured in, so the answer is one a
    /// reader can derive from the screen.
    pub(crate) fn ring_order(&self) -> Vec<PaneId> {
        self.layout.leaves()
    }

    /// Which pane a new view or split acts on: **the focused one. Always.**
    ///
    /// This used to carry an exception — the prompt is a fixed bar and the
    /// place you stand when typing a command, so it redirected to the pane you
    /// came from. That exception was the only thing making the prompt special,
    /// and it made two things wrong at once: `:sp` from the prompt silently
    /// split the *transcript* (so the new pane appeared above the prompt, while
    /// the same key from the transcript put it below), and `:graph` from one
    /// prompt pane opened the graph in a different pane entirely. An exception
    /// that fires from the pane you type commands in is not an exception; it is
    /// the common case wearing a disguise.
    ///
    /// The reason for it was real — hosting a viewer in a 6-row bar leaves you
    /// a 3-row graph and nowhere to type — but the answer is to let the user
    /// open the pane they need (`:sp prompt`, `:sp output`) rather than to
    /// guess on their behalf. One rule, no redirect, nothing to predict.
    ///
    /// **Still true here, and `App::host_pane` adds a narrower one** — read
    /// them together. This answer stands for splits, which is what kept `:sp`
    /// symmetric. But content (a viewer, a session) cannot be hosted in the
    /// prompt bar at all, so for content the focused answer fell through to
    /// manufacturing a pane, and `:graph` from the prompt silently became
    /// `:sg` — the other half of a deliberately two-verb family, chosen by
    /// where the cursor happened to be (reported from a drive, 2026-09-14).
    ///
    /// Two things make that redirect different from the one removed above.
    /// It targets the pane the **prompt you write in is aimed at**, DRAWN in
    /// that prompt's title bar (each prompt window its own aim since 10-26,
    /// the one you write in marked `%` while there are two) — where "the pane
    /// you came from" gave a different one per pane. And it is scoped to
    /// showing content, so splits never see it.
    pub(crate) fn viewer_target(&self) -> PaneId {
        self.focused()
    }

    // What a pane shows is its window's (`window_mut`; the app's
    // `App::show_in`): `show_in` here set the kind alone, the content a
    // second call's, until V5.3 (3d) read the kind from the content.

    // --- focus --------------------------------------------------------------

    /// The focused pane.
    pub(crate) fn focused(&self) -> PaneId {
        self.focus.pane()
    }

    /// What the focused pane shows — the key that input routing dispatches on.
    /// Routing by *kind* is right even with several panes of a kind: they all
    /// take the same keys. Only rendering and geometry need the id.
    pub(crate) fn focused_kind(&self) -> PaneKind {
        self.slot(self.focused())
            .map_or(PaneKind::Prompt, Slot::kind)
    }

    /// Focus a specific pane, if it is in the tree.
    pub(crate) fn focus_pane(&mut self, id: PaneId) {
        self.focus.set(id, &self.layout);
    }

    /// Focus the first pane showing `kind` — **for the prompt, the one you
    /// type into** ([`prompt_pane`](Self::prompt_pane)): "you stand at the
    /// prompt" means the one holding your draft, not the first in the ring.
    /// Returns whether one existed.
    pub(crate) fn focus_kind(&mut self, kind: PaneKind) -> bool {
        let pane = match kind {
            PaneKind::Prompt => self.prompt_pane(),
            _ => self.first_of(kind),
        };
        match pane {
            Some(id) => {
                self.focus_pane(id);
                true
            }
            None => false,
        }
    }

    /// Directional focus (vim `<C-w>h/j/k/l`) over the real geometry. No-op at
    /// the edge, as in vim.
    pub(crate) fn focus_dir(&mut self, dir: Direction, body: Rect) {
        let layout = self.fit(Some(body));
        self.focus.reconcile(&layout);
        self.focus.shift(&layout, body, wm_dir(dir));
    }

    /// Next/previous pane in the ring (vim `<C-w>w`), over whatever exists.
    pub(crate) fn focus_cycle(&mut self, forward: bool, body: Rect) {
        let layout = self.fit(Some(body));
        self.focus.reconcile(&layout);
        self.focus.cycle(&layout, forward);
    }

    /// Focus pane number `n` — vim's `{count}<C-w>w`, 1-based in the same ring
    /// order `<C-w>w` walks and the pane borders display. `false` when there is
    /// no such pane, so the caller can say so rather than doing nothing.
    ///
    /// Ring order, not creation order: the number you type is the number you
    /// can read off the screen (`[[feedback_predictable_from_screen]]`).
    pub(crate) fn focus_nth(&mut self, n: usize, body: Rect) -> bool {
        let layout = self.fit(Some(body));
        self.focus.reconcile(&layout);
        match layout.leaves().get(n.wrapping_sub(1)) {
            Some(&pane) if n >= 1 => {
                self.focus.set(pane, &layout);
                true
            }
            _ => false,
        }
    }

    /// Back to the pane focus came from (vim `<C-w>p`). `false` when there is
    /// none — no move yet, or it has since closed — so the caller can say so.
    pub(crate) fn focus_alternate(&mut self, body: Rect) -> bool {
        let layout = self.fit(Some(body));
        self.focus.reconcile(&layout);
        self.focus.alternate(&layout)
    }

    // --- opening and closing panes ------------------------------------------

    /// **Open a decision pane under `near`**, `rows` tall — a band across its
    /// window — or where, and as big as, you last put one
    /// ([`decision_place`](Self::decision_place)). Addressed by id from then
    /// on ([`fit_decision`](Self::fit_decision)): a session has its own.
    ///
    /// A new pane always becomes the `second` child of the split it creates,
    /// which is what makes [`Sizing::SecondCells`] the right lever for a
    /// [`Transient`](Extent::Transient) pane's height. Transient panes are not
    /// [`adjustable`](Self::adjustable), so nothing can swap them out of that
    /// position behind the invariant's back.
    pub(crate) fn open_decision(&mut self, near: PaneId, rows: u16, window: W) -> PaneId {
        debug_assert_eq!(
            window.kind(),
            PaneKind::Tool,
            "a decision pane shows a decision"
        );
        match self.decision_place {
            Some(place) => self.open_at(place, near, window),
            None => self.open(
                Extent::Transient { rows },
                near,
                SplitDir::Horizontal,
                window,
            ),
        }
    }

    /// **Fit decision pane `id` to `rows`**, as its report changes — unless
    /// you sized it, which made it a bar: content no longer decides it, and an
    /// edit re-fitting the report must not undo a resize.
    pub(crate) fn fit_decision(&mut self, id: PaneId, rows: u16) {
        if !self.is_bar(id) {
            self.resize_to(id, Extent::Transient { rows });
        }
    }

    /// Whether `pane` is sized by the user — a bar.
    fn is_bar(&self, pane: PaneId) -> bool {
        matches!(self.slot(pane).map(|s| s.extent), Some(Extent::Bar { .. }))
    }

    /// **A new pane where the user last put one like it**: on `place`'s axis
    /// and side of `near`, at its size — a bar, since that size is theirs.
    fn open_at(&mut self, place: Place, near: PaneId, window: W) -> PaneId {
        let extent = Extent::Bar {
            preferred: place.preferred,
        };
        let id = self.open(extent, near, place.dir, window);
        // `open` makes it the second child; a mirrored sizing goes with the
        // swap, as for any bar moved to the first side (`oriented`).
        if place.first && id != near {
            self.layout.swap_with_sibling(id);
        }
        id
    }

    /// **A content-sized pane becomes the user's the moment they size or move
    /// it** — `pane`, or the one across its divider, which a command aimed at
    /// `pane` moves too. It becomes a bar at the size it is drawn at, its
    /// other axis proportional until sized: vim's rule for a proportional
    /// split, which becomes explicit on the first resize. It refused instead,
    /// so the decision pane — and the conversation beside it — could not be
    /// sized or moved at all (Steven, 10-04).
    fn adopt_around(&mut self, pane: PaneId, frame_w: u16, frame_h: u16) {
        let across = self.layout.sibling_of(pane);
        let body = self.body(Rect::new(0, 0, frame_w, frame_h));
        for id in std::iter::once(pane).chain(across) {
            if !matches!(
                self.slot(id).map(|s| s.extent),
                Some(Extent::Transient { .. })
            ) {
                continue;
            }
            let dir = self.layout.dir_around(id).unwrap_or(SplitDir::Horizontal);
            let drawn = self
                .fit(Some(body))
                .compute(body)
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, r)| match dir {
                    SplitDir::Horizontal => r.height,
                    SplitDir::Vertical => r.width,
                })
                .unwrap_or(border_along(dir) + 1);
            let mut preferred = AxisSizing::uniform(Sizing::Ratio(50));
            preferred.set_along(dir, Sizing::SecondCells(drawn));
            if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id) {
                slot.extent = Extent::Bar { preferred };
            }
            let sizing = self.oriented(id, preferred);
            self.layout.set_axis_sizing_around(id, sizing);
        }
    }

    /// Remember where a decision pane is and how big, if you sized or moved
    /// it, for the next one ([`decision_place`](Self::decision_place)) —
    /// **the one you moved**: `around` (the pane a resize or a move was aimed
    /// at), or the pane across its divider, which moves with it. It took the
    /// first decision pane made, so resizing a second remembered the first's
    /// size (design review DR-A #2).
    fn remember_decision_place(&mut self, around: PaneId) {
        let moved = std::iter::once(around).chain(self.layout.sibling_of(around));
        let decision =
            moved
                .filter_map(|id| self.slot(id))
                .find_map(|s| match (s.kind(), s.extent) {
                    (PaneKind::Tool, Extent::Bar { preferred }) => Some((s.id, preferred)),
                    _ => None,
                });
        let Some((id, preferred)) = decision else {
            return;
        };
        if let Some(dir) = self.layout.dir_around(id) {
            let first = !self.layout.is_second_child(id);
            self.decision_place = Some(Place {
                dir,
                first,
                preferred,
            });
        }
    }

    /// Open a **new** pane showing what `window` shows, splitting off `near`
    /// along `dir`.
    ///
    /// Several panes may show the same kind: a `show` beside this, for a
    /// surface meant to be singular, made "at most one pane per kind" a
    /// property of a convenience (it went with the decision panes, A1.2).
    ///
    /// Born holding `window`; with `near` not in the tree, nothing is made
    /// and the window is dropped.
    pub(crate) fn open(
        &mut self,
        extent: Extent,
        near: PaneId,
        dir: SplitDir,
        window: W,
    ) -> PaneId {
        let id = PaneId::fresh();
        // Seed BOTH axes honestly: a row count means nothing as a column
        // count, so the off-axis gets a proportional share until something
        // sizes it — the same rule the prompt bar has always used.
        let sizing = AxisSizing {
            horizontal: sizing_for(extent, SplitDir::Horizontal, true),
            vertical: sizing_for(extent, SplitDir::Vertical, true),
        };
        if !self.layout.split_sized(near, id, dir, sizing) {
            return near; // `near` isn't in the tree; nothing to split
        }
        self.slots.push(Slot {
            id,
            extent,
            marked: false,
            window,
        });
        id
    }

    /// Toggle `pane`'s mark, returning whether it is now marked. Only a pane
    /// that owns a session may be marked — marking the graph or the prompt
    /// would name something `:Send` has no way to send to — and which panes
    /// do is the app's to say (`App::shown_transcript`), which asks first.
    pub(crate) fn toggle_mark(&mut self, pane: PaneId) -> Option<bool> {
        let slot = self.slots.iter_mut().find(|s| s.id == pane)?;
        slot.marked = !slot.marked;
        Some(slot.marked)
    }

    /// Mark or unmark `pane` **idempotently**, returning whether it changed —
    /// a session's pane, as for [`toggle_mark`](Self::toggle_mark).
    ///
    /// The verb for a caller that means "these panes are marked" rather than
    /// "flip this one" — `:Send 1,1` went through `toggle_mark` and cancelled
    /// itself out, dropping a pane the user had named twice.
    pub(crate) fn set_mark(&mut self, pane: PaneId, marked: bool) -> bool {
        match self.slots.iter_mut().find(|s| s.id == pane) {
            Some(slot) if slot.marked != marked => {
                slot.marked = marked;
                true
            }
            _ => false,
        }
    }

    /// Every marked pane, in ring order.
    /// Only panes **currently rendering as a transcript**. `App::session_panes`
    /// also matches a pane lent to a viewer, and the mark glyph is drawn by
    /// `transcript_title`, which a `:graph` pane never calls — so a marked pane
    /// you then opened `:graph` in kept routing your `<Enter>` while showing no
    /// mark at all. `submit`'s marks-win branch justifies overriding the aim on
    /// the grounds that "the marks are drawn, so the override is visible"; this
    /// is what makes that true (Reviewer A, reproduced).
    pub(crate) fn marked_panes(&self) -> Vec<PaneId> {
        self.ring_order()
            .into_iter()
            .filter(|&id| {
                self.slot(id)
                    .is_some_and(|s| s.marked && s.kind() == PaneKind::Output)
            })
            .collect()
    }

    /// Drop every mark, returning how many there were.
    pub(crate) fn clear_marks(&mut self) -> usize {
        let mut n = 0;
        for slot in &mut self.slots {
            if slot.marked {
                slot.marked = false;
                n += 1;
            }
        }
        n
    }

    /// Is `pane` marked? Drives the mark glyph in its title.
    pub(crate) fn is_marked(&self, pane: PaneId) -> bool {
        self.slot(pane).is_some_and(|s| s.marked)
    }

    /// **The prompt this arrangement types into** — the one you stand in,
    /// else the one you stood in last ([`note_prompt`](Self::note_prompt)),
    /// else the first; `None` only between a removal of the last and the
    /// owner's `ensure_prompt`. What `<Enter>` sends, what a session command
    /// acts on, and the one a transcript you stand in aims (Steven, 10-26:
    /// "make sure that the reaim is on the last prompt accessed").
    ///
    /// A `prompt_pane()` lived here once and answered with the FIRST prompt
    /// slot, while every prompt window was a view of one composition: `:vsp`
    /// from the prompt gave a focused second pane whose binding nothing read.
    /// It went because no "which one" was correct then. Each prompt window
    /// owns its draft and its aim now (`App`'s `PromptView`), so which one
    /// you are writing in is the question, and this is its answer: the
    /// focused one first, so a key in a prompt edits that prompt whatever
    /// the last note said.
    pub(crate) fn prompt_pane(&self) -> Option<PaneId> {
        let is_prompt = |pane: PaneId| self.kind_of(pane) == Some(PaneKind::Prompt);
        (Some(self.focused()).filter(|&pane| is_prompt(pane)))
            .or(self.last_prompt.filter(|&pane| is_prompt(pane)))
            .or_else(|| self.panes_of(PaneKind::Prompt).next())
    }

    /// **`pane` is the prompt you used last** — noted where focus lands
    /// (`App::focus_moved`), so leaving it for a transcript leaves it the
    /// one that transcript aims ([`prompt_pane`](Self::prompt_pane)). A pane
    /// that is no prompt is ignored.
    pub(crate) fn note_prompt(&mut self, pane: PaneId) {
        if self.kind_of(pane) == Some(PaneKind::Prompt) {
            self.last_prompt = Some(pane);
        }
    }

    /// Open a pane across the top of the **whole frame**, above every existing
    /// pane — vim's `:help` placement.
    ///
    /// For when the focused pane cannot host what was asked for: a pane sized
    /// to something other than the space available (the prompt bar, a
    /// content-sized decision) splits into two of the same, so halving it gives
    /// two slivers rather than one usable window. Taking the space from the
    /// FRAME is the only way to get a usable pane without taking away the one
    /// the user was standing in.
    ///
    /// Deliberately stateless: it is always the top, full width, whatever the
    /// arrangement. "Where did it go" has one answer you can predict without
    /// knowing anything about the current layout.
    ///
    /// A transcript's `window` is born on the session it was made to show
    /// (`App::session_host`), a viewer's showing it (`App::open_viewer`).
    pub(crate) fn open_across_top(&mut self, window: W) -> PaneId {
        let id = PaneId::fresh();
        // Half the frame on each axis until something sizes it — the honest
        // representation of "no size has been asked for yet".
        let sizing = AxisSizing::uniform(Sizing::Ratio(50));
        self.layout
            .split_root(id, SplitDir::Horizontal, sizing, true);
        self.slots.push(Slot {
            id,
            extent: Extent::Flexible,
            marked: false,
            window,
        });
        id
    }

    /// Whether `pane` is sized by the space available, rather than to a fixed
    /// or content-derived size.
    ///
    /// The property that decides whether a pane can host arbitrary content.
    /// Distinct from [`adjustable`](Self::adjustable), which asks whether the
    /// user may *change* the size: the prompt bar is adjustable (you can make
    /// it taller) and still a poor host, because its size is a declaration
    /// rather than a share.
    pub(crate) fn is_flexible(&self, pane: PaneId) -> bool {
        matches!(self.slot(pane).map(|s| s.extent), Some(Extent::Flexible))
    }

    /// Update one pane's extent and push it into the tree's sizing.
    fn resize_to(&mut self, id: PaneId, extent: Extent) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id) {
            slot.extent = extent;
            let dir = self.layout.dir_around(id).unwrap_or(SplitDir::Horizontal);
            // A transient pane's `rows` is a row count: written along a column
            // split it would be read as COLUMNS.
            if matches!(extent, Extent::Transient { .. }) && dir == SplitDir::Vertical {
                return;
            }
            let second = self.layout.is_second_child(id);
            self.layout
                .set_sizing_around(id, sizing_for(extent, dir, second));
        }
    }

    /// What `pane` currently shows, or `None` if it is not in the tree.
    pub(crate) fn kind_of(&self, pane: PaneId) -> Option<PaneKind> {
        self.slot(pane).map(Slot::kind)
    }

    /// Split a named pane — the pane [`viewer_target`](Self::viewer_target)
    /// chose, which is simply the focused one — the newcomer born holding
    /// `window`, which says what it shows: the app's, built before the split
    /// (`App::split_command`: the split pane's kind unless one is named, a
    /// transcript on the split pane's session, vim's `:split` — a second
    /// window on the same buffer). `None`, and the window dropped, for a
    /// pane that cannot be split.
    pub(crate) fn split_pane(
        &mut self,
        focused: PaneId,
        dir: SplitDir,
        side: NewPane,
        window: W,
    ) -> Option<PaneId> {
        // **The decision pane is never split**: there is one decision, and a
        // split inherits the pane's kind — two panes answering it. Asked of
        // the KIND, not the extent: a decision pane the user sized is a bar,
        // and "content-sized" stopped meaning "decision" when it became
        // theirs to size. (Splitting a content-sized pane would also re-parent
        // it away from the divider `fit` shrinks.)
        if self
            .slot(focused)
            .is_none_or(|s| s.kind() == PaneKind::Tool || !self.adjustable(focused))
        {
            return None;
        }
        // The new pane is a peer, not a bar or a band: it takes an even share of
        // what the focused pane had. A *bar* that splits keeps its bar-ness for
        // the pane that was already there and hands the newcomer half of it,
        // which is what vim does with a fixed-height window too.
        let id = self.open(Extent::Flexible, focused, dir, window);
        // `open` appends the newcomer as the second child (below / to the
        // right). vim's default is the opposite — `:split` opens ABOVE and
        // `:vsplit` opens LEFT, with `splitbelow`/`splitright` off — so unless
        // the user has turned those on, swap the newcomer into first position.
        if side == NewPane::Before {
            self.layout.swap_with_sibling(id);
        }
        self.focus_pane(id);
        Some(id)
    }

    /// Close the focused pane (vim `<C-w>c` / `:close`).
    ///
    /// Refused for the **last** pane — a workspace with no panes has nothing to
    /// type into and no way back. Returns whether it closed.
    pub(crate) fn close_focused(&mut self) -> bool {
        self.close(self.focused())
    }

    /// Close pane `id`, with [`close_focused`](Self::close_focused)'s refusals
    /// — for a pane that goes because another did (a graph's inspector). It
    /// may be the one you stand in (what you asked for in a companion shows
    /// in its host's window): focus then goes to the first pane left, and a
    /// caller that knows better says where (`App::release_companion`).
    pub(crate) fn close(&mut self, id: PaneId) -> bool {
        if self.is_last_prompt(id) {
            return false;
        }
        if !self.layout.remove(id) {
            return false;
        }
        self.remove_slots(|s| s.id == id);
        self.focus.reconcile(&self.layout);
        true
    }

    /// Is `id` the only prompt pane? Closing it would leave nowhere to type.
    ///
    /// [`ensure_prompt`] would undo such a close on the next reconcile anyway;
    /// refusing here is so `:close` on the prompt bar, or its `:q` beside two
    /// windows, is a *sentence* rather than a frame that flickers and changes
    /// nothing.
    ///
    /// **No "…and other panes remain" clause**, though the first version had
    /// one: quitting must stay allowed, so it looked load-bearing. It is not,
    /// and the mutation run said so — `layout.remove` already refuses the last
    /// pane, and `WindowCmd::Quit` decides to leave on the last window
    /// (`App::last_window`) before it ever reaches a close. Both other paths
    /// agree with this one on every input, so the clause could not change an
    /// outcome. A guard whose deletion nothing notices is a comment, not a
    /// guard.
    ///
    /// [`ensure_prompt`]: Self::ensure_prompt
    fn is_last_prompt(&self, id: PaneId) -> bool {
        self.kind_of(id) == Some(PaneKind::Prompt) && self.panes_of(PaneKind::Prompt).count() == 1
    }

    /// **A workspace always has somewhere to type.** Adds a prompt bar back
    /// across the bottom when no prompt pane is left, and says whether it had
    /// to. `None` means one was already there.
    ///
    /// `close_focused` refuses the LAST pane, and its comment gave the reason:
    /// "a workspace with no panes has nothing to type into and no way back."
    /// The reason was right and the guard was on the wrong quantity — what has
    /// to survive is a way to type, not a nonzero pane count. `:only` standing
    /// in a transcript closed the prompt and left exactly the state that
    /// comment describes: `:new` gives another transcript, `:Send` refuses
    /// because the prompt is empty, and nothing in `:help` gets you out. The
    /// process had to be killed (Steven, dogfooding round 1).
    ///
    /// Re-creating rather than refusing, because refusing `:only` would be the
    /// un-vim answer to a command whose whole meaning is "close the rest".
    /// A floor since 10-26: each prompt window owns its draft and aim, so a
    /// prompt closed and put back here is a draft lost — `only` keeps the one
    /// you type into and `close` refuses the last, so nothing reaches this
    /// (`App`'s `NO_PROMPT`); the bar that comes back is empty, aimed at the
    /// tab's first transcript (`App::reconcile_pane_state`).
    pub(crate) fn ensure_prompt(
        &mut self,
        prompt_height: u16,
        window: impl FnOnce() -> W,
    ) -> Option<PaneId> {
        if self.panes_of(PaneKind::Prompt).next().is_some() {
            return None;
        }
        let bar = prompt_bar(prompt_height);
        let id = PaneId::fresh();
        // `first: false` — the bar goes UNDER everything, where the prompt has
        // always been. Splitting the root rather than a pane keeps that true
        // whatever shape the survivors are in.
        self.layout.split_root(id, SplitDir::Horizontal, bar, false);
        let window = window();
        debug_assert_eq!(
            window.kind(),
            PaneKind::Prompt,
            "a prompt pane shows the prompt"
        );
        self.slots.push(Slot {
            id,
            extent: Extent::Bar { preferred: bar },
            marked: false,
            window,
        });
        self.focus.reconcile(&self.layout);
        Some(id)
    }

    /// Close every pane but `keep` (vim `<C-w>o` / `:only`) — the focused
    /// one, and what the app says goes with it (`App::only_command`: a
    /// window's decision panes) — and the prompt you type into
    /// ([`prompt_pane`](Self::prompt_pane)). Returns how many closed.
    pub(crate) fn only(&mut self, keep: &[PaneId]) -> usize {
        // **And the prompt you type into**, always: its draft is that
        // window's own (`App`'s `PromptView`), so a prompt closed here and
        // put back by `ensure_prompt` was a draft dropped — and a removal
        // that can take the last prompt leaves the app nothing to type into
        // until the owner's reconcile.
        let typing = self.prompt_pane();
        let doomed: Vec<PaneId> = self
            .slots
            .iter()
            .map(|s| s.id)
            .filter(|id| !keep.contains(id) && Some(*id) != typing)
            .collect();
        let mut closed = 0;
        for id in doomed {
            if self.layout.remove(id) {
                self.remove_slots(|s| s.id == id);
                closed += 1;
            }
        }
        self.focus.reconcile(&self.layout);
        closed
    }

    // `hide(kind)` closed every pane of a kind, for a companion whose host
    // kind was gone; a companion names its host since V5.3 (3e), and an
    // orphan is closed by `close` like any pane (`App::reconcile_pane_state`).

    // --- geometry -----------------------------------------------------------

    /// The arrangement **as it fits in `body`**: the retained tree with
    /// transient panes shrunk, and dropped if there is nothing to spare.
    ///
    /// Generalises the old `effective_tool_rows`, which asked the question for
    /// one named pane against one named floor. Here every
    /// [`Transient`](Extent::Transient) pane yields, newest first — the panes
    /// that were already there are what the user was reading — until every
    /// [`Flexible`](Extent::Flexible) pane clears [`MIN_FLEXIBLE_ROWS`]. With
    /// one transient and one flexible pane this is exactly the old arithmetic.
    ///
    /// `None` means "unconstrained": the tree's shape with no size to fit it to.
    fn fit(&self, body: Option<Rect>) -> wm::Layout {
        let mut layout = self.layout.clone();
        let Some(body) = body else { return layout };
        for slot in self.slots.iter().rev() {
            let Some(mut rows) = slot.transient_rows() else {
                continue;
            };
            // A row count says nothing about a column: beside its neighbour,
            // a transient pane takes no rows from it to give back.
            if layout.dir_around(slot.id) == Some(SplitDir::Vertical) {
                continue;
            }
            // Iterate to a fixed point rather than subtracting once: a pane
            // squeezed to zero *saturates*, so "give back d rows" does not
            // always relieve d rows of deficit. `rows` strictly decreases each
            // pass, so this terminates in at most `rows` steps and normally in
            // none at all (the common case exits on the first check).
            loop {
                let deficit = self.flexible_deficit(&layout, body);
                if deficit == 0 {
                    break;
                }
                // Shrink toward the floor first — a smaller decision beats no
                // decision. This used to remove the pane the moment the deficit
                // exceeded its height, so a four-row report in a frame three
                // rows short vanished instead of becoming a one-row report.
                if rows > MIN_TRANSIENT_ROWS {
                    rows = rows.saturating_sub(deficit).max(MIN_TRANSIENT_ROWS);
                    let dir = layout.dir_around(slot.id).unwrap_or(SplitDir::Horizontal);
                    layout.set_cells_around(slot.id, rows.saturating_add(border_along(dir)));
                    continue;
                }
                {
                    // **Take from the bars before giving up on the pane.**
                    // A transient pane is transient because it is urgent; a bar
                    // is a standing size preference. Dropping the urgent thing
                    // to honour the preference is backwards, and it is how a
                    // decision ended up unanswerable behind a full-height
                    // prompt.
                    //
                    // The retry is gated on the deficit ACTUALLY FALLING, not
                    // on `shrink_bars` reporting that it changed a number.
                    // Those are different claims: a bar whose computed rect
                    // does not respond to `set_cells_around` (it is not on the
                    // axis the split runs, say) reports a change every pass and
                    // relieves nothing, and the first version of this hung the
                    // whole test suite on exactly that. Progress is now
                    // measured on the quantity the loop is trying to reduce,
                    // which is what makes termination a fact rather than a hope.
                    if self.shrink_bars(&mut layout, body)
                        && self.flexible_deficit(&layout, body) < deficit
                    {
                        continue;
                    }
                    // Genuinely nothing left anywhere: better to surface the
                    // pane via the status line than to render useless slivers.
                    layout.remove(slot.id);
                    break;
                }
            }
        }
        // **Then the bars give way, while a flexible pane is still short.**
        // This ran only to save a transient pane, so with no decision open a
        // short terminal kept a ten-row prompt and squeezed the conversation
        // to its border (Steven's screenshot, 10-04). Gated, as above, on the
        // deficit actually falling — which is also why it ends.
        loop {
            let deficit = self.flexible_deficit(&layout, body);
            if deficit == 0
                || !(self.shrink_bars(&mut layout, body)
                    && self.flexible_deficit(&layout, body) < deficit)
            {
                break;
            }
        }
        layout
    }

    /// Take rows back from every bar that has more than [`MIN_BAR_ROWS`], up to
    /// what the frame is short by. Returns whether anything moved.
    ///
    /// Called only when a flexible pane is short of its floor and nothing
    /// transient has rows left to give, so a bar keeps the size the user chose
    /// in every frame with room for it. Terminates
    /// because each bar has a floor and each call strictly lowers at least one
    /// bar or returns `false`.
    fn shrink_bars(&self, layout: &mut wm::Layout, body: Rect) -> bool {
        let mut moved = false;
        for slot in &self.slots {
            if !matches!(slot.extent, Extent::Bar { .. }) {
                continue;
            }
            let deficit = self.flexible_deficit(layout, body);
            if deficit == 0 {
                break;
            }
            let rects = layout.compute(body);
            let Some((_, rect)) = rects.iter().find(|(id, _)| *id == slot.id) else {
                continue;
            };
            let have = content_size(*rect).1;
            let give = have.saturating_sub(MIN_BAR_ROWS).min(deficit);
            if give == 0 {
                continue;
            }
            let dir = layout.dir_around(slot.id).unwrap_or(SplitDir::Horizontal);
            layout.set_cells_around(slot.id, (have - give).saturating_add(border_along(dir)));
            moved = true;
        }
        moved
    }

    /// How many rows short the worst-off flexible pane is, in `layout`.
    fn flexible_deficit(&self, layout: &wm::Layout, body: Rect) -> u16 {
        let rects = layout.compute(body);
        self.slots
            .iter()
            .filter(|s| s.extent == Extent::Flexible)
            .filter_map(|s| rects.iter().find(|(id, _)| *id == s.id))
            .map(|(_, rect)| MIN_FLEXIBLE_ROWS.saturating_sub(content_size(*rect).1))
            .max()
            .unwrap_or(0)
    }

    /// Where every pane lands within `body`, in ring order, each tagged with
    /// what it shows and whether it is the focused one.
    pub(crate) fn rects(&self, body: Rect) -> Vec<PaneRect> {
        let focused = self.focused();
        self.fit(Some(body))
            .compute(body)
            .into_iter()
            .enumerate()
            .filter_map(|(i, (id, rect))| {
                self.slot(id).map(|slot| PaneRect {
                    id,
                    kind: slot.kind(),
                    rect,
                    focused: id == focused,
                    // 1-based ring position — the number `{count}<C-w>w` takes
                    // and the border shows. Derived from THIS list rather than
                    // computed separately, so the number you read and the
                    // number you type cannot disagree.
                    number: i + 1,
                })
            })
            .collect()
    }

    /// Which pane a screen position falls in, or `None` for the status row.
    ///
    /// Walks the panes that exist. The hand-written two-rect version is why the
    /// decision pane could not be clicked and why the wheel-scroll arm written
    /// to handle it was unreachable code. Borders count as their pane (clicking
    /// a frame edge focuses it); the tree tiles `body` exactly, so at most one
    /// pane contains a point.
    pub(crate) fn pane_at(&self, frame: Rect, pos: Position) -> Option<PaneId> {
        self.rects(self.body(frame))
            .into_iter()
            .find(|p| p.rect.contains(pos))
            .map(|p| p.id)
    }

    /// **The area the pane tree tiles in `frame`** — the frame less the rows
    /// kept above ([`reserve_top`](Self::reserve_top)) and the status row:
    /// what the draw, a click, a focus move and a resize all measure the
    /// panes by. `App::body_rect` restated it (the tab site list's finding
    /// 7: three statements of "the frame minus the status row").
    pub(crate) fn body(&self, frame: Rect) -> Rect {
        body_of(frame, self.top)
    }

    /// **Keep `rows` above the panes** — the tab line's (`App::sync_tabline`).
    pub(crate) fn reserve_top(&mut self, rows: u16) {
        self.top = rows;
    }

    /// Split a whole frame into panes, the tab line above them (no rows with
    /// one tab) and the status row.
    pub(crate) fn frame_layout(&self, frame: Rect) -> FrameLayout {
        FrameLayout {
            panes: self.rects(self.body(frame)),
            tabline: Rect {
                height: self.top.min(frame.height),
                ..frame
            },
            status: Rect {
                y: frame.y.saturating_add(frame.height.saturating_sub(1)),
                height: frame.height.min(1),
                ..frame
            },
        }
    }

    // --- window commands (`<C-w>…`) -----------------------------------------

    /// Whether a pane's geometry is **the user's to set**.
    ///
    /// A [`Bar`](Extent::Bar) is sized by the user and a
    /// [`Flexible`](Extent::Flexible) pane takes what is left, so `<C-w>`
    /// `+`/`-`/`<`/`>`/`H`/`J`/`K`/`L` mean something for both. A
    /// [`Transient`](Extent::Transient) pane is sized by its *content*: its
    /// `rows` are no size of the user's to move. Resize and relocate make it a
    /// bar first ([`adopt_around`](Self::adopt_around)), so it is never
    /// adjusted AS a row count.
    pub(crate) fn adjustable(&self, id: PaneId) -> bool {
        !matches!(
            self.slot(id).map(|s| s.extent),
            Some(Extent::Transient { .. }) | None
        )
    }

    /// Whether the **divider** around `pane` may be reoriented or moved.
    ///
    /// Stronger than [`adjustable`](Self::adjustable), and it has to be:
    /// reorienting a split changes the axis for *both* children, so a
    /// content-sized pane can have its geometry rewritten by a command aimed at
    /// its neighbour. `<C-w>H` on the transcript reoriented the split it shares
    /// with the decision pane, turning the decision's row count into a column
    /// count — the pane is "not adjustable" and was reshaped anyway, because
    /// the guard asked about the pane instead of about the divider. (Relocate
    /// now adopts a content-sized pane on either side first, so this holds
    /// for anything that reaches it un-adopted.)
    pub(crate) fn divider_adjustable(&mut self, pane: PaneId) -> bool {
        if !self.adjustable(pane) {
            return false;
        }
        match self.layout.sibling_of(pane) {
            Some(sibling) => self.adjustable(sibling),
            // A sibling that is a whole subtree is not one pane to ask about;
            // nothing in it is pinned to this divider's axis today.
            None => true,
        }
    }

    /// Grow/shrink the focused pane along `dir` by `delta` cells — a no-op
    /// unless the split *around it* currently runs along that axis (vim), and a
    /// no-op for a pane that is not [`adjustable`](Self::adjustable).
    ///
    /// Which side of its split the pane is on decides the sign: the two panes
    /// of a split share one divider, so growing one shrinks the other.
    pub(crate) fn resize(&mut self, dir: SplitDir, delta: i32, frame_w: u16, frame_h: u16) {
        let id = self.focused();
        // **The nearest ancestor along `dir`, not the immediate parent.** This
        // asked `dir_around(id) != Some(dir)` and returned — so with three
        // transcripts side by side (a vertical parent), `<C-w>+` (a height
        // change) matched nothing and did nothing, silently. Vim walks up: the
        // height of "this window" is set by the closest horizontal divider
        // above it, which there is the one between the transcripts and the
        // prompt. With no divider on that axis at all there is genuinely
        // nothing to move, and vim is silent about that too.
        self.adopt_around(id, frame_w, frame_h);
        if !self.adjustable(id) {
            return;
        }
        let Some(current) = self.layout.sizing_along(id, dir) else {
            return;
        };
        let is_second = self.layout.is_second_along(id, dir);
        let border = border_along(dir);
        // Move the divider **towards or away from me**. A `Sizing` is written
        // from one end of the divider, so whether `+delta` grows the focused
        // pane depends on which end that is and which side the pane sits on —
        // no frame arithmetic needed, and so nothing to get off by one.
        // The floor is one row/column of *content*, not of outer size.
        let bump = |n: u16, grows_me: bool| -> u16 {
            let d = if grows_me { delta } else { -delta };
            (i32::from(n) + d).clamp(i32::from(border) + 1, i32::from(u16::MAX)) as u16
        };
        let sizing = match current {
            Sizing::SecondCells(n) => Sizing::SecondCells(bump(n, is_second)),
            Sizing::FirstCells(n) => Sizing::FirstCells(bump(n, !is_second)),
            // A proportional split becomes explicit the moment the user sizes
            // it — vim's behaviour, and the only way "grow by three rows" means
            // anything against a percentage. Measured from where the pane
            // actually landed, so the first nudge doesn't jump.
            Sizing::Ratio(_) => {
                let body = self.body(Rect::new(0, 0, frame_w, frame_h));
                let cells = self
                    .fit(Some(body))
                    .compute(body)
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map(|(_, r)| match dir {
                        SplitDir::Horizontal => r.height,
                        SplitDir::Vertical => r.width,
                    })
                    .unwrap_or(border + 1);
                let n = bump(cells, true);
                if is_second {
                    Sizing::SecondCells(n)
                } else {
                    Sizing::FirstCells(n)
                }
            }
        };
        self.layout.set_sizing_along(id, dir, sizing);
        self.adopt_sizes_as_wishes();
        self.clamp(frame_w, frame_h);
        self.remember_decision_place(id);
    }

    /// Relocate the focused pane to an edge (vim `<C-w>H/J/K/L`): reorient the
    /// split around it to the right axis, and put it on the requested side.
    ///
    /// Order is a property of the **tree**, not a `prompt_first: bool` — which
    /// could only ever describe two panes and one of them by name.
    pub(crate) fn relocate(&mut self, dir: Direction, frame_w: u16, frame_h: u16) {
        let id = self.focused();
        // A content-sized pane on either side becomes a bar first: its row
        // count is about to be read along whichever axis the split turns to.
        self.adopt_around(id, frame_w, frame_h);
        if !self.divider_adjustable(id) {
            return;
        }
        let axis = match dir {
            Direction::Up | Direction::Down => SplitDir::Horizontal,
            Direction::Left | Direction::Right => SplitDir::Vertical,
        };
        // Entering a new axis: adopt the size remembered for it (the per-axis
        // memory now lives in the split, so this is just a reorient).
        self.layout.reorient_around(id, axis);
        let want_first = matches!(dir, Direction::Up | Direction::Left);
        if self.layout.is_second_child(id) == want_first {
            self.layout.swap_with_sibling(id);
        }
        self.clamp(frame_w, frame_h);
        self.remember_decision_place(id);
    }

    /// Reset sizes to defaults (vim `<C-w>=`): every bar returns to the size it
    /// declares, every proportional split evens out. Walks the panes; no count.
    ///
    /// **And the decision pane goes back to fitting its content**, a band
    /// below its neighbour, and where the user put it is forgotten: its
    /// declared size IS its content, which the app re-fits it to.
    pub(crate) fn equalize(&mut self, prompt_height: u16, frame_w: u16, frame_h: u16) {
        self.decision_place = None;
        let decisions: Vec<PaneId> = self
            .slots
            .iter()
            .filter(|s| s.kind() == PaneKind::Tool)
            .map(|s| s.id)
            .collect();
        for id in decisions {
            self.layout.reorient_around(id, SplitDir::Horizontal);
            if !self.layout.is_second_child(id) {
                self.layout.swap_with_sibling(id);
            }
            // Its floor until the app re-fits it to what it holds — the
            // workspace never knows the content (`App::fit_tool_pane`).
            self.resize_to(
                id,
                Extent::Transient {
                    rows: MIN_TRANSIENT_ROWS,
                },
            );
        }
        self.layout.equalize_ratios();
        let bars: Vec<(PaneId, AxisSizing)> = self
            .slots
            .iter()
            .filter_map(|slot| match &slot.extent {
                Extent::Bar { preferred } if slot.kind() != PaneKind::Prompt => {
                    Some((slot.id, *preferred))
                }
                _ => None,
            })
            .collect();
        for (id, preferred) in bars {
            let sizing = self.oriented(id, preferred);
            self.layout.set_axis_sizing_around(id, sizing);
        }
        self.set_prompt_height(prompt_height, frame_w, frame_h);
    }

    /// **The prompt takes `prompt_height` now**, and nothing else moves.
    ///
    /// The prompt's declared height is a live config value; its width goes
    /// back to "no size asked for" (proportional). `<C-w>=` applies it along
    /// with every other size, and a config change applies it ALONE: it used
    /// to wait for the next `<C-w>=`, so `:reload` said "changed:
    /// tui.prompt.height" and the prompt stayed exactly as it was (Steven,
    /// with a height of 50) — as vim's `:set cmdheight` applies at once.
    pub(crate) fn set_prompt_height(&mut self, prompt_height: u16, frame_w: u16, frame_h: u16) {
        let bars: Vec<(PaneId, AxisSizing)> = self
            .slots
            .iter_mut()
            .filter_map(|slot| {
                let kind = slot.kind();
                match (&mut slot.extent, kind) {
                    (Extent::Bar { preferred }, PaneKind::Prompt) => {
                        *preferred = prompt_bar(prompt_height);
                        Some((slot.id, *preferred))
                    }
                    _ => None,
                }
            })
            .collect();
        for (id, preferred) in bars {
            let sizing = self.oriented(id, preferred);
            self.layout.set_axis_sizing_around(id, sizing);
        }
        self.clamp(frame_w, frame_h);
    }

    /// A bar's remembered size, **oriented for the side of the divider it is on
    /// now**.
    ///
    /// `preferred` is stored in the `SecondCells` convention, but a bar
    /// relocated with `<C-w>K`/`H` becomes a FIRST child, and
    /// `swap_with_sibling` mirrors the *tree's* sizing without touching the
    /// slot's. Writing it back verbatim gave the **sibling** the bar's size:
    /// `<C-w>K` then `<C-w>=` left a 35-row prompt over a 10-row transcript.
    ///
    /// Shared by [`equalize`](Self::equalize) and [`clamp`](Self::clamp)
    /// because both turn a wish into a tree sizing, and for a while only one of
    /// them knew this.
    fn oriented(&mut self, id: PaneId, preferred: AxisSizing) -> AxisSizing {
        if self.layout.is_second_child(id) {
            preferred
        } else {
            AxisSizing {
                horizontal: preferred.horizontal.mirrored(),
                vertical: preferred.vertical.mirrored(),
            }
        }
    }

    /// After a **deliberate** resize, every bar's current size becomes its wish.
    ///
    /// Not just the focused pane's: a `Sizing` is written from one end of a
    /// divider, so growing the transcript is the same act as shrinking the
    /// prompt, and only one of the two panes is focused. Recording the wish on
    /// the focused pane alone left the prompt's old wish in place, and
    /// [`clamp`](Self::clamp) — which re-derives the live size from the wish —
    /// promptly undid the resize.
    ///
    /// Reading the *live* sizing here is the point: it is unclamped at this
    /// moment, so a wish larger than the frame is recorded as asked for. And
    /// if the frame is currently small, adopting the squeezed sizes is right
    /// too — the user is arranging **this** frame, and that is what they said
    /// they want.
    fn adopt_sizes_as_wishes(&mut self) {
        let bars: Vec<PaneId> = self
            .slots
            .iter()
            .filter(|s| matches!(s.extent, Extent::Bar { .. }))
            .map(|s| s.id)
            .collect();
        for id in bars {
            let (Some(dir), Some(live)) =
                (self.layout.dir_around(id), self.layout.sizing_around(id))
            else {
                continue;
            };
            // Back into the storage convention, undoing `oriented`.
            let wish = if self.layout.is_second_child(id) {
                live
            } else {
                live.mirrored()
            };
            if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id)
                && let Extent::Bar { preferred } = &mut slot.extent
            {
                match dir {
                    SplitDir::Horizontal => preferred.horizontal = wish,
                    SplitDir::Vertical => preferred.vertical = wish,
                }
            }
        }
    }

    /// Keep every bar inside the frame, leaving room for the panes it shares
    /// with, so a resize can't hide the conversation. Walks the panes.
    pub(crate) fn clamp(&mut self, frame_w: u16, frame_h: u16) {
        let bars: Vec<(PaneId, AxisSizing)> = self
            .slots
            .iter()
            .filter_map(|s| match s.extent {
                Extent::Bar { preferred } => Some((s.id, preferred)),
                _ => None,
            })
            .collect();
        for (id, preferred) in bars {
            let Some(dir) = self.layout.dir_around(id) else {
                continue;
            };
            let (frame_dim, reserve) = match dir {
                // A row of status, the tab line's, a sliver of the neighbour,
                // and both borders.
                SplitDir::Horizontal => (
                    frame_h,
                    1 + self.top + (1 + BORDER_SHAPE.0) + BORDER_SHAPE.0,
                ),
                SplitDir::Vertical => (frame_w, (1 + BORDER_SHAPE.1) + BORDER_SHAPE.1),
            };
            let border = border_along(dir);
            // Cells here are OUTER size, so the floor is one content cell plus
            // the border, and the ceiling leaves the neighbour its sliver.
            let lo = border + 1;
            let hi = frame_dim
                .saturating_sub(reserve)
                .saturating_add(border)
                .max(lo);
            // **From the WISH, not from the live value.** Clamping the live
            // value writes the squeeze back over the size the user asked for,
            // so a shrink is permanent: Steven squished a ghostty split, the
            // prompt fell to one line, and growing the terminal again left it
            // there — there was nothing left to grow back to (2026-08-24). The
            // wish is untouched by any frame, so every frame can re-derive
            // "what fits" from it and a squeeze lasts exactly as long as the
            // small terminal does.
            let wish = self.oriented(id, preferred).along(dir);
            // A bar that asked for no particular size on this axis (the
            // prompt's width is proportional) has no wish to restore — clamp
            // whatever the tree holds, exactly as before.
            let sizing = match wish {
                Sizing::Ratio(_) => self.layout.sizing_around(id),
                cells => Some(cells),
            };
            if let Some(sizing) = sizing {
                let clamped = match sizing {
                    Sizing::FirstCells(n) => Sizing::FirstCells(n.clamp(lo, hi)),
                    Sizing::SecondCells(n) => Sizing::SecondCells(n.clamp(lo, hi)),
                    ratio => ratio,
                };
                self.layout.set_sizing_around(id, clamped);
            }
        }
    }
}

/// A pane's border contribution along `dir` — the difference between its outer
/// size (what [`Sizing`] counts) and its content.
fn border_along(dir: SplitDir) -> u16 {
    match dir {
        SplitDir::Horizontal => BORDER_SHAPE.0,
        SplitDir::Vertical => BORDER_SHAPE.1,
    }
}

/// The `Sizing` a slot's extent asks for **along `dir`**, the axis it is
/// split along.
///
/// `dir` is a parameter and not an assumption: a `Bar` remembers a size per
/// axis, and a `Transient` pane's `rows` is a row count that means nothing as a
/// column count. Reading `preferred.horizontal` unconditionally was correct
/// only because the one bar in existence is built in `new()` and never routed
/// through here — an n=1 shortcut wearing a general signature.
fn sizing_for(extent: Extent, dir: SplitDir, second: bool) -> Sizing {
    let cells = |n: u16| {
        let n = n.saturating_add(border_along(dir));
        if second {
            Sizing::SecondCells(n)
        } else {
            Sizing::FirstCells(n)
        }
    };
    match extent {
        Extent::Transient { rows } => cells(rows),
        Extent::Bar { preferred } => preferred.along(dir),
        Extent::Flexible => Sizing::Ratio(50),
    }
}

/// The keymap's direction as the window manager's. Two enums because the keymap
/// is about keys and the wm is about geometry; this is the one seam.
fn wm_dir(dir: Direction) -> wm::Direction {
    match dir {
        Direction::Left => wm::Direction::Left,
        Direction::Right => wm::Direction::Right,
        Direction::Up => wm::Direction::Up,
        Direction::Down => wm::Direction::Down,
    }
}

#[cfg(test)]
mod tests;
