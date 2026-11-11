//! The `:graph` viewer model (M3 P4 — "see the graph before you mutate it").
//!
//! `GraphView` is a *projection* of a [`Graph`] snapshot into a navigable,
//! **collapsible** tree — the fractal zoom of design.md §1: one recursive
//! [`Node`] abstraction walked uniformly across every level (Episode → Turn →
//! message, and a branch / sub-agent Episode nested under the turn it forked
//! from). You drill in and out of the hierarchy with expand/collapse; the
//! nesting is whatever [`Graph::children`] reports, so it stays correct as the
//! graph grows new shapes (branches in P5, sub-agents in P6) with no viewer change.
//!
//! **Built once, read O(viewport).** Opening `:graph` flattens the *entire* tree
//! into `full` (preorder, with each node's depth, label, parent, and whether it
//! has children) a single time. From then on, only `expanded` toggles and a
//! `visible_order` index list change — recomputed on a fold toggle, never per
//! frame. The render slices a viewport-sized window out of `visible_order`, so
//! the hot path is `O(viewport)`, not `O(history)` (`[[feedback_perf_target]]`).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use shannon_core::claude_import::{Found, Imported, ImportedTurn};
use shannon_core::{
    EdgeKind, EpisodeId, Graph, GraphChars, GraphIds, GraphSort, Node, Role, TurnId,
};

use crate::scrub::Scrubber;
use crate::wm::BufferId;

/// **The tree a test's view is on** ([`GraphView::new`]) — a number no
/// test's app reaches, so it can be no other buffer's.
#[cfg(test)]
pub(crate) const TEST_TREE: BufferId = BufferId(u64::MAX);

/// Number of distinct lane colors in the `git` style. The renderer maps a
/// [`LaneCell::color`] slot (`0..PALETTE_LEN`) to a concrete color.
pub const PALETTE_LEN: usize = 6;

/// The resolved node-glyph set for `:graph` — which character marks each KIND
/// of session head, plus the busy marker. The defaults come from the
/// `graph_chars` set (unicode or ascii) so both terminals get a coherent look;
/// `[tui] graph_glyphs` overrides any role sparsely (the keymap pattern).
/// Children get a distinct default (`◆`) — a spawned session is a different
/// *kind* of thing than a branch you cut yourself, and the tree should say so
/// at a glance. Single `char`s on purpose: lane columns must stay aligned.
/// This is the seed the M4 theme/window-chrome config grows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeGlyphs {
    session: char,
    child: char,
    clone: char,
    merge: char,
    running: char,
    held: char,
    /// A turn row's mark.
    turn: char,
    /// A turn that is a SUMMARY — where a compaction (`:compact`, `S`)
    /// happened in the conversation (Steven, 10-16: "a marker for the turn
    /// in the conversation where a compaction has happened").
    compacted: char,
    /// A Claude Code conversation's row.
    claude: char,
    /// A Claude Code conversation a Shannon session continues — so a fold
    /// cannot hide that it was continued (Steven, 10-16).
    continued: char,
}

impl NodeGlyphs {
    /// The coded defaults for a glyph set: unicode `● ◆ ● ● ▶`, ascii
    /// `* o * * >`. Clone/merge deliberately match `session` — they're user
    /// surgery, the same standing as a branch; only *children* differ by
    /// default. Every role stays individually overridable: "we provide good
    /// defaults, but don't force opinions on others. Aesthetics are often
    /// very personal too" (Steven, 10-16).
    pub fn base(chars: GraphChars) -> Self {
        match chars {
            GraphChars::Unicode => Self {
                session: '●',
                child: '◆',
                clone: '●',
                merge: '●',
                running: '▶',
                // A pause bar, not a warning sign: the child is fine, it is
                // stopped and waiting for you. Width 1, so it lines up with
                // `▶` in a column of rows.
                held: '‖',
                turn: '◦',
                // A circle in a circle, among the tree's circles: a turn that
                // holds others. One cell in every terminal (East Asian width
                // N); `⊙` is ambiguous-width, two cells in a CJK-wide one.
                compacted: '⊚',
                claude: '◇',
                // A bowtie: two things joined at a point — the conversation
                // and where it went on. Solid, so it is seen; one cell (N).
                continued: '⧓',
            },
            GraphChars::Ascii => Self {
                session: '*',
                child: 'o',
                clone: '*',
                merge: '*',
                running: '>',
                held: '!',
                // `◦` was drawn in ascii too, before turns had a role.
                turn: '.',
                compacted: '@',
                claude: '+',
                continued: 'X',
            },
        }
    }

    /// The base set for `chars` with the user's `[tui] graph_glyphs` overrides
    /// applied — each that [`read_override`](Self::read_override) takes; the
    /// rest keep the default, and the graph still renders (degrade, don't
    /// crash). Why one was not taken is said on screen, at launch and when the
    /// setting changes (`App::glyph_notes`), not here: this runs on every
    /// rebuild of every graph.
    pub fn resolve(chars: GraphChars, overrides: &BTreeMap<String, String>) -> Self {
        let mut glyphs = Self::base(chars);
        for (role, value) in overrides {
            if let Ok(c) = Self::read_override(role, value) {
                glyphs.set_role(role, c);
            }
        }
        glyphs
    }

    /// **One glyph override, read**: the mark, or why it is not one — a role
    /// we know, and one character ONE CELL wide. A row's columns line up by
    /// it: a wide mark (`一`, an emoji) pushed the label a cell over, and a
    /// zero-width or control one vanished (ratatui drops them), and either
    /// was taken with a log line alone (review stop 2, B 5). The rule's one
    /// statement: [`resolve`](Self::resolve) applies what it takes,
    /// `App::glyph_notes` says what it refuses. Marks only — text in the
    /// chat shows as written, wide or not (Steven, 10-16).
    pub fn read_override(role: &str, value: &str) -> Result<char, String> {
        // The valid set comes from `ROLES`, not from a sentence typed beside
        // the match — there were THREE hand-written copies of this list (the
        // arms, this message, and the `DEFAULT_TOML` comment a user actually
        // reads), and adding `held` updated one of them
        // (`[[feedback_rules_rot_by_retyping]]`).
        if !Self::ROLES.contains(&role) {
            return Err(format!(
                "`[tui.graph_glyphs]` has no role `{role}` — {}",
                Self::ROLES.join(", ")
            ));
        }
        let said = |why: &str| format!("`[tui.graph_glyphs] {role} = {value:?}` {why}");
        let mut chars = value.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return Err(said("is not one character — the default stays"));
        };
        match (c.is_control(), crate::wrap::char_cells(c)) {
            (false, 1) => Ok(c),
            (true, _) | (_, 0) => Err(said("shows nothing — the default stays")),
            (false, cells) => Err(said(&format!(
                "takes {cells} cells, and a mark takes one (the rows line up by it) \
                 — the default stays"
            ))),
        }
    }

    /// Every configurable role, in `DEFAULT_TOML` order.
    ///
    /// The one list. `set_role` is the only thing that may accept a name, this
    /// is the only thing that may enumerate them, and
    /// `every_glyph_role_is_accepted_and_documented` ties both to the config
    /// template the user actually reads.
    pub const ROLES: &'static [&'static str] = &[
        "session",
        "child",
        "clone",
        "merge",
        "running",
        "held",
        "turn",
        "compacted",
        "claude",
        "continued",
    ];

    /// Apply one role override; `false` if `role` is not one we know.
    ///
    /// Returning a bool rather than warning here keeps the *policy* (what to do
    /// about a bad name) with the caller that has the config context, and makes
    /// the accepted set testable without going through a `Config`.
    fn set_role(&mut self, role: &str, c: char) -> bool {
        match role {
            "session" => self.session = c,
            "child" => self.child = c,
            "clone" => self.clone = c,
            "merge" => self.merge = c,
            "running" => self.running = c,
            "held" => self.held = c,
            "turn" => self.turn = c,
            "compacted" => self.compacted = c,
            "claude" => self.claude = c,
            "continued" => self.continued = c,
            _ => return false,
        }
        true
    }

    /// A turn row's mark — the `compacted` role's when the turn is
    /// a summary, the ordinary one otherwise.
    pub fn for_turn(&self, summary: bool) -> char {
        match summary {
            true => self.compacted,
            false => self.turn,
        }
    }

    /// A Claude Code conversation's mark — the continued one when a Shannon
    /// session goes on from it.
    pub fn for_claude(&self, continued: bool) -> char {
        match continued {
            true => self.continued,
            false => self.claude,
        }
    }

    /// The head glyph for a session of origin-kind `kind` (`None` = a root).
    pub fn for_kind(&self, kind: Option<EdgeKind>) -> char {
        match kind {
            Some(EdgeKind::ToolCall) => self.child,
            Some(EdgeKind::Clone) => self.clone,
            Some(EdgeKind::Merge) => self.merge,
            Some(EdgeKind::Branch) | None => self.session,
        }
    }

    /// The busy marker (`▶` by default) — running rows and the inspector.
    pub fn running(&self) -> char {
        self.running
    }

    /// The marker for an [`Activity`], so tree rows and the inspector cannot
    /// disagree about which glyph means which state.
    pub fn for_activity(&self, activity: Activity) -> char {
        match activity {
            Activity::Running => self.running,
            Activity::Held => self.held,
        }
    }
}

impl Default for NodeGlyphs {
    fn default() -> Self {
        Self::base(GraphChars::Unicode)
    }
}

/// **What a row of `:graph` stands for**: a node of Shannon's graph, or an
/// entry of Claude Code's — its section's heading, one of its conversations.
///
/// One tree holds both (Steven, 10-16: a session continued from Claude Code
/// sits under the conversation it continues, "the same principle we use
/// everywhere else"), and the two stay apart in the TYPE: a Claude Code entry
/// is no [`Node`], so every verb that acts on nodes passes over it by
/// construction — [`selected_node`](GraphView::selected_node),
/// [`acts_on`](GraphView::acts_on) and the marks answer only for `Node`
/// rows. Shannon never changes or deletes Claude Code's records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Node(Node),
    /// The Claude Code section's heading.
    ClaudeHeading,
    /// A conversation, by its place in the section's `found`.
    Conversation(usize),
    /// A conversation's turn — `turn` its place in the conversation as
    /// read ([`ClaudeSection::read`]), every compaction kept.
    ClaudeTurn {
        conversation: usize,
        turn: usize,
    },
}

impl Entry {
    /// The graph node, for a row that is one.
    pub fn node(self) -> Option<Node> {
        match self {
            Entry::Node(node) => Some(node),
            _ => None,
        }
    }
}

/// What a row is, **by identity** — the node, or the Claude Code entry by
/// its conversation's id — so what the user holds (the cursor, folds, marks,
/// a Visual anchor, the window's top) survives a rebuild that renumbers the
/// rows.
///
/// **A turn by its rank, not its place**: the `nth` occurrence of its content
/// in its session. A [`Node::Turn`]'s `index` is its place, and a turn
/// deleted above renumbers every place after it — a mark on a later turn
/// was dropped, and a cursor there landed where the deleted one had stood,
/// in every other tree window too (V4). Ranks move only when an earlier
/// copy of the SAME content goes — a session holds one content twice only by a
/// composition, since a seal is timestamped — and then what was held on the
/// later copy is lost, and what was held on the copy that went lands on the
/// one that stays: the rank cannot tell copies apart across their deletion.
/// Only the deleting door knows which went (vim's `mark_adjust`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Session(EpisodeId),
    Turn {
        episode: EpisodeId,
        turn: TurnId,
        nth: usize,
    },
    Message {
        episode: EpisodeId,
        turn: TurnId,
        nth: usize,
        index: usize,
    },
    Heading,
    Conversation(String),
    ClaudeTurn(String, usize),
}

/// What the user holds in the tree, by [`Key`], across a rebuild — or into
/// another view of the tree ([`GraphView::restore`]).
#[derive(Debug, Default)]
pub(crate) struct Held {
    selected: Option<Key>,
    /// The folds the user set — those not as their row's default.
    expanded: HashMap<Key, bool>,
    /// The Claude Code conversations shown with their turns, by id.
    with_turns: BTreeSet<String>,
    marked: Vec<Key>,
    visual: Option<Key>,
    /// The rows shown above the cursor, nearest first — for a cursor whose
    /// row goes, which lands just past the first of them still there
    /// ([`GraphView::in_place_of`]).
    above: Vec<Key>,
    /// Where the cursor goes when none was held — a restore over a tree
    /// that showed nothing: where the fresh view stands.
    fallback: Option<Key>,
    /// **The window's top, held by identity as the cursor is**: how many of
    /// the rows in `above` the window showed above the cursor — its top is
    /// `above[top - 1]`, or the cursor's own row at 0. A refresh built a
    /// fresh view whose top was 0, so every background event scrolled the
    /// window until the cursor sat on its bottom margin (V4).
    top: usize,
    /// The height the window was last settled at ([`GraphView::window_top`])
    /// — carried so a refresh does not read as a resize, which refills a
    /// window `zt` put past the end.
    height: Option<usize>,
    /// A half-typed chord ([`GraphView::arm_chord`]): something arriving
    /// must not change what the next key means — a refresh between `m` and
    /// `f` made the `f` a key of its own.
    chord: Option<char>,
    /// Where the window was when last settled ([`GraphView::moved`]) —
    /// carried, so a refresh's fresh view has not "moved" and a split's copy
    /// has not until it does.
    settled: Option<Spot>,
}

/// **Where a window was in a tree, set aside** (a window's history,
/// `App`'s `Away::Tree`) — what it held there, and its Claude Code section
/// with what was read of it: what a tree is built again from where it was
/// ([`GraphView::restore`]). Not the tree: one set aside is not kept current
/// as one shown is (`App::refresh_graph_views`), so it is built again from
/// the store when it comes back, its section listed again if the leave or
/// the secrets changed meanwhile (`App::current_section`) — a place, as a
/// session's is (V5.3 design review, finding 4: kept whole, every window's
/// history held a graph).
#[derive(Debug)]
pub(crate) struct TreePlace {
    /// The tree it is a place in.
    tree: BufferId,
    held: Held,
    claude: Option<ClaudeSection>,
}

impl TreePlace {
    /// The tree it is a place in — what `:b N` finds it by, and what it is
    /// built on again.
    pub(crate) fn tree(&self) -> BufferId {
        self.tree
    }

    /// Its Claude Code section, with what was read of it.
    pub(crate) fn claude(&self) -> Option<&ClaudeSection> {
        self.claude.as_ref()
    }

    /// **Its section let go** — listed again as `:graph` lists one when the
    /// tree comes back: leave given or taken back since, what was read under
    /// the old leave is not kept (`App::relist_claude_sections`).
    pub(crate) fn forget_claude(&mut self) {
        self.claude = None;
    }

    /// [`GraphView::take_reading`] for a tree set aside: the section takes
    /// it, and the tree shows it when it is built again.
    pub(crate) fn take_reading(
        &mut self,
        path: &Path,
        id: &str,
        read: Option<&Arc<Imported>>,
        by: &Scrubber,
    ) -> bool {
        (self.claude.as_mut()).is_some_and(|section| section.take_reading(path, id, read, by).0)
    }

    /// Its tree, what it held, and its section — for a view built on that
    /// tree and [`GraphView::restore`].
    pub(crate) fn into_parts(self) -> (BufferId, Held, Option<ClaudeSection>) {
        (self.tree, self.held, self.claude)
    }
}

/// **Where you were in a tree** — its cursor's row, by identity ([`Key`]):
/// what `:b N` opens the session tree at, as a page's line is its `'"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spot(Key);

/// **A session tree, as a buffer** (`buffers.md` §10.4, V4 identity) —
/// unlisted, as a page is: numbered from the sessions' counter, named by
/// `:ls!`, shown by `:b N`, and kept with where you were in it and its marks
/// when its last window closes. As netrw's listing of a directory is
/// (measured at review stop 10, Neovim 0.12.6, netrw v184): ONE buffer,
/// opened again by any spelling, shown elsewhere or hidden, its marks with
/// it — so one per store, every `:graph` a window on it (`App::tree_buffer`;
/// Steven, 10-26). It was one per `:graph` shown together, on a record of
/// netrw that did not reproduce.
///
/// Its content is still each window's own snapshot (`GraphView::graph`):
/// one shared is docketed until profiled, since a body loaded for the
/// inspector is kept inside it.
#[derive(Debug)]
pub(crate) struct GraphBuffer {
    id: BufferId,
    left_at: Option<Spot>,
    /// **The tree's marks** — netrw's local marked list, one for the
    /// buffer: every window on it shows these (`App::settle_tree_marks`).
    marked: Vec<Key>,
}

impl GraphBuffer {
    pub(crate) fn new(id: BufferId) -> GraphBuffer {
        GraphBuffer {
            id,
            left_at: None,
            marked: Vec::new(),
        }
    }

    /// **What a tree is called** — its `:ls!` row's and every message's
    /// that names it ("`b5` is the session tree, not a session"): one name,
    /// both read, where each spelt it (review stop 10, D 7).
    pub(crate) fn name(&self) -> &'static str {
        "the session tree"
    }

    /// **Are `view`'s marks not the tree's** — one was made or spent there
    /// since the tree last took them?
    pub(crate) fn marks_differ(&self, view: &GraphView) -> bool {
        let tree: std::collections::HashSet<&Key> = self.marked.iter().collect();
        let keys = view.marked_keys();
        keys.len() != tree.len() || keys.iter().any(|k| !tree.contains(k))
    }

    /// Take `view`'s marks as the tree's.
    pub(crate) fn take_marks(&mut self, view: &GraphView) {
        self.marked = view.marked_keys();
    }

    /// Show the tree's marks in `view`, a window on it.
    pub(crate) fn show_marks(&self, view: &mut GraphView) {
        view.set_marked(&self.marked);
    }

    pub(crate) fn id(&self) -> BufferId {
        self.id
    }

    /// Where you last were in it — vim's `'"`.
    pub(crate) fn left_at(&self) -> Option<&Spot> {
        self.left_at.as_ref()
    }

    /// Note where a window on it is.
    pub(crate) fn leave_at(&mut self, spot: Spot) {
        self.left_at = Some(spot);
    }
}

/// **What `l` shows next** ([`GraphView::deeper`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deeper {
    /// A closed fold's children — on a Claude Code conversation, what hangs
    /// from it.
    Fold,
    /// A Claude Code conversation's turns.
    Turns,
}

/// One row of the fully-flattened tree (every level, always present). Folding
/// hides/shows rows via [`GraphView::visible_order`]; it never rebuilds `full`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    entry: Entry,
    depth: usize,
    label: String,
    has_children: bool,
    /// Whether this row is a turn **borrowed** (shared by reference) from another
    /// episode rather than owned here — computed once at build from the owner tag,
    /// so the renderer never derives it per frame (`[[feedback_perf_target]]`). Always
    /// `false` for episode/message rows.
    shared: bool,
    /// Whether this row is a turn that is a summary — where a compaction
    /// happened — so both styles mark it ([`NodeGlyphs::for_turn`]: the tree's
    /// label, the `git` style's dot). Computed at build, as `shared` is.
    summary: bool,
}

/// One cell of the `git`-style lane graph: a glyph and the palette slot to color
/// it (the branch that owns the lane). Rendered left-to-right before the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneCell {
    pub ch: char,
    pub color: usize,
}

/// What the renderer needs for one on-screen row: the tree-connector prefix or
/// the `git` lane graph, the label, its fold state (for the triangle), and
/// whether it's selected / active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRow<'a> {
    pub depth: usize,
    /// The box-drawing connectors that draw this row's place in the tree —
    /// already indented to its depth, so the renderer prepends it verbatim.
    /// (The `tree` style.)
    pub prefix: String,
    /// The `git` style's lane graph for this row: persistent per-branch columns
    /// with fork connectors, the node's commit dot last. Colored per lane.
    pub lane: Vec<LaneCell>,
    pub label: &'a str,
    /// What this row stands for — lets a style-specific renderer (e.g. the
    /// `git` style) rebuild its own label from the graph instead of the
    /// precomputed tree `label`, and draw a Claude Code entry as its own.
    pub entry: Entry,
    /// `None` for a leaf; `Some(true)` expanded; `Some(false)` collapsed.
    pub fold: Option<bool>,
    pub selected: bool,
    /// True iff this row is the **active** (live / `HEAD`) episode.
    pub active: bool,
    /// True iff this row is **marked** for a `pick` (`<Space>` multi-select) —
    /// drawn with a highlighted background.
    pub marked: bool,
    /// True iff this row is inside the **Visual** range (`V`) — drawn as the
    /// prompt and the transcript draw theirs.
    pub visual: bool,
    /// True iff this row is a turn **borrowed** from another episode (drawn faded +
    /// italic). Precomputed at build, so the renderer never derives sharing per frame.
    pub shared: bool,
    /// What background work this row's episode has, if any — drawn as a marker.
    ///
    /// A *value*, not a `running: bool`. The bool could not say "held", so a
    /// child stopped on a command it may not run unattended rendered `▶
    /// running` — the safety state wearing the working state's face (Reviewers
    /// A and E). A second parallel `held: HashSet` would have been a new
    /// mechanism where the first one was merely under-specified
    /// (`[[feedback_pivot_speed_measures_design]]`); a third state now lands on
    /// this axis instead.
    pub activity: Option<Activity>,
}

/// **Claude Code's conversations, below Shannon's sessions** (Steven, 10-04:
/// "I don't see our session in `:graph`. Let's get that going. And make sure
/// we don't let `shannon` delete `claude code` sessions").
///
/// Not graph nodes: a conversation here is one of Claude Code's own files,
/// which `<Enter>` imports as a Shannon session — a copy — and which nothing
/// in Shannon deletes. Its rows are the tree's ([`Entry`]), after Shannon's
/// roots, and carry no `Node`: every verb that acts on nodes passes over
/// them by construction, not by a check each one makes.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSection {
    /// The conversations, newest first — each title scrubbed as the listing
    /// entered (`App::claude_section`): Claude Code's title can be the
    /// user's last prompt, word for word.
    pub found: Vec<Found>,
    /// **The conversations read so far**, by id — each with every compaction
    /// kept (`Scope::Lineage`), its turns the rows under it, every text
    /// scrubbed as it was read (`Scrubber::imported`, `app::claude_reads`),
    /// under the secrets as they were then: the rows and the inspector
    /// draw from these, and nothing else of a conversation's words reaches
    /// a view. A conversation
    /// is read when it is opened (`l`), not when listed: reading one long
    /// transcript took ~0.7 s (Steven's, 10-16), and the section lists them
    /// all. Carried by a rebuild, never re-read by one.
    pub read: HashMap<String, Arc<Imported>>,
    /// **The conversations this view asked for that are being read** off
    /// the UI thread (`app::claude_reads`) — its row says so until the
    /// reading comes (`GraphView::take_reading`).
    pub awaiting: BTreeSet<String>,
    /// **Whether the user let Shannon read here** (`ClaudeSessions::
    /// may_read`) — and so whether anything is listed. Not yet: the heading
    /// stands alone and says so, and `l` on it asks.
    pub listed: bool,
    /// **The secrets it was listed under** — what scrubbed its titles, and
    /// every reading it holds: one scrubbed otherwise is never taken
    /// ([`take_reading`](Self::take_reading)). The secrets changed since (a
    /// mark, an unmark, `:recall`, a config read again), it is listed again
    /// and what was open in it asked for again (`App::current_section`), as
    /// when the leave does (`listed`) — a title is scrubbed as listed and
    /// its words are not kept, so an unmark can only list it again.
    pub(crate) scrubbed: Scrubber,
}

/// **What a heading not read yet says** — both styles' row and what `/`
/// reads: that nothing is listed, and the key that asks.
pub(crate) const NOT_READ: &str = "not read — l asks";

impl ClaudeSection {
    /// **The reading of the file at `path` (conversation `id`), come back** —
    /// taken if this section asked for it or shows an older one
    /// ([`GraphView::take_reading`], which lays the rows out again): whether
    /// it had asked, and whether it took it. One for a tree shown and one
    /// set aside ([`TreePlace::take_reading`]), so the two cannot differ in
    /// what they keep. A reading scrubbed otherwise than this section was
    /// listed (`by`) is not taken, and one asked for stays asked: the section
    /// is listed again before it is shown, and asks again then — so every
    /// reading it holds is scrubbed as its titles are
    /// ([`scrubbed`](Self::scrubbed)).
    fn take_reading(
        &mut self,
        path: &Path,
        id: &str,
        read: Option<&Arc<Imported>>,
        by: &Scrubber,
    ) -> (bool, bool) {
        if !self.found.iter().any(|f| f.path == path) {
            return (false, false);
        }
        if !self.scrubbed.same(by) {
            return (self.awaiting.contains(id), false);
        }
        let asked = self.awaiting.remove(id);
        let shows = self.read.contains_key(id);
        let taken = read.filter(|_| asked || shows);
        if let Some(read) = taken {
            self.read.insert(id.to_string(), Arc::clone(read));
        }
        (asked, taken.is_some())
    }

    /// **A section Shannon may not read yet** — nothing listed, nothing
    /// read, its heading saying how to ask.
    pub(crate) fn not_read(scrubbed: Scrubber) -> Self {
        ClaudeSection {
            found: Vec::new(),
            read: HashMap::new(),
            awaiting: BTreeSet::new(),
            listed: false,
            scrubbed,
        }
    }

    /// **What a conversation reads as, given to this section** — `shown`,
    /// the reading to show now (kept as the file stands, or kept from before
    /// while it is read again), and whether it is `awaiting` one. The one
    /// way a reading becomes what a section holds: those that asked for one
    /// — `:graph` opening on a conversation a session continues, and `l`,
    /// then — each wrote it their own way, and drifted (review stop 4, C M7).
    pub fn offer(&mut self, id: &str, shown: Option<Arc<Imported>>, awaiting: bool) {
        if let Some(shown) = shown {
            self.read.insert(id.to_string(), shown);
        }
        if awaiting {
            self.awaiting.insert(id.to_string());
        }
    }
}

/// **What `<CR>` opens on a Claude Code row** ([`GraphView::claude_open`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeOpen {
    /// The session going on with the conversation — the most recently
    /// active, `others` more going on with it too.
    Session { id: EpisodeId, others: usize },
    /// A copy of the conversation: to its end, or `through` one of its turns.
    Import {
        path: PathBuf,
        through: Option<Through>,
    },
}

/// **Where a copy of a turn ends** — its turn `turn` of `of`, as the
/// section's reading counts them, the reading stopping where `until` says
/// (`claude_import::read_until`): before the entry that opens the turn after,
/// or, on the reading's last turn, after the reading's leaf — the file may
/// have gone on since (review stop 4, A 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Through {
    pub turn: usize,
    pub of: usize,
    pub until: shannon_core::claude_import::Until,
}

/// One row of the Claude Code section, as the renderer draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClaudeRow<'a> {
    /// The section's heading: how many, and from where — `None` many when
    /// Shannon may not read there yet ([`ClaudeSection::listed`]).
    Heading {
        count: Option<usize>,
        selected: bool,
        visual: bool,
    },
    /// One conversation.
    Conversation {
        found: &'a Found,
        /// A Shannon session continues it ([`NodeGlyphs::for_claude`]).
        continued: bool,
        /// It holds rows it does not show — unread, or closed.
        folded: bool,
        /// Asked for, and being read.
        awaiting: bool,
        /// Resumed from another listed: where it was picked up
        /// (`Resumes::note`).
        resumes: Option<&'a str>,
        selected: bool,
        visual: bool,
    },
}

/// How long ago `then` was, as a listing says it: `just now`, `5m ago`,
/// `3h ago`, `2d ago`. A clock that went backwards reads as just now.
pub fn ago(now: SystemTime, then: SystemTime) -> String {
    let secs = now.duration_since(then).map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "just now".into(),
        60..3_600 => format!("{}m ago", secs / 60),
        3_600..86_400 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// What a background job on an episode is *doing* — the distinction the graph,
/// the status row and the busy refusals all need and none of them had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Mid-turn: talking to the backend, or running a command it was allowed.
    Running,
    /// Stopped on something it may not do unattended, waiting for a human. Not
    /// running, and waiting will not help — the thing holding it is waiting on
    /// *you*.
    Held,
}

impl Activity {
    /// The one word every surface uses for this state.
    ///
    /// **Named once because it is reported in three places** — the `:graph`
    /// row, the pane border, and the inspector — and a border that said
    /// `· running` for a *held* child was the defect that made this an enum in
    /// the first place: it read off a surviving `bool` that could not express
    /// "held" (Reviewer D). A shared vocabulary is the half of that fix which
    /// stops the next surface re-spelling it
    /// (`[[feedback_rules_rot_by_retyping]]`).
    ///
    /// Colour is deliberately NOT here: it is the renderer's own decision (cyan
    /// for held, because yellow is addressed to the reader).
    pub fn word(self) -> &'static str {
        match self {
            Activity::Running => "running",
            Activity::Held => "held",
        }
    }
}

/// A navigable, collapsible projection of a [`Graph`] — the `:graph` viewer model.
#[derive(Debug, Clone)]
pub struct GraphView {
    /// The graph snapshot this view was built from. Kept so the inspector can
    /// read full node details (provenance, config, message content) on demand —
    /// the tree only stores precomputed labels. A snapshot, not a live handle:
    /// the viewer shows a consistent picture from when `:graph` opened.
    graph: Graph,
    /// The active (live / `HEAD`) episode, marked in the tree. `None` until the
    /// App sets it via [`set_active`](Self::set_active).
    active: Option<EpisodeId>,
    /// The whole tree, preorder — laid out by each [`rebuild`](Self::rebuild),
    /// and by nothing else.
    full: Vec<Row>,
    /// `parent[i]` is the index in `full` of row `i`'s parent (`None` for a root).
    /// Parallel to `full`. Preorder guarantees a parent precedes its children.
    parent: Vec<Option<usize>>,
    /// Each row's identity ([`Key`]), parallel to `full` and laid out with
    /// it: a turn's rank reads its session's whole sequence, too dear to ask
    /// per row each time something is held.
    keys: Vec<Key>,
    /// Whether each node is expanded (its children shown). Parallel to `full`.
    expanded: Vec<bool>,
    /// Whether each row opens by default — what [`held`](Self::held)
    /// compares against, to carry only the folds the user set. Parallel to
    /// `full`.
    default_open: Vec<bool>,
    /// Indices into `full` that are currently visible (all ancestors expanded),
    /// in preorder. Recomputed on a fold toggle, not per frame.
    visible_order: Vec<usize>,
    /// Whether each node is the **last visible child** of its parent — drives the
    /// `└─` vs `├─` connector. Parallel to `full`; recomputed with `visible_order`.
    is_last: Vec<bool>,
    /// The highlighted row as an index into `visible_order` (so it tracks the
    /// visible list, and a fold keeps the selected *node* stable by re-finding it).
    selected_vis: usize,
    /// A half-typed multi-key chord: the prefix already pressed (`m` of netrw's
    /// `mf`/`mu`), waiting for the key that completes it. `None` when no chord
    /// is in flight. Navigation chords (`gg`, `zt`) are not here: they belong
    /// to the shared keymap, like every other pane's.
    ///
    /// **One slot, not one flag per chord.** As a `bool` this was cleared by
    /// hand at the top of eight handlers, which meant the four dispatch arms
    /// that never learned to — `<Space>`, `Enter`, `:`, `<S-Tab>` — let a
    /// stray `g` outlive an unrelated key, so `g<Space>g` jumped to the top.
    /// The prefix is now taken by the **dispatcher** before it dispatches, so
    /// aborting a chord is not something a handler can forget to do
    /// ([[feedback_one_door_per_invariant]]). A second chord was the n=2
    /// signal to stop adding flags ([[project_shannon_n_equals_two]]).
    pending: Option<char>,
    /// Palette slot per episode for the `git` style's lane colors — assigned
    /// greedily so an episode never shares a color with its parent or siblings
    /// (adjacent branches always differ). Keyed by episode id.
    episode_color: HashMap<EpisodeId, usize>,
    /// Rows (indices into `full`) the user has **marked** with `<Space>` for a
    /// `pick` — the multi-select set. A `BTreeSet` so marked nodes iterate in
    /// preorder (= graph / top-to-bottom order), which is the order they compose in.
    ///
    /// **This window's copy of its tree's marks** ([`GraphBuffer`]'s, netrw's
    /// local marked list): one made or spent here is the tree's at the event's
    /// end, and every other window on it shows it (`App::settle_tree_marks`).
    marked: BTreeSet<usize>,
    /// **The tree this is a window on** ([`GraphBuffer`]) — born on it
    /// ([`with_style`](Self::with_style)), and a refresh's fresh view and a
    /// split's copy built on the same one.
    tree: BufferId,
    /// Where the window was when last settled ([`moved`](Self::moved)),
    /// carried through a rebuild in [`Held`].
    settled: Option<Spot>,
    /// The other end of a Visual selection — a row by its index into `full`,
    /// the stable key, so a fold that moves rows on screen does not move it.
    /// `None` outside Visual.
    visual: Option<usize>,
    /// Claude Code's conversations, rows after Shannon's roots — `None`: no
    /// section (Claude Code's directory is unknown, or a test).
    claude: Option<ClaudeSection>,
    /// How sibling sessions are ordered, and where a named session's id goes
    /// — construction input, kept for a [`rebuild`](Self::rebuild).
    sort: GraphSort,
    ids: GraphIds,
    /// The Claude Code conversations a Shannon session continues — by its
    /// import record ([`shannon_core::Inherited`]), read once from the graph
    /// at build, so a row knows without a per-frame walk.
    continued: BTreeSet<String>,
    /// The Claude Code conversations shown with their turns, by id — the
    /// rest show what hangs from them under their row ([`Layout`]). Not
    /// `opened`: `Inherited::opened` is each imported turn's opening entry.
    with_turns: BTreeSet<String>,
    /// **Where each resumed conversation was picked up**, by its place in the
    /// list — said on its row, which hangs flat from its lineage's top
    /// ([`Resumes::note`]). Set by each rebuild.
    resume_notes: HashMap<usize, String>,
    /// **Folds the user set on rows not laid out now** — a conversation's
    /// turns hidden take their rows with them, and a fold set on one was
    /// forgotten by the next rebuild: `held` carried only rows in `full`
    /// (review stop 5, A 5). Carried until the row is back.
    folds_away: HashMap<Key, bool>,
    /// Rows of context kept visible above/below the selection before the window
    /// scrolls — vim's `scrolloff`, so scrolling reads fluidly instead of pinning
    /// the cursor to a window edge. Sourced from `tui.scrolloff`.
    scrolloff: usize,
    /// The window's first visible row, remembered between frames so the viewport
    /// only moves when the selection presses into the scrolloff margin. A `Cell`
    /// because the render pass (immutable) is what knows the height and slides it.
    top: std::cell::Cell<usize>,
    /// The height `top` was last settled against — a changed one is a resize,
    /// which refills a window scrolled past the end (`viewport::top_ceiling`).
    last_height: std::cell::Cell<Option<usize>>,
    /// **Where the next frame puts the cursor's row** in the window — for a
    /// placement asked before any height is known ([`go_to`](Self::go_to)).
    placing: std::cell::Cell<Option<crate::viewport::Place>>,
    /// Episodes with background work, and what that work is doing — for the
    /// row markers. Set by the App from its generations at build/refresh.
    activity: std::collections::HashMap<EpisodeId, Activity>,
    /// The resolved node-glyph set (per `graph_chars` + user overrides) — what
    /// marks each kind of session head. Fixed at construction: labels are
    /// precomputed, so glyphs are construction input, not mutable view state.
    glyphs: NodeGlyphs,
}

impl GraphView {
    /// Project `graph` into a collapsible tree. Default fold state: episodes
    /// open (so their turns show immediately), everything deeper collapsed — so
    /// the first view is the episode→turn outline, and you drill into a turn to
    /// reveal its messages and any branches forked from it. A test's view
    /// ([`with_sort`](Self::with_sort)); the app builds one on its tree
    /// (`App::graph_view_over`).
    #[cfg(test)]
    pub(crate) fn new(graph: Graph) -> Self {
        Self::with_sort(graph, GraphSort::default())
    }

    /// Project `graph`, ordering sibling **sessions** (root episodes) by `sort` —
    /// `Recency` (most-recently-active first; a blank just-started session sorts
    /// newest) by default, or `Hash` (the stable id order). Turns within a
    /// session always stay chronological. Uses the default (unicode) glyph set;
    /// [`with_style`](Self::with_style) is the fully-configured constructor.
    /// A test's view: on [`TEST_TREE`], a number no test's app reaches.
    #[cfg(test)]
    pub(crate) fn with_sort(graph: Graph, sort: GraphSort) -> Self {
        let (glyphs, ids) = (NodeGlyphs::default(), GraphIds::default());
        Self::with_style(graph, sort, glyphs, ids, TEST_TREE)
    }

    /// The fully-configured constructor: `sort` orders sibling sessions,
    /// `glyphs` (resolved from `graph_chars` + `[tui] graph_glyphs`) marks the
    /// session heads, `ids` says where a named session's id goes
    /// (`session_heading`). Both are construction input because row labels
    /// are precomputed once here — the read path stays O(viewport). `tree`:
    /// the tree buffer it is a window on ([`GraphBuffer`]) — born on it, as
    /// a transcript is born on its session (V5.3 (2)); it was attached after,
    /// by three doors, an `Option` held only for a test's tree (review stop
    /// 10, C 3).
    pub(crate) fn with_style(
        graph: Graph,
        sort: GraphSort,
        glyphs: NodeGlyphs,
        ids: GraphIds,
        tree: BufferId,
    ) -> Self {
        let continued = graph
            .episode_ids()
            .into_iter()
            .filter_map(|id| graph.episode(&id)?.inherited.as_ref())
            .map(|from| from.session.clone())
            .collect();
        let mut view = Self {
            graph,
            active: None,
            full: Vec::new(),
            parent: Vec::new(),
            keys: Vec::new(),
            expanded: Vec::new(),
            default_open: Vec::new(),
            visible_order: Vec::new(),
            is_last: Vec::new(),
            selected_vis: 0,
            pending: None,
            episode_color: HashMap::new(),
            marked: BTreeSet::new(),
            tree,
            settled: None,
            visual: None,
            claude: None,
            sort,
            ids,
            continued,
            with_turns: BTreeSet::new(),
            resume_notes: HashMap::new(),
            folds_away: HashMap::new(),
            scrolloff: 3,
            top: std::cell::Cell::new(0),
            last_height: std::cell::Cell::new(None),
            placing: std::cell::Cell::new(None),
            activity: std::collections::HashMap::new(),
            glyphs,
        };
        view.rebuild(Held::default());
        view
    }

    /// **Lay the rows out again**, keeping what the user holds — the cursor,
    /// folds, marks, a Visual anchor — by identity ([`Held`]): Shannon's
    /// roots, then, with a section, its heading and conversations. The one
    /// builder of `full`, for construction and for a section that changes
    /// under a tree the user is working in (`:unread`, leave given, a
    /// refresh).
    ///
    /// **A session that continues a listed conversation hangs under it**,
    /// not among Shannon's roots (Steven, 10-16: "If I started from
    /// `5f6e7d8c` then the continued conversation should be shown underneath
    /// `5f6e7d8c`. That's the same principle we use everywhere else"): at the
    /// turn it parted at (`Inherited::at`), its imported turns left out —
    /// they are the conversation's, drawn once, there — and a branch cut from
    /// one of them hangs under that turn of the conversation's. A session
    /// whose turn cannot be found (the conversation unread, a record from
    /// before `at`) hangs from the conversation's row, never lost.
    ///
    /// **And a conversation a `claude --resume` began hangs under the one it
    /// began from** ([`Resumes`]), at the turn it was picked up — the turns
    /// the two share drawn once, as the source's, and what hangs at one of
    /// those hung there — each lineage listed by its newest conversation.
    ///
    /// A fold never seen before takes the default: sessions open, so their
    /// turns show at once; a conversation something hangs from open to what
    /// hangs from it — its turns only once the user opens them (`l`, Steven,
    /// 10-18: 900 turns opened at once "shows too much all at once") — and,
    /// those open, the turn a session hangs from, so moving a session there
    /// does not hide it; every other fold closed.
    fn rebuild(&mut self, held: Held) {
        self.with_turns = held.with_turns.clone();
        let look = Look {
            glyphs: self.glyphs,
            ids: self.ids,
        };
        let graph = &self.graph;
        let mut roots = graph.roots();
        sort_roots(graph, &mut roots, self.sort);
        // Each continued session's conversation, by its place in the list.
        let mut homes: HashMap<usize, Vec<EpisodeId>> = HashMap::new();
        if let Some(section) = &self.claude {
            for root in &roots {
                let Node::Episode(id) = *root else { continue };
                let Some(from) = graph.episode(&id).and_then(|e| e.inherited.as_ref()) else {
                    continue;
                };
                if let Some(at) = section.found.iter().position(|f| f.id == from.session) {
                    homes.entry(at).or_default().push(id);
                }
            }
        }
        let homed: std::collections::HashSet<EpisodeId> =
            homes.values().flatten().copied().collect();
        let mut out = Laid {
            full: Vec::new(),
            parent: Vec::new(),
            pulls: Vec::new(),
        };
        for root in roots {
            if matches!(root, Node::Episode(id) if homed.contains(&id)) {
                continue;
            }
            build(
                graph,
                Drawn::plain(root),
                0,
                None,
                look,
                &mut out.full,
                &mut out.parent,
                0,
            );
        }
        let mut notes = HashMap::new();
        if let Some(section) = &self.claude {
            out.full.push(Row::claude(
                Entry::ClaudeHeading,
                match section.listed {
                    true => format!("Claude Code · {} here", section.found.len()),
                    false => format!("Claude Code · here: {NOT_READ}"),
                },
            ));
            out.parent.push(None);
            let resumes = Resumes::of(section);
            notes = (resumes.source.keys())
                .filter_map(|&at| Some((at, resumes.note(section, at)?)))
                .collect();
            let placed = Placed::of(graph, section, &homes, &resumes);
            let layout = Layout {
                graph,
                section,
                resumes: &resumes,
                placed: &placed,
                notes: &notes,
                with_turns: &self.with_turns,
                look,
            };
            // Each lineage by its newest conversation: a resumed one is
            // under its source now, and the newest is the likeliest sought.
            let mut tops: Vec<usize> = (0..section.found.len())
                .filter(|at| !resumes.source.contains_key(at))
                .collect();
            tops.sort_by_key(|&at| std::cmp::Reverse(resumes.newest(section, at)));
            for at in tops {
                layout.conversation(at, 0, None, &mut out);
            }
        }
        self.resume_notes = notes;
        // **The Claude Code rows above what they hold open by default** — a
        // session going on from a conversation, a conversation resumed in its
        // lineage: its turn, its conversation, and the lineage's top.
        let mut opens: BTreeSet<usize> = BTreeSet::new();
        for &pulled in &out.pulls {
            let mut above = out.parent[pulled];
            while let Some(f) = above {
                // A row already opened opened everything above it: up a long
                // lineage the walk was O(rows × depth) (review stop 5, B 2).
                if !matches!(
                    out.full[f].entry,
                    Entry::Conversation(_) | Entry::ClaudeTurn { .. }
                ) || !opens.insert(f)
                {
                    break;
                }
                above = out.parent[f];
            }
        }
        let Laid { full, parent, .. } = out;
        self.episode_color = assign_colors(&self.graph, &full);
        self.full = full;
        self.parent = parent;
        self.default_open = (0..self.full.len())
            .map(|f| {
                opens.contains(&f) || matches!(self.full[f].entry, Entry::Node(Node::Episode(_)))
            })
            .collect();
        // Every row's identity, once, and one index from it back to the row:
        // a key per held thing scanned against every row was quadratic in a
        // refresh that runs on every background event (Reviewer C). Kept as
        // `self.keys` at the end, once nothing borrows it.
        let keys = self.lay_keys();
        self.expanded = keys
            .iter()
            .zip(&self.default_open)
            .map(|(key, &default)| held.expanded.get(key).copied().unwrap_or(default))
            .collect();
        let rows: HashMap<&Key, usize> = keys.iter().enumerate().map(|(f, k)| (k, f)).collect();
        let at = |key: &Key| rows.get(key).copied();
        self.folds_away = held
            .expanded
            .iter()
            .filter(|(key, _)| !rows.contains_key(key))
            .map(|(key, &open)| (key.clone(), open))
            .collect();
        self.marked = held.marked.iter().filter_map(at).collect();
        self.visual = held.visual.as_ref().and_then(at);
        self.recompute_visible();
        let selected = match held.selected.as_ref().map(at) {
            Some(Some(f)) => Some(f),
            Some(None) => held
                .selected
                .as_ref()
                .and_then(|key| self.stand_in(key))
                .and_then(|key| at(&key))
                .or_else(|| self.in_place_of(held.above.iter().find_map(at))),
            None => held.fallback.as_ref().and_then(at),
        };
        match selected {
            Some(f) => self.reveal_and_select(f),
            None => self.reselect(None),
        }
        // **An anchor that is gone re-anchors at the cursor**, so a range and
        // the mode that says V-LINE stay together — for every rebuild, not
        // only a refresh's: a re-list in Visual (`a` then, `:unread` now),
        // begun on a conversation the new list lacks, left V-LINE over
        // nothing (review stop 3, A F1).
        if held.visual.is_some() && self.visual.is_none() {
            self.visual = self.selected_full();
        }
        // **The window's top by identity, as vim keeps a window's top line
        // while the buffer changes under it**: the row it showed first, or —
        // that row gone — the nearest above it still here (measured in
        // Neovim 0.12, 10-24: a deleted top becomes the line above what went).
        // Only a held cursor moves it: a view that held none stands as a
        // fresh one.
        //
        // **Rows that arrive at the top's edge show**, as Neovim shows lines
        // inserted just above the top (measured 10-25: n of them become the
        // top, the cursor n screen rows lower; a row two above the top stays
        // hidden; at the first line likewise). Arrived: every row between the
        // top row and the nearest row above the window still here, none of
        // them a row that was above the window — a row above it that
        // re-sorted over those (the default sort takes a session with a new
        // turn to the top) arrived nowhere near the edge. Steven, 10-25,
        // over my call that an arrival should only signal: the window at the
        // list's start stayed one row down when a session sorted to the top,
        // and nothing on the screen said so.
        //
        // **So long as it still shows the cursor.** A re-sort moves rows
        // wholesale — under the default sort a session's new turn takes it to
        // the top, its rows with it — and the top that followed its row put
        // the cursor on the window's bottom margin, the very scroll this keeps
        // (V4 verify, L1). Then the cursor keeps its row on the screen.
        if held.selected.is_some() {
            let top_row = match held.top {
                0 => held.selected.as_ref(),
                n => held.above.get(n - 1),
            };
            // The rows above the window when it was held, nearest first.
            let beyond = held.above.get(held.top..).unwrap_or_default();
            let arrived = |from: usize, top: usize| {
                let was_beyond: std::collections::HashSet<&Key> = beyond.iter().collect();
                (self.visible_order[from..top].iter()).all(|&f| !was_beyond.contains(&keys[f]))
            };
            let at_edge = (top_row.and_then(at)).and_then(|f| {
                let top = self.visible_pos(f);
                let from = (beyond.iter().find_map(at)).map_or(0, |f| self.visible_pos(f) + 1);
                (from < top && arrived(from, top)).then_some(from)
            });
            let by_identity = match held.top {
                0 => Some(self.selected_vis),
                n => (held.above.get(n - 1..).unwrap_or_default().iter())
                    .find_map(at)
                    .map(|f| self.visible_pos(f)),
            };
            let cursor = self.selected_vis;
            let shows = |top: usize| top <= cursor && held.height.is_none_or(|h| cursor - top < h);
            let top = [at_edge, by_identity]
                .into_iter()
                .flatten()
                .find(|&top| shows(top))
                .unwrap_or_else(|| cursor.saturating_sub(held.top));
            self.top.set(top);
        }
        self.last_height.set(held.height);
        self.pending = held.chord;
        self.settled = held.settled;
        self.keys = keys;
    }

    /// **Put back what the user held in another view of this tree** — one
    /// rebuilt over newer state (`App::refresh_graph_views`, run on every
    /// background event), or a split's copy of its window
    /// (`App::split_command`) — with `claude` the
    /// section to show, the other view's, with what was read of it: the
    /// cursor, folds, marks, a Visual anchor and the window's top, by
    /// identity, and a half-typed chord. A cursor that is gone
    /// lands on the row now in its place ([`in_place_of`](Self::in_place_of))
    /// and an anchor that is gone re-anchors at the cursor — both in
    /// [`rebuild`](Self::rebuild), as for every caller. Only a view that held
    /// no cursor takes where this one stands. One rebuild for the section and
    /// the holding both.
    pub(crate) fn restore(&mut self, claude: Option<ClaudeSection>, mut held: Held) {
        held.fallback = self.selected_full().map(|f| self.key_of(f));
        self.claude = claude;
        self.rebuild(held);
    }

    /// What the user holds now, by identity, for a [`rebuild`](Self::rebuild)
    /// or a [`restore`](Self::restore) to put back — a split's copy of the
    /// window included (`App::split_command`).
    ///
    /// **Only the folds the user set**: a fold still as its row's default was
    /// never chosen, so it follows the default as that changes. Carried
    /// whole, a conversation closed by default stayed closed when a session
    /// came to continue it — an import made while `:graph` was open
    /// elsewhere vanished into the fold (review stop 3, A F5).
    pub(crate) fn held(&self) -> Held {
        Held {
            selected: self.selected_full().map(|f| self.key_of(f)),
            expanded: (0..self.full.len())
                .filter(|&f| self.expanded[f] != self.default_open[f])
                .map(|f| (self.key_of(f), self.expanded[f]))
                .chain(
                    self.folds_away
                        .iter()
                        .map(|(key, &open)| (key.clone(), open)),
                )
                .collect(),
            with_turns: self.with_turns.clone(),
            marked: self.marked.iter().map(|&f| self.key_of(f)).collect(),
            visual: self.visual.map(|f| self.key_of(f)),
            above: self.visible_order[..self.selected_vis.min(self.visible_order.len())]
                .iter()
                .rev()
                .map(|&f| self.key_of(f))
                .collect(),
            // A top below the cursor — moved above the window since the
            // last frame — reads as the cursor's row: the next frame
            // scrolls to the cursor from either.
            top: self.selected_vis.saturating_sub(self.top.get()),
            fallback: None,
            height: self.last_height.get(),
            chord: self.pending,
            settled: self.settled.clone(),
        }
    }

    /// **Where a cursor whose row went lands: the row now in its place** —
    /// vim's `dd` (Steven, 10-17: "the row that takes its place, else the
    /// one above"). `survivor` is the nearest row shown above it that is
    /// still there: the cursor lands just past it, or on it when nothing
    /// follows; on the fold that now hides it, when one does (a
    /// conversation's last continuing session deleted closes the
    /// conversation, by default, over the place); on the first row when
    /// nothing above it survived. It landed where a fresh `:graph` does — on
    /// your session, a jump to the top after deleting a row near the bottom.
    ///
    /// Anchored ABOVE because what is above keeps its identity: a turn's key
    /// carries its place in its session, so removing a turn renumbers every
    /// turn after it, and a search forward for "the next row" by key found
    /// none of them.
    fn in_place_of(&self, survivor: Option<usize>) -> Option<usize> {
        let Some(f) = survivor else {
            return self.visible_order.first().copied();
        };
        match self.visible_order.binary_search(&f) {
            Ok(pos) => Some(self.visible_order.get(pos + 1).copied().unwrap_or(f)),
            Err(_) => self.visible_order.get(self.visible_pos(f)).copied(),
        }
    }

    /// **Where a row with no row now stands in — what still holds it**, or
    /// `None` for a row truly gone (`dd`'s landing then, `in_place_of`).
    ///
    /// A Shannon turn still in its session lands on the session: an import's
    /// first turns leave their rows when its conversation's reading lands —
    /// they are the conversation's, drawn once there — and `in_place_of` put
    /// the cursor on the next row, ANOTHER conversation, whose `<CR>` then
    /// imported that one (review stop 5, A 3). A Claude Code turn lands on its
    /// conversation, the row that reads it again: a reading let go
    /// (`:unread`), or a tree that has not read it (a split's copy before it
    /// carried the section, `:b N` to where you were) stood on the next
    /// conversation instead (V4 verify, M1).
    fn stand_in(&self, key: &Key) -> Option<Key> {
        match key {
            Key::Turn { episode, turn, nth } => (self.graph.placed(episode).iter())
                .filter(|p| p.turn == *turn)
                .nth(*nth)
                .map(|_| Key::Session(*episode)),
            Key::ClaudeTurn(id, _) => Some(Key::Conversation(id.clone())),
            _ => None,
        }
    }

    /// Row `f`'s identity ([`Key`]), as the last rebuild laid it out.
    fn key_of(&self, f: usize) -> Key {
        debug_assert_eq!(
            self.keys.len(),
            self.full.len(),
            "keys laid out with the rows"
        );
        self.keys[f].clone()
    }

    /// **Every row's identity**, from the rows just laid out: a turn's rank
    /// among its session's copies of the same content from one read of each
    /// session's sequence, not one per row.
    fn lay_keys(&self) -> Vec<Key> {
        let mut ranks: HashMap<EpisodeId, Vec<usize>> = HashMap::new();
        let mut rank = |episode: EpisodeId, index: usize| {
            let ranks = ranks.entry(episode).or_insert_with(|| {
                let mut seen: HashMap<TurnId, usize> = HashMap::new();
                (self.graph.placed(&episode).iter())
                    .map(|p| {
                        let n = seen.entry(p.turn).or_default();
                        *n += 1;
                        *n - 1
                    })
                    .collect()
            });
            // A place past the sequence is laid out by nothing; its place
            // stands in, as every key did before ranks.
            ranks.get(index).copied().unwrap_or(index)
        };
        let id = |at: usize| {
            self.claude
                .as_ref()
                .and_then(|c| c.found.get(at))
                .map(|found| found.id.clone())
                .unwrap_or_default()
        };
        (self.full.iter())
            .map(|row| match row.entry {
                Entry::Node(Node::Episode(id)) => Key::Session(id),
                Entry::Node(Node::Turn {
                    episode,
                    turn,
                    index,
                }) => Key::Turn {
                    episode,
                    turn,
                    nth: rank(episode, index),
                },
                Entry::Node(Node::Message {
                    episode,
                    turn,
                    turn_index,
                    index,
                }) => Key::Message {
                    episode,
                    turn,
                    nth: rank(episode, turn_index),
                    index,
                },
                Entry::ClaudeHeading => Key::Heading,
                Entry::Conversation(at) => Key::Conversation(id(at)),
                Entry::ClaudeTurn { conversation, turn } => Key::ClaudeTurn(id(conversation), turn),
            })
            .collect()
    }

    /// The listed conversation at `at` in the section.
    fn found_at(&self, at: usize) -> Option<&Found> {
        self.claude.as_ref()?.found.get(at)
    }

    /// **The Claude Code turn a row stands for**, as read — what the `git`
    /// style labels it with, and the inspector shows.
    pub fn claude_turn(&self, entry: Entry) -> Option<&ImportedTurn> {
        let Entry::ClaudeTurn { conversation, turn } = entry else {
            return None;
        };
        let found = self.found_at(conversation)?;
        self.claude.as_ref()?.read.get(&found.id)?.turns.get(turn)
    }

    /// **The Claude Code turn under the cursor** — its conversation, its
    /// place and how many there are, and the turn as read — for the
    /// inspector's page on it.
    pub fn selected_claude_turn(&self) -> Option<(&Found, usize, usize, &ImportedTurn)> {
        let entry = self.full[self.selected_full()?].entry;
        let Entry::ClaudeTurn { conversation, turn } = entry else {
            return None;
        };
        let found = self.found_at(conversation)?;
        let read = self.claude.as_ref()?.read.get(&found.id)?;
        Some((found, turn, read.turns.len(), read.turns.get(turn)?))
    }

    /// **The conversation under the cursor, if it has not been read** — what
    /// opening it (`l`) must read first, for its turns to be rows.
    pub fn unread_selected(&self) -> Option<&Found> {
        let found = self.selected_claude()?;
        let read = self.claude.as_ref()?.read.contains_key(&found.id);
        let f = self.selected_full()?;
        (!read && matches!(self.full[f].entry, Entry::Conversation(_))).then_some(found)
    }

    /// **The section's heading under the cursor, not read yet** — for `l`
    /// to ask whether Shannon may read here.
    pub fn unlisted_selected(&self) -> bool {
        self.claude.as_ref().is_some_and(|c| !c.listed)
            && self
                .selected_full()
                .is_some_and(|f| matches!(self.full[f].entry, Entry::ClaudeHeading))
    }

    /// **What a conversation reads as, given to this view**
    /// ([`ClaudeSection::offer`]) — its rows laid out again, what the user
    /// holds kept. No-op without a section.
    pub fn offer(&mut self, id: &str, shown: Option<Arc<Imported>>, awaiting: bool) {
        let held = self.held();
        if let Some(section) = self.claude.as_mut() {
            section.offer(id, shown, awaiting);
        }
        self.rebuild(held);
    }

    /// **The reading of the file at `path` (conversation `id`), come back
    /// from a reading thread** — `None`: it could not be read. Taken if this
    /// view asked for it or shows an older one, never pressed on a view that
    /// did neither: what a search finds would then turn on what happened to
    /// be read. Whether this view had asked. `by`: what scrubbed it
    /// ([`ClaudeSection::scrubbed`]).
    pub(crate) fn take_reading(
        &mut self,
        path: &Path,
        id: &str,
        read: Option<&Arc<Imported>>,
        by: &Scrubber,
    ) -> bool {
        let Some(section) = self.claude.as_mut() else {
            return false;
        };
        let (asked, taken) = section.take_reading(path, id, read, by);
        if taken {
            let held = self.held();
            self.rebuild(held);
        }
        asked
    }

    /// **Where this window is in its tree, as a place** ([`TreePlace`]) —
    /// to set it aside, or for a split's copy to start from
    /// (`App::split_command`): what it holds, and its section (the readings
    /// shared).
    pub(crate) fn place(&self) -> TreePlace {
        TreePlace {
            tree: self.tree,
            held: self.held(),
            claude: self.claude.clone(),
        }
    }

    /// Record what background work each episode has; their rows draw a marker.
    pub fn set_activity(&mut self, activity: std::collections::HashMap<EpisodeId, Activity>) {
        self.activity = activity;
    }

    /// What background work this episode has, if any — the inspector's state
    /// row reads it (the tree rows carry it via `DisplayRow::activity`).
    pub fn activity_of(&self, id: &EpisodeId) -> Option<Activity> {
        self.activity.get(id).copied()
    }

    /// The glyph set this view was built with — the renderer reads the busy
    /// marker (and anything else chrome-level) from here so tree, lanes, and
    /// status decorations can never disagree.
    pub fn glyphs(&self) -> NodeGlyphs {
        self.glyphs
    }

    /// The head-dot glyph for an episode row — its origin kind's mark from the
    /// resolved set (child sessions read differently from branches at a glance).
    fn head_glyph(&self, id: &EpisodeId) -> char {
        self.glyphs.for_kind(
            self.graph
                .episode(id)
                .and_then(|e| e.origin)
                .map(|o| o.kind),
        )
    }

    /// Set the scrolloff margin (rows of context kept around the selection).
    /// The default matches `tui.scrolloff`'s coded default; the App threads the
    /// user's configured value through here.
    pub fn set_scrolloff(&mut self, rows: usize) {
        self.scrolloff = rows;
    }

    /// Mark which episode is `HEAD` — where a `resume = last` launch opens — so
    /// the tree can flag it. The App passes the stored ref.
    pub fn set_active(&mut self, episode: Option<EpisodeId>) {
        self.active = episode;
    }

    /// Where the cursor is, by identity ([`Spot`]) — `None` in a tree with
    /// no rows.
    pub(crate) fn spot(&self) -> Option<Spot> {
        self.selected_full().map(|f| Spot(self.key_of(f)))
    }

    /// **Where it is, if it moved since last asked** — for
    /// `App::settle_graph`, which records it as where you were in the tree,
    /// as `PageWindow::moved` is for a page: a window that did not move is
    /// not where you were reading. With two on one tree (`:sp`), read in
    /// one and `<C-w>w` through the other, the place kept was the other's
    /// (review stop 10, D 3, probe P2).
    pub(crate) fn moved(&mut self) -> Option<Spot> {
        let spot = self.spot()?;
        if self.settled.as_ref() == Some(&spot) {
            return None;
        }
        self.settled = Some(spot.clone());
        Some(spot)
    }

    /// **Put the cursor back on `spot`** — its row revealed, and put in the
    /// window's middle at the next frame, as vim's `:b` restores a buffer's
    /// `'"` (the cursor, not the old scroll; a page's line, likewise) — or
    /// on what stands in for its row ([`stand_in`](Self::stand_in): a Claude
    /// Code turn this tree has not read, its conversation). `false`,
    /// nothing moved, when the tree no longer holds it.
    pub(crate) fn go_to(&mut self, spot: &Spot) -> bool {
        let row = |key: &Key| self.keys.iter().position(|k| k == key);
        let found = row(&spot.0).or_else(|| self.stand_in(&spot.0).and_then(|key| row(&key)));
        let Some(f) = found else {
            return false;
        };
        self.reveal_and_select(f);
        self.placing.set(Some(crate::viewport::Place::Middle));
        true
    }

    /// Reveal and select an episode's row — expanding the path to it if it's a
    /// nested branch — so opening `:graph` lands on *your* session rather than the
    /// top of the tree. No-op if the episode isn't in the graph.
    pub fn reveal_and_select_episode(&mut self, id: EpisodeId) {
        let Some(target) = self
            .full
            .iter()
            .position(|r| r.entry == Entry::Node(Node::Episode(id)))
        else {
            return;
        };
        self.reveal_and_select(target);
    }

    /// Land on an episode's **last turn** rather than its header row, falling
    /// back to the header for a session that has not spoken yet.
    ///
    /// Where you are in a conversation is the last thing in it, and that is what
    /// `:graph` should open onto — Steven asked for exactly this ("when entering
    /// `:graph` we should land on the current turn too"). Landing on the session
    /// header instead means the first thing you do every time is press `G`.
    ///
    /// Matches on the LAST row belonging to this episode, which is its own
    /// occurrence identity — never a search by `TurnId`, which is content and
    /// recurs (`[[feedback_content_addressed_ids]]`).
    pub fn reveal_and_select_head_turn(&mut self, id: EpisodeId) {
        let last = self.full.iter().rposition(
            |r| matches!(r.entry, Entry::Node(Node::Turn { episode, .. }) if episode == id),
        );
        match last {
            Some(target) => self.reveal_and_select(target),
            None => self.reveal_and_select_episode(id),
        }
    }

    /// **Land on turn `index` of `id`** — the turn you were reading when
    /// `:graph` opened from a transcript (Steven, 11-01, image 11: it landed
    /// on the session's last turn while the cursor stood on its second) —
    /// else on its last ([`reveal_and_select_head_turn`]). By the turn's
    /// occurrence, `index`, never its content.
    ///
    /// **Where the tree draws it**: a branch's row holds only the turns it
    /// owns, and one it shares is drawn once, under the session that owns it
    /// (`Graph::placed`'s `owner`, `owner_index`). Read on a branch, it found
    /// no row of the branch's and fell to the branch's own row — the turn
    /// you were reading not selected (Steven, 11-01, image 15; review stop
    /// 15, A M7 / B / C H3).
    ///
    /// [`reveal_and_select_head_turn`]: Self::reveal_and_select_head_turn
    pub fn reveal_and_select_turn(&mut self, id: EpisodeId, index: usize) {
        let at = |rows: &[Row], (id, index): (EpisodeId, usize)| {
            rows.iter().position(|r| {
                matches!(r.entry, Entry::Node(Node::Turn { episode, index: at, .. })
                    if episode == id && at == index)
            })
        };
        let owner = (self.graph.placed(&id).get(index)).map(|p| (p.owner, p.owner_index));
        let row = at(&self.full, (id, index)).or_else(|| owner.and_then(|o| at(&self.full, o)));
        match row {
            Some(target) => self.reveal_and_select(target),
            None => self.reveal_and_select_head_turn(id),
        }
    }

    /// Jump the selection to the next (`forward`) or previous **episode** row in
    /// preorder — a session-to-session hop that skips over turns and messages, the
    /// nodes you rarely want to land on when picking a session to act on. Searches
    /// the *whole* tree (not just visible rows) and reveals a buried branch when it
    /// lands there, so every session is reachable in a couple of keystrokes. A
    /// no-op past the last / before the first session.
    pub fn select_next_episode(&mut self) {
        self.jump_episode(true);
    }

    /// Previous-episode hop — see [`select_next_episode`](Self::select_next_episode).
    pub fn select_prev_episode(&mut self) {
        self.jump_episode(false);
    }

    fn jump_episode(&mut self, forward: bool) {
        let is_episode = |i: usize| matches!(self.full[i].entry, Entry::Node(Node::Episode(_)));
        let cur = self.visible_order.get(self.selected_vis).copied();
        let target = if forward {
            (cur.map_or(0, |c| c + 1)..self.full.len()).find(|&i| is_episode(i))
        } else {
            (0..cur.unwrap_or(0)).rev().find(|&i| is_episode(i))
        };
        if let Some(t) = target {
            self.reveal_and_select(t);
        }
    }

    /// Reveal a `full` row (expand every ancestor) and select it. Shared by
    /// [`reveal_and_select_episode`](Self::reveal_and_select_episode) and the
    /// session hops.
    fn reveal_and_select(&mut self, target: usize) {
        let mut ancestor = self.parent[target];
        while let Some(a) = ancestor {
            self.expanded[a] = true;
            ancestor = self.parent[a];
        }
        self.recompute_visible();
        if let Some(pos) = self.visible_order.iter().position(|&f| f == target) {
            self.selected_vis = pos;
        }
    }

    /// The graph snapshot this view projects — the inspector reads node details
    /// (provenance, config, message content) out of it.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Load `turns`' bodies into the snapshot, through the store's one door
    /// ([`hydrate_turns`](shannon_core::store::hydrate_turns)) — for the
    /// inspector, the one reader of a graph view that shows a message's words.
    /// A body already held costs nothing. The tree itself draws from outlines;
    /// nothing about its shape depends on a body, so this never moves a row or
    /// the selection.
    pub fn hydrate(
        &mut self,
        store: &dyn shannon_core::Store,
        turns: &[shannon_core::TurnId],
    ) -> shannon_core::StoreResult<()> {
        shannon_core::store::hydrate_turns(store, &mut self.graph, turns)
    }

    /// The active (live / `HEAD`) episode, if set — the `git` style decorates it
    /// with `(HEAD)`.
    pub fn active(&self) -> Option<EpisodeId> {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.full.is_empty()
    }

    /// Number of rows the cursor can be on — every visible row, Shannon's and
    /// Claude Code's (the denominator the status line shows).
    pub fn visible_len(&self) -> usize {
        self.visible_order.len()
    }

    /// **Show Claude Code's conversations after Shannon's sessions**, or
    /// none — rebuilt in place, keeping what the user holds.
    pub fn set_claude(&mut self, section: Option<ClaudeSection>) {
        let held = self.held();
        self.claude = section;
        self.rebuild(held);
    }

    /// The Claude Code section, if there is one — for a rebuild to carry.
    pub fn claude(&self) -> Option<&ClaudeSection> {
        self.claude.as_ref()
    }

    /// The Claude Code conversations a session of this tree continues, by
    /// id — what a section built for it must list and read.
    pub fn continued(&self) -> &BTreeSet<String> {
        &self.continued
    }

    /// The Claude Code conversation under the cursor — on its row or one of
    /// its turns — if that is where it is.
    pub fn selected_claude(&self) -> Option<&Found> {
        match self.full[self.selected_full()?].entry {
            Entry::Conversation(at)
            | Entry::ClaudeTurn {
                conversation: at, ..
            } => self.found_at(at),
            Entry::Node(_) | Entry::ClaudeHeading => None,
        }
    }

    /// **The sessions going on with the Claude Code conversation `id`** —
    /// the ones hung under it, as `rebuild` homes them — the
    /// most recently active first.
    pub fn continuing(&self, id: &str) -> Vec<EpisodeId> {
        let graph = &self.graph;
        let goes_on = |node: &Node| match node {
            Node::Episode(e) => graph
                .episode(e)
                .and_then(|e| e.inherited.as_ref())
                .is_some_and(|from| from.session == id),
            _ => false,
        };
        let mut sessions: Vec<Node> = graph.roots().into_iter().filter(goes_on).collect();
        sessions.sort_by(|a, b| recency_cmp(graph, *a, *b));
        sessions
            .into_iter()
            .filter_map(|node| match node {
                Node::Episode(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    /// **What `<CR>` opens on a Claude Code row** (Steven, 10-17, review stop
    /// 3 A F8: "import up to that turn and open the continuing session"): on
    /// a conversation a session goes on with, that session; on one of its
    /// turns, a copy up to it; on any other conversation, a copy as
    /// `claude --resume` holds it. `None` off the section's rows.
    pub fn claude_open(&self) -> Option<ClaudeOpen> {
        let found = self.selected_claude()?;
        match self.full[self.selected_full()?].entry {
            Entry::Conversation(_) => Some(match self.continuing(&found.id).split_first() {
                Some((&id, rest)) => ClaudeOpen::Session {
                    id,
                    others: rest.len(),
                },
                None => ClaudeOpen::Import {
                    path: found.path.clone(),
                    through: None,
                },
            }),
            Entry::ClaudeTurn { turn, .. } => {
                use shannon_core::claude_import::Until;
                let read = self.claude.as_ref()?.read.get(&found.id)?;
                // Before the turn after; on the last, after the reading's
                // leaf — the copy what was shown, however far the file has
                // gone on since (A 3). A next turn named by no entry would be
                // a reading's first, never a next.
                let until = match read.turns.get(turn + 1) {
                    Some(next) => next.uuid.clone().map(Until::Before),
                    None => read.leaf.clone().map(Until::After),
                };
                let through = until.map(|until| Through {
                    turn,
                    of: read.turns.len(),
                    until,
                });
                Some(ClaudeOpen::Import {
                    path: found.path.clone(),
                    through,
                })
            }
            Entry::Node(_) | Entry::ClaudeHeading => None,
        }
    }

    /// Whether the cursor is on a Claude Code row — the heading, a
    /// conversation, or one of its turns.
    pub fn on_claude(&self) -> bool {
        self.selected_full()
            .is_some_and(|f| self.full[f].entry.node().is_none())
    }

    /// **How many Claude Code conversations the Visual range covers** — what
    /// a verb acting on the range passes over, for it to say so. Not the
    /// cursor's: without a range a verb acts on the marks, if any, and the
    /// cursor is [`on_claude`](Self::on_claude).
    ///
    /// Counted by CONVERSATION, its row or any of its turns in the range: a
    /// range over a conversation's turns, beside a session hung under one,
    /// passed them over and said nothing (review stop 3, A F9, C 10).
    pub fn claude_chosen(&self) -> usize {
        self.range_rows()
            .iter()
            .filter_map(|&f| match self.full[f].entry {
                Entry::Conversation(at)
                | Entry::ClaudeTurn {
                    conversation: at, ..
                } => Some(at),
                Entry::Node(_) | Entry::ClaudeHeading => None,
            })
            .collect::<BTreeSet<usize>>()
            .len()
    }

    /// The highlighted row's position within the visible list (`0` when empty).
    pub fn selected(&self) -> usize {
        self.selected_vis
    }

    /// The row under the cursor, whatever it stands for — the inspector's
    /// page identity.
    pub fn selected_entry(&self) -> Option<Entry> {
        self.selected_full().map(|f| self.full[f].entry)
    }

    /// The selected graph node — what the inspector pane details (P4.3).
    /// `None` on a Claude Code row, which is no node.
    pub fn selected_node(&self) -> Option<Node> {
        self.selected_full().and_then(|f| self.full[f].entry.node())
    }

    /// `<Space>`: toggle the **mark** on the selected node (the multi-select for a
    /// `pick`). A no-op if nothing's selected, or on a row that is no node.
    pub fn toggle_mark(&mut self) {
        if let Some(f) = self.selected_full()
            && self.full[f].entry.node().is_some()
            && !self.marked.insert(f)
        {
            self.marked.remove(&f);
        }
    }

    /// The marked nodes in **preorder** (graph / top-to-bottom order — the order a
    /// `pick` composes them). Empty if nothing's marked.
    pub fn marked_nodes(&self) -> Vec<Node> {
        self.marked
            .iter()
            .filter_map(|&f| self.full[f].entry.node())
            .collect()
    }

    /// Drop every mark — `mu`. A verb spends only the marks it acted on
    /// ([`unmark`](Self::unmark)).
    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// The tree this is a window on ([`GraphBuffer`]).
    pub(crate) fn tree(&self) -> BufferId {
        self.tree
    }

    /// **This test's view, on `tree`** — a view the tests build
    /// ([`new`](Self::new)) put in a window on a real tree
    /// (`App::set_graph_view_for_test`).
    #[cfg(test)]
    pub(crate) fn on_tree(mut self, tree: BufferId) -> Self {
        self.tree = tree;
        self
    }

    /// The marked rows, by identity — what the tree keeps of them.
    fn marked_keys(&self) -> Vec<Key> {
        self.marked.iter().map(|&f| self.key_of(f)).collect()
    }

    /// Mark the rows `keys` name, and only those — the tree's marks, shown
    /// in this window. A key with no row here marks nothing.
    fn set_marked(&mut self, keys: &[Key]) {
        let wanted: std::collections::HashSet<&Key> = keys.iter().collect();
        self.marked = (0..self.keys.len())
            .filter(|&f| wanted.contains(&self.keys[f]))
            .collect();
    }

    /// **The one answer to "what does this verb act on?"**
    ///
    /// Precedence: a **Visual range** — the gesture just made — then netrw's
    /// **marked set**, then the **row under the cursor**. A range holds the
    /// rows as shown: a folded session is the session, and the turns a fold
    /// hides are not separately in it.
    ///
    /// Every verb asks this. It used to be answered independently at six sites
    /// — `p` and `S` read the marks, while `d`, `c`, `m` and `r` read only
    /// `selected_node` and never looked. So `d` with two sessions marked
    /// deleted one and the confirmation named one, which meant the marks were a
    /// lie for four of the six verbs a user could reach them from
    /// (`[[feedback_one_door_per_invariant]]`).
    ///
    /// In **preorder**, so a caller that composes (`pick`) gets graph order for
    /// free and one that iterates (`d`) is deterministic. Empty only when the
    /// tree is empty; callers refuse on empty rather than silently doing
    /// nothing.
    pub fn acts_on(&self) -> Vec<Node> {
        let chosen = self.chosen();
        if chosen.is_empty() {
            return self.selected_node().into_iter().collect();
        }
        chosen
    }

    /// **What was picked out**: the Visual range, else the marks —
    /// [`acts_on`](Self::acts_on) without its cursor fallback, for a verb
    /// whose "nothing picked" means something else (`p` refuses; `S` squashes
    /// the branch under the cursor). Read from here rather than from the
    /// marks, or such a verb ignores a range on screen.
    pub fn chosen(&self) -> Vec<Node> {
        match self.visual_rows() {
            Some(rows) => rows,
            None => self.marked_nodes(),
        }
    }

    /// Unmark `nodes` — what a verb spends of the marks: those it acted on.
    /// A verb that took a range leaves the marks elsewhere alone (netrw's
    /// marks are spent only by what used them).
    pub fn unmark(&mut self, nodes: &[Node]) {
        let full = &self.full;
        self.marked
            .retain(|&f| full[f].entry.node().is_none_or(|n| !nodes.contains(&n)));
    }

    /// The Visual range's rows, as indices into `full`, top to bottom —
    /// empty outside Visual. Clamped to the rows there are.
    fn range_rows(&self) -> &[usize] {
        let Some(span) = crate::rows::RowSelect::selection(self) else {
            return &[];
        };
        let end = span.end().saturating_add(1).min(self.visible_order.len());
        self.visible_order
            .get((*span.start()).min(end)..end)
            .unwrap_or_default()
    }

    /// The Visual range's NODES, top to bottom — `None` outside Visual, and
    /// empty when the range lies wholly on Claude Code rows, which are no
    /// nodes. `Some(empty)` is still a range: it must not fall through to
    /// the marks, as an unclamped `get` once made it.
    fn visual_rows(&self) -> Option<Vec<Node>> {
        self.visual?;
        Some(
            self.range_rows()
                .iter()
                .filter_map(|&f| self.full[f].entry.node())
                .collect(),
        )
    }

    /// **What `mf` and `<Space>` do** — one answer for both keys: in Visual,
    /// mark the range; otherwise toggle the cursor row. How many rows of the
    /// range were marked (`None` outside Visual), for a range of rows that
    /// are no nodes to say so.
    pub fn mark_key(&mut self) -> Option<usize> {
        if self.visual.is_some() {
            Some(self.mark_selection())
        } else {
            self.toggle_mark();
            None
        }
    }

    /// Mark every row of the Visual range (netrw's Visual `mf`), so ranges
    /// build a set. Adds, never toggles: a range half marked comes out whole.
    /// How many rows that was.
    pub fn mark_selection(&mut self) -> usize {
        let rows: Vec<usize> = self
            .range_rows()
            .iter()
            .copied()
            .filter(|&f| self.full[f].entry.node().is_some())
            .collect();
        self.marked.extend(&rows);
        rows.len()
    }

    /// Where a Visual selection is anchored, by node — `None` on a Claude
    /// Code row. A test's reading: a rebuild carries the anchor by identity
    /// ([`held`](Self::held)).
    #[cfg(test)]
    pub fn visual_anchor(&self) -> Option<Node> {
        self.full[self.visual?].entry.node()
    }

    /// Whether a Visual range is up — anchored on any row.
    pub fn in_visual(&self) -> bool {
        self.visual.is_some()
    }

    /// Anchor a Visual selection at `node`, if the tree still has it — for a
    /// test to set a range up by node.
    #[cfg(test)]
    pub fn anchor_visual_at(&mut self, node: Node) -> bool {
        self.visual = self.row_of(node);
        self.visual.is_some()
    }

    /// The `full` row that is `node`, if the tree has it.
    #[cfg(test)]
    fn row_of(&self, node: Node) -> Option<usize> {
        self.full
            .iter()
            .position(|row| row.entry == Entry::Node(node))
    }

    /// Where the anchor row `f` is on screen, as a cursor position.
    fn anchor_pos(&self, f: usize) -> usize {
        self.visible_pos(f)
    }

    /// The visible position of `full[f]` — or, when a fold hides it, of the
    /// row that hides it. `visible_order` is a preorder subsequence, so it is
    /// sorted, and the nearest visible row before a hidden one is its
    /// outermost collapsed ancestor: everything between the two is inside
    /// that fold.
    fn visible_pos(&self, f: usize) -> usize {
        match self.visible_order.binary_search(&f) {
            Ok(pos) => pos,
            Err(pos) => pos.saturating_sub(1),
        }
    }

    /// Mark `nodes` **by identity** — for a test to set marks up. (A rebuild
    /// carries marks the same way, through [`held`](Self::held): `marked`
    /// holds row indices, which a rebuild invalidates, so the node is what
    /// travels.) Returns how many of `nodes` the tree has.
    #[cfg(test)]
    pub fn mark_nodes(&mut self, nodes: &[Node]) -> usize {
        let mut found = 0;
        for node in nodes {
            if let Some(idx) = self.row_of(*node) {
                self.marked.insert(idx);
                found += 1;
            }
        }
        found
    }

    /// Move the cursor to `node` if the tree still has it, revealing it if it
    /// sits under a fold — for a test to put the cursor somewhere. `false`
    /// when it is gone.
    #[cfg(test)]
    pub fn select_node(&mut self, node: Node) -> bool {
        let Some(idx) = self.row_of(node) else {
            return false;
        };
        self.reveal_and_select(idx);
        true
    }

    /// The episode the selected node belongs to — the target of "load this
    /// session" (`Enter`). For an episode node it's that episode; for a turn or
    /// message it's the episode that contains it.
    pub fn selected_episode(&self) -> Option<EpisodeId> {
        self.selected_node().map(|node| match node {
            Node::Episode(id) => id,
            Node::Turn { episode, .. } | Node::Message { episode, .. } => episode,
        })
    }

    /// The viewport-sized slice of visible rows to draw, top-most first.
    /// Only the `height` rows the window shows are materialized —
    /// `O(viewport)`.
    pub fn window(&self, height: usize) -> Vec<DisplayRow<'_>> {
        self.tree_rows(self.window_top(height), height)
    }

    /// **A Claude Code row as the renderer draws it** — its section's
    /// heading, or a conversation and whether a Shannon session continues
    /// it. `None` for a row drawn as a tree's is — a node, or a
    /// conversation's turn, which sits on its conversation's lane.
    pub fn claude_row<'a>(&'a self, row: &DisplayRow<'_>) -> Option<ClaudeRow<'a>> {
        let section = self.claude.as_ref()?;
        let (selected, visual) = (row.selected, row.visual);
        match row.entry {
            Entry::Node(_) | Entry::ClaudeTurn { .. } => None,
            Entry::ClaudeHeading => Some(ClaudeRow::Heading {
                count: section.listed.then_some(section.found.len()),
                selected,
                visual,
            }),
            Entry::Conversation(at) => {
                let found = section.found.get(at)?;
                Some(ClaudeRow::Conversation {
                    found,
                    continued: self.continued.contains(&found.id),
                    // Open to what hangs from it, its turns are still folded.
                    folded: row.fold == Some(false)
                        || (row.fold.is_some() && !self.with_turns.contains(&found.id)),
                    awaiting: section.awaiting.contains(&found.id),
                    resumes: self.resume_notes.get(&at).map(String::as_str),
                    selected,
                    visual,
                })
            }
        }
    }

    /// The rows in a window `height` rows tall whose first row is `top`.
    pub fn tree_rows(&self, top: usize, height: usize) -> Vec<DisplayRow<'_>> {
        let end = top.saturating_add(height).min(self.visible_order.len());
        let top = top.min(end);
        let visual = crate::rows::RowSelect::selection(self);
        self.visible_order[top..end]
            .iter()
            .enumerate()
            .map(|(k, &f)| {
                let at = top + k;
                self.display_row(
                    f,
                    at == self.selected_vis,
                    visual.as_ref().is_some_and(|v| v.contains(&at)),
                )
            })
            .collect()
    }

    /// **The rows `/` searches**, in preorder, as drawn
    /// (`view::graph_row_texts`) — the order [`search`](Self::search) maps
    /// its hit back through, both read from one `searched`.
    pub fn searched_tree_rows(&self) -> Vec<DisplayRow<'_>> {
        self.searched()
            .into_iter()
            .map(|f| self.display_row(f, false, false))
            .collect()
    }

    /// **Which tree rows `/` searches**, as `full` indices in preorder: every
    /// row drawn, and every session and turn a fold hides — as vim's search
    /// opens a closed fold to reach a match (`'foldopen'` holds `search`); a
    /// session forked from a turn hangs in that turn's fold (Steven, 10-15:
    /// "it appears it is gone"). Not a folded turn's MESSAGES: each repeats
    /// its turn's opening words, so every match would be found twice and
    /// open every fold on the way. A folded row is built as it would draw
    /// once opened; its connectors are a guess (which sibling is last is
    /// known only for drawn rows), its words are not.
    ///
    /// **Nor a closed conversation's turns**: they are not laid out at all
    /// until `l` opens them (`show_turns`), so there are no rows here to
    /// search — the boundary, not a fold (Steven, 10-18: "searching stuff
    /// that isn't read or not 'allowed' makes sense with the boundary").
    fn searched(&self) -> Vec<usize> {
        let mut drawn = vec![false; self.full.len()];
        for &f in &self.visible_order {
            drawn[f] = true;
        }
        (0..self.full.len())
            .filter(|&f| {
                drawn[f] || !matches!(self.full[f].entry, Entry::Node(Node::Message { .. }))
            })
            .collect()
    }

    /// Row `f` of the tree as drawn — the one builder [`tree_rows`] and
    /// [`searched_tree_rows`] share.
    ///
    /// [`tree_rows`]: Self::tree_rows
    /// [`searched_tree_rows`]: Self::searched_tree_rows
    fn display_row(&self, f: usize, selected: bool, visual: bool) -> DisplayRow<'_> {
        let row = &self.full[f];
        let episode = match row.entry {
            Entry::Node(Node::Episode(id)) => Some(id),
            _ => None,
        };
        DisplayRow {
            depth: row.depth,
            prefix: self.connector_prefix(f),
            lane: self.lane_cells(f),
            label: &row.label,
            entry: row.entry,
            fold: row.has_children.then(|| self.expanded[f]),
            selected,
            active: episode.is_some() && episode == self.active,
            marked: self.marked.contains(&f),
            visual,
            shared: row.shared,
            activity: episode.and_then(|id| self.activity.get(&id).copied()),
        }
    }

    /// First visible-row index for a viewport `height` rows tall. **Stateful,
    /// vim-style**: the window moves only when the selection presses into the
    /// [`scrolloff`](Self::set_scrolloff) margin at either edge, so the cursor
    /// travels through the window with context visible above *and* below it —
    /// never pinned to an edge mid-list. At the ends of the list the cursor is
    /// allowed into the margin (there's nothing more to reveal), exactly like
    /// vim. The previous top lives in a `Cell` because the (immutable) render
    /// pass is what knows the height and slides it. `O(1)`.
    ///
    /// **A window `zt` put past the end stays there** until a resize, as a text
    /// page's does — the same two rules, from the same module. The early return
    /// this had (`len <= height` → top 0) is what would have snapped it back:
    /// `follow` already answers 0 for a list that fits, unless the window was
    /// deliberately placed further down.
    pub fn window_top(&self, height: usize) -> usize {
        let len = self.visible_len();
        let resized = self.last_height.replace(Some(height)) != Some(height);
        if let Some(place) = self.placing.take() {
            (self.top).set(crate::viewport::place_top(
                place,
                self.selected_vis,
                1,
                height,
                self.scrolloff,
            ));
        }
        let ceiling = crate::viewport::top_ceiling(len, len.saturating_sub(1), height, resized);
        // **The shared rule, not a second copy of it.** `viewport::follow`'s
        // own module doc says this scroll-follow was "written twice, in
        // `Output::follow_cursor` and `GraphView::window_top`, independently and
        // identically… one gets a fix and the other does not, and nothing
        // fails" — and then only two of the three call sites were migrated.
        // This one kept the pre-fix arithmetic, including the `selected_vis +
        // off + 1` that overflows before its `saturating_sub` can run
        // (Reviewer B). Behaviourally identical, verified against all 19
        // `graph_view` tests; what changes is that there is one rule again.
        let top = crate::viewport::follow(
            self.selected_vis,
            self.top.get().min(ceiling),
            len,
            height,
            self.scrolloff,
        );
        self.top.set(top);
        top
    }

    /// `H` `M` `L`: select a row by its place in a window `height` rows tall —
    /// vim's rule, from [`viewport::screen_line`](crate::viewport::screen_line).
    ///
    /// Settles the window first, because the top it reads is the one the last
    /// frame drew and the selection may have moved since.
    pub fn select_screen_row(&mut self, which: crate::viewport::ScreenLine, height: usize) {
        let len = self.visible_len();
        if len == 0 || height == 0 {
            return;
        }
        let first = self.window_top(height);
        let last = first.saturating_add(height - 1);
        self.selected_vis =
            crate::viewport::screen_line(which, first, last, len, self.scrolloff, false);
    }

    /// `zt` `zz` `zb`: put the selected row at the top, middle or bottom of a
    /// window `height` rows tall — past the end if that is where it lands, as
    /// in vim. One row per node, so the row count is 1.
    pub fn place_selection(&mut self, place: crate::viewport::Place, height: usize) {
        self.window_top(height); // settle: this is not a resize
        self.top.set(crate::viewport::place_top(
            place,
            self.selected_vis,
            1,
            height,
            self.scrolloff,
        ));
    }

    /// `{n}G`: select visible row `row` (0-based), clamped to the last.
    pub fn select_row(&mut self, row: usize) {
        self.selected_vis = row.min(self.visible_len().saturating_sub(1));
    }

    /// **`/` `n` `N` over the rows** (Steven, 10-14: "Searching is helpful for
    /// moving, not _just_ finding") — `rows` the text of every row it
    /// searches ([`searched_tree_rows`](Self::searched_tree_rows): the drawn
    /// ones, and the sessions and turns a fold hides; Claude Code's with
    /// them), as drawn (`view::graph_row_texts`), searched by the one search
    /// every pane with text shares (`prompt::Buffer::search`): its regex, its
    /// wrap past an end. A match in a fold opens the folds above it, as vim's
    /// search does. A row is the unit: each step starts past the whole row it
    /// stands on (or before it, going back), so `3n` is three rows and a
    /// match on the selected row is reached only by going round.
    /// `Some(wrapped)`, or `None` when no row matches, the selection unmoved.
    pub fn search(
        &mut self,
        rows: Vec<String>,
        re: &regex::Regex,
        forward: bool,
        count: usize,
    ) -> Option<bool> {
        let order = self.searched();
        let from = self
            .selected_full()
            .and_then(|f| order.iter().position(|&o| o == f))
            .unwrap_or(0);
        let mut text = crate::prompt::Buffer::from_lines(rows);
        text.place_cursor(from, 0);
        let mut wrapped = false;
        for _ in 0..count.max(1) {
            let row = text.cursor().0;
            text.place_cursor(row, if forward { usize::MAX } else { 0 });
            wrapped |= text.search(re, forward, 1)?;
        }
        if let Some(&f) = order.get(text.cursor().0) {
            self.reveal_and_select(f);
        }
        Some(wrapped)
    }

    /// `5j`, `<C-d>`: move the selection `delta` rows, clamped at both ends —
    /// one step for any count, not `count` steps of one.
    pub fn move_selection(&mut self, delta: isize) {
        self.select_row(self.selected_vis.saturating_add_signed(delta));
    }

    // --- navigation -------------------------------------------------------

    /// Move the selection to the next visible row (no-op at the bottom).
    pub fn select_down(&mut self) {
        if self.selected_vis + 1 < self.visible_len() {
            self.selected_vis += 1;
        }
    }

    /// Move the selection to the previous visible row (no-op at the top).
    pub fn select_up(&mut self) {
        self.selected_vis = self.selected_vis.saturating_sub(1);
    }

    /// Jump to the first visible row (`gg`).
    pub fn select_top(&mut self) {
        self.selected_vis = 0;
    }

    /// Jump to the last visible row (`G`).
    pub fn select_bottom(&mut self) {
        self.selected_vis = self.visible_len().saturating_sub(1);
    }

    /// `l` / `→`: expand the selected node (reveal its children). A no-op on a
    /// leaf or an already-open node — for that case `l` does *not* descend (use
    /// `j`), matching the conservative tree-`l`. On a Claude Code
    /// conversation, one level more each time (`deeper`).
    pub fn expand(&mut self) {
        let Some(f) = self.selected_full() else {
            return;
        };
        match self.deeper(f) {
            Some(Deeper::Fold) => self.set_expanded(f, true),
            Some(Deeper::Turns) => self.show_turns(true),
            None => {}
        }
    }

    /// `h` / `←`: collapse the selected node if it's open; otherwise jump to its
    /// parent (vim's tree-`h` — back out of the current level). A Claude Code
    /// conversation's turns are hidden first, then what hangs from it.
    pub fn collapse(&mut self) {
        let Some(f) = self.selected_full() else {
            return;
        };
        if self.turns_shown(f) {
            self.show_turns(false);
        } else if self.full[f].has_children && self.expanded[f] {
            self.set_expanded(f, false);
        } else if let Some(p) = self.parent[f] {
            self.reselect(Some(p));
        }
    }

    /// **What `l` shows next on row `f`**, if anything — a fold's children;
    /// on a Claude Code conversation, what hangs from it, then its turns
    /// (Steven, 10-18: "somewhere in between"). Its reading is asked for at
    /// the first `l` either way (`App::expand_graph_row`), so the turns are
    /// likelier read by the second.
    fn deeper(&self, f: usize) -> Option<Deeper> {
        if !self.full[f].has_children {
            return None;
        }
        let closed = !self.expanded[f];
        let Entry::Conversation(_) = self.full[f].entry else {
            return closed.then_some(Deeper::Fold);
        };
        // Its turns hidden, what hangs from it is under its row.
        let hangs = self.parent.get(f + 1) == Some(&Some(f));
        match (self.turns_shown(f), closed) {
            (true, _) => closed.then_some(Deeper::Fold),
            (false, true) if hangs => Some(Deeper::Fold),
            (false, _) => Some(Deeper::Turns),
        }
    }

    /// Whether row `f` is a Claude Code conversation with its turns shown.
    fn turns_shown(&self, f: usize) -> bool {
        match self.full[f].entry {
            Entry::Conversation(at) => self
                .found_at(at)
                .is_some_and(|found| self.with_turns.contains(&found.id)),
            _ => false,
        }
    }

    /// **Show or hide the turns of the conversation under the cursor** —
    /// what hangs from one of them moves between it and the row, so the rows
    /// are laid out again, the cursor kept on the conversation (by its key,
    /// through `rebuild`). Hidden with nothing hanging from it, it is closed.
    fn show_turns(&mut self, show: bool) {
        let Some(f) = self.selected_full() else {
            return;
        };
        let Some(id) = (match self.full[f].entry {
            Entry::Conversation(at) => self.found_at(at).map(|found| found.id.clone()),
            _ => None,
        }) else {
            return;
        };
        match show {
            true => self.with_turns.insert(id),
            false => self.with_turns.remove(&id),
        };
        self.expanded[f] = true;
        let held = self.held();
        self.rebuild(held);
        if let Some(f) = self.selected_full()
            && !show
            && self.parent.get(f + 1) != Some(&Some(f))
        {
            self.set_expanded(f, false);
        }
    }

    /// `Enter`: toggle the selected node's fold (expand⇄collapse). No-op on a leaf.
    pub fn toggle(&mut self) {
        if let Some(f) = self.selected_full()
            && self.full[f].has_children
        {
            self.set_expanded(f, !self.expanded[f]);
        }
    }

    /// Arm `c` as a chord prefix, so the **next** key can complete it (`m` for
    /// `mf`/`mu`). Replaces any prefix already armed: a prefix only ever waits
    /// for one key.
    pub fn arm_chord(&mut self, c: char) {
        self.pending = Some(c);
    }

    /// Take the half-typed chord prefix, clearing it.
    ///
    /// The **dispatcher** calls this once per key, before dispatching, because
    /// every key either completes the armed chord or aborts it — there is no
    /// third case. Putting the take in the one place keys arrive is what makes
    /// "a chord never outlives the next key" a property of the code rather
    /// than a rule eight handlers each had to remember (and four never did).
    /// The `pending` field carries the full history.
    pub fn take_chord(&mut self) -> Option<char> {
        self.pending.take()
    }

    /// Is a chord of the graph's own armed? It owns the next key, whatever
    /// that key means alone — so the router asks this before anything else.
    pub fn chord_armed(&self) -> bool {
        self.pending.is_some()
    }

    // --- internals --------------------------------------------------------

    /// The selected row as an index into `full`, if any.
    fn selected_full(&self) -> Option<usize> {
        self.visible_order.get(self.selected_vis).copied()
    }

    /// Set a node's fold state and refresh the visible list, keeping the
    /// selected *node* fixed (toggling a node never hides the node itself, so it
    /// stays visible and we just re-find its row).
    fn set_expanded(&mut self, full_idx: usize, value: bool) {
        self.expanded[full_idx] = value;
        let keep = self.selected_full();
        self.recompute_visible();
        self.reselect(keep);
    }

    /// Rebuild `visible_order`: a row is visible iff every ancestor is expanded.
    /// One forward pass works because `full` is preorder (parent before child).
    fn recompute_visible(&mut self) {
        let mut vis = vec![false; self.full.len()];
        let mut order = Vec::new();
        for i in 0..self.full.len() {
            let visible = match self.parent[i] {
                None => true,
                Some(p) => vis[p] && self.expanded[p],
            };
            vis[i] = visible;
            if visible {
                order.push(i);
            }
        }
        self.visible_order = order;
        if self.selected_vis >= self.visible_len() {
            self.selected_vis = self.visible_len().saturating_sub(1);
        }

        // Which nodes are the last visible child of their parent (for `└─`): the
        // last row encountered for each parent in preorder wins.
        let mut last_for_parent: HashMap<Option<usize>, usize> = HashMap::new();
        for &f in &self.visible_order {
            last_for_parent.insert(self.parent[f], f);
        }
        self.is_last = vec![false; self.full.len()];
        for &f in &self.visible_order {
            self.is_last[f] = last_for_parent.get(&self.parent[f]) == Some(&f);
        }
    }

    /// The box-drawing connector prefix for a visible node, already indented to
    /// its depth: a `│ ` / `  ` trunk for each ancestor (a bar if that ancestor's
    /// subtree continues below) then `├─` / `└─` to the node. Roots get no prefix.
    fn connector_prefix(&self, f: usize) -> String {
        let mut ancestors = Vec::new();
        let mut cur = self.parent[f];
        while let Some(a) = cur {
            ancestors.push(a);
            cur = self.parent[a];
        }
        ancestors.reverse(); // root-most first
        let mut s = String::new();
        for i in 0..ancestors.len() {
            if i + 1 < ancestors.len() {
                // Trunk under ancestors[i]; the path continues through ancestors[i+1].
                s.push_str(if self.is_last[ancestors[i + 1]] {
                    "  "
                } else {
                    "│ "
                });
            } else {
                // The connector from the immediate parent to this node.
                s.push_str(if self.is_last[f] { "└─" } else { "├─" });
            }
        }
        s
    }

    /// The `git`-style lane graph for a visible row. Each **session** is a pair of
    /// columns `[head, rail]`: the head dot `●` sits in the head column, and the
    /// session's turns run down the **rail** column one indent to the right, so
    /// you can see turns falling under their session. The head **curves** into the
    /// rail with `╰` at the first turn (follow the line from the session into its
    /// turns); the rail is `│` between turns and past messages. A **branch** hangs
    /// off its parent's rail one lane further right, and its own first turn curves
    /// off it the same way. At the fork the parent rail **physically hands off**
    /// to the branch with `├` (a junction — it both continues down and branches
    /// right; a pure corner can't do both) or `╰` when it forks off the parent's
    /// last turn. Ancestor rails run `│` straight down past a branch (they don't
    /// converge — merge is a strategy). Each cell carries its lane's color.
    fn lane_cells(&self, f: usize) -> Vec<LaneCell> {
        let entry = self.full[f].entry;
        // The heading is a rule, on no lane; a conversation's row draws as
        // its own (`view::claude_row_line`), its mark included — it heads
        // the lane its turns run down. At the top it draws none; resumed from
        // another, hung under one of that one's turns, it draws the lanes it
        // hangs from, its mark where a session's head dot would be.
        if matches!(entry, Entry::ClaudeHeading)
            || (matches!(entry, Entry::Conversation(_)) && self.parent[f].is_none())
        {
            return Vec::new();
        }
        // The path root..=f, and the lane heads along it (one per lane): a
        // session, or a Claude Code conversation, whose turns run down its
        // rail as a session's do.
        let mut path = vec![f];
        let mut cur = self.parent[f];
        while let Some(a) = cur {
            path.push(a);
            cur = self.parent[a];
        }
        path.reverse();
        let episodes: Vec<usize> = path
            .iter()
            .copied()
            .filter(|&pi| self.is_lane_head(pi))
            .collect();
        let Some(d) = episodes.len().checked_sub(1) else {
            return Vec::new();
        };
        let e_idx = episodes[d];
        let is_head = self.is_lane_head(f);
        let own = self.color_of(e_idx);
        // A turn is the episode's first when it's the child right after the head
        // in preorder — that turn takes the head's curve into the rail.
        let is_first_turn = !is_head && self.parent[f] == Some(e_idx) && f == e_idx + 1;

        // Each session is two columns `[head, rail]`. The head dot sits in the
        // head column; the session's turns run down the rail column one indent
        // right, so you see turns falling under their session. The head **curves**
        // into the rail (`╰`) at the first turn; the rail is `│` between turns; a
        // branch hangs off its parent's rail one lane further right.
        let mut cells = Vec::with_capacity(d * 2 + 2);
        for (k, &ep) in episodes.iter().take(d).enumerate() {
            let color = self.color_of(ep);
            let rail = self
                .child_on_path(ep, &path)
                .is_some_and(|c| !self.is_last[c]);
            cells.push(LaneCell { ch: ' ', color }); // head column (blank below the head)
            // The parent's rail, at the row where its branch forks off, physically
            // hands off to the new lane on its right: `├` when the rail keeps going
            // below (a real junction — a pure corner can't both continue and
            // branch), or `╰` when the branch forks off the parent's last turn.
            let ch = if is_head && k == d - 1 {
                if rail { '├' } else { '╰' }
            } else if rail {
                '│'
            } else {
                ' '
            };
            cells.push(LaneCell { ch, color });
        }
        match entry {
            // Its mark is the row's own, drawn after these.
            Entry::Conversation(_) => {}
            Entry::Node(Node::Episode(id)) if is_head => {
                // Root head sits at column 0; a branch head hangs off its parent's
                // rail (the last cell above), the rail column of its own lane. The
                // dot is the origin KIND's glyph — a child session reads
                // differently from a branch at a glance.
                cells.push(LaneCell {
                    ch: self.head_glyph(&id),
                    color: own,
                });
            }
            _ if is_first_turn => {
                cells.push(LaneCell {
                    ch: '╰',
                    color: own,
                }); // head curves into the rail
                cells.push(LaneCell {
                    ch: self.commit_dot(f),
                    color: own,
                });
            }
            Entry::Node(Node::Message { .. }) => {
                cells.push(LaneCell {
                    ch: ' ',
                    color: own,
                });
                cells.push(LaneCell {
                    ch: '│',
                    color: own,
                }); // rail runs on past the message
            }
            _ => {
                // A later turn: a dot on the rail.
                cells.push(LaneCell {
                    ch: ' ',
                    color: own,
                });
                cells.push(LaneCell {
                    ch: self.commit_dot(f),
                    color: own,
                });
            }
        }
        cells
    }

    /// A turn's dot on its lane: `●`, or the compaction mark where the turn is
    /// a summary — the same mark the tree style's label carries.
    fn commit_dot(&self, f: usize) -> char {
        match self.full[f].summary {
            true => self.glyphs.for_turn(true),
            false => '●',
        }
    }

    /// The node in `path` immediately below episode-row `ep_idx` (the child on the
    /// way to the leaf) — used to decide whether that branch's column continues.
    fn child_on_path(&self, ep_idx: usize, path: &[usize]) -> Option<usize> {
        let pos = path.iter().position(|&p| p == ep_idx)?;
        path.get(pos + 1).copied()
    }

    /// The palette slot for the lane head at `ep_idx` — a session's
    /// assigned one; a conversation's by its place in the list, so
    /// neighbours differ.
    fn color_of(&self, ep_idx: usize) -> usize {
        match self.full[ep_idx].entry {
            Entry::Node(Node::Episode(id)) => self.episode_color.get(&id).copied().unwrap_or(0),
            Entry::Conversation(at) => at % PALETTE_LEN,
            _ => 0,
        }
    }

    /// Whether row `f` heads a lane: a session, or a Claude Code
    /// conversation.
    fn is_lane_head(&self, f: usize) -> bool {
        matches!(
            self.full[f].entry,
            Entry::Node(Node::Episode(_)) | Entry::Conversation(_)
        )
    }

    /// Point the selection at `full_idx`'s row in the (possibly rebuilt) visible
    /// list, clamping if it's gone (shouldn't happen for a fold-of-self).
    fn reselect(&mut self, full_idx: Option<usize>) {
        if let Some(f) = full_idx
            && let Some(pos) = self.visible_order.iter().position(|&i| i == f)
        {
            self.selected_vis = pos;
            return;
        }
        self.selected_vis = self.selected_vis.min(self.visible_len().saturating_sub(1));
    }

    /// Put the cursor on the Claude Code conversation `id`, if listed — for
    /// a test to put the cursor there (a rebuild carries it by identity).
    #[cfg(test)]
    pub fn select_claude(&mut self, id: &str) -> bool {
        let key = Key::Conversation(id.to_string());
        let Some(f) = self.keys.iter().position(|k| *k == key) else {
            return false;
        };
        self.reveal_and_select(f);
        true
    }
}

/// The graph's Visual: visible rows, the unit its cursor moves in, with the
/// anchor kept by row identity so a fold elsewhere does not drag it.
impl crate::rows::RowSelect for GraphView {
    fn start_selection(&mut self) {
        self.visual = self.selected_full();
    }

    fn clear_selection(&mut self) {
        self.visual = None;
    }

    fn swap_selection_ends(&mut self) {
        let (Some(anchor), Some(here)) = (self.visual, self.selected_full()) else {
            return;
        };
        self.visual = Some(here);
        self.selected_vis = self.anchor_pos(anchor);
    }

    fn selection(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.anchor_pos(self.visual?);
        Some(crate::rows::between(anchor, self.selected_vis))
    }
}

/// Order the root sessions in place. `Hash` keeps `roots()`'s id order; `Recency`
/// puts the most-recently-active first, with a blank (no-turn) session newest.
fn sort_roots(graph: &Graph, roots: &mut [Node], sort: GraphSort) {
    match sort {
        GraphSort::Hash => {} // `roots()` is already id-sorted
        GraphSort::Recency => roots.sort_by(|a, b| recency_cmp(graph, *a, *b)),
    }
}

/// Compare two root episodes newest-first: a blank session (no turns → no time)
/// sorts ahead of any timed one; otherwise later last-turn time wins; ties (and
/// blank-vs-blank) break by id so the order is deterministic.
fn recency_cmp(graph: &Graph, a: Node, b: Node) -> Ordering {
    let (Node::Episode(ida), Node::Episode(idb)) = (a, b) else {
        return Ordering::Equal;
    };
    match (root_recency(graph, ida), root_recency(graph, idb)) {
        (None, None) => ida.as_bytes().cmp(idb.as_bytes()),
        (None, Some(_)) => Ordering::Less, // blank `a` is newest
        (Some(_), None) => Ordering::Greater, // blank `b` is newest
        (Some(x), Some(y)) => y.cmp(&x).then_with(|| ida.as_bytes().cmp(idb.as_bytes())),
    }
}

/// An episode's last-activity time — its head (most recent) turn's timestamp, or
/// `None` if it has no turns (a blank session).
fn root_recency(graph: &Graph, id: EpisodeId) -> Option<SystemTime> {
    // The session's full tip (own or shared prefix) — that occurrence's time.
    Some(graph.placed(&id).last()?.provenance.at)
}

/// Preorder build of the *full* tree (every level) into the parallel `full` /
/// `parent` arrays. Recurses through every child the graph reports — messages,
/// and branch / sub-agent episodes hung off a turn — so the flattened tree is
/// the graph's true hierarchy.
///
/// `skip`: how many of an episode's first turns to leave out — a session hung
/// under the Claude Code conversation it continues, whose first turns ARE
/// that conversation's, drawn once, there. 0 everywhere else, and for every
/// row below the one it is given for.
#[allow(clippy::too_many_arguments)] // a private recursive builder, not an API
fn build(
    graph: &Graph,
    drawn: Drawn,
    depth: usize,
    parent_idx: Option<usize>,
    look: Look,
    full: &mut Vec<Row>,
    parent: &mut Vec<Option<usize>>,
    skip: usize,
) {
    let idx = full.len();
    let node = drawn.node;
    // A top-level **session** (forest root: a fresh root, or a clone/re-root) shows
    // its FULL history — including the turns it *shares* by reference, so a clone
    // isn't an empty node. A nested **branch** shows only its own divergent turns
    // (the shared prefix is drawn once, under the ancestor). Turn children come
    // owner-tagged (shared iff borrowed); a turn's own children (messages / sub-
    // episodes) are never "shared turns", nor summaries.
    let children: Vec<Drawn> = match node {
        Node::Episode(id) => episode_children(graph, id)
            .into_iter()
            .filter(|d| matches!(d.node, Node::Turn { index, .. } if index >= skip))
            .collect(),
        other => graph
            .children(other)
            .into_iter()
            .map(Drawn::plain)
            .collect(),
    };
    full.push(Row {
        entry: Entry::Node(node),
        depth,
        label: label(graph, node, drawn.summary, look),
        has_children: !children.is_empty(),
        shared: drawn.shared,
        summary: drawn.summary,
    });
    parent.push(parent_idx);
    for child in children {
        build(graph, child, depth + 1, Some(idx), look, full, parent, 0);
    }
}

/// **Where the sessions continuing one conversation hang** — each at the
/// conversation's turn it parted at, its first `Inherited::turns` turns
/// left out, being that conversation's — and the branches cut from those
/// imported turns, at the conversation's turn each one is.
struct Hangs {
    /// A session, the turns of it to leave out, the turn it parted at:
    /// `None` hangs it from the conversation's row, every turn its own
    /// drawn — what it imported may be in no row there (unread, or a chain
    /// cut short), and a branch cut from it would hang under nothing.
    placed: Vec<(EpisodeId, usize, Option<usize>)>,
    /// A branch, and the conversation's turn it was cut from.
    forks: Vec<(usize, Node)>,
}

impl Hangs {
    fn of(graph: &Graph, sessions: &[EpisodeId], turns: &[ImportedTurn]) -> Hangs {
        let (mut placed, mut forks) = (Vec::new(), Vec::new());
        let held = |entry: &str| turns.iter().position(|t| t.holds(entry));
        for &id in sessions {
            let Some(from) = graph.episode(&id).and_then(|e| e.inherited.as_ref()) else {
                continue;
            };
            let Some(at) = from.at.as_deref().and_then(held) else {
                placed.push((id, 0, None));
                continue;
            };
            let skip = from.turns;
            // **Each imported turn where the entry that opened it is held**,
            // as recorded: an import from the latest compaction holds what it
            // kept after its summary, this reading before, so a count back
            // from the parting hung a branch of the summary's turn, and a
            // session ending on a kept question, before the compaction (review
            // stop 4, A 1). A record from before, by that count; a turn from
            // before the first read, at the parting.
            let places: Vec<usize> = (0..skip)
                .map(|i| match from.opened.get(i) {
                    Some(opened) => opened.as_deref().and_then(held).unwrap_or(at),
                    None => (at + 1 + i).checked_sub(skip).unwrap_or(at),
                })
                .collect();
            // Parted at the latest turn it holds.
            let p = places.iter().copied().fold(at, usize::max);
            placed.push((id, skip, Some(p)));
            for (i, occurrence) in graph.placed(&id).iter().enumerate().take(skip) {
                let j = places[i];
                let turn = Node::Turn {
                    episode: id,
                    turn: occurrence.turn,
                    index: i,
                };
                forks.extend(
                    graph
                        .children(turn)
                        .into_iter()
                        .filter(|n| matches!(n, Node::Episode(_)))
                        .map(|n| (j, n)),
                );
            }
        }
        Hangs { placed, forks }
    }
}

/// **How the listed conversations hang together** — each one a `claude
/// --resume` began under the one it began from ([`Found::resumed_in`];
/// Steven, 10-17: "Seeing *where* we had picked up in Shannon compared to
/// where that conversation currently is"), by place in the list.
///
/// Where: a resumed file copies its source's chain from the compaction
/// latest then, its entries' ids kept, so the turns the two share are the
/// source's — drawn once, there — and it was picked up at the latest of
/// them, which its row names ([`note`](Self::note)). Both must be read to
/// say where (Steven, 10-18: no new eager reads). One source each — the
/// first listed to name it — and none that would close a cycle.
///
/// **Drawn flat**: every conversation of a lineage hangs from its top's
/// row, at one depth (`Placed::of`; Steven, 10-19: "I'll go with your
/// proposal… one thing we'll have to feel out"). Each hung at the turn it
/// was picked up at, one level deeper per resume — a staircase off the
/// pane's edge at 30, a rebuild of 84 ms at 2,000, the stack gone at 8,000
/// (review stop 5, B 2).
struct Resumes {
    /// The conversation each one was resumed from — never a cycle: [`of`]
    /// passes over a name that would close one (pinned by
    /// `resumes_that_name_each_other_draw_each_once`), so every walk up it
    /// ends. The walks carried their own bounds as well, which nothing could
    /// reach (review stop 5, B, C 5); they went.
    ///
    /// [`of`]: Self::of
    source: HashMap<usize, usize>,
    /// Each one's lineage's top — itself, for a top.
    top: Vec<usize>,
    /// Each one's resumes, in the list's order.
    began: HashMap<usize, Vec<usize>>,
    /// A resumed conversation read beside its source: for each of its turns,
    /// the source's turn it is — shared — or `None`, its own.
    shared: HashMap<usize, Vec<Option<usize>>>,
}

impl Resumes {
    fn of(section: &ClaudeSection) -> Resumes {
        let place: HashMap<&str, usize> = section
            .found
            .iter()
            .enumerate()
            .map(|(at, f)| (f.id.as_str(), at))
            .collect();
        let mut source: HashMap<usize, usize> = HashMap::new();
        // Each one's top as far as known, kept as a union-find: whether a
        // name would close a cycle is one lookup, where it was a walk up the
        // lineage per name (review stop 5, B 2).
        let mut up: Vec<usize> = (0..section.found.len()).collect();
        for (s, found) in section.found.iter().enumerate() {
            for id in &found.resumed_in {
                let Some(&n) = place.get(id.as_str()) else {
                    continue;
                };
                // With no source yet, `n` is its lineage's top: under `s` it
                // closes a cycle exactly when it is `s`'s top already.
                if !source.contains_key(&n) && top_of(&mut up, s) != n {
                    source.insert(n, s);
                    up[n] = top_of(&mut up, s);
                }
            }
        }
        let top = (0..up.len()).map(|at| top_of(&mut up, at)).collect();
        let mut began: HashMap<usize, Vec<usize>> = HashMap::new();
        for (&n, &s) in &source {
            began.entry(s).or_default().push(n);
        }
        for resumes in began.values_mut() {
            resumes.sort_unstable();
        }
        let turns = |at: usize| {
            section
                .read
                .get(&section.found[at].id)
                .map(|r| &r.turns[..])
        };
        // The source's turns indexed once by every entry each holds (its
        // own, and those it took in — `ImportedTurn::holds`), the first turn
        // holding one winning as a scan would: a scan per turn was
        // O(mine × theirs) on every rebuild (review stop 5, B 5).
        let shared = source
            .iter()
            .filter_map(|(&n, &s)| {
                let (mine, theirs) = (turns(n)?, turns(s)?);
                let mut held: HashMap<&str, usize> = HashMap::new();
                for (i, t) in theirs.iter().enumerate() {
                    for entry in t.uuid.iter().chain(&t.joined) {
                        held.entry(entry.as_str()).or_insert(i);
                    }
                }
                let at = |turn: &ImportedTurn| held.get(turn.uuid.as_deref()?).copied();
                Some((n, mine.iter().map(at).collect()))
            })
            .collect();
        Resumes {
            source,
            top,
            began,
            shared,
        }
    }

    /// **The conversations resumed in `top`'s lineage** — each before the
    /// ones resumed from it, in the list's order — what hangs, flat, from
    /// `top`'s row. A stack, not recursion: a lineage can be any length.
    fn lineage(&self, top: usize) -> Vec<usize> {
        let began = |at: &usize| self.began.get(at).map_or(&[][..], Vec::as_slice);
        let mut stack: Vec<usize> = began(&top).iter().rev().copied().collect();
        let mut out = Vec::new();
        while let Some(at) = stack.pop() {
            out.push(at);
            stack.extend(began(&at).iter().rev());
        }
        out
    }

    /// **Where conversation `at` was picked up, said on its row** —
    /// `resumes <source> after "<that turn's words>"`, or `resumes <source>`
    /// until both are read: the place it hung at before it hung flat. "After":
    /// the turn named is the last the two share, and the resume goes on past
    /// it (review stop 6, F; it said "at").
    fn note(&self, section: &ClaudeSection, at: usize) -> Option<String> {
        let source = *self.source.get(&at)?;
        let (from, _) = conversation_name(&section.found[source]);
        let words = self.picked_up_at(at).and_then(|(c, t)| {
            let read = section.read.get(&section.found[c].id)?;
            Some(snippet(&read.turns.get(t)?.prompt))
        });
        Some(match words {
            Some(words) => format!("resumes {from} after {words}"),
            None => format!("resumes {from}"),
        })
    }

    /// The source's turn conversation `at`'s turn `t` is, if it is shared.
    fn shared_turn(&self, at: usize, t: usize) -> Option<usize> {
        self.shared.get(&at)?.get(t).copied().flatten()
    }

    /// **Where conversation `at`'s turn `t` is drawn** — its own row, or,
    /// shared, its source's turn, and that one's if shared again.
    fn drawn_at(&self, mut at: usize, mut t: usize) -> (usize, usize) {
        while let (Some(theirs), Some(&source)) = (self.shared_turn(at, t), self.source.get(&at)) {
            (at, t) = (source, theirs);
        }
        (at, t)
    }

    /// **Where resumed conversation `at` was picked up** — the latest turn of
    /// its source it holds, as drawn ([`drawn_at`](Self::drawn_at)), which
    /// its row names ([`note`](Self::note)); `None`: the two not both read,
    /// or sharing no turn.
    fn picked_up_at(&self, at: usize) -> Option<(usize, usize)> {
        let source = *self.source.get(&at)?;
        let latest = self.shared.get(&at)?.iter().flatten().copied().max()?;
        Some(self.drawn_at(source, latest))
    }

    /// The newest time in conversation `at`'s lineage — itself and every
    /// resume under it — which places its lineage in the list: the newest
    /// conversation is the one most likely sought, and it is under its
    /// source now.
    fn newest(&self, section: &ClaudeSection, at: usize) -> Option<SystemTime> {
        let mut newest = section.found[at].modified;
        let mut stack = vec![at];
        while let Some(c) = stack.pop() {
            for &n in self.began.get(&c).map_or(&[][..], Vec::as_slice) {
                newest = newest.max(section.found[n].modified);
                stack.push(n);
            }
        }
        newest
    }
}

/// **`at`'s top in the union-find `up`** — following each one's link up to
/// the one that links to itself, and linking every one passed straight to
/// it, so the next lookup is one step ([`Resumes::of`]).
fn top_of(up: &mut [usize], at: usize) -> usize {
    let mut top = at;
    while up[top] != top {
        top = up[top];
    }
    let mut on = at;
    while up[on] != top {
        on = std::mem::replace(&mut up[on], top);
    }
    top
}

/// **What hangs under one Claude Code row** — a conversation's turn, or its
/// row: the sessions going on from it (with the turns of each to leave
/// out), the branches cut from its imported turns, and the conversations
/// resumed from it.
#[derive(Default)]
struct Under {
    sessions: Vec<(EpisodeId, usize)>,
    forks: Vec<Node>,
    conversations: Vec<usize>,
}

impl Under {
    fn any(&self) -> bool {
        !self.sessions.is_empty() || !self.forks.is_empty() || !self.conversations.is_empty()
    }
}

/// **Everything that hangs under the section's rows**, where it is drawn:
/// what a session or branch hung at a turn a resume shares hangs at the
/// source's turn instead, where that turn is drawn; and a lineage's resumes
/// hang, flat, from its top's row ([`Resumes`]).
struct Placed {
    /// By (conversation, turn).
    at_turn: HashMap<(usize, usize), Under>,
    /// By conversation, from its row.
    at_row: HashMap<usize, Under>,
}

impl Placed {
    fn of(
        graph: &Graph,
        section: &ClaudeSection,
        homes: &HashMap<usize, Vec<EpisodeId>>,
        resumes: &Resumes,
    ) -> Placed {
        let mut placed = Placed {
            at_turn: HashMap::new(),
            at_row: HashMap::new(),
        };
        for (at, found) in section.found.iter().enumerate() {
            let turns = section
                .read
                .get(&found.id)
                .map_or(&[][..], |r| &r.turns[..]);
            let sessions = homes.get(&at).map_or(&[][..], Vec::as_slice);
            let hangs = Hangs::of(graph, sessions, turns);
            for &(id, skip, p) in &hangs.placed {
                let under = match p {
                    Some(p) => placed.turn(resumes.drawn_at(at, p)),
                    None => placed.at_row.entry(at).or_default(),
                };
                under.sessions.push((id, skip));
            }
            for &(j, fork) in &hangs.forks {
                placed.turn(resumes.drawn_at(at, j)).forks.push(fork);
            }
        }
        // A lineage hangs flat from its top's row, each conversation saying
        // where it was picked up ([`Resumes`]).
        for top in (0..section.found.len()).filter(|&at| resumes.top[at] == at) {
            let lineage = resumes.lineage(top);
            if !lineage.is_empty() {
                placed.at_row.entry(top).or_default().conversations = lineage;
            }
        }
        placed
    }

    fn turn(&mut self, at: (usize, usize)) -> &mut Under {
        self.at_turn.entry(at).or_default()
    }
}

/// **The rows a rebuild lays out** — and the ones that open the Claude Code
/// rows above them by default (`pulls`): a session going on from a
/// conversation, a conversation resumed from one, so moving either there
/// hides nothing.
struct Laid {
    full: Vec<Row>,
    parent: Vec<Option<usize>>,
    pulls: Vec<usize>,
}

/// What laying out the section reads.
struct Layout<'a> {
    graph: &'a Graph,
    section: &'a ClaudeSection,
    resumes: &'a Resumes,
    placed: &'a Placed,
    /// Where each resume was picked up ([`GraphView::resume_notes`]).
    notes: &'a HashMap<usize, String>,
    /// The conversations shown with their turns ([`GraphView::with_turns`]).
    with_turns: &'a BTreeSet<String>,
    look: Look,
}

impl Layout<'_> {
    /// **Conversation `at`, at `depth` under row `under`** — its row, its
    /// own turns (those a resume shares with its source are the source's,
    /// drawn there), and what hangs under each and under its row.
    ///
    /// **Its turns only once opened** (Steven, 10-18, of the section before
    /// its readings came and after: the first "looks pretty nice … especially
    /// how the line connects the conversations", the second "shows too much
    /// all at once"): until then what hangs from any of its turns hangs from
    /// its row, in its turns' order — so a reading landing moves no
    /// conversation and no session (a session's imported turns go to the
    /// conversation, drawn once its turns are), and the conversations below
    /// stay within reach.
    fn conversation(&self, at: usize, depth: usize, under: Option<usize>, out: &mut Laid) {
        let found = &self.section.found[at];
        let (id, title) = conversation_name(found);
        let read = self.section.read.get(&found.id);
        let turns = read.map_or(&[][..], |r| &r.turns[..]);
        let own: Vec<usize> = (0..turns.len())
            .filter(|&t| self.resumes.shared_turn(at, t).is_none())
            .collect();
        let at_row = self.placed.at_row.get(&at);
        let head = out.full.len();
        // Where it was picked up is what `/` finds too.
        let label = match self.notes.get(&at) {
            Some(note) => format!("{id}  {title}  {note}"),
            None => format!("{id}  {title}"),
        };
        out.full.push(Row {
            entry: Entry::Conversation(at),
            depth,
            label,
            // Unread, it still folds: opening it is what reads it.
            has_children: read.is_none() || !own.is_empty() || at_row.is_some_and(Under::any),
            shared: false,
            summary: false,
        });
        out.parent.push(under);
        if !self.with_turns.contains(&found.id) {
            for t in own {
                if let Some(here) = self.placed.at_turn.get(&(at, t)) {
                    self.under(here, depth + 1, head, out);
                }
            }
            if let Some(at_row) = at_row {
                self.under(at_row, depth + 1, head, out);
            }
            return;
        }
        for t in own {
            let here = self.placed.at_turn.get(&(at, t));
            let row = out.full.len();
            out.full.push(Row {
                entry: Entry::ClaudeTurn {
                    conversation: at,
                    turn: t,
                },
                depth: depth + 1,
                label: claude_turn_label(&turns[t], self.look.glyphs),
                has_children: here.is_some_and(Under::any),
                shared: false,
                summary: turns[t].summary,
            });
            out.parent.push(Some(head));
            if let Some(here) = here {
                self.under(here, depth + 2, row, out);
            }
        }
        if let Some(at_row) = at_row {
            self.under(at_row, depth + 1, head, out);
        }
    }

    /// What hangs under row `row`, at `depth`.
    fn under(&self, under: &Under, depth: usize, row: usize, out: &mut Laid) {
        for &(session, skip) in &under.sessions {
            out.pulls.push(out.full.len());
            let drawn = Drawn::plain(Node::Episode(session));
            build(
                self.graph,
                drawn,
                depth,
                Some(row),
                self.look,
                &mut out.full,
                &mut out.parent,
                skip,
            );
        }
        for &fork in &under.forks {
            let drawn = Drawn::plain(fork);
            build(
                self.graph,
                drawn,
                depth,
                Some(row),
                self.look,
                &mut out.full,
                &mut out.parent,
                0,
            );
        }
        for &resumed in &under.conversations {
            out.pulls.push(out.full.len());
            self.conversation(resumed, depth, Some(row), out);
        }
    }
}

#[cfg(test)]
impl Row {
    /// The node a test knows this row to be.
    fn node(&self) -> Node {
        self.entry.node().expect("a node row")
    }
}

#[cfg(test)]
impl DisplayRow<'_> {
    /// The node a test knows this row to be.
    fn node(&self) -> Node {
        self.entry.node().expect("a node row")
    }
}

impl Row {
    /// A Claude Code row at the top level: `label` is what `/` reads of it
    /// in the tree style's terms (the renderer draws it as its own).
    fn claude(entry: Entry, label: String) -> Row {
        Row {
            entry,
            depth: 0,
            label,
            has_children: false,
            shared: false,
            summary: false,
        }
    }
}

impl Drawn {
    /// A row that is neither borrowed nor a summary — a root, a message, a
    /// session hung off a turn.
    fn plain(node: Node) -> Drawn {
        Drawn {
            node,
            shared: false,
            summary: false,
        }
    }
}

/// What the tree's precomputed labels are drawn with — the configured
/// glyphs and where a named session's id goes, carried down the build as one.
#[derive(Debug, Clone, Copy)]
struct Look {
    glyphs: NodeGlyphs,
    ids: GraphIds,
}

/// **How a session's row names it** — `(lead, id after it)`: a named
/// session leads with its name, its short id after it or not at all as
/// `[tui] graph_ids` says; one with no name leads with its id, which is all
/// it is called (Steven, 10-14: "show the human readable name first"). The
/// one answer for both styles — the tree's precomputed label and the git
/// style's spans.
pub(crate) fn session_heading(
    graph: &Graph,
    id: EpisodeId,
    ids: GraphIds,
) -> (String, Option<String>) {
    match graph.episode(&id).and_then(|e| e.label.name.as_deref()) {
        Some(name) => (
            name.to_string(),
            (ids == GraphIds::After).then(|| id.short_hex()),
        ),
        None => (id.short_hex(), None),
    }
}

/// **The one statement of "what is under this row".**
///
/// `pub(crate)` because marking a row must expand to exactly what the row
/// DRAWS. `marked_turn_range` used `Graph::children` instead — the same filter
/// minus the `shows_full` term — and its comment claimed they were the same
/// function. They agreed for roots and branches and diverged for clones and
/// merges, so marking a clone's row and pressing `s` refused with "owns no
/// turns" about a row with turns visibly drawn beneath it (Reviewer D).
///
/// The turn nodes to draw under an episode: a root, a clone, or a merge shows
/// its full derived history (they own little or nothing — the derived sequence
/// IS their content; shared turns draw grayed); a nested **branch** shows only
/// its own divergent turns (the shared prefix is drawn once, under the ancestor
/// that owns it). Placement (root vs nested) is a separate concern — see
/// `Graph::roots`.
pub(crate) fn episode_children(graph: &Graph, id: EpisodeId) -> Vec<Drawn> {
    let Some(ep) = graph.episode(&id) else {
        return Vec::new();
    };
    let shows_full = ep.shows_full_history();
    // Enumerate the FULL derived sequence so each turn carries its full-sequence
    // position (its occurrence index — stable under content-addressed repeats).
    // Owner-tagged, so "owned vs shared" (`owner != id`) is known here for free —
    // the renderer never recomputes it per frame; and the occurrence's own
    // provenance is in hand, so "is it a summary" is too.
    graph
        .placed(&id)
        .into_iter()
        .enumerate()
        .filter(|(_, p)| shows_full || p.owner == id)
        .map(|(index, p)| Drawn {
            node: Node::Turn {
                episode: id,
                turn: p.turn,
                index,
            },
            shared: p.owner != id,
            summary: !p.provenance.distilled_from.is_empty(),
        })
        .collect()
}

/// One turn under a session's row, as [`episode_children`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Drawn {
    pub(crate) node: Node,
    /// Borrowed from another session (drawn faded).
    pub(crate) shared: bool,
    /// A summary — what a compaction made, marked where it happened
    /// ([`NodeGlyphs::for_turn`]).
    pub(crate) summary: bool,
}

/// Greedily assign each episode a palette slot (`0..PALETTE_LEN`) that differs
/// from its **parent** episode and its already-colored **siblings** (episodes
/// sharing the same parent; roots are siblings of each other). So adjacent
/// branches — and sibling sessions — never collide, which is the whole point of
/// coloring: telling neighbors apart. Processed in `full` preorder so a parent is
/// colored before its children; falls back to cycling once a neighborhood uses
/// every slot. Stable for a given graph, and adding a branch doesn't recolor
/// existing ones.
fn assign_colors(graph: &Graph, full: &[Row]) -> HashMap<EpisodeId, usize> {
    let mut colors: HashMap<EpisodeId, usize> = HashMap::new();
    let mut used_by_parent: HashMap<Option<EpisodeId>, Vec<usize>> = HashMap::new();
    for row in full {
        let Entry::Node(Node::Episode(id)) = row.entry else {
            continue;
        };
        if colors.contains_key(&id) {
            continue;
        }
        let parent_ep = graph.episode(&id).and_then(|e| e.origin).map(|p| p.episode);
        let parent_color = parent_ep.and_then(|pe| colors.get(&pe).copied());
        let siblings = used_by_parent.entry(parent_ep).or_default();
        let idx = (0..PALETTE_LEN)
            .find(|i| Some(*i) != parent_color && !siblings.contains(i))
            .unwrap_or(siblings.len() % PALETTE_LEN);
        colors.insert(id, idx);
        siblings.push(idx);
    }
    colors
}

/// Build a one-line label for a node. Episodes show their name (else their
/// short id — [`session_heading`]), why they exist (root, branch or
/// tool-call, and where they forked), turn count, and model; turns show their
/// short id, a first-words snippet (git-log-subject style), and the model;
/// messages show who spoke ([`speaker`]) and a snippet. Missing ids degrade to a marker
/// rather than panicking, so a partially hydrated graph still labels.
fn label(graph: &Graph, node: Node, summary: bool, look: Look) -> String {
    match node {
        Node::Episode(id) => {
            let ep = graph.episode(&id);
            let kind = ep.and_then(|e| e.origin).map(|o| o.kind);
            let origin = (ep.and_then(shannon_core::Episode::origin_clause))
                .unwrap_or_else(|| root_word(ep));
            let turns = graph.full_turns(&id).len(); // full history, not just divergent
            let model = ep.map(|e| e.config.model.as_str()).unwrap_or("?");
            let (lead, id) = session_heading(graph, id, look.ids);
            let id = id.map(|hex| format!(" {hex}")).unwrap_or_default();
            format!(
                "{} {lead}{id}  {origin} · {turns} turns · {model}",
                look.glyphs.for_kind(kind),
            )
        }
        // Outlines, never bodies: the tree is built from the structure alone,
        // so opening `:graph` over a long history reads no messages.
        Node::Turn { turn, .. } => {
            let snippet = graph.outline(&turn).map(turn_snippet).unwrap_or_default();
            let model = graph.model(&turn).unwrap_or("?");
            let mark = look.glyphs.for_turn(summary);
            format!("{mark} {}  {snippet} · {model}", turn.short_hex())
        }
        Node::Message { turn, index, .. } => {
            let msg = graph.outline(&turn).and_then(|m| m.get(index));
            match msg {
                Some(m) => match speaker(m) {
                    Some(who) => format!("{who}: {}", snippet(&m.first_line)),
                    None => crate::view::snippet(&m.first_line, 48),
                },
                None => "message".to_string(),
            }
        }
    }
}

/// **Who a message's row says spoke** — `you` or `reply` before someone's
/// words; no one before a message of only calls or results, whose line names
/// what it holds (`[Bash: rg …]`, `[result: …]`). It was the API's role, so
/// a tool's output read `user: [result: …]`, words no one typed (Steven,
/// 10-17, image 63). Both styles' one rule (`view::git_label_spans`).
pub(crate) fn speaker(message: &shannon_core::MessageOutline) -> Option<&'static str> {
    message.words.then_some(match message.role {
        Role::User => "you",
        Role::Assistant => "reply",
    })
}

/// **A conversation as every surface names it** — its short id and its title
/// (`(untitled)` without one): its row, what `/` reads of it, and both
/// inspector pages. The title arrives scrubbed (`App::claude_section`).
pub(crate) fn conversation_name(found: &Found) -> (&str, &str) {
    (
        shannon_core::short_hex(&found.id),
        found.title.as_deref().unwrap_or("(untitled)"),
    )
}

/// **What a session with no origin in Shannon is**: `root`, or — continued
/// from a Claude Code conversation — `continues <its id>`, as a branch names
/// the turn it forked from. Both styles' one word for it.
pub(crate) fn root_word(episode: Option<&shannon_core::Episode>) -> String {
    match episode.and_then(|e| e.inherited.as_ref()) {
        Some(from) => format!("continues {}", shannon_core::short_hex(&from.session)),
        None => "root".to_string(),
    }
}

/// **A Claude Code turn's row** — its mark (`⊚` where Claude Code compacted:
/// the summary is the turn), its prompt's opening words and the model that
/// answered, as a Shannon turn's row reads them.
fn claude_turn_label(turn: &ImportedTurn, glyphs: NodeGlyphs) -> String {
    let model = turn
        .model
        .as_deref()
        .map(|m| format!(" · {m}"))
        .unwrap_or_default();
    format!(
        "{} {}{model}",
        glyphs.for_turn(turn.summary),
        snippet(&turn.prompt)
    )
}

/// A git-log-subject-style snippet of a turn: the first line of its user message.
fn turn_snippet(outline: &[shannon_core::MessageOutline]) -> String {
    let text = outline
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| m.first_line.as_str())
        .unwrap_or("");
    snippet(text)
}

/// A row's opening words, quoted — the `tree` style's spelling of the one
/// rule both styles show a text by ([`crate::view::snippet`]: its first line
/// with anything on it, cut to the row). This had its own: the first line
/// even when blank, so a prompt opening with an empty line read `""` here
/// and its words in the `git` style (review stop 3, C 6).
fn snippet(text: &str) -> String {
    format!("\"{}\"", crate::view::snippet(text, 48))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shannon_core::{Episode, EpisodeId, Message, Origin, Turn, TurnConfig, TurnId};
    use std::time::SystemTime;

    fn cfg() -> TurnConfig {
        TurnConfig {
            model: "claude-haiku".into(),
            max_tokens: None,
            effort: None,
        }
    }

    /// Seal a user+assistant turn whose user text is `user`.
    fn turn(user: &str) -> Turn {
        Turn::seal(
            vec![Message::user(user), Message::assistant("ok")],
            cfg(),
            SystemTime::UNIX_EPOCH,
        )
    }

    /// Root episode with turns A, B; a branch episode off B carrying turn C.
    /// (Same shape as the graph.rs traversal test: 11 nodes in full preorder.)
    fn sample() -> (Graph, EpisodeId, EpisodeId, TurnId, TurnId, TurnId) {
        let (a, b, c) = (
            turn("first question"),
            turn("second"),
            turn("on the branch"),
        );
        let root_id = EpisodeId::from_seed(b"root");
        let child_id = EpisodeId::from_seed(b"child");

        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        let mut child = Episode::branch(
            child_id,
            cfg(),
            Origin {
                episode: root_id,
                turn: b.id,
                index: Some(1),
                kind: EdgeKind::Branch,
            },
            2,
        );
        child.append(c.occurrence());

        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_content(c.clone());
        g.insert_episode(root);
        g.insert_episode(child);
        (g, root_id, child_id, a.id, b.id, c.id)
    }

    /// The nodes currently visible, top to bottom.
    fn visible(view: &GraphView) -> Vec<Node> {
        view.visible_order
            .iter()
            .filter_map(|&f| view.full[f].entry.node())
            .collect()
    }

    /// A listed Claude Code conversation with only an id.
    fn found(id: &str) -> Found {
        Found {
            id: id.to_string(),
            path: std::path::PathBuf::new(),
            title: None,
            modified: None,
            bytes: 0,
            resumed_in: Vec::new(),
        }
    }

    /// **A section that changes keeps what you hold** — a re-list (leave
    /// given, `:unread`) lays the rows out again, as will a session moving
    /// under the conversation it continues; the cursor, a fold, the
    /// marks and a Visual anchor come through by identity, the cursor on a
    /// conversation by its id wherever the list now puts it.
    #[test]
    fn a_changed_section_keeps_what_you_hold() {
        let (g, root, _, a, ..) = sample();
        let mut view = GraphView::new(g);
        let section = |ids: &[&str]| {
            Some(ClaudeSection {
                found: ids.iter().map(|id| found(id)).collect(),
                read: HashMap::new(),
                awaiting: BTreeSet::new(),
                listed: true,
                scrubbed: Scrubber::defaults(),
            })
        };
        view.set_claude(section(&["aaaa0000", "bbbb0000"]));
        assert!(view.select_node(Node::Episode(root)), "premise");
        view.toggle_mark();
        view.collapse();
        let first = Node::Turn {
            episode: root,
            turn: a,
            index: 0,
        };
        assert!(!visible(&view).contains(&first), "premise: folded");
        assert!(view.select_claude("bbbb0000"), "premise");
        crate::rows::RowSelect::start_selection(&mut view);
        assert!(view.select_node(Node::Episode(root)), "premise");

        view.set_claude(section(&["cccc0000", "bbbb0000", "aaaa0000"]));
        assert_eq!(
            view.selected_node(),
            Some(Node::Episode(root)),
            "the cursor"
        );
        assert_eq!(view.marked_nodes(), vec![Node::Episode(root)], "the mark");
        assert!(!visible(&view).contains(&first), "the fold");
        crate::rows::RowSelect::swap_selection_ends(&mut view);
        assert_eq!(
            view.selected_claude().map(|f| f.id.as_str()),
            Some("bbbb0000"),
            "the range's other end, by id, though the list moved it"
        );
    }

    /// **A Visual anchor is a row, not a position.** Folded under its session,
    /// the range reaches the row that hides it — and the session is what a
    /// verb then acts on, as shown; unfolded, the anchor is where it was.
    #[test]
    fn a_visual_anchor_survives_a_fold() {
        use crate::rows::RowSelect;
        let (g, root, _, a, b, _) = sample();
        let mut view = GraphView::new(g);
        let turn = |turn, index| Node::Turn {
            episode: root,
            turn,
            index,
        };
        assert_eq!(
            visible(&view),
            vec![Node::Episode(root), turn(a, 0), turn(b, 1)],
            "premise"
        );
        view.select_row(2);
        view.start_selection();
        view.select_row(0);
        assert_eq!(view.selection(), Some(0..=2));
        view.collapse();
        assert_eq!(view.selection(), Some(0..=0), "folded: the session row");
        assert_eq!(view.acts_on(), vec![Node::Episode(root)]);
        view.expand();
        assert_eq!(
            view.selection(),
            Some(0..=2),
            "unfolded: the anchor is back"
        );
        view.swap_selection_ends();
        assert_eq!(view.selected(), 2, "`o`: the cursor to the anchor");
        assert_eq!(view.selection(), Some(0..=2));
    }

    /// How long ago, at each unit's edges — and a clock that went backwards
    /// reads as just now, not as a panic or a negative.
    #[test]
    fn ago_says_how_long_at_each_edge() {
        use std::time::Duration;
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let back = |s: u64| super::ago(now, now - Duration::from_secs(s));
        assert_eq!(back(59), "just now");
        assert_eq!(back(60), "1m ago");
        assert_eq!(back(3_599), "59m ago");
        assert_eq!(back(3_600), "1h ago");
        assert_eq!(back(86_399), "23h ago");
        assert_eq!(back(86_400), "1d ago");
        assert_eq!(super::ago(now, now + Duration::from_secs(5)), "just now");
    }

    #[test]
    fn full_tree_is_the_graph_hierarchy_with_correct_depths() {
        // Every level is present in `full`, in preorder, with depth = nesting:
        // episode(0) → turn(1) → message(2); a branch episode nests under the
        // turn it forked from (depth 2) and its turn/messages go deeper (3/4).
        let (g, ..) = sample();
        let view = GraphView::new(g);

        let shape: Vec<(usize, &str)> = view
            .full
            .iter()
            .map(|r| {
                let kind = match r.node() {
                    Node::Episode(_) => "ep",
                    Node::Turn { .. } => "turn",
                    Node::Message { .. } => "msg",
                };
                (r.depth, kind)
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, "ep"),
                (1, "turn"),
                (2, "msg"),
                (2, "msg"),
                (1, "turn"),
                (2, "msg"),
                (2, "msg"),
                (2, "ep"),   // the branch, nested under turn B
                (3, "turn"), // its turn C
                (4, "msg"),
                (4, "msg"),
            ]
        );
    }

    #[test]
    fn default_fold_shows_episode_then_turns_only() {
        // Opening the view: the root episode is open (its turns show); turns are
        // collapsed (their messages/branches hidden) — the outline to drill into.
        let (g, root_id, _child, a, b, _c) = sample();
        let view = GraphView::new(g);
        assert_eq!(
            visible(&view),
            vec![
                Node::Episode(root_id),
                Node::Turn {
                    episode: root_id,
                    turn: a,
                    index: 0,
                },
                Node::Turn {
                    episode: root_id,
                    turn: b,
                    index: 1,
                },
            ]
        );
    }

    #[test]
    fn a_repeated_turn_gets_distinct_occurrence_indices() {
        // Content-addressed ids recur: the same turn can legitimately appear twice
        // in one episode (an identical clone_n reply, a repeated "continue"). The
        // viewer must give each occurrence its OWN node (distinct index), so
        // selecting the second one branches/checks-out there — not at the first (the
        // [[content-addressed-ids]] data-loss trap).
        let (x, y, z) = (turn("same"), turn("other"), turn("tail"));
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(x.occurrence());
        root.append(y.occurrence());
        root.append(x.occurrence()); // x again — a legitimate repeat
        root.append(z.occurrence());
        let mut g = Graph::new();
        for t in [&x, &y, &z] {
            g.insert_content((*t).clone());
        }
        g.insert_episode(root);

        let view = GraphView::new(g);
        let turns: Vec<(TurnId, usize)> = view
            .full
            .iter()
            .filter_map(|r| match r.node() {
                Node::Turn { turn, index, .. } => Some((turn, index)),
                _ => None,
            })
            .collect();
        assert_eq!(
            turns,
            vec![(x.id, 0), (y.id, 1), (x.id, 2), (z.id, 3)],
            "each occurrence of the repeated turn x carries its own index"
        );
    }

    #[test]
    fn expanding_a_turn_reveals_its_messages_and_the_branch_under_it() {
        let (g, root_id, child_id, _a, b, c) = sample();
        let mut view = GraphView::new(g);

        // Select turn B (row 2) and expand it.
        view.select_down();
        view.select_down();
        assert_eq!(
            view.selected_node(),
            Some(Node::Turn {
                episode: root_id,
                turn: b,
                index: 1,
            })
        );
        view.expand();

        // B's two messages and the branch episode (itself open → its turn shows)
        // now appear under B. The selection stays on B.
        let now = visible(&view);
        assert!(
            now.contains(&Node::Episode(child_id)),
            "branch revealed: {now:?}"
        );
        assert!(
            now.contains(&Node::Turn {
                episode: child_id,
                turn: c,
                index: 2,
            }),
            "branch's turn revealed: {now:?}"
        );
        assert!(
            now.iter()
                .any(|n| matches!(n, Node::Message { turn, .. } if *turn == b)),
            "B's messages revealed: {now:?}"
        );
        assert_eq!(
            view.selected_node(),
            Some(Node::Turn {
                episode: root_id,
                turn: b,
                index: 1,
            }),
            "selection stays on the toggled node"
        );

        // Collapsing B hides all of that again.
        view.collapse();
        assert_eq!(view.visible_len(), 3, "back to the episode→turns outline");
    }

    #[test]
    fn h_on_a_collapsed_node_jumps_to_the_parent() {
        let (g, root_id, ..) = sample();
        let mut view = GraphView::new(g);
        view.select_down(); // turn A (collapsed leaf-ish: has children but folded)
        view.collapse(); // already collapsed → jump to parent (the episode)
        assert_eq!(view.selected_node(), Some(Node::Episode(root_id)));
    }

    #[test]
    fn message_rows_label_who_spoke_and_snippet() {
        let (g, ..) = sample();
        let mut view = GraphView::new(g);
        view.select_down(); // turn A
        view.expand();
        // A's first child message is the user message "first question".
        let rows = view.window(usize::MAX);
        let user_row = rows
            .iter()
            .find(|r| r.label.starts_with("you:"))
            .expect("a user message row");
        assert!(user_row.label.contains("first question"), "{user_row:?}");
        assert!(rows.iter().any(|r| r.label == "reply: \"ok\""), "{rows:?}");
    }

    /// A turn with one tool round: who spoke, in each style, row by row.
    fn tool_round() -> (Graph, TurnId) {
        let held = |role, block| Message {
            role,
            content: String::new(),
            blocks: vec![block],
        };
        let turn = Turn::seal(
            vec![
                Message::user("look"),
                held(Role::Assistant, shannon_core::Block::bash_call("t", "ls")),
                held(
                    Role::User,
                    shannon_core::Block::ToolResult {
                        tool_use_id: "t".into(),
                        content: "a.txt".into(),
                        is_error: false,
                    },
                ),
                Message::assistant("done"),
            ],
            cfg(),
            SystemTime::UNIX_EPOCH,
        );
        let id = EpisodeId::from_seed(b"tools");
        let mut episode = Episode::root(id, cfg());
        episode.append(turn.occurrence());
        let mut g = Graph::new();
        g.insert_content(turn.clone());
        g.insert_episode(episode);
        (g, turn.id)
    }

    /// **A tool's call and output are named, not voiced** — the call by its
    /// tool, the output as a result, with no one before them: they read
    /// `assistant: "[bash: ls]"` and `user: "[result: a.txt]"`, the result
    /// as words the user typed (Steven, 10-17, image 63). The words around
    /// them say who: `you`, `reply`.
    #[test]
    fn a_tool_round_reads_as_what_it_holds() {
        let (g, _) = tool_round();
        let mut view = GraphView::new(g);
        view.select_down(); // the turn
        view.expand();
        let labels: Vec<String> = view
            .window(usize::MAX)
            .iter()
            .filter(|r| matches!(r.entry.node(), Some(Node::Message { .. })))
            .map(|r| r.label.to_string())
            .collect();
        assert_eq!(
            labels,
            [
                "you: \"look\"",
                "[bash: ls]",
                "[result: a.txt]",
                "reply: \"done\""
            ]
        );
    }

    /// The same in the `git` style, as drawn — its labels are spans of its
    /// own (`view::git_label_spans`), which spelled the role `asst`.
    #[test]
    fn a_tool_round_reads_as_what_it_holds_in_the_git_style() {
        let (g, _) = tool_round();
        let mut view = GraphView::new(g);
        view.select_down(); // the turn
        view.expand();
        let tui = shannon_core::config::TuiConfig {
            graph_style: shannon_core::GraphStyle::Git,
            ..Default::default()
        };
        let rows = crate::view::graph_row_texts(
            &view,
            crate::view::GraphLook::of(&tui),
            &crate::theme::Theme::default(),
        );
        let messages: Vec<&str> = rows.iter().skip(2).map(|r| r.trim_end()).collect();
        let labels = ["you: look", "[bash: ls]", "[result: a.txt]", "reply: done"];
        assert_eq!(messages.len(), labels.len(), "{rows:?}");
        for (row, label) in messages.iter().zip(labels) {
            // The label, after nothing but the lanes: `user: [bash: ls]`
            // ends with `[bash: ls]` too.
            let lanes = row.strip_suffix(label).unwrap_or_else(|| panic!("{row:?}"));
            assert!(
                lanes.chars().all(|c| " │●○─├╰└".contains(c)),
                "{row:?} is {label:?} after its lanes"
            );
        }
    }

    #[test]
    fn navigation_clamps_and_chords() {
        let (g, ..) = sample();
        let mut view = GraphView::new(g); // 3 visible by default
        assert_eq!(view.selected(), 0);
        view.select_up();
        assert_eq!(view.selected(), 0, "clamp at top");
        view.select_bottom();
        assert_eq!(view.selected(), 2);
        view.select_down();
        assert_eq!(view.selected(), 2, "clamp at bottom");

        // The slot itself. What `gg` and `mf` *mean* is the dispatcher's, and
        // is tested there — through the path the user's keys actually take
        // ([[feedback_mutation_validates_the_path]]).
        assert_eq!(view.take_chord(), None, "nothing armed to begin with");
        view.arm_chord('g');
        assert_eq!(view.take_chord(), Some('g'));
        assert_eq!(view.take_chord(), None, "taking empties the slot");
        // A prefix waits for exactly one key, so arming again replaces it
        // rather than stacking.
        view.arm_chord('g');
        view.arm_chord('m');
        assert_eq!(view.take_chord(), Some('m'));

        // **Motions must not clear the slot themselves.** Eight of them used
        // to, and the four dispatch arms that didn't were the bug. The take
        // now happens once, in the dispatcher, before any of these run; if a
        // motion re-grows its own clear, this fails.
        view.arm_chord('g');
        view.select_down();
        view.select_up();
        view.expand();
        view.collapse();
        view.select_top();
        view.select_bottom();
        view.toggle();
        assert_eq!(
            view.take_chord(),
            Some('g'),
            "a motion cleared the chord behind the dispatcher's back"
        );
    }

    #[test]
    fn window_keeps_selection_visible_and_marks_folds() {
        let (g, ..) = sample();
        let mut view = GraphView::new(g);

        // Height 2 over 3 rows: selecting the last scrolls so it's the bottom row.
        view.select_bottom();
        let win = view.window(2);
        assert_eq!(win.len(), 2);
        assert!(win[1].selected, "selection on the bottom visible row");
        // A collapsed turn advertises a fold; the episode is open.
        assert_eq!(win[1].fold, Some(false), "turn is collapsed");

        view.select_top();
        let win = view.window(2);
        assert_eq!(win[0].fold, Some(true), "episode is expanded");
        assert!(win[0].selected);
    }

    #[test]
    fn window_top_scrolls_with_a_scrolloff_margin() {
        // The dogfooding bug: from the bottom, moving up scrolled the page
        // immediately with the cursor pinned to the last row — no context below.
        // Vim rule: the cursor travels through the window; the viewport moves
        // only when the selection presses into the scrolloff margin.
        // `window_top` reads only lengths/indices, so a synthetic long visible
        // list exercises the geometry without building a 30-node graph.
        let (g, ..) = sample();
        let mut view = GraphView::new(g);
        view.visible_order = vec![0; 30];
        view.set_scrolloff(3);
        let h = 10;

        // `G`: the last page shows in full — at the list end the cursor may sit
        // inside the margin (there is nothing more to reveal below).
        view.selected_vis = 29;
        assert_eq!(view.window_top(h), 20);

        // Moving up: the viewport HOLDS while the cursor is inside the band…
        view.selected_vis = 24;
        assert_eq!(view.window_top(h), 20, "cursor travels, page stays");
        // …and scrolls once only 3 rows of context would remain above.
        view.selected_vis = 22;
        assert_eq!(view.window_top(h), 19, "keeps `scrolloff` rows above");

        // Same on the way down: hold inside the band, scroll at the margin.
        view.selected_vis = 25;
        assert_eq!(view.window_top(h), 19, "3 rows below — still at the edge");
        view.selected_vis = 26;
        assert_eq!(view.window_top(h), 20, "keeps `scrolloff` rows below");

        // At the very top the cursor enters the margin (list start).
        view.selected_vis = 0;
        assert_eq!(view.window_top(h), 0);

        // An oversized margin degrades to keep-centered (vim's `so=999`).
        view.set_scrolloff(999);
        view.selected_vis = 15;
        assert_eq!(view.window_top(h), 10, "cursor held mid-window");
    }

    /// **`zt` past the end survives a frame, not a resize** — the rule a text
    /// page keeps, measured in Neovim (`viewport::top_ceiling`). A list that
    /// fits its window is the sharpest case: the early return this had
    /// (`len <= height` → 0) snapped every such `zt` back on the next frame.
    #[test]
    fn a_past_end_window_survives_a_frame_but_not_a_resize() {
        let (g, ..) = sample();
        let mut view = GraphView::new(g);
        view.visible_order = vec![0; 30];
        view.set_scrolloff(0);
        let h = 10;
        view.window_top(h); // drawn once at this height

        view.select_row(27);
        view.place_selection(crate::viewport::Place::Top, h);
        assert_eq!(view.window_top(h), 27, "zt near the end: past it");
        assert_eq!(view.window_top(h), 27, "and it stays for the next frame");
        assert_eq!(
            view.window(h).len(),
            3,
            "three rows left to draw; `~` below"
        );

        assert_eq!(view.window_top(h + 1), 19, "a resize refills the last page");

        // A list shorter than its window, the early return's case.
        view.visible_order = vec![0; 5];
        view.select_row(3);
        view.window_top(h);
        view.place_selection(crate::viewport::Place::Top, h);
        assert_eq!(view.window_top(h), 3, "zt in a list that fits");
    }

    #[test]
    fn reveal_and_select_lands_on_a_nested_branch() {
        // The sample's child episode is a branch nested under turn B, hidden by
        // default (turns collapsed). Reveal+select must expand the path to it.
        let (g, _root, child_id, ..) = sample();
        let mut view = GraphView::new(g);
        assert!(
            !visible(&view).contains(&Node::Episode(child_id)),
            "the branch starts hidden"
        );
        view.reveal_and_select_episode(child_id);
        assert_eq!(
            view.selected_node(),
            Some(Node::Episode(child_id)),
            "selection lands on the revealed nested branch"
        );
    }

    /// The rows `/` searches, as these tests read them: every row's label —
    /// the Claude Code section's among them. (The view draws more around
    /// each — `view::graph_row_texts` — but the words are these.)
    fn searched(view: &GraphView) -> Vec<String> {
        view.searched_tree_rows()
            .iter()
            .map(|r| r.label.to_string())
            .collect()
    }

    /// **`/` finds a row inside a fold and opens it**, as vim's search opens
    /// a closed fold (`'foldopen'`). A session forked from a turn hangs in
    /// that turn's fold, folded by default, and a search of the drawn rows
    /// could not reach it (Steven, 10-15: "it appears it is gone"). The
    /// Claude Code section, after the whole tree, is still reached, and the
    /// search goes round from it into the tree.
    #[test]
    fn a_search_opens_the_fold_its_match_is_in() {
        let (g, _root, child_id, ..) = sample();
        let mut view = GraphView::new(g);
        assert!(
            !visible(&view).contains(&Node::Episode(child_id)),
            "premise: folded"
        );
        let re = |s: &str| regex::Regex::new(&regex::escape(s)).unwrap();
        let child = re(&child_id.short_hex());
        assert_eq!(view.search(searched(&view), &child, true, 1), Some(false));
        assert_eq!(
            view.selected_node(),
            Some(Node::Episode(child_id)),
            "found, its folds opened"
        );

        view.set_claude(Some(ClaudeSection {
            found: vec![found("c1a0de00")],
            read: HashMap::new(),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        }));
        assert_eq!(
            view.search(searched(&view), &re("c1a0de00"), true, 1),
            Some(false)
        );
        assert_eq!(
            view.selected_claude().map(|f| f.id.as_str()),
            Some("c1a0de00")
        );
        assert_eq!(view.search(searched(&view), &child, true, 1), Some(true));
        assert_eq!(view.selected_node(), Some(Node::Episode(child_id)));
    }

    #[test]
    fn a_branch_shows_only_its_divergent_turns() {
        // Root [a, b, c]; a branch off b that shares [a, b] then diverges to d.
        // Git-style: the branch shows only d — a, b stay under the root.
        let (a, b, c, d) = (turn("a"), turn("b"), turn("c"), turn("d"));
        let root_id = EpisodeId::from_seed(b"root");
        let branch_id = EpisodeId::from_seed(b"branch");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        root.append(c.occurrence());
        let mut branch = Episode::branch(
            branch_id,
            cfg(),
            Origin {
                episode: root_id,
                turn: b.id,
                index: Some(1),
                kind: EdgeKind::Branch,
            },
            2,
        );
        branch.append(d.occurrence()); // only the divergent turn
        let mut g = Graph::new();
        for t in [&a, &b, &c, &d] {
            g.insert_content((*t).clone());
        }
        g.insert_episode(root);
        g.insert_episode(branch);

        let view = GraphView::new(g);
        let branch_turns: Vec<TurnId> = view
            .full
            .iter()
            .filter_map(|r| match r.node() {
                Node::Turn { episode, turn, .. } if episode == branch_id => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(
            branch_turns,
            vec![d.id],
            "the branch re-shows neither a nor b — only its divergent turn d"
        );
    }

    /// **A session continuing a conversation hangs at the turn it parted at**
    /// (Steven, 10-16: "the same principle we use everywhere else") — out of
    /// Shannon's roots, its imported turns left to the conversation, drawn
    /// once; a branch cut from an imported turn at that turn of the
    /// conversation's; one whose turn is not found from the conversation's
    /// row, every turn its own. The conversation and the turn holding a
    /// session open by default — moving it there must not hide it — and a
    /// branch's turn stays folded, as everywhere.
    #[test]
    fn a_continued_session_hangs_at_the_turn_it_parted_at() {
        let [i0, i1, mine, branched, x, own_u, plain] =
            ["first", "second", "mine", "branched", "x", "own u", "plain"].map(turn);
        let [s_id, b_id, u_id, p_id] = [b"s", b"b", b"u", b"p"].map(|s| EpisodeId::from_seed(s));
        let continues = |id, turns, at: &str| {
            let mut ep = Episode::root(id, cfg());
            ep.inherited = Some(shannon_core::Inherited {
                session: "cc-1".into(),
                turns,
                at: Some(at.into()),
                opened: Vec::new(),
            });
            ep
        };
        let mut s = continues(s_id, 2, "u1");
        for t in [&i0, &i1, &mine] {
            s.append(t.occurrence());
        }
        let mut b = Episode::branch(
            b_id,
            cfg(),
            Origin {
                episode: s_id,
                turn: i0.id,
                index: Some(0),
                kind: EdgeKind::Branch,
            },
            1,
        );
        b.append(branched.occurrence());
        let mut u = continues(u_id, 1, "not-in-it");
        u.append(x.occurrence());
        u.append(own_u.occurrence());
        let mut p = Episode::root(p_id, cfg());
        p.append(plain.occurrence());
        let mut g = Graph::new();
        for t in [&i0, &i1, &mine, &branched, &x, &own_u, &plain] {
            g.insert_content(t.clone());
        }
        for ep in [s, b, u, p] {
            g.insert_episode(ep);
        }
        let claude = |uuid: &str, prompt: &str| ImportedTurn {
            prompt: prompt.into(),
            uuid: Some(uuid.into()),
            ..ImportedTurn::default()
        };
        let read = Imported {
            id: "cc-1".into(),
            cwd: None,
            turns: vec![
                claude("u0", "first"),
                claude("u1", "second"),
                claude("u2", "went on in Claude Code"),
            ],
            skipped: Default::default(),
            earlier: None,
            leaf: None,
        };
        let mut view = GraphView::new(g);
        view.set_claude(Some(ClaudeSection {
            found: vec![found("cc-1")],
            read: HashMap::from([("cc-1".to_string(), Arc::new(read))]),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        }));
        // **Its turns closed until opened** (Steven, 10-18): what hangs from
        // it under its row, in its turns' order — the branch's turn, then the
        // session's — then what hangs from the row itself; no turn drawn.
        let conversation = row_of(&view, Entry::Conversation(0)).unwrap();
        let under: Vec<Entry> = (0..view.full.len())
            .filter(|&f| view.parent[f] == Some(conversation))
            .map(|f| view.full[f].entry)
            .collect();
        assert_eq!(
            under,
            [b_id, s_id, u_id].map(|id| Entry::Node(Node::Episode(id))),
            "what hangs from it, and no turn"
        );
        assert!(view.expanded[conversation], "open to what hangs from it");
        open_turns(&mut view, 0);

        let row = |entry: Entry| view.full.iter().position(|r| r.entry == entry).unwrap();
        let session = |id| row(Entry::Node(Node::Episode(id)));
        let claude_turn = |turn| {
            row(Entry::ClaudeTurn {
                conversation: 0,
                turn,
            })
        };
        let turns_of = |id| -> Vec<usize> {
            view.full
                .iter()
                .filter_map(|r| match r.entry {
                    Entry::Node(Node::Turn { episode, index, .. }) if episode == id => Some(index),
                    _ => None,
                })
                .collect()
        };
        assert!(
            session(p_id) < row(Entry::ClaudeHeading),
            "born in Shannon, among Shannon's roots"
        );
        assert_eq!(
            view.parent[session(s_id)],
            Some(claude_turn(1)),
            "where it parted"
        );
        assert_eq!(
            turns_of(s_id),
            [2],
            "its imported turns, the conversation's"
        );
        assert_eq!(
            view.parent[session(b_id)],
            Some(claude_turn(0)),
            "a branch of an imported turn, at that turn of the conversation's"
        );
        assert_eq!(
            view.parent[session(u_id)],
            Some(row(Entry::Conversation(0))),
            "its turn not found: from the conversation's row"
        );
        assert_eq!(turns_of(u_id), [0, 1], "every turn its own");
        assert!(view.expanded[row(Entry::Conversation(0))] && view.expanded[claude_turn(1)]);
        assert!(
            !view.expanded[claude_turn(0)],
            "a branch's turn stays folded"
        );
    }

    /// A section listing `ids`, with `read` of them read: each turn
    /// `u0`, `u1`, … prompting with its uuid.
    fn section_of(ids: &[&str], read: &[(&str, usize)]) -> Option<ClaudeSection> {
        let reading = |id: &str, turns: usize| Imported {
            id: id.into(),
            cwd: None,
            turns: (0..turns)
                .map(|t| ImportedTurn {
                    prompt: format!("{id} u{t}"),
                    uuid: Some(format!("u{t}")),
                    ..ImportedTurn::default()
                })
                .collect(),
            skipped: Default::default(),
            earlier: None,
            leaf: None,
        };
        Some(ClaudeSection {
            found: ids.iter().map(|id| found(id)).collect(),
            read: read
                .iter()
                .map(|&(id, turns)| (id.to_string(), Arc::new(reading(id, turns))))
                .collect(),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        })
    }

    /// **A session parted at a question the summary's turn took in hangs
    /// there** — an import holds what the compaction kept after its summary,
    /// so the question after it opens a turn of its own, and the import's
    /// last turn is named by it; the conversation as `:graph` reads it has
    /// the summary unanswered there, the question in its turn (Steven,
    /// 10-17, the kept messages spliced). Looked up by the opening entry
    /// alone, it hung from the conversation's row, every turn drawn again.
    #[test]
    fn a_session_parted_at_a_joined_question_hangs_at_its_turn() {
        let [summary, question] = ["the summary", "now go on"].map(turn);
        let id = EpisodeId::from_seed(b"s");
        let mut s = Episode::root(id, cfg());
        s.inherited = Some(shannon_core::Inherited {
            session: "cc-1".into(),
            turns: 2,
            at: Some("u2".into()),
            opened: Vec::new(),
        });
        let mut g = Graph::new();
        for t in [&summary, &question] {
            g.insert_content(t.clone());
            s.append(t.occurrence());
        }
        g.insert_episode(s);
        let mut section = section_of(&["cc-1"], &[("cc-1", 2)]).unwrap();
        let read = section.read.get_mut("cc-1").unwrap();
        let read = Arc::make_mut(read);
        read.turns[1].summary = true;
        read.turns[1].joined = vec!["u2".into()];
        let mut view = GraphView::new(g);
        view.set_claude(Some(section));
        open_turns(&mut view, 0);
        let row = |entry: Entry| view.full.iter().position(|r| r.entry == entry).unwrap();
        assert_eq!(
            view.parent[row(Entry::Node(Node::Episode(id)))],
            Some(row(Entry::ClaudeTurn {
                conversation: 0,
                turn: 1,
            })),
            "at the summary's turn, which took the question in"
        );
    }

    /// A session continuing conversation `cc-1` with `turns` of its own, all
    /// imported, opened by `opened` and parted at the last; and the view of
    /// it under a reading of `cc-1` whose turns are `u0`, `u1`, …, `summary`
    /// the one a compaction opened, taking `joined` in — its turns open.
    fn continuing(
        opened: &[&str],
        read: usize,
        summary: usize,
        joined: &[&str],
        branch_at: Option<usize>,
    ) -> (GraphView, EpisodeId, Option<EpisodeId>) {
        let id = EpisodeId::from_seed(b"s");
        let mut s = Episode::root(id, cfg());
        s.inherited = Some(shannon_core::Inherited {
            session: "cc-1".into(),
            turns: opened.len(),
            at: opened.last().map(|o| o.to_string()),
            opened: opened.iter().map(|o| Some(o.to_string())).collect(),
        });
        let mut g = Graph::new();
        for t in opened.iter().map(|o| turn(o)) {
            g.insert_content(t.clone());
            s.append(t.occurrence());
        }
        g.insert_episode(s);
        let branch = branch_at.map(|at| {
            let b = EpisodeId::from_seed(b"b");
            g.branch_prefix(id, at, b).expect("a branch");
            b
        });
        let mut section = section_of(&["cc-1"], &[("cc-1", read)]).unwrap();
        let reading = Arc::make_mut(section.read.get_mut("cc-1").unwrap());
        reading.turns[summary].summary = true;
        reading.turns[summary].joined = joined.iter().map(|j| j.to_string()).collect();
        let mut view = GraphView::new(g);
        view.set_claude(Some(section));
        open_turns(&mut view, 0);
        (view, id, branch)
    }

    /// **A branch cut from an imported summary hangs at the summary's turn**
    /// — the import's turn 0 is the compaction's summary, the conversation's
    /// turn 1; a count back from the parting put it at turn 0, before the
    /// compaction (review stop 4, A 1 a). Each imported turn is placed by
    /// the entry that opened it.
    #[test]
    fn a_branch_of_an_imported_summary_hangs_at_the_summary() {
        let (view, _, branch) = continuing(&["u1", "u2"], 2, 1, &["u2"], Some(0));
        let row = |entry: Entry| view.full.iter().position(|r| r.entry == entry).unwrap();
        assert_eq!(
            view.parent[row(Entry::Node(Node::Episode(branch.unwrap())))],
            Some(row(Entry::ClaudeTurn {
                conversation: 0,
                turn: 1,
            })),
        );
    }

    /// **A session ending on a question its compaction kept hangs after the
    /// compaction** — the import reads the summary, then the kept question;
    /// the conversation, the question, then the summary: parted where the
    /// question is, it hung before the compaction it holds (review stop 4,
    /// A 1 b). It parts at the latest turn it holds.
    #[test]
    fn a_session_ending_on_a_kept_question_hangs_after_its_compaction() {
        let (view, id, _) = continuing(&["u2", "u1"], 3, 2, &[], None);
        let row = |entry: Entry| view.full.iter().position(|r| r.entry == entry).unwrap();
        assert_eq!(
            view.parent[row(Entry::Node(Node::Episode(id)))],
            Some(row(Entry::ClaudeTurn {
                conversation: 0,
                turn: 2,
            })),
        );
    }

    /// **A reading lands only for the file its row stands for** — one id can
    /// name a file in two directories, and a reading of the other file
    /// was taken as this one's, its turns under this row (review stop 4,
    /// A 4.5: the check deleted, every test passed).
    #[test]
    fn a_reading_of_another_file_is_not_taken() {
        let (g, ..) = sample();
        let mut section = section_of(&["cc-1"], &[]).unwrap();
        section.found[0].path = PathBuf::from("/here/cc-1.jsonl");
        section.awaiting.insert("cc-1".into());
        let mut view = GraphView::new(g);
        view.set_claude(Some(section));
        let reading = Arc::new(Imported {
            id: "cc-1".into(),
            cwd: None,
            turns: vec![ImportedTurn::default()],
            skipped: Default::default(),
            earlier: None,
            leaf: None,
        });
        let there = Path::new("/there/cc-1.jsonl");
        let by = Scrubber::defaults();
        assert!(
            !view.take_reading(there, "cc-1", Some(&reading), &by),
            "not its file"
        );
        assert!(
            view.claude.as_ref().unwrap().read.is_empty(),
            "nothing shown"
        );
        let here = Path::new("/here/cc-1.jsonl");
        assert!(
            view.take_reading(here, "cc-1", Some(&reading), &by),
            "premise: its own lands"
        );
    }

    /// **A section takes no reading scrubbed otherwise than it was listed**
    /// — and one it asked for stays asked: it is listed again before it is
    /// shown, and asks again then (`ClaudeSection::scrubbed`; review stop
    /// 10, E 2). Taken, a set-aside tree would hold one reading scrubbed
    /// under the new secrets beside titles under the old.
    #[test]
    fn a_section_takes_no_reading_scrubbed_otherwise() {
        let (g, ..) = sample();
        let mut section = section_of(&["cc-1"], &[]).unwrap();
        section.found[0].path = PathBuf::from("/here/cc-1.jsonl");
        section.awaiting.insert("cc-1".into());
        let mut view = GraphView::new(g);
        view.set_claude(Some(section));
        let reading = Arc::new(Imported {
            id: "cc-1".into(),
            cwd: None,
            turns: vec![ImportedTurn::default()],
            skipped: Default::default(),
            earlier: None,
            leaf: None,
        });
        let here = Path::new("/here/cc-1.jsonl");
        let marked = Scrubber::new(
            &shannon_core::SecretsConfig::default(),
            &Arc::new(shannon_core::Aliases::default()),
        );
        assert!(
            view.take_reading(here, "cc-1", Some(&reading), &marked),
            "still asked"
        );
        let section = view.claude.as_ref().unwrap();
        assert!(section.read.is_empty(), "not taken");
        assert!(section.awaiting.contains("cc-1"), "still awaited");
    }

    /// **An anchor that is gone re-anchors at the cursor, on every
    /// rebuild** — a re-list in Visual (`a` then, `:unread` now), the range
    /// begun on a conversation the new list lacks: V-LINE was left over
    /// nothing (review stop 3, A F1, B 6, C 4a). The rule lived in the
    /// refresh's `restore` alone.
    #[test]
    fn a_gone_anchor_reanchors_at_the_cursor_on_every_rebuild() {
        let (g, root, ..) = sample();
        let mut view = GraphView::new(g);
        view.set_claude(section_of(&["here0000", "there000"], &[]));
        assert!(view.select_claude("there000"), "premise");
        crate::rows::RowSelect::start_selection(&mut view);
        assert!(view.select_node(Node::Episode(root)), "premise");

        view.set_claude(section_of(&["here0000"], &[]));
        assert!(view.in_visual(), "still a range");
        assert_eq!(
            view.visual_anchor(),
            Some(Node::Episode(root)),
            "at the cursor"
        );
    }

    /// **A cursor whose row went lands on the row now in its place** — vim's
    /// `dd` (Steven, 10-17): the turn after a deleted turn; the one above when
    /// nothing follows; the first row when nothing above it is left; the fold
    /// that now hides its place. Each fresh view stands on its session's row,
    /// where the cursor went before, whatever was deleted (C 4b pinned that).
    #[test]
    fn a_gone_cursor_lands_on_the_row_now_in_its_place() {
        let [a, b, c, q_turn] = ["a", "b", "c", "q's"].map(turn);
        let (r, q) = (EpisodeId::from_seed(b"r"), EpisodeId::from_seed(b"q"));
        // `r` holds a, b, c; `q`, when kept, q's — `r` less the turns at `drop`.
        let graph = |drop: &[usize], keep_q: bool| {
            let mut g = Graph::new();
            let mut ep = Episode::root(r, cfg());
            for t in [&a, &b, &c, &q_turn] {
                g.insert_content(t.clone());
            }
            for t in [&a, &b, &c] {
                ep.append(t.occurrence());
            }
            g.insert_episode(ep);
            if keep_q {
                let mut other = Episode::root(q, cfg());
                other.append(q_turn.occurrence());
                g.insert_episode(other);
            }
            assert!(drop.is_empty() || g.remove_turns(&r, drop), "premise");
            g
        };
        let on = |id, turn: &Turn, index| Node::Turn {
            episode: id,
            turn: turn.id,
            index,
        };
        // The cursor on `from` in `before`; where it lands over `after`.
        let lands = |before: Graph, from: Node, after: Graph| {
            let mut old = GraphView::with_sort(before, GraphSort::Hash);
            assert!(old.select_node(from), "premise: {from:?}");
            let mut fresh = GraphView::with_sort(after, GraphSort::Hash);
            fresh.reveal_and_select_episode(r);
            fresh.restore(None, old.held());
            fresh.selected_entry()
        };
        let row = |node| Some(Entry::Node(node));

        assert_eq!(
            lands(graph(&[], false), on(r, &b, 1), graph(&[1], false)),
            row(on(r, &c, 1)),
            "the turn after it, renumbered into its place"
        );
        assert_eq!(
            lands(graph(&[], false), on(r, &c, 2), graph(&[2], false)),
            row(on(r, &b, 1)),
            "nothing follows: the one above"
        );
        let both = GraphView::with_sort(graph(&[], true), GraphSort::Hash);
        let first = both.visible_order[0];
        let Entry::Node(Node::Episode(top)) = both.full[first].entry else {
            panic!("premise: a session row first");
        };
        let (gone, kept) = if top == q { (q, r) } else { (r, q) };
        let without = if gone == q {
            graph(&[], false)
        } else {
            // `r` cannot go from the builder; `q` alone stands for the tree
            // without it.
            let mut g = Graph::new();
            g.insert_content(q_turn.clone());
            let mut other = Episode::root(q, cfg());
            other.append(q_turn.occurrence());
            g.insert_episode(other);
            g
        };
        let mut old = both.clone();
        assert!(old.select_node(Node::Episode(gone)), "premise");
        let mut fresh = GraphView::with_sort(without, GraphSort::Hash);
        fresh.select_bottom();
        fresh.restore(None, old.held());
        assert_eq!(
            fresh.selected_entry(),
            row(Node::Episode(kept)),
            "nothing above it left: the first row"
        );

        // A session hung where it parted; deleted, its conversation closes
        // by default over the place, and the cursor stands on that fold.
        let mut s = Episode::root(q, cfg());
        s.inherited = Some(shannon_core::Inherited {
            session: "cc-1".into(),
            turns: 1,
            at: Some("u1".into()),
            opened: Vec::new(),
        });
        s.append(a.occurrence());
        s.append(q_turn.occurrence());
        let mut with_s = graph(&[], false);
        with_s.insert_episode(s);
        let mut old = GraphView::with_sort(with_s, GraphSort::Hash);
        old.set_claude(section_of(&["cc-1"], &[("cc-1", 3)]));
        assert!(old.select_node(Node::Episode(q)), "premise");
        let mut fresh = GraphView::with_sort(graph(&[], false), GraphSort::Hash);
        fresh.restore(section_of(&["cc-1"], &[("cc-1", 3)]), old.held());
        assert_eq!(
            fresh.selected_entry(),
            Some(Entry::Conversation(0)),
            "the fold that hides its place"
        );
    }

    /// **A turn deleted above renumbers every place after it; what is held
    /// there stays** (V4): a mark kept for a `p`, the cursor, a turn opened
    /// to its messages, the window's top. Held by place, the mark went, the
    /// cursor landed where the deleted turn had stood and the fold closed —
    /// in every other tree window as well, since each holds its own copy.
    #[test]
    fn a_turn_deleted_above_leaves_what_is_held_below() {
        // Five turns, the last two the same content: `twice` at 3 and 4.
        let once: Vec<Turn> = (0..3).map(|i| turn(&format!("t{i}"))).collect();
        let twice = turn("asked twice");
        let r = EpisodeId::from_seed(b"r");
        let mut g = Graph::new();
        let mut ep = Episode::root(r, cfg());
        for t in once.iter().chain([&twice, &twice]) {
            g.insert_content(t.clone());
            ep.append(t.occurrence());
        }
        g.insert_episode(ep);
        let on = |t: &Turn, index| Node::Turn {
            episode: r,
            turn: t.id,
            index,
        };
        let reply = |turn_index| Node::Message {
            episode: r,
            turn: once[2].id,
            turn_index,
            index: 1,
        };
        let without = |drop: &[usize]| {
            let mut after = g.clone();
            assert!(after.remove_turns(&r, drop), "premise");
            after
        };
        let h = 4;
        let mut old = GraphView::with_sort(g.clone(), GraphSort::Hash);
        old.set_scrolloff(0);
        // t2 opened to its messages, its reply and the second copy marked,
        // the cursor on the first copy, the window's top on t2.
        assert!(old.select_node(on(&once[2], 2)), "premise");
        old.toggle();
        assert_eq!(old.mark_nodes(&[reply(2), on(&twice, 4)]), 2, "premise");
        assert!(old.select_node(on(&twice, 3)), "premise");
        old.place_selection(crate::viewport::Place::Bottom, h);
        let top = old.window_top(h);
        let t2 = old
            .visible_order
            .iter()
            .position(|&f| old.full[f].entry == Entry::Node(on(&once[2], 2)));
        assert_eq!(Some(top), t2, "premise: t2 on top");

        let mut fresh = GraphView::with_sort(without(&[1]), GraphSort::Hash);
        fresh.set_scrolloff(0);
        fresh.restore(None, old.held());
        assert_eq!(
            fresh.marked_nodes(),
            vec![reply(1), on(&twice, 3)],
            "the marks, renumbered"
        );
        assert_eq!(
            fresh.selected_entry(),
            Some(Entry::Node(on(&twice, 2))),
            "the cursor, renumbered"
        );
        let t2 = fresh.row_of(on(&once[2], 1)).expect("t2 still there");
        assert!(fresh.expanded[t2], "its fold still open");
        assert_eq!(
            fresh.visible_order.get(fresh.window_top(h)),
            Some(&t2),
            "and t2 still on top"
        );

        // The limit `Key` names: the first copy deleted, the second takes
        // its rank — what was held on the second is lost, what was held on
        // the first lands on the second.
        let mut fresh = GraphView::with_sort(without(&[3]), GraphSort::Hash);
        fresh.restore(None, old.held());
        assert_eq!(
            fresh.marked_nodes(),
            vec![reply(2)],
            "the later copy's mark gone"
        );
        assert_eq!(
            fresh.selected_entry(),
            Some(Entry::Node(on(&twice, 3))),
            "the cursor, from the copy that went to the one that stays"
        );
    }

    /// Ten sessions of one turn each — twenty rows, a session's then its
    /// turn's — less `without`.
    fn ten_sessions(without: Option<EpisodeId>) -> Graph {
        let mut g = Graph::new();
        for i in 0..10u8 {
            let id = EpisodeId::from_seed(&[b'e', i]);
            if Some(id) == without {
                continue;
            }
            let t = turn(&format!("question {i}"));
            g.insert_content(t.clone());
            let mut ep = Episode::root(id, cfg());
            ep.append(t.occurrence());
            g.insert_episode(ep);
        }
        g
    }

    /// A view of `g` with no scroll margin, so a window's rows are exact.
    fn flat_view(g: Graph) -> GraphView {
        let mut v = GraphView::with_sort(g, GraphSort::Hash);
        v.set_scrolloff(0);
        v
    }

    /// **`:b N`'s landing: the row you were on, mid-window at the next
    /// frame** — once; the frame after follows the cursor as any does. A
    /// spot the tree no longer holds moves nothing.
    #[test]
    fn go_to_puts_the_spot_mid_window_once() {
        let mut was = flat_view(ten_sessions(None));
        was.select_row(15);
        let spot = was.spot().expect("a row");
        let mut back = flat_view(ten_sessions(None));
        assert!(back.go_to(&spot), "found");
        assert_eq!(back.selected_vis, 15);
        assert_eq!(back.window_top(6), 13, "15 in the middle of six rows");
        back.select_row(17);
        assert_eq!(back.window_top(6), 13, "then the window follows, as any");

        let Entry::Node(Node::Turn { episode, .. }) = was.full[was.visible_order[15]].entry else {
            panic!("premise: a turn's row");
        };
        let mut without = flat_view(ten_sessions(Some(episode)));
        let before = without.selected_vis;
        assert!(!without.go_to(&spot), "its session deleted");
        assert_eq!(without.selected_vis, before, "nothing moved");
        assert_eq!(
            without.window_top(6),
            flat_view(ten_sessions(Some(episode))).window_top(6)
        );
    }

    /// **A split's copy starts on the same window** — at the same height (a
    /// `:vs`), the same top, as vim's `:split` copies the top line. Nothing
    /// pinned the top (V4 verify, L2: `top: 0, height: None` in what the
    /// split copied passed the suite).
    #[test]
    fn a_splits_copy_starts_on_the_same_window() {
        use crate::viewport::Place;
        let h = 6;
        let mut source = flat_view(ten_sessions(None));
        source.window_top(h);
        source.select_row(10);
        source.place_selection(Place::Top, h);
        source.select_row(13);
        assert_eq!(source.window_top(h), 10, "premise: scrolled");
        let mut copy = flat_view(ten_sessions(None));
        copy.restore(None, source.held());
        assert_eq!(copy.selected_vis, 13, "the cursor");
        assert_eq!(copy.window_top(h), 10, "the top");
    }

    /// **A tree's marks, by identity, through any window on it** — taken
    /// from one, shown in another by row; and a window whose marks are as
    /// many as the tree's but on other rows is one whose marks moved.
    #[test]
    fn a_trees_marks_go_by_identity_between_its_windows() {
        let mut tree = GraphBuffer::new(BufferId(9));
        let mut one = flat_view(ten_sessions(None));
        let mut two = flat_view(ten_sessions(None));
        one.select_row(2);
        one.toggle_mark();
        assert!(tree.marks_differ(&one), "a mark made here");
        tree.take_marks(&one);
        assert!(!tree.marks_differ(&one), "taken");
        tree.show_marks(&mut two);
        assert_eq!(two.marked_nodes(), one.marked_nodes(), "shown in the other");

        two.select_row(2);
        two.toggle_mark();
        two.select_row(4);
        two.toggle_mark();
        assert_eq!(
            two.marked_nodes().len(),
            1,
            "premise: as many, on another row"
        );
        assert!(tree.marks_differ(&two), "moved, though as many");
    }

    /// **A re-sort under the window keeps the cursor's row on the screen**
    /// (V4 verify, L1) — under the default sort, a session's new turn takes
    /// its rows to the top, the window's top row among them; the top that
    /// followed that row by identity left the cursor below the window, and
    /// the next frame put it on the bottom margin.
    #[test]
    fn a_resort_under_the_window_keeps_the_cursors_screen_row() {
        use crate::viewport::Place;
        let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let sealed = |user: String, secs| {
            Turn::seal(
                vec![Message::user(user), Message::assistant("ok")],
                cfg(),
                at(secs),
            )
        };
        let ids: Vec<EpisodeId> = (0..10u8)
            .map(|i| EpisodeId::from_seed(&[b'r', i]))
            .collect();
        // Session `i`'s turn at second `i + 1`; `bumped` takes a turn at 100.
        let graph = |bumped: Option<EpisodeId>| {
            let mut g = Graph::new();
            for (i, &id) in ids.iter().enumerate() {
                let mut ep = Episode::root(id, cfg());
                let mut turns = vec![sealed(format!("q{i}"), i as u64 + 1)];
                if bumped == Some(id) {
                    turns.push(sealed("again".into(), 100));
                }
                for t in turns {
                    g.insert_content(t.clone());
                    ep.append(t.occurrence());
                }
                g.insert_episode(ep);
            }
            g
        };
        let view = |g: Graph| {
            let mut v = GraphView::with_sort(g, GraphSort::Recency);
            v.set_scrolloff(0);
            v
        };
        let h = 6;
        let mut old = view(graph(None));
        old.window_top(h);
        old.select_row(10);
        old.place_selection(Place::Top, h);
        old.select_row(13);
        assert_eq!(old.window_top(h), 10, "premise: the cursor three rows down");
        let Entry::Node(Node::Episode(top)) = old.full[old.visible_order[10]].entry else {
            panic!("premise: a session's row on top");
        };
        let mut fresh = view(graph(Some(top)));
        fresh.restore(None, old.held());
        let top_now = fresh
            .row_of(Node::Episode(top))
            .map(|f| fresh.visible_pos(f));
        assert_eq!(
            top_now,
            Some(0),
            "premise: the top's session went to the top"
        );
        let cursor = fresh.selected_vis;
        assert!(cursor >= h, "premise: below a window from 0");
        assert_eq!(fresh.window_top(h), cursor - 3, "still three rows down");

        // The cursor's own session re-sorted: the top, followed, is below it.
        let Some(Node::Turn { episode: mine, .. }) = old.selected_node() else {
            panic!("premise: the cursor on a turn");
        };
        let mut fresh = view(graph(Some(mine)));
        fresh.restore(None, old.held());
        let cursor = fresh.selected_vis;
        assert!(cursor < 3, "premise: its session went to the top");
        assert_eq!(
            fresh.window_top(h),
            0,
            "as near three rows down as it can be"
        );
    }

    /// **A row arriving at the window's top edge shows** (Steven, 10-25:
    /// Neovim's way) — a turn of the session just above the window, landing
    /// between its last row and the window's top, becomes the top and the
    /// cursor goes a row down the screen, as Neovim shows a line inserted
    /// just above another window's top (measured). Two rows above, it stays
    /// hidden, as Neovim's does.
    #[test]
    fn rows_arriving_at_the_tops_edge_show() {
        use crate::viewport::Place;
        let ids: Vec<EpisodeId> = (0..10u8)
            .map(|i| EpisodeId::from_seed(&[b'e', i]))
            .collect();
        // `ten_sessions`, with `more` given a second turn.
        let graph = |more: Option<EpisodeId>| {
            let mut g = Graph::new();
            for (i, &id) in ids.iter().enumerate() {
                let mut ep = Episode::root(id, cfg());
                let mut turns = vec![turn(&format!("question {i}"))];
                if more == Some(id) {
                    turns.push(turn("and then"));
                }
                for t in turns {
                    g.insert_content(t.clone());
                    ep.append(t.occurrence());
                }
                g.insert_episode(ep);
            }
            g
        };
        let h = 6;
        let held_at = |top: usize, cursor: usize| {
            let mut v = flat_view(graph(None));
            v.window_top(h);
            v.select_row(top);
            v.place_selection(Place::Top, h);
            v.select_row(cursor);
            assert_eq!(v.window_top(h), top, "premise: the window from {top}");
            v
        };
        let old = held_at(10, 13);
        let Entry::Node(Node::Episode(above)) = old.full[old.visible_order[8]].entry else {
            panic!("premise: the session above the window's top session");
        };
        let mut fresh = flat_view(graph(Some(above)));
        fresh.restore(None, old.held());
        assert!(
            matches!(fresh.full[fresh.visible_order[10]].entry,
                Entry::Node(Node::Turn { episode, .. }) if episode == above),
            "premise: its new turn just above the window's top"
        );
        assert_eq!(fresh.selected_vis, 14, "premise: the cursor kept its row");
        assert_eq!(
            fresh.window_top(h),
            10,
            "the arrival on top, the cursor a row lower"
        );

        // The cursor the window's top row: the arrival above it, likewise.
        let old = held_at(10, 10);
        let mut fresh = flat_view(graph(Some(above)));
        fresh.restore(None, old.held());
        assert_eq!(fresh.selected_vis, 11, "premise: the cursor kept its row");
        assert_eq!(fresh.window_top(h), 10, "on the cursor's own row too");

        let old = held_at(11, 13);
        let mut fresh = flat_view(graph(Some(above)));
        fresh.restore(None, old.held());
        assert_eq!(fresh.window_top(h), 12, "two above the top: hidden");
    }

    /// **A session sorted to the top shows, with the window at the list's
    /// start** — under the default sort a session's new turn takes it to the
    /// top; a window from the first row stayed a row down from it (the top
    /// held by identity), and nothing on the screen said the list went on
    /// above. Neovim shows lines inserted before line 1 likewise (measured
    /// 10-25). But a row from above the window re-sorting over the rows
    /// that were there arrived nowhere near its edge.
    #[test]
    fn a_row_sorted_to_the_top_shows_at_the_lists_start() {
        use crate::viewport::Place;
        let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let sealed = |user: String, secs| {
            Turn::seal(
                vec![Message::user(user), Message::assistant("ok")],
                cfg(),
                at(secs),
            )
        };
        let ids: Vec<EpisodeId> = (0..10u8)
            .map(|i| EpisodeId::from_seed(&[b'r', i]))
            .collect();
        // Session `i`'s turn at second `i + 1`; `bumped` takes a turn at 100.
        let graph = |bumped: Option<EpisodeId>| {
            let mut g = Graph::new();
            for (i, &id) in ids.iter().enumerate() {
                let mut ep = Episode::root(id, cfg());
                let mut turns = vec![sealed(format!("q{i}"), i as u64 + 1)];
                if bumped == Some(id) {
                    turns.push(sealed("again".into(), 100));
                }
                for t in turns {
                    g.insert_content(t.clone());
                    ep.append(t.occurrence());
                }
                g.insert_episode(ep);
            }
            g
        };
        let view = |g: Graph| {
            let mut v = GraphView::with_sort(g, GraphSort::Recency);
            v.set_scrolloff(0);
            v
        };
        let h = 10;
        let mut old = view(graph(None));
        old.select_row(3);
        assert_eq!(
            old.window_top(h),
            0,
            "premise: the window from the first row"
        );
        let Entry::Node(Node::Episode(bumped)) = old.full[old.visible_order[8]].entry else {
            panic!("premise: a session's row");
        };
        let mut fresh = view(graph(Some(bumped)));
        fresh.restore(None, old.held());
        assert_eq!(
            fresh
                .row_of(Node::Episode(bumped))
                .map(|f| fresh.visible_pos(f)),
            Some(0),
            "premise: sorted to the top"
        );
        assert_eq!(fresh.selected_vis, 6, "premise: the cursor three rows on");
        assert_eq!(
            fresh.window_top(h),
            0,
            "still the list's start, the arrival on top"
        );

        // From row 10 of a tall window, the session just above it sorts to
        // the top, over the eight rows that were above it: the window keeps
        // its top. (Tall, so the cursor would show from there too.)
        let h = 20;
        let mut old = view(graph(None));
        old.window_top(h);
        old.select_row(10);
        old.place_selection(Place::Top, h);
        old.select_row(13);
        assert_eq!(old.window_top(h), 10, "premise: the window from row 10");
        let Entry::Node(Node::Episode(above)) = old.full[old.visible_order[8]].entry else {
            panic!("premise: the session just above the window");
        };
        let Entry::Node(Node::Episode(top)) = old.full[old.visible_order[10]].entry else {
            panic!("premise: a session's row on top");
        };
        let mut fresh = view(graph(Some(above)));
        fresh.restore(None, old.held());
        let top_now = fresh
            .row_of(Node::Episode(top))
            .map(|f| fresh.visible_pos(f));
        assert_eq!(top_now, Some(11), "premise: the top's row, one lower");
        assert_eq!(
            fresh.window_top(h),
            11,
            "the top kept, not the row past the jump"
        );
    }

    /// **A refresh leaves the window as it was** (V4): its top by identity,
    /// a `zt` past the end, a half-typed chord. Each was the fresh view's —
    /// top 0, never drawn, no chord — so every background event scrolled the
    /// window and ate the `m` of an `mf`.
    #[test]
    fn a_refresh_keeps_the_window_and_its_chord() {
        use crate::viewport::Place;
        let graph = ten_sessions;
        let view = flat_view;
        let refreshed = |after: Graph, old: &GraphView| {
            let mut fresh = view(after);
            fresh.restore(None, old.held());
            fresh
        };
        let h = 6;
        let mut old = view(graph(None));
        assert_eq!(old.visible_len(), 20, "premise: two rows a session");
        old.window_top(h);
        old.select_row(10);
        old.place_selection(Place::Top, h);
        old.select_row(13);
        assert_eq!(
            old.window_top(h),
            10,
            "premise: the cursor inside the window"
        );

        assert_eq!(
            refreshed(graph(None), &old).window_top(h),
            10,
            "nothing changed: the window stays"
        );

        // The top's session deleted, rows 10 and 11: the top is the row
        // above what went — not where the cursor's screen row would put it
        // (8), nor a fresh view's (6).
        let Entry::Node(Node::Episode(gone)) = old.full[old.visible_order[10]].entry else {
            panic!("premise: a session's row on top");
        };
        let after = refreshed(graph(Some(gone)), &old);
        assert_eq!(after.selected_vis, 11, "premise: the cursor kept its row");
        assert_eq!(after.window_top(h), 9, "the row above what went");
        // Nothing above it left: the first row, as vim's line 1.
        let mut first = view(graph(None));
        first.select_row(3);
        assert_eq!(first.window_top(h), 0, "premise: the top row on top");
        let Entry::Node(Node::Episode(gone)) = first.full[first.visible_order[0]].entry else {
            panic!("premise: a session's row first");
        };
        assert_eq!(
            refreshed(graph(Some(gone)), &first).window_top(h),
            0,
            "the first row"
        );

        // `zt` on the last row: past the end, and a refresh is not a resize.
        old.select_row(19);
        old.place_selection(Place::Top, h);
        assert_eq!(old.window_top(h), 19, "premise: past the end");
        old.arm_chord('m');
        let mut same = refreshed(graph(None), &old);
        assert_eq!(same.window_top(h), 19, "still past the end");
        assert_eq!(
            same.take_chord(),
            Some('m'),
            "the `m` still waits for its key"
        );

        // A view that held no cursor moves no window: the fresh one's stands.
        let none = view(Graph::new());
        none.window_top(h);
        let mut fresh = view(graph(None));
        fresh.select_row(13);
        let own = fresh.clone().window_top(h);
        fresh.restore(None, none.held());
        assert_eq!(fresh.window_top(h), own, "as a fresh view shows it");
    }

    /// **A Claude Code turn is keyed by its conversation as well as its
    /// place** — turn 0 of one conversation is not turn 0 of the next: the
    /// cursor on the second's stays on the second's when the list reorders
    /// (C 4d: a key without the conversation passed every test).
    #[test]
    fn a_claude_turn_is_keyed_by_its_conversation() {
        let (g, ..) = sample();
        let mut view = GraphView::new(g);
        view.set_claude(section_of(
            &["aaaa0000", "bbbb0000"],
            &[("aaaa0000", 1), ("bbbb0000", 1)],
        ));
        assert!(view.select_claude("bbbb0000"), "premise");
        view.expand();
        view.select_down();
        let on = |view: &GraphView| {
            view.selected_claude_turn()
                .map(|(f, at, ..)| (f.id.clone(), at))
        };
        assert_eq!(on(&view), Some(("bbbb0000".to_string(), 0)), "premise");

        view.set_claude(section_of(
            &["bbbb0000", "aaaa0000"],
            &[("aaaa0000", 1), ("bbbb0000", 1)],
        ));
        assert_eq!(
            on(&view),
            Some(("bbbb0000".to_string(), 0)),
            "the same turn"
        );
    }

    /// **A record whose import reaches before the reading still hangs its
    /// branches** — `Inherited { turns: 5, at: <turn 1> }` over two read
    /// turns: an imported turn before the first read maps below zero, and
    /// hangs at the parting turn rather than panicking (a debug build, on
    /// every `:graph`) or vanishing (B 7, C 5: the guard passed every test).
    /// Shannon writes no such record; a stale or crafted one is read.
    #[test]
    fn an_import_reaching_before_the_reading_hangs_its_branches() {
        let turns: Vec<Turn> = (0..5).map(|t| turn(&format!("imported {t}"))).collect();
        let forked = turn("forked");
        let (s_id, b_id) = (EpisodeId::from_seed(b"s"), EpisodeId::from_seed(b"b"));
        let mut s = Episode::root(s_id, cfg());
        s.inherited = Some(shannon_core::Inherited {
            session: "cc-1".into(),
            turns: 5,
            at: Some("u1".into()),
            opened: Vec::new(),
        });
        for t in &turns {
            s.append(t.occurrence());
        }
        let mut b = Episode::branch(
            b_id,
            cfg(),
            Origin {
                episode: s_id,
                turn: turns[0].id,
                index: Some(0),
                kind: EdgeKind::Branch,
            },
            1,
        );
        b.append(forked.occurrence());
        let mut g = Graph::new();
        for t in turns.iter().chain([&forked]) {
            g.insert_content(t.clone());
        }
        g.insert_episode(s);
        g.insert_episode(b);
        let mut view = GraphView::new(g);
        view.set_claude(section_of(&["cc-1"], &[("cc-1", 2)]));
        open_turns(&mut view, 0);
        let row = |entry: Entry| view.full.iter().position(|r| r.entry == entry);
        let parting = row(Entry::ClaudeTurn {
            conversation: 0,
            turn: 1,
        });
        let branch = row(Entry::Node(Node::Episode(b_id))).expect("the branch is drawn");
        assert_eq!(view.parent[branch], parting, "at the parting turn");
    }

    /// **A fold the user never touched follows the default as it changes;
    /// one they set is kept** — a conversation closed by default opens when a
    /// session comes to continue it (an import made with `:graph` open
    /// elsewhere vanished into the fold: A F5), and one the user closed
    /// stays closed through a rebuild.
    #[test]
    fn an_untouched_fold_follows_its_default_and_a_set_one_is_kept() {
        let (g, root, ..) = sample();
        let mut before = GraphView::new(g.clone());
        before.set_claude(section_of(&["cc-1"], &[]));
        let conversation = |v: &GraphView| {
            v.full
                .iter()
                .position(|r| r.entry == Entry::Conversation(0))
                .unwrap()
        };
        assert!(
            !before.expanded[conversation(&before)],
            "premise: closed by default"
        );
        assert!(before.select_node(Node::Episode(root)), "premise");
        before.collapse();
        let mut held = before.held();
        // The cursor outside the conversation: its reveal would open the
        // fold whatever the rule, and test nothing.
        held.selected = Some(Key::Heading);

        // The root now continues the conversation.
        let mut g = g;
        let mut ep = g.episode(&root).unwrap().clone();
        ep.inherited = Some(shannon_core::Inherited {
            session: "cc-1".into(),
            turns: 0,
            at: Some("u0".into()),
            opened: Vec::new(),
        });
        g.insert_episode(ep);
        let mut after = GraphView::new(g);
        after.restore(section_of(&["cc-1"], &[("cc-1", 1)]), held);
        assert!(
            after.expanded[conversation(&after)],
            "opened: it holds a session now"
        );
        let session = after
            .full
            .iter()
            .position(|r| r.entry == Entry::Node(Node::Episode(root)))
            .unwrap();
        assert!(!after.expanded[session], "the fold the user set, kept");
    }

    /// Build root `[a,b,c]` + a branch off turn `b` diverging to `d`, then reveal
    /// the branch. Returns the view plus the two episode ids.
    fn branched_view() -> (GraphView, EpisodeId, EpisodeId) {
        let (a, b, c, d) = (turn("a"), turn("b"), turn("c"), turn("d"));
        let root_id = EpisodeId::from_seed(b"root");
        let branch_id = EpisodeId::from_seed(b"branch");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        root.append(c.occurrence());
        let mut branch = Episode::branch(
            branch_id,
            cfg(),
            Origin {
                episode: root_id,
                turn: b.id,
                index: Some(1),
                kind: EdgeKind::Branch,
            },
            2,
        );
        branch.append(d.occurrence()); // only the divergent turn
        let mut g = Graph::new();
        for t in [&a, &b, &c, &d] {
            g.insert_content((*t).clone());
        }
        g.insert_episode(root);
        g.insert_episode(branch);
        let mut view = GraphView::new(g);
        view.reveal_and_select_episode(branch_id);
        (view, root_id, branch_id)
    }

    #[test]
    fn git_lane_branch_forks_into_its_own_column() {
        let (view, root_id, branch_id) = branched_view();
        let rows = view.window(usize::MAX);
        let lane_str = |node_pred: &dyn Fn(&Node) -> bool| -> String {
            rows.iter()
                .find(|r| node_pred(&r.node()))
                .unwrap()
                .lane
                .iter()
                .map(|c| c.ch)
                .collect()
        };

        // The root's first turn curves off the head into the rail: `╰●`.
        assert_eq!(
            lane_str(&|n| matches!(n, Node::Turn { episode, .. } if *episode == root_id)),
            "╰●",
        );
        // The branch head physically connects: the root rail tees (`├`) into the
        // branch and keeps going (turn `c` is still below).
        assert_eq!(
            lane_str(&|n| matches!(n, Node::Episode(e) if *e == branch_id)),
            " ├●",
        );
        // Its divergent turn curves off the branch head, root rail `│` beside it.
        assert_eq!(
            lane_str(&|n| matches!(n, Node::Turn { episode, .. } if *episode == branch_id)),
            " │╰●",
        );
        // Greedy coloring: a branch never shares its parent's color.
        assert_ne!(
            view.episode_color[&root_id], view.episode_color[&branch_id],
            "branch color differs from its parent",
        );
    }

    #[test]
    fn a_child_session_head_wears_its_own_glyph() {
        // The user's ask: children must be tellable from branches in :graph.
        // Root [a]; a BRANCH and a CHILD (ToolCall) both hang off turn a — the
        // branch head stays `●`, the child head is `◆`, and an override remaps
        // it (the [tui] graph_glyphs foundation).
        let a = turn("a");
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_episode(root);
        let branch_id = EpisodeId::from_seed(b"branch");
        let child_id = EpisodeId::from_seed(b"childx");
        for (id, kind) in [
            (branch_id, EdgeKind::Branch),
            (child_id, EdgeKind::ToolCall),
        ] {
            g.insert_episode(Episode::from_segments(
                id,
                cfg(),
                vec![],
                Some(Origin {
                    episode: root_id,
                    turn: a.id,
                    index: Some(0),
                    kind,
                }),
            ));
        }

        let head_char = |view: &GraphView, id: EpisodeId| -> char {
            view.window(usize::MAX)
                .into_iter()
                .find(|r| matches!(r.node(), Node::Episode(e) if e == id))
                .unwrap()
                .lane
                .last()
                .unwrap()
                .ch
        };
        let mut view = GraphView::new(g.clone());
        view.reveal_and_select_episode(child_id);
        assert_eq!(head_char(&view, root_id), '●');
        assert_eq!(head_char(&view, branch_id), '●', "a branch stays a dot");
        assert_eq!(head_char(&view, child_id), '◆', "a child is a diamond");
        // The tree label carries the same glyph (both styles agree).
        let label_of = |view: &GraphView, id: EpisodeId| -> String {
            view.window(usize::MAX)
                .into_iter()
                .find(|r| matches!(r.node(), Node::Episode(e) if e == id))
                .unwrap()
                .label
                .to_string()
        };
        assert!(label_of(&view, child_id).starts_with('◆'));

        // Overrides: a sparse map remaps one role; junk values keep defaults.
        let overrides: BTreeMap<String, String> = [
            ("child".to_string(), "C".to_string()),
            ("merge".to_string(), "toolong".to_string()),
        ]
        .into();
        let glyphs = NodeGlyphs::resolve(GraphChars::Unicode, &overrides);
        let (sort, ids) = (GraphSort::default(), GraphIds::default());
        let mut view = GraphView::with_style(g, sort, glyphs, ids, TEST_TREE);
        view.reveal_and_select_episode(child_id);
        assert_eq!(head_char(&view, child_id), 'C', "the override wins");
        assert_eq!(
            glyphs.for_kind(Some(EdgeKind::Merge)),
            '●',
            "a multi-char override is refused; the default stays"
        );
        // The ascii base set downgrades everything coherently.
        let ascii = NodeGlyphs::base(GraphChars::Ascii);
        assert_eq!(ascii.for_kind(Some(EdgeKind::ToolCall)), 'o');
        assert_eq!(ascii.for_kind(None), '*');
        assert_eq!(ascii.running(), '>');
        assert!(
            [
                ascii.for_turn(false),
                ascii.for_turn(true),
                ascii.for_claude(false),
                ascii.for_claude(true)
            ]
            .iter()
            .all(char::is_ascii),
            "the ascii set is ascii — a turn drew `◦` there before turns had a role"
        );
    }

    /// **A summary turn wears the compaction mark** (Steven, 10-16: "a marker
    /// for the turn in the conversation where a compaction has happened") —
    /// `⊚` in the tree's label and on the `git` style's lane, where an
    /// ordinary turn has `◦` and `●`; and a role like every other, so
    /// `[tui.graph_glyphs] compacted` repaints both.
    #[test]
    fn a_summary_turn_wears_the_compaction_mark() {
        let asked = turn("asked");
        let summary = turn("the summary").with_distilled_from(vec![asked.id]);
        let id = EpisodeId::from_seed(b"compacted");
        let mut episode = Episode::root(id, cfg());
        episode.append(summary.occurrence());
        episode.append(asked.occurrence());
        let mut g = Graph::new();
        g.insert_content(asked.clone());
        g.insert_content(summary.clone());
        g.insert_episode(episode);
        let marks = |view: &GraphView| -> Vec<(char, char)> {
            view.window(usize::MAX)
                .into_iter()
                .filter(|r| matches!(r.node(), Node::Turn { .. }))
                .map(|r| (r.label.chars().next().unwrap(), r.lane.last().unwrap().ch))
                .collect()
        };
        let view = GraphView::new(g.clone());
        assert_eq!(
            marks(&view),
            [('⊚', '⊚'), ('◦', '●')],
            "the summary marked in both styles; the ordinary turn as it was"
        );
        let overrides: BTreeMap<String, String> =
            [("compacted".to_string(), "S".to_string())].into();
        let glyphs = NodeGlyphs::resolve(GraphChars::Unicode, &overrides);
        let (sort, ids) = (GraphSort::default(), GraphIds::default());
        let view = GraphView::with_style(g, sort, glyphs, ids, TEST_TREE);
        assert_eq!(marks(&view)[0], ('S', 'S'), "the user's mark, in both");
    }

    #[test]
    fn git_lane_branch_off_the_last_turn_corners_off() {
        // Root [a,b]; branch off b, the LAST turn — the root doesn't continue
        // below the fork, so it's a ╰ corner, not a ├ tee.
        let (a, b, d) = (turn("a"), turn("b"), turn("d"));
        let root_id = EpisodeId::from_seed(b"root2");
        let branch_id = EpisodeId::from_seed(b"branch2");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        let mut branch = Episode::branch(
            branch_id,
            cfg(),
            Origin {
                episode: root_id,
                turn: b.id,
                index: Some(1),
                kind: EdgeKind::Branch,
            },
            2,
        );
        branch.append(d.occurrence()); // only the divergent turn
        let mut g = Graph::new();
        for t in [&a, &b, &d] {
            g.insert_content((*t).clone());
        }
        g.insert_episode(root);
        g.insert_episode(branch);
        let mut view = GraphView::new(g);
        view.reveal_and_select_episode(branch_id);
        let head = view
            .window(usize::MAX)
            .into_iter()
            .find(|r| matches!(r.node(), Node::Episode(e) if e == branch_id))
            .unwrap();
        let glyphs: String = head.lane.iter().map(|c| c.ch).collect();
        // Root ends at the fork (b is its last turn), so the rail corners into the
        // branch with `╰` instead of teeing — nothing continues below.
        assert_eq!(
            glyphs, " ╰●",
            "the root's rail corners into the branch off its last turn"
        );
    }

    #[test]
    fn active_episode_is_flagged() {
        let (g, root_id, ..) = sample();
        let mut view = GraphView::new(g);
        view.set_active(Some(root_id));
        let win = view.window(usize::MAX);
        assert!(win[0].active, "the root episode is flagged active");
        assert!(
            win.iter().skip(1).all(|r| !r.active),
            "only the active episode is flagged"
        );
    }

    #[test]
    fn tab_hops_between_sessions_revealing_a_buried_branch() {
        // sample(): root [a,b] + a branch (child) nested under turn b. The branch
        // is collapsed out of view initially; a session hop must find and reveal it.
        let (g, root_id, child_id, ..) = sample();
        let mut view = GraphView::new(g);
        view.select_top();
        assert_eq!(view.selected_node(), Some(Node::Episode(root_id)));

        view.select_next_episode();
        assert_eq!(
            view.selected_node(),
            Some(Node::Episode(child_id)),
            "next-session jumps straight to the branch, revealing it (it was buried under turn b)"
        );

        // No session past the branch — the hop is a no-op, selection holds.
        view.select_next_episode();
        assert_eq!(view.selected_node(), Some(Node::Episode(child_id)));

        // And back.
        view.select_prev_episode();
        assert_eq!(view.selected_node(), Some(Node::Episode(root_id)));
    }

    #[test]
    fn recency_sort_orders_newest_first_with_blank_newest() {
        use std::time::Duration;
        let old_t = Turn::seal(vec![Message::user("o")], cfg(), SystemTime::UNIX_EPOCH);
        let new_t = Turn::seal(
            vec![Message::user("n")],
            cfg(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(100),
        );
        let (old_id, new_id, blank_id) = (
            EpisodeId::from_seed(b"o"),
            EpisodeId::from_seed(b"n"),
            EpisodeId::from_seed(b"b"),
        );
        let mut old_ep = Episode::root(old_id, cfg());
        old_ep.append(old_t.occurrence());
        let mut new_ep = Episode::root(new_id, cfg());
        new_ep.append(new_t.occurrence());
        let blank_ep = Episode::root(blank_id, cfg()); // no turns → newest

        let mut g = Graph::new();
        g.insert_content(old_t);
        g.insert_content(new_t);
        g.insert_episode(old_ep);
        g.insert_episode(new_ep);
        g.insert_episode(blank_ep);

        let view = GraphView::with_sort(g, GraphSort::Recency);
        let roots: Vec<EpisodeId> = view
            .full
            .iter()
            .filter_map(|r| match r.node() {
                Node::Episode(id) if r.depth == 0 => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(
            roots,
            vec![blank_id, new_id, old_id],
            "blank session first, then newest-last-turn first"
        );
    }

    #[test]
    fn empty_graph_projects_no_rows() {
        let view = GraphView::new(Graph::new());
        assert!(view.is_empty());
        assert_eq!(view.visible_len(), 0);
        assert_eq!(view.selected(), 0);
        assert!(view.selected_node().is_none());
        assert!(view.window(10).is_empty());
        assert_eq!(view.window_top(10), 0);
    }

    /// **Every configurable glyph role is accepted by the parser AND written
    /// down where a user will find it.**
    ///
    /// The roles were a hand-written list in three places — the match arms, the
    /// "unknown role" warning, and the commented block in `DEFAULT_TOML` that
    /// the user reads and copies from. Adding `held` updated one of the three,
    /// which is the ordinary outcome: a rule survives per what the compiler
    /// re-derives and rots per what a human re-types
    /// (`[[feedback_rules_rot_by_retyping]]`).
    ///
    /// Starts from `ROLES` in both directions, so it measures something: a role
    /// the parser silently ignores fails, and a role nobody documented fails.
    /// (A test that iterated `DEFAULT_TOML` and compared it to itself would be
    /// the completeness test that measures nothing —
    /// `[[feedback_mutation_check_guards]]`.) And from the TYPE: a glyph no
    /// listed role sets fails too — starting only from `ROLES`, a role left
    /// out of it went unseen, still parsed, dropped from the warning's list
    /// and the template's check (review stop 2, C 6).
    #[test]
    fn every_glyph_role_is_accepted_and_documented() {
        for role in NodeGlyphs::ROLES {
            let mut g = NodeGlyphs::default();
            assert!(
                g.set_role(role, 'X'),
                "{role} is listed but the parser drops it"
            );
            assert_ne!(
                g,
                NodeGlyphs::default(),
                "{role} is accepted but changes nothing"
            );
            assert!(
                shannon_core::config::DEFAULT_TOML.contains(&format!("#   {role}")),
                "{role} is configurable and undocumented — the template is what \
                 a user copies from"
            );
        }
        assert!(
            !NodeGlyphs::default().set_role("nonesuch", 'X'),
            "an unknown role must be refused, not silently applied"
        );

        // Destructured without `..`: a new glyph does not compile until it is
        // named here, and one no listed role sets keeps its default and fails.
        let mut all = NodeGlyphs::default();
        for role in NodeGlyphs::ROLES {
            all.set_role(role, '§');
        }
        let NodeGlyphs {
            session,
            child,
            clone,
            merge,
            running,
            held,
            turn,
            compacted,
            claude,
            continued,
        } = all;
        for (glyph, c) in [
            ("session", session),
            ("child", child),
            ("clone", clone),
            ("merge", merge),
            ("running", running),
            ("held", held),
            ("turn", turn),
            ("compacted", compacted),
            ("claude", claude),
            ("continued", continued),
        ] {
            assert_eq!(c, '§', "the {glyph} glyph is set by no listed role");
        }
    }

    /// **A glyph override is one character one cell wide, or it is said**
    /// (review stop 2, B 5; Steven, 10-16: refused with the reason — for the
    /// marks; the chat shows text as written). Wide shifted the row a cell;
    /// zero-width and control vanished; both were taken, with a log line.
    #[test]
    fn a_glyph_override_is_one_cell_or_said() {
        for ok in ["⊚", "◇", "⧓", "*", "X"] {
            assert_eq!(
                NodeGlyphs::read_override("turn", ok),
                Ok(ok.chars().next().unwrap()),
                "{ok}"
            );
        }
        for (bad, why) in [
            ("一", "takes 2 cells"),
            ("🙂", "takes 2 cells"),
            ("\u{200b}", "shows nothing"),
            ("\u{301}", "shows nothing"),
            ("\u{1b}", "shows nothing"),
            ("\t", "shows nothing"),
            ("ab", "is not one character"),
            ("", "is not one character"),
        ] {
            let said = NodeGlyphs::read_override("turn", bad).unwrap_err();
            assert!(said.contains(why), "{bad:?}: {said}");
        }
        assert!(
            NodeGlyphs::read_override("nonesuch", "x")
                .unwrap_err()
                .contains("has no role `nonesuch`")
        );
        // And the graph takes only what the rule reads: the default stays.
        let wide = BTreeMap::from([("turn".to_string(), "🙂".to_string())]);
        assert_eq!(
            NodeGlyphs::resolve(GraphChars::Unicode, &wide),
            NodeGlyphs::base(GraphChars::Unicode)
        );
    }

    /// A reading of `id` whose turns are opened by `uuids`.
    fn opened_by(id: &str, uuids: &[&str]) -> Arc<Imported> {
        Arc::new(Imported {
            id: id.into(),
            cwd: None,
            turns: uuids
                .iter()
                .map(|u| ImportedTurn {
                    prompt: format!("{id} {u}"),
                    uuid: Some(u.to_string()),
                    ..ImportedTurn::default()
                })
                .collect(),
            skipped: Default::default(),
            earlier: None,
            leaf: None,
        })
    }

    /// `cc-src`, resumed in `cc-new` — whose file copied `cc-src`'s turns
    /// `u1` and `u2` before its own `n0` and `n1` — read as `readings` says.
    fn resumed(read_src: bool, read_new: bool) -> ClaudeSection {
        let mut src = found("cc-src");
        src.resumed_in = vec!["cc-new".into()];
        let mut read = HashMap::new();
        if read_src {
            read.insert("cc-src".into(), opened_by("cc-src", &["u0", "u1", "u2"]));
        }
        if read_new {
            read.insert(
                "cc-new".into(),
                opened_by("cc-new", &["u1", "u2", "n0", "n1"]),
            );
        }
        ClaudeSection {
            found: vec![found("cc-new"), src],
            read,
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        }
    }

    /// Row `entry`'s index, if laid out.
    fn row_of(view: &GraphView, entry: Entry) -> Option<usize> {
        view.full.iter().position(|r| r.entry == entry)
    }

    /// Conversation `at`'s turns shown, as `l` shows them — what hangs from
    /// it first, if it is closed over that.
    fn open_turns(view: &mut GraphView, at: usize) {
        let row = |view: &GraphView| row_of(view, Entry::Conversation(at)).expect("laid out");
        for _ in 0..2 {
            if view.turns_shown(row(view)) {
                return;
            }
            view.reveal_and_select(row(view));
            view.expand();
        }
        assert!(view.turns_shown(row(view)), "premise: its turns shown");
    }

    /// **`/` reaches an open conversation's turns, and not a closed one's**
    /// (review stop 5, C 6: untested). A closed conversation's turns are not
    /// laid out — the boundary (Steven, 10-18) — so nothing of them is
    /// searched; `l` opens them, and then they are, the other's still not.
    #[test]
    fn slash_reaches_an_open_conversations_turns_and_not_a_closed_ones() {
        let mut view = GraphView::new(Graph::new());
        view.set_claude(section_of(&["cc-1", "cc-2"], &[("cc-1", 2), ("cc-2", 2)]));
        let words = |view: &GraphView| searched(view).join("\n");
        assert!(
            words(&view).contains("cc-2"),
            "premise: the rows are searched"
        );
        assert!(
            !words(&view).contains("cc-2 u1"),
            "closed: {}",
            words(&view)
        );
        open_turns(&mut view, 1);
        assert!(words(&view).contains("cc-2 u1"), "open: {}", words(&view));
        assert!(!words(&view).contains("cc-1 u1"), "the other still closed");
    }

    /// The entries in sight, top to bottom.
    fn in_sight(view: &GraphView) -> Vec<Entry> {
        view.visible_order
            .iter()
            .map(|&f| view.full[f].entry)
            .collect()
    }

    /// **A conversation's turns stay closed until opened, and its reading
    /// landing moves no conversation and no session** (Steven, 10-18: the
    /// section before its readings came "looks pretty nice", after "shows
    /// too much all at once") — what hangs from it is under its row before
    /// and after, a session's imported turns going to the conversation; `l`
    /// shows its turns, the session moving to the one it parted at; `h`
    /// hides them, then what hangs from it, one level each; `l` again, one
    /// level each. One with nothing hanging from it opens to its turns at
    /// once, and `h` closes it at once.
    #[test]
    fn a_conversations_turns_open_one_level_at_a_time() {
        let id = EpisodeId::from_seed(b"s");
        let mut s = Episode::root(id, cfg());
        s.inherited = Some(shannon_core::Inherited {
            session: "cc-1".into(),
            turns: 1,
            at: Some("u1".into()),
            opened: vec![Some("u1".into())],
        });
        let mut g = Graph::new();
        let t = turn("u1");
        g.insert_content(t.clone());
        s.append(t.occurrence());
        g.insert_episode(s);
        let mut view = GraphView::new(g);
        view.set_claude(section_of(&["cc-1", "cc-2"], &[("cc-2", 2)]));
        let unread = in_sight(&view);
        view.offer("cc-1", Some(opened_by("cc-1", &["u0", "u1", "u2"])), false);
        let before = in_sight(&view);
        let held = |rows: &[Entry]| -> Vec<Entry> {
            rows.iter()
                .filter(|e| !matches!(e, Entry::Node(Node::Turn { .. })))
                .copied()
                .collect()
        };
        assert_eq!(
            held(&before),
            held(&unread),
            "no conversation, no session moved"
        );
        assert_eq!(
            before.len() + 1,
            unread.len(),
            "its imported turn the conversation's now, drawn once its turns are"
        );
        let session = Entry::Node(Node::Episode(id));
        let parent = |view: &GraphView| {
            let f = row_of(view, session).unwrap();
            view.parent[f].map(|p| view.full[p].entry)
        };
        assert_eq!(parent(&view), Some(Entry::Conversation(0)));
        let turn = |turn| Entry::ClaudeTurn {
            conversation: 0,
            turn,
        };
        let on = |view: &mut GraphView, at: usize| {
            let f = row_of(view, Entry::Conversation(at)).unwrap();
            view.reveal_and_select(f);
        };

        on(&mut view, 0);
        view.expand();
        assert_eq!(parent(&view), Some(turn(1)), "at the turn it parted at");
        assert!(in_sight(&view).contains(&turn(0)) && in_sight(&view).contains(&session));
        view.collapse();
        assert_eq!(
            in_sight(&view),
            before,
            "its turns hidden, the rest as it was"
        );
        view.collapse();
        assert!(!in_sight(&view).contains(&session), "closed");
        view.expand();
        assert_eq!(in_sight(&view), before, "what hangs from it");
        view.expand();
        assert_eq!(parent(&view), Some(turn(1)), "its turns");

        let other = |turn| Entry::ClaudeTurn {
            conversation: 1,
            turn,
        };
        on(&mut view, 1);
        assert!(!in_sight(&view).contains(&other(0)), "premise: closed");
        view.expand();
        assert!(in_sight(&view).contains(&other(0)), "its turns, at once");
        view.collapse();
        let f = row_of(&view, Entry::Conversation(1)).unwrap();
        assert!(
            !in_sight(&view).contains(&other(0)) && !view.expanded[f],
            "closed, at once"
        );
    }

    /// **A resumed conversation hangs from its source's row and says where
    /// it was picked up** — the source's latest turn its file holds (Steven,
    /// 10-17: "Seeing *where* we had picked up in Shannon compared to where
    /// that conversation currently is"), named on its row, where it once hung
    /// at that turn: a lineage hangs flat (Steven, 10-19, review stop 5,
    /// B 2), with its turns open or closed. Its turns the two share are drawn
    /// once, as the source's; its source opens, so the move hides nothing;
    /// `/` finds the note too.
    #[test]
    fn a_resumed_conversation_says_where_it_was_picked_up() {
        let (g, ..) = sample();
        let mut view = GraphView::new(g);
        view.set_claude(Some(resumed(true, true)));
        let (src, new) = (1, 0); // by place in the list
        let src_row = |view: &GraphView| row_of(view, Entry::Conversation(src));
        let new_row = |view: &GraphView| row_of(view, Entry::Conversation(new)).unwrap();
        assert_eq!(view.parent[new_row(&view)], src_row(&view), "turns closed");
        open_turns(&mut view, src);
        open_turns(&mut view, new);
        let turn = |conversation, turn| Entry::ClaudeTurn { conversation, turn };
        assert_eq!(view.parent[new_row(&view)], src_row(&view), "turns open");
        assert_eq!(
            view.full[new_row(&view)].depth,
            view.full[src_row(&view).unwrap()].depth + 1
        );
        let note = view.resume_notes.get(&new).expect("its note");
        assert_eq!(note, "resumes cc-src after \"cc-src u2\"", "after u2");
        assert!(
            view.full[new_row(&view)].label.contains(note.as_str()),
            "what `/` reads"
        );
        assert_eq!(view.resume_notes.get(&src), None, "a top resumes nothing");
        for shared in [0, 1] {
            assert_eq!(row_of(&view, turn(new, shared)), None, "the source's");
        }
        assert_eq!(
            view.parent[row_of(&view, turn(new, 2)).unwrap()],
            Some(new_row(&view))
        );
        let tops: Vec<Entry> = (0..view.full.len())
            .filter(|&f| view.parent[f].is_none())
            .map(|f| view.full[f].entry)
            .filter(|e| matches!(e, Entry::Conversation(_)))
            .collect();
        assert_eq!(
            tops,
            [Entry::Conversation(src)],
            "listed once, under its source"
        );
        assert!(view.expanded[src_row(&view).unwrap()], "its source open");
    }

    /// **A lineage of any length hangs flat from its top** — each resume one
    /// level under the top, in the order they were resumed, its row saying
    /// where it was picked up — where each hung one level under the last: off
    /// the pane at 30, 84 ms a rebuild at 2,000, the stack overflowed at
    /// 8,000 (review stop 5, B 2, measured). 3,000 here, on a test thread's
    /// 2 MiB stack, which the old recursion did not survive.
    #[test]
    fn a_long_resume_chain_hangs_flat_from_its_top() {
        let n = 3_000;
        let mut found: Vec<Found> = (0..n).map(|i| found(&format!("cc-{i}"))).collect();
        for (i, f) in found.iter_mut().enumerate().take(n - 1) {
            f.resumed_in = vec![format!("cc-{}", i + 1)];
        }
        let mut view = GraphView::new(Graph::new());
        view.set_claude(Some(ClaudeSection {
            found,
            read: HashMap::new(),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        }));
        let rows: Vec<(usize, usize)> = (0..view.full.len())
            .filter_map(|f| match view.full[f].entry {
                Entry::Conversation(at) => Some((at, f)),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), n, "each once");
        let top = rows[0].1;
        assert_eq!(rows[0].0, 0, "cc-0, which nothing resumed, is the top");
        for (i, &(at, f)) in rows.iter().enumerate().skip(1) {
            assert_eq!(at, i, "in the order they were resumed");
            assert_eq!(
                (view.parent[f], view.full[f].depth),
                (Some(top), 1),
                "cc-{i}"
            );
        }
        assert_eq!(
            view.resume_notes.get(&2).map(String::as_str),
            Some("resumes cc-1"),
            "its source named, unread"
        );
    }

    /// **A lineage that fans out lists each resume before the ones resumed
    /// from it, siblings in the list's order** — `cc-a` resumed twice, the
    /// first of those once more: `b`, then `d` (from `b`), then `c`.
    #[test]
    fn a_lineage_lists_each_resume_before_its_own_in_list_order() {
        let mut list: Vec<Found> = ["cc-a", "cc-b", "cc-c", "cc-d"].map(found).into();
        list[0].resumed_in = vec!["cc-c".into(), "cc-b".into()];
        list[1].resumed_in = vec!["cc-d".into()];
        let mut view = GraphView::new(Graph::new());
        view.set_claude(Some(ClaudeSection {
            found: list,
            read: HashMap::new(),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        }));
        let top = row_of(&view, Entry::Conversation(0));
        let under: Vec<usize> = (0..view.full.len())
            .filter(|&f| view.parent[f] == top && top.is_some())
            .filter_map(|f| match view.full[f].entry {
                Entry::Conversation(at) => Some(at),
                _ => None,
            })
            .collect();
        assert_eq!(under, [1, 3, 2], "b, d from b, then c");
        let note = |at: usize| view.resume_notes.get(&at).map(String::as_str);
        assert_eq!(
            [note(1), note(3), note(2)],
            [
                Some("resumes cc-a"),
                Some("resumes cc-b"),
                Some("resumes cc-a")
            ]
        );
    }

    /// **Until both are read, its row names its source but not where it was
    /// picked up** (Steven, 10-18: no new eager reads) — the source unread,
    /// or itself — and it hangs from the source's row, as a read one does: a
    /// lineage hangs flat whatever is read (review stop 6, C 5: this test's
    /// doc still had a placed one hang at a turn).
    #[test]
    fn a_resume_not_yet_read_names_its_source_alone() {
        for (read_src, read_new) in [(false, false), (true, false), (false, true)] {
            let (g, ..) = sample();
            let mut view = GraphView::new(g);
            view.set_claude(Some(resumed(read_src, read_new)));
            open_turns(&mut view, 1);
            let new_row = row_of(&view, Entry::Conversation(0)).expect("laid out");
            assert_eq!(
                view.parent[new_row],
                row_of(&view, Entry::Conversation(1)),
                "read: {read_src}, {read_new}"
            );
            assert_eq!(
                view.resume_notes.get(&0).map(String::as_str),
                Some("resumes cc-src"),
                "read: {read_src}, {read_new}"
            );
            let src_row = row_of(&view, Entry::Conversation(1)).unwrap();
            assert!(view.full[src_row].has_children && view.expanded[src_row]);
            if read_new && !read_src {
                open_turns(&mut view, 0);
                assert!(
                    row_of(
                        &view,
                        Entry::ClaudeTurn {
                            conversation: 0,
                            turn: 0
                        }
                    )
                    .is_some(),
                    "nothing known shared: every turn its own"
                );
            }
        }
    }

    /// **A branch cut from an imported turn a resume shares with its source
    /// hangs at the source's turn** — where that turn is drawn — and from the
    /// source's row while its turns are closed (review stop 5, A 4: keyed to
    /// the resume's own turn, which the resume never draws, the branch was
    /// drawn nowhere, every test green).
    #[test]
    fn a_branch_of_a_shared_turn_hangs_at_the_sources() {
        let id = EpisodeId::from_seed(b"s");
        let mut s = Episode::root(id, cfg());
        s.inherited = Some(shannon_core::Inherited {
            session: "cc-new".into(),
            turns: 1,
            at: Some("u1".into()),
            opened: vec![Some("u1".into())],
        });
        let mut g = Graph::new();
        let t = turn("shared words");
        g.insert_content(t.clone());
        s.append(t.occurrence());
        g.insert_episode(s);
        let b = EpisodeId::from_seed(b"b");
        g.branch_prefix(id, 0, b).expect("a branch");
        let mut view = GraphView::new(g);
        view.set_claude(Some(resumed(true, true)));
        let parent_of = |view: &GraphView| {
            let f = row_of(view, Entry::Node(Node::Episode(b))).expect("the branch is drawn");
            view.parent[f].map(|p| view.full[p].entry)
        };
        assert_eq!(
            parent_of(&view),
            Some(Entry::Conversation(1)),
            "the source's row"
        );
        open_turns(&mut view, 1);
        assert_eq!(
            parent_of(&view),
            Some(Entry::ClaudeTurn {
                conversation: 1,
                turn: 1
            }),
            "the source's turn"
        );
    }

    /// **`l` opens a fold that closed over open turns** — a conversation's
    /// turns opened, then the last session going on with it deleted: its
    /// fold follows its default and closes, its turns stay open, and `l`
    /// must open the fold rather than do nothing (review stop 5, C 3).
    #[test]
    fn l_opens_a_fold_closed_over_open_turns() {
        let (view, _, _) = continuing(&["u1"], 2, 0, &[], None);
        // The tree a refresh builds once the session is gone.
        let mut fresh = GraphView::new(Graph::new());
        fresh.restore(view.claude().cloned(), view.held());
        let f = row_of(&fresh, Entry::Conversation(0)).unwrap();
        assert!(
            fresh.turns_shown(f) && !fresh.expanded[f],
            "premise: its turns open, its fold closed"
        );
        fresh.reveal_and_select(f);
        fresh.expand();
        assert!(fresh.expanded[f], "`l` opened it");
    }

    /// **A fold set inside a conversation's turns outlives hiding them**
    /// (review stop 5, A 5): close the turn a session hangs at (open by
    /// default), `h` the conversation, `l` it again — the turn stays closed.
    /// `held` carried only the folds of rows laid out, and hidden turns are
    /// not laid out.
    #[test]
    fn a_turns_fold_outlives_hiding_the_turns() {
        let (mut view, _, _) = continuing(&["u1"], 2, 0, &[], None);
        let parted = Entry::ClaudeTurn {
            conversation: 0,
            turn: 1,
        };
        let f = row_of(&view, parted).unwrap();
        assert!(view.expanded[f], "premise: open, a session at it");
        view.reveal_and_select(f);
        view.collapse();
        for step in [GraphView::collapse, GraphView::expand] {
            let conversation = row_of(&view, Entry::Conversation(0)).unwrap();
            view.reveal_and_select(conversation);
            step(&mut view);
        }
        assert!(view.turns_shown(row_of(&view, Entry::Conversation(0)).unwrap()));
        let f = row_of(&view, parted).unwrap();
        assert!(!view.expanded[f], "still closed");
    }

    /// **A session going on from a resume at a turn it shares hangs at the
    /// source's turn** — where that turn is drawn — and one at its own turn,
    /// there.
    #[test]
    fn a_session_at_a_shared_turn_hangs_at_the_sources() {
        let at = |seed: &[u8], uuid: &str| {
            let id = EpisodeId::from_seed(seed);
            let mut s = Episode::root(id, cfg());
            s.inherited = Some(shannon_core::Inherited {
                session: "cc-new".into(),
                turns: 1,
                at: Some(uuid.into()),
                opened: vec![Some(uuid.into())],
            });
            (id, s)
        };
        let (shared_id, shared) = at(b"shared", "u1");
        let (own_id, own) = at(b"own", "n0");
        let mut g = Graph::new();
        for (mut s, words) in [(shared, "shared words"), (own, "own words")] {
            let t = turn(words);
            g.insert_content(t.clone());
            s.append(t.occurrence());
            g.insert_episode(s);
        }
        let mut view = GraphView::new(g);
        view.set_claude(Some(resumed(true, true)));
        open_turns(&mut view, 1);
        open_turns(&mut view, 0);
        let session = |id| row_of(&view, Entry::Node(Node::Episode(id))).unwrap();
        let turn = |conversation, turn| Entry::ClaudeTurn { conversation, turn };
        assert_eq!(view.parent[session(shared_id)], row_of(&view, turn(1, 1)));
        assert_eq!(view.parent[session(own_id)], row_of(&view, turn(0, 2)));
    }

    /// **Resumes that name each other draw each once** — a crafted file can
    /// say anything: the first listed to name one is its source, and a name
    /// that would close a cycle is passed over.
    #[test]
    fn resumes_that_name_each_other_draw_each_once() {
        let (g, ..) = sample();
        let mut section = resumed(false, false);
        section.found[0].resumed_in = vec!["cc-src".into(), "cc-new".into()];
        let mut view = GraphView::new(g);
        view.set_claude(Some(section));
        for at in [0, 1] {
            let rows = view
                .full
                .iter()
                .filter(|r| r.entry == Entry::Conversation(at))
                .count();
            assert_eq!(rows, 1, "conversation {at}");
        }
        // Three round, each naming the next: the union-find's one lookup
        // refuses the name that closes it, as the walk up did.
        let mut ring: Vec<Found> = ["cc-a", "cc-b", "cc-c"].map(found).into();
        for (i, next) in ["cc-b", "cc-c", "cc-a"].iter().enumerate() {
            ring[i].resumed_in = vec![next.to_string()];
        }
        let (g, ..) = sample();
        let mut view = GraphView::new(g);
        view.set_claude(Some(ClaudeSection {
            found: ring,
            read: HashMap::new(),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed: Scrubber::defaults(),
        }));
        for at in 0..3 {
            let rows = (view.full.iter())
                .filter(|r| r.entry == Entry::Conversation(at))
                .count();
            assert_eq!(rows, 1, "ring {at}");
        }
    }

    /// **A lineage is listed by its newest conversation** — the resume is
    /// under its source now, and the newest conversation is the one most
    /// likely sought.
    #[test]
    fn a_lineage_is_listed_by_its_newest() {
        let (g, ..) = sample();
        let mut section = resumed(false, false);
        let now = SystemTime::now();
        let ago = |secs| Some(now - std::time::Duration::from_secs(secs));
        section.found[0].modified = ago(10); // cc-new
        section.found[1].modified = ago(300); // cc-src
        let mut between = found("cc-between");
        between.modified = ago(100);
        section.found.insert(1, between);
        let mut view = GraphView::new(g);
        view.set_claude(Some(section));
        let tops: Vec<Entry> = (0..view.full.len())
            .filter(|&f| view.parent[f].is_none())
            .map(|f| view.full[f].entry)
            .filter(|e| matches!(e, Entry::Conversation(_)))
            .collect();
        assert_eq!(tops, [Entry::Conversation(2), Entry::Conversation(1)]);
    }

    /// **A resume is drawn hanging from its source, saying where it was
    /// picked up** — from the source's row whether its turns are open or
    /// closed (a lineage hangs flat) — in either style, its mark after what
    /// it hangs from, as a session's head dot is, never flush with the
    /// conversations (review stop 5, C 2: only the closed layout was drawn).
    #[test]
    fn a_resume_is_drawn_under_its_source() {
        let styles = [
            shannon_core::GraphStyle::Git,
            shannon_core::GraphStyle::Tree,
        ];
        for (style, open) in styles.into_iter().flat_map(|s| [(s, false), (s, true)]) {
            let (g, ..) = sample();
            let mut view = GraphView::new(g);
            view.set_claude(Some(resumed(true, true)));
            if open {
                open_turns(&mut view, 1);
            }
            let new = row_of(&view, Entry::Conversation(0)).unwrap();
            assert_eq!(
                view.parent[new],
                row_of(&view, Entry::Conversation(1)),
                "premise: from its source's row"
            );
            let tui = shannon_core::config::TuiConfig {
                graph_style: style,
                ..Default::default()
            };
            let rows = crate::view::graph_row_texts(
                &view,
                crate::view::GraphLook::of(&tui),
                &crate::theme::Theme::default(),
            );
            let src = rows
                .iter()
                .find(|r| r.contains("cc-src"))
                .expect("the source");
            let new = rows
                .iter()
                .find(|r| r.contains("cc-new"))
                .expect("the resume");
            let lead = |r: &str| r.split("cc-").next().unwrap().to_string();
            let (src_lead, new_lead) = (lead(src), lead(new));
            assert!(
                new_lead.chars().count() > src_lead.chars().count()
                    && new_lead.chars().any(|c| "│├╰└─".contains(c)),
                "{style:?}, open {open}: {src:?} / {new:?}"
            );
            assert!(
                new.contains("resumes cc-src after \"cc-src u2\""),
                "{style:?}, open {open}: where, drawn: {new:?}"
            );
        }
    }
}
