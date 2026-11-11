//! **A pane's window** — what it shows ([`View`]) and what it showed before
//! ([`Away`], its history), the other half of the buffer/window split
//! (`docs/DesignDocs/buffers.md`; V5.3, 10-25, a module of its own).
//!
//! A buffer holds what every window on it must agree on; a window holds what
//! each is free to disagree about — where it is in what it shows, and what it
//! showed. It lives in its pane's workspace slot (`Workspace<Window>`), born
//! with the pane and gone with it, and its kind IS the pane's
//! (`workspace::Pane`).

use super::decisions;
use crate::graph_view::GraphView;
use crate::prompt::Buffer;
use crate::wm::BufferId;
use crate::workspace::PaneKind;

/// Where **one window** is looking at its buffer — and, for a viewer, what it
/// is looking at.
///
/// The other half of the buffer/window split (`docs/DesignDocs/buffers.md`): a
/// buffer holds what every window must agree on, and this holds what they are
/// each free to disagree about. One per pane, held in the pane's slot
/// (V5.3 (4)), so two panes on the same session keep their own cursors —
/// which is the point, and was impossible while this lived as one field on
/// `App`. What it shows is a window ON a buffer (a session, a page, a tree)
/// or an instrument that is no buffer's (the prompt, a decision, a
/// companion; [`View::buffer`]).
///
/// Everything a pane displays lives here, not on `App`. That is the rule, and
/// it is the one that keeps being learned the hard way: a viewer held as a
/// single `Option` on the app is *the* viewer, so two panes showing it share a
/// scroll position, "open it" has to mean "focus the one that exists", and
/// closing one closes it everywhere. Reading two chapters side by side is
/// ordinary — we do it ourselves — and it was impossible for exactly this
/// reason.
///
/// No `Default` (V5.3 (3d)): a window is born showing something
/// ([`on`](Self::on), [`showing`](Self::showing)). `PaneView::default()` (its
/// name until V5.3 (5)) stood in for a window with no view yet, at each
/// pane's birth, until the content came by a second call.
#[derive(Debug)]
pub(crate) struct Window {
    /// **What the window shows, as one value** ([`View`]) — the prompt, a
    /// session, a tree, a page, the inspector, the preview or a decision —
    /// and so what kind of pane it is ([`View::kind`]).
    pub(crate) view: View,
    /// **What this window showed before what it shows** — its history,
    /// newest last, each a buffer and where the window was in it ([`Away`]),
    /// for `:q` to give back (`App::reclaim_viewport`): the views, and the
    /// sessions — the newest, under a view, the window's own, beneath it
    /// ([`Window::session`]).
    ///
    /// vim's rule, which Steven named (10-16, `:messages` from `:graph`):
    /// a view opens IN the pane you are in, from every pane — the graph's too.
    /// The graph was the exception, opening across the top, because its marks,
    /// folds and inspector were retired with it and a pane gave back only a
    /// transcript (`:help graph-keys`, 10-14). Kept here instead, nothing
    /// needs placing elsewhere to survive. **A buffer in it once**, the
    /// latest ([`set_aside`](Self::set_aside)), as Emacs's
    /// `window-prev-buffers`: reopening one supersedes its older place, so
    /// going back is never longer than the buffers the window showed. It
    /// kept one of each KIND until V5.4 (2), so `:b` to a second tree
    /// dropped the first and `<C-^>` could not return to it (V5.3 verify,
    /// F1) — moot for views since the tree is one (10-26), a page one of
    /// each kind; true still of sessions, many of one kind.
    ///
    /// **One history, here** (V4b, 10-25): the views were `under`, and the
    /// transcript beneath them a `lent` bit on the workspace's slot — two
    /// halves kept by the same doors in two homes, and unable to say in
    /// which order a transcript and the views around it came.
    pub(super) history: Vec<Away>,
}

/// **What a window shows** (V5.3 (3b)) — one value, so a window holding a
/// page and a tree at once, or an inspector's page beside a decision, is
/// not a state anything can write. They were one field each on the
/// window, `Option`s that any two could fill, and kind and content were
/// set by different calls ("`:open` restored the kind and left the help
/// page sitting there"). The large ones are boxed: a window is held in
/// its pane's slot, and the slots are scanned by id.
///
/// **Its kind is the pane's** (V5.3 (3d), [`View::kind`], which the layout
/// asks through `workspace::Pane`): a pane's kind and its content were set
/// by two calls, and nothing now sets one without the other. No `Default`:
/// every window is born showing something, and nothing stands in for a
/// view not there yet.
#[derive(Debug)]
pub(crate) enum View {
    /// **A prompt** — this window's own draft and where it is going
    /// ([`PromptView`]). It rendered the app's one composition, every prompt
    /// window a view of it, until two prompts shared one aim and Steven's
    /// changed both ("Both change when I change my aim where only the last
    /// should", 10-26).
    Prompt(Box<PromptView>),
    /// **A transcript** — a window on a session ([`SessionView`]).
    Session(Box<SessionView>),
    /// The session tree.
    Tree(Box<GraphView>),
    /// **A window on a page buffer** — the manual, `:analyze`'s report, the
    /// notice history, or a listing (`:ls`, `:alias`, `:rules`,
    /// `:scriptnames`, `:themes`; `Buffer::Page`). Its own copy of the page,
    /// at its own position, so two `:help` panes are two readers and not
    /// one; where you were when it closes is the buffer's
    /// (`PageBuffer::left_at`).
    ///
    /// A listing's lines and settings were the pane's until V5.1 — a
    /// `TextView` of its own, `:ls!`'s bang and `:themes`' list beside it
    /// here — so two `:ls` panes derived it twice, and `:b N` could name none.
    Page(Box<crate::page::PageWindow>),
    /// The graph's inspector ([`InspectorView`]).
    Inspector(Box<InspectorView>),
    /// `:themes`' preview, beside the list it follows.
    Preview {
        /// **The list it follows** — the `:themes` pane it describes, named
        /// (V5.3 (3e)) as the inspector names its tree.
        host: crate::wm::PaneId,
        /// What the theme under the list's cursor looks like — the theme
        /// loaded once per name, its page rebuilt when the name or the width
        /// changes; `None` until the first frame loads one.
        preview: Option<Box<crate::theme_picker::Preview>>,
    },
    /// A decision pane ([`DecisionView`]).
    Decision(DecisionView),
}

impl View {
    /// **The kind of pane this is** — the one answer, read by the layout
    /// (`workspace::Pane`) and by everything asking `kind_of`. Exhaustive
    /// over the content, so new content is a kind decided here.
    pub(crate) fn kind(&self) -> PaneKind {
        match self {
            View::Prompt(_) => PaneKind::Prompt,
            View::Session(_) => PaneKind::Output,
            View::Tree(_) => PaneKind::Graph,
            View::Page(window) => window.kind(),
            View::Inspector(_) => PaneKind::Inspector,
            View::Preview { .. } => PaneKind::ThemePreview,
            View::Decision(_) => PaneKind::Tool,
        }
    }

    /// **The buffer it is a window on** — a session's, a page's, a tree's
    /// (V5.4 (2)); `None` for an instrument — the prompt, a decision, a
    /// companion — which is no buffer's. Every view of content is born on
    /// its buffer, a tree too since review stop 10 (C 3). The answer to "does
    /// this window show buffer X", asked per kind at eleven sites until V5.4
    /// (2) (the design review's count, M5). Asked now (`rg
    /// 'Window::buffer|\.buffer\(\)'`, 10-25): here by [`Window::holds`] (so
    /// `App::panes_holding`, under `panes_with_session` and `panes_on`),
    /// [`Window::shows_as`] (`App::show_in`) and the history's supersede; in
    /// `app.rs` by `pane_shows` (so `show_view_buffer`), `enter_session`,
    /// `listing_view` and `renew_listings`; `split_command`'s page copy asks
    /// it of the place (`Viewed::buffer`, stop 10). Not by
    /// `settle_tree_marks`, which needs the tree, not only its number.
    pub(crate) fn buffer(&self) -> Option<BufferId> {
        match self {
            View::Session(session) => Some(session.buffer),
            View::Page(window) => Some(window.buffer()),
            View::Tree(tree) => Some(tree.tree()),
            View::Prompt(_) | View::Inspector(_) | View::Preview { .. } | View::Decision(_) => None,
        }
    }
}

/// **A prompt, as a window holds it** ([`View::Prompt`]) — its own draft
/// and its own aim (Steven, 10-26: "each prompt pane has its own draft and
/// its own aim"). A tab's prompt windows are its own, so a draft is the
/// tab's it was typed in; parked with it. Which one you type into is the
/// workspace's question (`Workspace::prompt_pane`), and the app reads and
/// writes that one (`App::prompt`, `App::prompt_target`).
#[derive(Debug)]
pub(crate) struct PromptView {
    /// What is being written here — its text, cursor, undo; the register is
    /// every draft's ([`crate::prompt::Registers`]).
    pub(crate) draft: crate::prompt::Prompt,
    /// **The session it commits to** — set at its birth (where the window
    /// it was split from went, its tab's session) and moved by standing in
    /// a transcript while this is the prompt you used last, or by `:aim`.
    /// Never "follow the foreground": a prompt that followed it was aimed,
    /// in a parked tab, at whatever the tab you are in had shown (Q2's
    /// `pin_aim`, which a concrete aim made needless).
    pub(crate) aim: BufferId,
}

impl PromptView {
    /// An empty draft sharing `registers`, aimed at `aim`.
    pub(crate) fn new(registers: &crate::prompt::Registers, aim: BufferId) -> Self {
        PromptView {
            draft: crate::prompt::Prompt::sharing(registers),
            aim,
        }
    }

    /// **Does it hold a draft** — text that is not only blanks: what a close
    /// of its window would drop (`held::Held`).
    pub(crate) fn has_draft(&self) -> bool {
        !self.draft.text().trim().is_empty()
    }
}

/// **How a window holds a buffer** ([`Window::holds`]) — the three answers
/// its callers need apart (V5.4 design review, M2): what is SHOWN; the
/// session BENEATH its views, the window's own (`Window::session`); and
/// one set ASIDE, which `:b N` and `<C-^>` bring back but which is no longer
/// the window's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holding {
    Shown,
    Beneath,
    Aside,
}

impl crate::workspace::Pane for Window {
    fn kind(&self) -> PaneKind {
        self.view.kind()
    }
}

/// **A window on a session** ([`View::Session`]) — which session, and where
/// the window is in it. Bound from birth (V5.3 (2)), and since (3c) by its
/// type: the binding was the workspace slot's, kept under a view so a
/// covered transcript knew its session, beside the window's own cursor —
/// one window's session in two homes.
#[derive(Debug)]
pub(crate) struct SessionView {
    /// The session it shows.
    pub(crate) buffer: BufferId,
    /// This window's read-only navigation cursor — a [`Buffer`] mirroring
    /// the transcript's lines, carrying the cursor and any selection.
    /// Re-synced from the buffer's `Output` when this pane is focused; when
    /// it is not, the pane renders the streaming transcript and the cursor
    /// merely waits. A window that sets its session aside keeps where it
    /// was ([`Place`]), not the lines: they are copied in again when it
    /// comes back (`App::put_place`).
    pub(crate) chat: Buffer,
    /// What `chat` copied of the transcript, block by block — so a re-sync
    /// copies only what changed since (`App::sync_chat`, which copies whole a
    /// `chat` that does not hold what this says: reset, or written by
    /// another hand).
    pub(crate) synced: crate::output::Synced,
    /// **This window's scroll position.** Beside the cursor, because it is the
    /// same kind of thing: `buffers.md` §2 says a window owns "cursor, scroll,
    /// selection", and P2 moved the cursor and stopped. Until 2026-08-07 the
    /// viewport lived on `Output` — per *buffer* — so two windows on one
    /// session paged together, and ordinary `j`/`k` in one dragged its twin.
    /// That single missing degree of freedom is what made a split of one
    /// session read as "the panes are welded" (Reviewer F).
    ///
    /// The wrap cache stays on `Output`, correctly: how text wraps at a width
    /// is a property of the content, and sharing it is the point.
    pub(crate) viewport: crate::viewport::Viewport,
    /// **This window's folds** (N5a) — beside its scroll, for the same
    /// reason: vim's folds are the window's, so two windows on one session
    /// fold apart. Every question about its rows asks them
    /// (`App::pane_shut`), so what it draws and where a key lands agree.
    pub(crate) folds: crate::output::Folds,
    /// **The size it was last drawn at**, inside its border — noted as it is
    /// drawn (`App::note_transcript_sizes`), `None` until then: the width
    /// what it draws shut is cut at (`Output::shut_lines`), and how a new
    /// width is told. Not where its top is: that is text (`Viewport`), and
    /// its row is read at whatever width the window has.
    pub(crate) drawn: Option<(u16, u16)>,
}

impl SessionView {
    /// A window on `buffer` that never was in it: at the start, following
    /// the end, its lines not copied yet, every block as its kind's rule
    /// says.
    pub(crate) fn new(buffer: BufferId) -> Self {
        SessionView {
            buffer,
            chat: Buffer::default(),
            synced: crate::output::Synced::default(),
            viewport: crate::viewport::Viewport::default(),
            folds: crate::output::Folds::default(),
            drawn: None,
        }
    }

    /// Where it is — its cursor, scroll and folds.
    pub(super) fn place(&self) -> Place {
        Place {
            cursor: self.chat.cursor(),
            viewport: self.viewport,
            folds: self.folds.clone(),
        }
    }
}

/// **The graph's inspector, as a window shows it** ([`View::Inspector`]).
#[derive(Debug)]
pub(crate) struct InspectorView {
    /// **The tree it inspects** — the graph pane it describes, named (V5.3
    /// (3e), review finding 7). It was paired by ring proximity: the graph
    /// pane nearest it in ring order, so with two tree windows open an
    /// inspector left alone adopted the other one (two inspectors on one), and
    /// a window drawn off the tiling — a float — is in no ring at all.
    pub(crate) host: crate::wm::PaneId,
    /// **Which row the inspector is showing** — the identity, not the content.
    ///
    /// Two different questions were being answered by one comparison. "Is the
    /// content stale?" is answered by comparing the rendered lines. "Is this a
    /// different page, so the reader should start at the top?" is answered
    /// here. Collapsing them meant a background job finishing — which rewrites
    /// the activity row with no selection change — scrolled a reader back to
    /// line 0 (Reviewer A).
    ///
    /// A row, not a node: every Claude Code row is no node, so by the node
    /// one turn's page was the next one's, and the reader stayed scrolled
    /// down a page that had changed under them.
    pub(crate) row: Option<crate::graph_view::Entry>,
    /// The inspector body, `None` until the first frame derives one.
    ///
    /// **A window onto derived content, not a copy of it.** The lines are a
    /// function of the neighbouring graph pane's selection, which changes under
    /// this pane every time somebody presses `j` over there — so what is stored
    /// is where *you* are in the page, and the content is re-derived every frame
    /// by `App::refresh_inspector` and replaced only when it actually differs.
    /// A field assigned once from a source that changes at runtime is stale by
    /// construction; this one cannot be, because nothing ever decides it is
    /// still fresh.
    pub(crate) text: Option<crate::text_view::TextView>,
    /// **A message's painted words, kept** — the one part of the inspector's
    /// page not re-derived each frame ([`crate::view::PaintedMessage`]): its
    /// styling runs syntect, ~120 ms over a long reply, and was costing the
    /// whole app its frame rate (review of 34033e8, Reviewer B, measured).
    pub(crate) painted: crate::view::PaintedMessage,
}

/// **A decision pane, as a window shows it** ([`View::Decision`]) — its
/// call or review is the session's (`App::decisions`); this is whose it is
/// and how it was last drawn.
#[derive(Debug)]
pub(crate) struct DecisionView {
    /// **The session this decision pane decides for, and the window it sits
    /// under** (its host) — given when it opens (`open_decision`), and the
    /// one answer to "whose call is in this pane" (`App::decides_for`).
    pub(crate) decides: (BufferId, crate::wm::PaneId),
    /// **What it shows, by identity** — recorded when it came there
    /// (`settle_decisions`, `:decide`), so what is new on it is fitted and
    /// armed once, however many doors put it there (review stop 8, D 5), and
    /// a close knows whether it closed over a review (`reconcile_pane_state`).
    pub(crate) shows: Option<decisions::Shown>,
    /// **The width it was last drawn at**, in cells — what its report is
    /// measured at when it is fitted (`content_rows` counts rendered rows).
    /// `0` until drawn. It was one app-wide width, read from the first
    /// decision pane, so a second at another width would be sized against
    /// the first (design review DR-A #2).
    pub(crate) drawn_width: u16,
    /// **The rows it was last drawn with**, inside its border — `Some(0)` a
    /// frame with no room for it, `None` never framed. A call coming into
    /// view by ROOM (`:only`, `<C-w>_`, a resize) is new on screen as much as
    /// a call shown: its keys wait again when it goes from no rows to some
    /// (`clamp_text_panes`), and a key that would answer it before any frame
    /// drew it is refused — a burst of keys is handled before the draw, so
    /// `:only<CR>a` allowed a call no frame had shown (review stop 8, E1 2).
    pub(crate) drawn_rows: Option<u16>,
}

impl DecisionView {
    /// A decision pane deciding for `buffer`, under `host`, not yet drawn.
    pub(crate) fn new(buffer: BufferId, host: crate::wm::PaneId) -> Self {
        DecisionView {
            decides: (buffer, host),
            shows: None,
            drawn_width: 0,
            drawn_rows: None,
        }
    }
}

/// **One thing a window showed before what it shows** — an entry of its
/// history ([`Window::history`]): a buffer and where the window was in
/// it, as Emacs's `window-prev-buffers` keeps one (V5.3 design review,
/// finding 4). What a view held beyond its place was built again when it
/// came back anyway; kept whole, every window's history held a graph.
#[derive(Debug)]
pub(super) enum Away {
    /// A view — a page or a tree — and where the window was in it.
    View(Viewed),
    /// **A session this window set aside**, with where it was in it — left
    /// for another (Steven, 10-25, V4b Q1–Q2: vim's `:b` keeps what it
    /// leaves, and going back puts the window where it was), or covered by
    /// a view. The NEWEST is the window's own while a view shows
    /// ([`Window::session`]): what reads a covered transcript's session or
    /// scroll reads it there (`pane_session`, `aim_viewport`, a landing's
    /// window). It was its own entry, `Beneath`, the place on the window
    /// and the session on the slot; always after the sessions left, so the
    /// newest was it (V5.3 design review, Q5, held by construction), and
    /// one entry says both (V5.3 (3c)).
    Session(BufferId, Place),
}

impl Away {
    /// **The buffer it is a place in** — a session's, a page's, a tree's:
    /// what the history keeps once ([`Window::set_aside`]) and what `:b N`
    /// finds a held one by ([`Window::take`]).
    pub(super) fn buffer(&self) -> BufferId {
        match self {
            Away::Session(buffer, _) => *buffer,
            Away::View(view) => view.buffer(),
        }
    }

    /// **Is it a place in what `opening` shows** — the same buffer, the one
    /// statement of "the same thing again" for the history's supersede
    /// ([`Window::set_aside`]), as [`Window::shows_as`] is for a view opened
    /// where it is shown. A test's tree with no buffer once made it a pair
    /// of (buffer, kind), compared by kind where neither had one (review
    /// stop 10, C 3: that arm was dead even in tests).
    fn is_of(&self, opening: &View) -> bool {
        opening.buffer() == Some(self.buffer())
    }
}

/// **A view a window set aside** ([`Away::View`]): a buffer that is not a
/// session, and where the window was in it.
#[derive(Debug)]
pub(super) enum Viewed {
    /// **A page**, this window's window on it as it was left — brought up
    /// to date with the page when it comes back, as every window on a page
    /// is each frame (`PageBuffer::sync`).
    Page(Box<crate::page::PageWindow>),
    /// **A tree**, as what the window held in it and its Claude Code
    /// section — built again on the latest state when it comes back
    /// (`App::view_back`).
    Tree(Box<crate::graph_view::TreePlace>),
}

impl Viewed {
    /// The kind it shows as when it comes back.
    pub(super) fn kind(&self) -> PaneKind {
        match self {
            Viewed::Page(window) => window.kind(),
            Viewed::Tree(_) => PaneKind::Graph,
        }
    }

    /// The buffer it is a window on.
    pub(super) fn buffer(&self) -> BufferId {
        match self {
            Viewed::Page(window) => window.buffer(),
            Viewed::Tree(place) => place.tree(),
        }
    }
}

/// **Where a window was in a session** — its cursor and scroll, kept while
/// it shows another ([`Away::Session`]). Not the lines: the transcript's
/// own, copied in again when it comes back (`App::put_place`). A window with
/// no place of its own in a session takes the session's ENTRY
/// (`SessionBuffer::entry`), asked of the buffer. No `Default`, so a place
/// made from nothing is named, and `rg 'at_the_end'` finds each: the
/// type does not forbid a door its own (the fields are this module's, and
/// `SessionView::new` is a window at its end), so that count, and the entry
/// mutant turning each door's test red, are what hold it (review of the
/// 10-26 answers, L4).
#[derive(Debug, Clone)]
pub(super) struct Place {
    pub(super) cursor: (usize, usize),
    pub(super) viewport: crate::viewport::Viewport,
    /// **Its folds there** (N5a): a window that shows another session and
    /// comes back finds them as it left them, and a `:split` starts with
    /// them, opened and shut and `zR`'s alike — as vim keeps a window's
    /// folds per buffer and its `:split` copies them (measured in Neovim
    /// 0.12.6, 11-01).
    pub(super) folds: crate::output::Folds,
}

impl Place {
    /// **At the session's end, following it**: the cursor at the top and the
    /// window following the tail, as a window born on it is
    /// (`SessionView::new`). What [`SessionBuffer::entry`] answers today —
    /// a door asks that, not this (`rg 'at_the_end'`: it and `App::entry_of`'s
    /// fallback for a buffer that is no session).
    ///
    /// [`SessionBuffer::entry`]: super::buffers::SessionBuffer::entry
    pub(super) fn at_the_end() -> Place {
        Place {
            cursor: (0, 0),
            viewport: crate::viewport::Viewport::default(),
            folds: crate::output::Folds::default(),
        }
    }
}

impl Window {
    /// **A window born on session `buffer`** — at its end, following it:
    /// every transcript pane is born on its session (V5.3 (2)). A door that
    /// births one beside others puts the session's entry after
    /// (`App::entry_of`), the lines in by then.
    pub(crate) fn on(buffer: BufferId) -> Self {
        Self::showing(View::Session(Box::new(SessionView::new(buffer))))
    }

    /// **A prompt window born empty, aimed at `aim`** — its draft sharing
    /// `registers` ([`PromptView::new`]).
    pub(crate) fn prompt(registers: &crate::prompt::Registers, aim: BufferId) -> Self {
        Self::showing(View::Prompt(Box::new(PromptView::new(registers, aim))))
    }

    /// A window born showing `view`, with no history.
    pub(crate) fn showing(view: View) -> Self {
        Window {
            view,
            history: Vec::new(),
        }
    }

    /// **Where this window is in what it shows, as its history keeps it** —
    /// the one table for each kind's content: read when a window sets what
    /// it shows aside ([`set_aside`](Self::set_aside)), and when a split's
    /// copy starts where the window is (`App::split_command`), each brought
    /// back through the same doors (`App::view_back`, `App::put_place`).
    /// The split copied the three by hand and built its tree by the door a
    /// new one takes, a Claude Code listing read and thrown away (review
    /// stop 10, D 1). `None` for what a history does not keep: a companion,
    /// which describes its host and is re-derived from it — set aside, it
    /// came back as a second inspector beside a tree that had brought its
    /// own (review stop 3, A F4) — and the prompt and a decision, which are
    /// never set aside (`PaneKind::dedicated`).
    ///
    /// Taken by reference, so a page's window is copied, its lines with it
    /// (the manual's ~4000): at a command's rate, never a frame's.
    ///
    /// Exhaustive over [`View`], so content that gains a variant is a
    /// compile error here rather than a view that outlives the pane's use
    /// of it; and what a history can hold is [`Viewed`]'s two and a
    /// session's place, so keeping a new one is a variant added there,
    /// never an arm written here by habit.
    pub(super) fn place(&self) -> Option<Away> {
        match &self.view {
            // Where you were reading, not the lines: a viewer BORROWS the
            // window, and giving it back puts you where you were reading
            // (`App::put_place`), not merely in what you were reading.
            View::Session(session) => Some(Away::Session(session.buffer, session.place())),
            View::Tree(tree) => Some(Away::View(Viewed::Tree(Box::new(tree.place())))),
            // Every page — the listings' since V5.1, their settings the
            // buffer's, not a window's to carry.
            View::Page(window) => Some(Away::View(Viewed::Page(window.clone()))),
            // The inspector's CONTENT belongs to the graph next door, and
            // where this pane is in it goes with it; the preview is the
            // list's. The decision pane is answered rather than dropped, and
            // a prompt's draft is never set aside (`PaneKind::dedicated`).
            View::Prompt(_) | View::Inspector(_) | View::Preview { .. } | View::Decision(_) => None,
        }
    }

    /// The tree this window shows, if it shows one.
    pub(crate) fn tree(&self) -> Option<&GraphView> {
        match &self.view {
            View::Tree(tree) => Some(tree),
            _ => None,
        }
    }

    /// [`tree`](Self::tree), to change.
    pub(crate) fn tree_mut(&mut self) -> Option<&mut GraphView> {
        match &mut self.view {
            View::Tree(tree) => Some(tree),
            _ => None,
        }
    }

    /// The page window this window shows, if it shows one.
    pub(crate) fn page(&self) -> Option<&crate::page::PageWindow> {
        match &self.view {
            View::Page(window) => Some(window),
            _ => None,
        }
    }

    /// [`page`](Self::page), to change.
    pub(crate) fn page_mut(&mut self) -> Option<&mut crate::page::PageWindow> {
        match &mut self.view {
            View::Page(window) => Some(window),
            _ => None,
        }
    }

    /// The inspector this window shows, if it shows one.
    pub(crate) fn inspector(&self) -> Option<&InspectorView> {
        match &self.view {
            View::Inspector(inspector) => Some(inspector),
            _ => None,
        }
    }

    /// [`inspector`](Self::inspector), to change.
    pub(crate) fn inspector_mut(&mut self) -> Option<&mut InspectorView> {
        match &mut self.view {
            View::Inspector(inspector) => Some(inspector),
            _ => None,
        }
    }

    /// The theme this window previews, once one is loaded.
    pub(crate) fn preview(&self) -> Option<&crate::theme_picker::Preview> {
        match &self.view {
            View::Preview { preview, .. } => preview.as_deref(),
            _ => None,
        }
    }

    /// [`preview`](Self::preview), to change.
    pub(crate) fn preview_mut(&mut self) -> Option<&mut crate::theme_picker::Preview> {
        match &mut self.view {
            View::Preview { preview, .. } => preview.as_deref_mut(),
            _ => None,
        }
    }

    /// **The pane this companion describes** — the tree an inspector
    /// inspects, the list a preview follows; `None` for every other window.
    pub(crate) fn companion_host(&self) -> Option<crate::wm::PaneId> {
        match &self.view {
            View::Inspector(inspector) => Some(inspector.host),
            View::Preview { host, .. } => Some(*host),
            _ => None,
        }
    }

    /// The prompt this window is, if it is one — its draft and aim.
    pub(crate) fn prompt_view(&self) -> Option<&PromptView> {
        match &self.view {
            View::Prompt(prompt) => Some(prompt),
            _ => None,
        }
    }

    /// [`prompt_view`](Self::prompt_view), to change.
    pub(crate) fn prompt_view_mut(&mut self) -> Option<&mut PromptView> {
        match &mut self.view {
            View::Prompt(prompt) => Some(prompt),
            _ => None,
        }
    }

    /// The decision this window shows, if it is a decision pane.
    pub(crate) fn decision(&self) -> Option<&DecisionView> {
        match &self.view {
            View::Decision(decision) => Some(decision),
            _ => None,
        }
    }

    /// [`decision`](Self::decision), to change.
    pub(crate) fn decision_mut(&mut self) -> Option<&mut DecisionView> {
        match &mut self.view {
            View::Decision(decision) => Some(decision),
            _ => None,
        }
    }

    /// **The session a decision pane decides for, and the window it sits
    /// under** ([`DecisionView::decides`]) — `None` for every other pane.
    pub(crate) fn decides(&self) -> Option<(BufferId, crate::wm::PaneId)> {
        self.decision().map(|d| d.decides)
    }

    /// **Show `opening`, what this pane showed set aside** — the one
    /// statement of what the history keeps: a buffer once (the one opening
    /// supersedes any older place in it, so the buffer shown is never in
    /// it), a session as where the window was in it — the newest, the
    /// window's own, beneath what opens — and never a companion
    /// ([`place`](Self::place) keeps nothing of one). A swap: the window
    /// shows `opening` from the moment what it showed leaves. A place the
    /// caller means to go back to is taken out FIRST ([`take`](Self::take),
    /// as [`enter`](Self::enter) does): the swap would drop it.
    pub(super) fn set_aside(&mut self, opening: View) {
        self.history.retain(|away| !away.is_of(&opening));
        let aside = self.place();
        self.view = opening;
        self.history.extend(aside);
    }

    /// **What was set aside last, taken out of the history** — a view, or
    /// a session (the one beneath, or one left with none beneath to show);
    /// `None` with nothing set aside. What the window shows is the caller's
    /// to replace with it: a view at `App::view_back`, a session at its
    /// place (`App::put_place`).
    pub(super) fn give_back(&mut self) -> Option<Away> {
        self.history.pop()
    }

    /// **Show session `buffer` in this window** — what it shows set aside
    /// (a view, or the session it shows, as one left), and where it was in
    /// `buffer` taken back out of its history: the place for the caller to
    /// put it at (`App::put_place`, which copies the lines in), the
    /// session's `entry` (`App::entry_of`) if it never showed it. The one way a window comes
    /// to show a session after its birth but `:q` (`App::reclaim_viewport`):
    /// `:b N`, `:open`, `<CR>`, `:new`, a checkout, `<C-^>`, a landing in
    /// the window that shows the session it lands beside.
    ///
    /// **`None`, and nothing changed, when it already shows `buffer`** —
    /// the buffer shown is never in its history, held here rather than by
    /// each caller: two asked first, and the one alone in front of `:new`
    /// staying put and of a landing already shown (`App::enter_session`)
    /// passed every test without its check, which would have filed the
    /// session shown under itself for `<C-^>` to "go back" to (review stop
    /// 10, A 1 and C 1).
    pub(super) fn enter(&mut self, buffer: BufferId, entry: Place) -> Option<Place> {
        if self.buffer() == Some(buffer) {
            return None;
        }
        // The place out first: the swap keeps a buffer once, and would drop
        // it (V5.4 design review, L6).
        let place = match self.take(buffer) {
            Some(Away::Session(_, place)) => place,
            Some(Away::View(_)) | None => entry,
        };
        self.set_aside(View::Session(Box::new(SessionView::new(buffer))));
        Some(place)
    }

    /// **The session beneath this window's views, made `buffer`** — the one
    /// that lay there kept as one left, and `buffer` put where it lay: so
    /// `:q` still gives back the views above it first (V5.3 design review,
    /// finding 6). A composition landing beside a session a viewer covers —
    /// a buffer of its own (`hold(None, …)`), so one the window never held,
    /// put at its `entry`; asserted, as a premise no caller today can break
    /// (a second entry for one buffer would come back twice).
    pub(super) fn repoint_beneath(&mut self, buffer: BufferId, entry: Place) {
        debug_assert!(
            !(self.history.iter()).any(|away| matches!(away, Away::Session(b, _) if *b == buffer)),
            "a landing is a buffer the window never held"
        );
        let at = (self.history.iter())
            .rposition(|away| matches!(away, Away::Session(..)))
            .map_or(self.history.len(), |newest| newest + 1);
        self.history.insert(at, Away::Session(buffer, entry));
    }

    /// **The window's session** — the one it shows, else the one beneath
    /// its views: the newest it set aside ([`Away::Session`]). `None` for a
    /// window with none (a viewer opened in a pane of its own, the prompt,
    /// a decision).
    pub(crate) fn session(&self) -> Option<BufferId> {
        match &self.view {
            View::Session(session) => Some(session.buffer),
            _ => self.beneath().map(|(buffer, _)| buffer),
        }
    }

    /// The session beneath its views, and where the window was in it — the
    /// newest it set aside, when a view shows.
    fn beneath(&self) -> Option<(BufferId, &Place)> {
        if matches!(self.view, View::Session(_)) {
            return None;
        }
        self.history.iter().rev().find_map(|away| match away {
            Away::Session(buffer, place) => Some((*buffer, place)),
            Away::View(_) => None,
        })
    }

    /// **The window's session lies beneath what it shows** — a viewer
    /// borrowed its transcript. For the tests' premises: production asks
    /// [`session`](Self::session), which says which.
    #[cfg(test)]
    pub(super) fn has_beneath(&self) -> bool {
        self.beneath().is_some()
    }

    /// The session this window shows, if it shows one.
    pub(crate) fn session_view(&self) -> Option<&SessionView> {
        match &self.view {
            View::Session(session) => Some(session),
            _ => None,
        }
    }

    /// [`session_view`](Self::session_view), to change.
    pub(crate) fn session_view_mut(&mut self) -> Option<&mut SessionView> {
        match &mut self.view {
            View::Session(session) => Some(session),
            _ => None,
        }
    }

    /// **Where the window is looking in its session** — the scroll of the
    /// one it shows, else where it was in the one beneath (the status row
    /// beside a viewer lent over the aim, `App::aim_viewport`); a window
    /// with none, following the tail.
    pub(super) fn viewport(&self) -> crate::viewport::Viewport {
        match (self.session_view(), self.beneath()) {
            (Some(session), _) => session.viewport,
            (None, Some((_, place))) => place.viewport,
            (None, None) => crate::viewport::Viewport::default(),
        }
    }

    /// **This window's place in the session it shows, let go** — a window
    /// as one is born, its lines to be copied again (the rows they index
    /// have changed), for its caller to put at the session's entry
    /// (`App::forget_cursors_on`).
    pub(super) fn forget_place(&mut self) {
        if let Some(session) = self.session_view_mut() {
            *session = SessionView::new(session.buffer);
        }
    }

    /// **The buffer this window shows** ([`View::buffer`]).
    pub(crate) fn buffer(&self) -> Option<BufferId> {
        self.view.buffer()
    }

    /// **Does it already show what `view` would** — the same buffer, as
    /// [`Away::is_of`] asks of a place: the manual at another topic, a tree
    /// listed again. What `App::show_in` replaces in place rather than sets
    /// aside. An instrument is no buffer's, so it never shows as another —
    /// never asked of one today: `show_in` takes content.
    pub(super) fn shows_as(&self, view: &View) -> bool {
        view.buffer().is_some_and(|b| self.buffer() == Some(b))
    }

    /// **How this window holds `buffer`, if it does** ([`Holding`]): shown;
    /// beneath its views, its own session; or set aside. The one question
    /// "which windows hold X" asks of each (`App::panes_holding`) —
    /// [`held`](Self::held) asked of one buffer, so the two cannot differ.
    pub(crate) fn holds(&self, buffer: BufferId) -> Option<Holding> {
        self.held()
            .find(|&(held, _)| held == buffer)
            .map(|(_, how)| how)
    }

    /// **Every buffer this window holds, and how** ([`Holding`]), each once:
    /// what it shows, the session beneath its views, and each it set aside.
    /// A walk asking it of every window answers "which windows hold X" for
    /// every X at once (`App::holdings`, for `:ls`, which asked it twice a
    /// row — review stop 10, B 2).
    pub(crate) fn held(&self) -> impl Iterator<Item = (BufferId, Holding)> + '_ {
        let shown = self.buffer();
        let beneath = self.beneath().map(|(buffer, _)| buffer);
        let aside = (self.history.iter())
            .map(Away::buffer)
            .filter(move |&held| Some(held) != shown && Some(held) != beneath)
            .map(|held| (held, Holding::Aside));
        (shown.map(|buffer| (buffer, Holding::Shown)).into_iter())
            .chain(beneath.map(|buffer| (buffer, Holding::Beneath)))
            .chain(aside)
    }

    /// **`buffer`'s place in this window's history, taken out of it** — a
    /// view as it was left, or where the window was in a session; `None` if
    /// it holds none. Three ways leave the history, each keeping a buffer
    /// once: this, by buffer — `:b N` of a view held (`App::bring_back_held`)
    /// and a session entered ([`enter`](Self::enter), so `<C-^>` to a
    /// session too); [`give_back`](Self::give_back), the newest — `:q`, and
    /// `<C-^>` to a view; and [`set_aside`](Self::set_aside)'s supersede.
    /// It was two takes, by index and by session, and `:b N`'s search a
    /// third (V5.4 design review, M2; this said "the one way", review stop
    /// 10, D 8).
    pub(super) fn take(&mut self, buffer: BufferId) -> Option<Away> {
        let at = (self.history.iter()).position(|away| away.buffer() == buffer)?;
        Some(self.history.remove(at))
    }

    /// **The trees set aside in this window**, as places — what reaches
    /// every `:graph` reaches these too: a reading landing, leave given or
    /// taken back. Both reached only the tree on screen, so one set aside
    /// under `:help` came back showing what `:unread` had let go, or waiting
    /// on a reading that had come (design review A of the Claude Code
    /// section, 10-21, finding 1).
    pub(super) fn tree_places_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut crate::graph_view::TreePlace> {
        (self.history.iter_mut()).filter_map(|away| match away {
            Away::View(Viewed::Tree(place)) => Some(&mut **place),
            Away::View(Viewed::Page(_)) | Away::Session(..) => None,
        })
    }
}
