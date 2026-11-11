//! **The buffers** — what every window on one must agree on, the content
//! half of the buffer/window split (`docs/DesignDocs/buffers.md`; V5.4 (1),
//! 10-25, a module of its own; the window half is `app/window.rs`).
//!
//! **One list** ([`Buffers`], V5.4 (3)): a session's buffer, a page's and a
//! tree's, in number order, each one [`Buffer`] — Emacs's buffer list, each
//! buffer with its mode. A session's buffer is here whole: its
//! conversation, the transcript rendered from it, what the store holds of
//! it, what it has spent, and what you have been told of it. Where a window
//! is in it is the window's.

use shannon_core::{Conversation, EpisodeId, TurnCost, Usage};

use super::{Landing, Opens, news};
use crate::graph_view::GraphBuffer;
use crate::output::Output;
use crate::page::PageBuffer;
use crate::wm::BufferId;
use crate::workspace::{PageKind, PaneKind};

/// **One buffer** — what a window can be on, and what `:b N` names
/// (V5.4 (3)). A kind added is a variant whose every `match` the compiler
/// lists, which is the point: "what is buffer N" was asked of three stores
/// in each reader's own order (the V5.4 design, (a)).
pub(crate) enum Buffer {
    /// A session and its transcript — listed by `:ls`. Boxed: at 664 bytes
    /// against a page's 192 it set every element's size, and the list is
    /// scanned by number.
    Session(Box<SessionBuffer>),
    /// A page: the manual, a report, the notice history, a listing —
    /// unlisted, named by `:ls!`.
    Page(PageBuffer),
    /// A session tree — unlisted, as a page is.
    Tree(GraphBuffer),
}

impl Buffer {
    /// Its number's handle, minted by [`Buffers::add`].
    pub(crate) fn id(&self) -> BufferId {
        match self {
            Buffer::Session(session) => session.id,
            Buffer::Page(page) => page.id(),
            Buffer::Tree(tree) => tree.id(),
        }
    }

    /// **Listed by `:ls`**, as vim's `'buflisted'` — a session, and a page
    /// you edit (`PageKind::modifiable`: the tutor, your copy of the
    /// lessons, as vim lists a file and not its help); a page you read and a
    /// tree are `:ls!`'s (the `u` flag). Every session, never one hidden
    /// (Steven, 11-09: "Show the tutor in `:ls`"; 11-11, of a session kept
    /// out of it: "That is a secret buffer").
    pub(crate) fn listed(&self) -> bool {
        match self {
            Buffer::Session(_) => true,
            Buffer::Page(page) => matches!(page.kind(), PaneKind::Page(kind) if kind.modifiable()),
            Buffer::Tree(_) => false,
        }
    }

    /// **What a message calls an unlisted buffer** — the manual, a report
    /// ([`PageBuffer::described`]), the session tree; `None` for a session,
    /// which a message names by its label instead (`App::session_label`).
    pub(crate) fn described(&self) -> Option<String> {
        match self {
            Buffer::Session(_) => None,
            Buffer::Page(page) => Some(page.described()),
            Buffer::Tree(tree) => Some(tree.name().to_string()),
        }
    }
}

/// **The buffers open this run** — one list in number order (V5.4 (3)),
/// minted and pushed in one step ([`add`](Self::add)), never shrunk: a
/// buffer outlives its windows, as vim's hidden buffers do, and its number
/// is never reused.
///
/// No `len`: a count of every buffer would still compile where a count of
/// SESSIONS was meant (`delete_flow`'s "every buffer but theirs" — the V5.4
/// design review, L7); [`sessions`](Self::sessions)`().count()` says which.
pub(crate) struct Buffers {
    list: Vec<Buffer>,
    /// The next number to mint — private, so [`add`](Self::add) is the one
    /// place a [`BufferId`] is minted (it was `App::mint_buffer`'s comment),
    /// and [`add_as`](Self::add_as) the one that spends [`held`](Self::held).
    next: u64,
    /// **The number a launch holds for the page it opens on** — `shannon
    /// tutor`'s lessons, `b1`, as vimtutor's file is buffer 1 (Steven, 11-09:
    /// `:ls` then `:b1` opened an empty chat; the tutor was `b2`). The
    /// session [`new`](Self::new) makes is numbered after it, and the page
    /// takes it when it is made ([`add_as`](Self::add_as)) — after launch has
    /// read the rules its pictures are drawn by.
    held: Option<(PaneKind, BufferId)>,
}

impl Buffers {
    /// The list a launch starts with, and the number the first window and
    /// the foreground are born on: a session's, `b1`, as vim's first buffer
    /// is the file it was started on — or, for `shannon tutor`, NO session
    /// and `b1` held for the lessons ([`Opens`]), which the first window and
    /// the prompt name until a session is called. A session made there and
    /// kept out of `:ls` was a buffer you could not see (Steven, 11-11: "That
    /// is a secret buffer").
    pub(super) fn new(
        opens: Opens,
        first: impl FnOnce(BufferId) -> SessionBuffer,
    ) -> (Self, BufferId) {
        match opens {
            Opens::Session => {
                let mut buffers = Buffers {
                    list: Vec::new(),
                    next: 0,
                    held: None,
                };
                let id = buffers.add(|id| Buffer::Session(Box::new(first(id))));
                (buffers, id)
            }
            Opens::Tutor => {
                let page = BufferId(0);
                let buffers = Buffers {
                    list: Vec::new(),
                    next: 1,
                    held: Some((PaneKind::Page(PageKind::Tutor), page)),
                };
                (buffers, page)
            }
        }
    }

    /// [`add`](Self::add), for a buffer of `kind` — numbered with the number
    /// [`held`](Self::held) for it, if one is, in its place by number.
    pub(super) fn add_as(
        &mut self,
        kind: PaneKind,
        make: impl FnOnce(BufferId) -> Buffer,
    ) -> BufferId {
        let Some((_, id)) = self.held.take_if(|(held, _)| *held == kind) else {
            return self.add(make);
        };
        let buffer = make(id);
        debug_assert_eq!(
            buffer.id(),
            id,
            "a buffer carries the number it was made with"
        );
        let at = self.list.partition_point(|b| b.id() < id);
        self.list.insert(at, buffer);
        id
    }

    /// **A new buffer, numbered** — `make` given the number minted for it,
    /// and the buffer pushed, in one step: a page's, a tree's, a session's.
    pub(super) fn add(&mut self, make: impl FnOnce(BufferId) -> Buffer) -> BufferId {
        let id = BufferId(self.next);
        self.next += 1;
        let buffer = make(id);
        debug_assert_eq!(
            buffer.id(),
            id,
            "a buffer carries the number it was made with"
        );
        self.list.push(buffer);
        id
    }

    /// The tests' seam: the next buffer numbered `next + 1` (`b{next+1}`),
    /// for one whose number must read as something else. Forward only: the
    /// list is in number order by construction, and a number minted twice
    /// would break that and `get` with it (review stop 10, B 1 and C 10).
    #[cfg(test)]
    pub(super) fn skip_to(&mut self, next: u64) {
        debug_assert!(next >= self.next, "the counter goes forward");
        self.next = next;
    }

    /// The buffer with this number's handle, of any kind — by binary search:
    /// the list is in number order by construction ([`add`](Self::add) mints
    /// a rising number and pushes; nothing is removed; `skip_to` only goes
    /// forward). It was a linear scan, asked for each session by each fact
    /// an alert rings on every frame, so a frame cost alerts × sessions ×
    /// buffers ever opened: 139 µs a frame at 500 (review stop 10, B 1,
    /// measured in release).
    pub(crate) fn get(&self, id: BufferId) -> Option<&Buffer> {
        self.at(id).map(|at| &self.list[at])
    }

    /// [`get`](Self::get), to change.
    fn get_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.at(id).map(|at| &mut self.list[at])
    }

    /// Where `id` is in the list, found by its number.
    fn at(&self, id: BufferId) -> Option<usize> {
        (self.list)
            .binary_search_by_key(&id.0, |b| {
                #[cfg(test)]
                LOOKUP_STEPS.with(|n| n.set(n.get() + 1));
                b.id().0
            })
            .ok()
    }

    /// Every buffer, in number order — `:ls!`'s order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Buffer> {
        self.list.iter()
    }

    /// The session buffer `id`, if it is one.
    pub(crate) fn session(&self, id: BufferId) -> Option<&SessionBuffer> {
        match self.get(id)? {
            Buffer::Session(session) => Some(session),
            Buffer::Page(_) | Buffer::Tree(_) => None,
        }
    }

    /// [`session`](Self::session), to change.
    pub(crate) fn session_mut(&mut self, id: BufferId) -> Option<&mut SessionBuffer> {
        match self.get_mut(id)? {
            Buffer::Session(session) => Some(session),
            Buffer::Page(_) | Buffer::Tree(_) => None,
        }
    }

    /// Every session buffer, in number order — `:ls`'s.
    pub(crate) fn sessions(&self) -> impl Iterator<Item = &SessionBuffer> {
        self.list.iter().filter_map(|b| match b {
            Buffer::Session(session) => Some(&**session),
            Buffer::Page(_) | Buffer::Tree(_) => None,
        })
    }

    /// [`sessions`](Self::sessions), to change.
    pub(crate) fn sessions_mut(&mut self) -> impl Iterator<Item = &mut SessionBuffer> {
        self.list.iter_mut().filter_map(|b| match b {
            Buffer::Session(session) => Some(&mut **session),
            Buffer::Page(_) | Buffer::Tree(_) => None,
        })
    }

    /// The page buffer `id`, if it is one.
    pub(crate) fn page(&self, id: BufferId) -> Option<&PageBuffer> {
        match self.get(id)? {
            Buffer::Page(page) => Some(page),
            Buffer::Session(_) | Buffer::Tree(_) => None,
        }
    }

    /// [`page`](Self::page), to change.
    pub(crate) fn page_mut(&mut self, id: BufferId) -> Option<&mut PageBuffer> {
        match self.get_mut(id)? {
            Buffer::Page(page) => Some(page),
            Buffer::Session(_) | Buffer::Tree(_) => None,
        }
    }

    /// Every page buffer, in number order.
    pub(crate) fn pages(&self) -> impl Iterator<Item = &PageBuffer> {
        self.list.iter().filter_map(|b| match b {
            Buffer::Page(page) => Some(page),
            Buffer::Session(_) | Buffer::Tree(_) => None,
        })
    }

    /// [`pages`](Self::pages), to change.
    pub(crate) fn pages_mut(&mut self) -> impl Iterator<Item = &mut PageBuffer> {
        self.list.iter_mut().filter_map(|b| match b {
            Buffer::Page(page) => Some(page),
            Buffer::Session(_) | Buffer::Tree(_) => None,
        })
    }

    /// The tree buffer `id`, if it is one.
    pub(crate) fn tree(&self, id: BufferId) -> Option<&GraphBuffer> {
        match self.get(id)? {
            Buffer::Tree(tree) => Some(tree),
            Buffer::Session(_) | Buffer::Page(_) => None,
        }
    }

    /// [`tree`](Self::tree), to change.
    pub(crate) fn tree_mut(&mut self, id: BufferId) -> Option<&mut GraphBuffer> {
        match self.get_mut(id)? {
            Buffer::Tree(tree) => Some(tree),
            Buffer::Session(_) | Buffer::Page(_) => None,
        }
    }

    /// Every tree buffer, oldest first.
    pub(crate) fn trees(&self) -> impl Iterator<Item = &GraphBuffer> {
        self.list.iter().filter_map(|b| match b {
            Buffer::Tree(tree) => Some(tree),
            Buffer::Session(_) | Buffer::Page(_) => None,
        })
    }

    /// [`trees`](Self::trees), to change.
    pub(crate) fn trees_mut(&mut self) -> impl Iterator<Item = &mut GraphBuffer> {
        self.list.iter_mut().filter_map(|b| match b {
            Buffer::Tree(tree) => Some(tree),
            Buffer::Session(_) | Buffer::Page(_) => None,
        })
    }
}

/// **What the store holds of a session** — a buffer's persist cursor, typed.
///
/// It was one `usize`, and one number carried two facts: whether the session
/// is registered at all, and how many of its own turns are written — with `0`
/// standing for "never written". Each reading of that proxy shipped a bug
/// (Reviewers A and D, round 18, and three earlier sites that had already
/// stopped trusting it): a child's header is on disk at spawn with nothing
/// owned, so `0` sent it to be registered twice, and that registration's
/// rollback deletes a log; a turn delete passed the derived length, so a
/// branch's cursor sat past its end and later turns never reached disk; and a
/// lazy clone, merge or prune owns nothing, so `:write` compared `0 <= 0`,
/// said "already on disk", and `:wq` quit without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnDisk {
    /// Not known yet; the next persist asks the store
    /// ([`Store::episode_len`](shannon_core::Store::episode_len)). What a launch
    /// starts with, where a fresh session and a resumed one look alike.
    Unknown,
    /// Not on disk: the next persist registers it — header, the slices it
    /// shares, its own turns. Only a session minted this run is known absent.
    Absent,
    /// Registered, with this many of its OWN turns written (shared history is
    /// its owner's to count).
    Own(usize),
}

/// One **buffer**: a session and the transcript rendered from it.
///
/// The content half of the buffer/window split (`docs/DesignDocs/buffers.md`).
/// Everything here is what any number of windows must agree on — the turns and
/// the text derived from them. Where you are *looking* (cursor, scroll,
/// selection) is window state and deliberately not here; putting it here is
/// what made two transcript panes unable to disagree.
pub(crate) struct SessionBuffer {
    /// This buffer's handle — what a pane stores to say "I show *that* session".
    ///
    /// An **occurrence**, not content: deliberately not the `EpisodeId`, because
    /// two buffers may sit on one episode and closing one must not close the
    /// other. A plain counter, like `PaneId`.
    pub(crate) id: BufferId,
    /// The conversation transcript + turn lifecycle. The domain model
    /// (role-alternation invariant, commit-on-complete / drop-on-error) lives
    /// in `shannon-core`, not here; `App` drives it in tandem with the view but
    /// does not own the transcript shape.
    pub(crate) conversation: Conversation,
    /// The transcript as rendered rows — wrapping, styling, and the block
    /// structure derived from `conversation`.
    pub(crate) output: Output,
    /// What the store holds of **this** session — the cursor for incremental
    /// persistence ([`OnDisk`]).
    ///
    /// Per-buffer and not per-app, which is a correctness property rather than
    /// tidiness: leaving another session open and coming back must not resume
    /// with a cursor describing a different transcript, or the append either
    /// duplicates turns or skips them. Counted, NOT keyed by `TurnId` — ids are
    /// content-addressed, so two identical turns share one and id-dedup would
    /// silently drop the repeat.
    pub(crate) on_disk: OnDisk,
    /// What this session has spent, and how full its context is.
    ///
    /// Per-buffer, which is a correctness property rather than tidiness:
    /// switching away and back must not re-tally from zero or inherit another
    /// session's bill.
    pub(crate) billing: Billing,
    /// **Why this session's last write failed**, until one succeeds — set and
    /// cleared by the one persist ([`App::persist_buffer`](super::App::persist_buffer)).
    /// `:q` names it with its reason: it said "`:wa` writes it" of a session
    /// whose write had just failed, and `:wa` would fail the same way (review
    /// stop 2, B 6; Steven: "say what buffer failed, to help a user debug").
    pub(crate) write_failed: Option<String>,
    /// **Your marks in it** (`m{a-z}`), each a place in the conversation
    /// ([`Anchor`](crate::output::Anchor)), not a line: every window on the
    /// session has them, as vim's lowercase marks are the buffer's
    /// (`navigation.md` §7). One whose block is gone is not set. Saved with
    /// the session (`App::save_marks`), and opened with it
    /// (`marks::restored`).
    pub(crate) marks: std::collections::BTreeMap<char, crate::output::Anchor>,
    /// **The agent's marks in it** (its `mark` tool), a name space of their
    /// own — `` `a `` beside your `'a` — each with the note it gave
    /// ([`AgentMark`](super::marks::AgentMark)). Saved and opened as yours.
    pub(crate) agent_marks: std::collections::BTreeMap<char, super::marks::AgentMark>,
    /// **Marks it saved that this transcript does not draw** — a session
    /// reopened by a build that draws it differently. Listed by `:marks`,
    /// and saved again as they were: never dropped unsaid.
    pub(crate) unplaced_marks: Vec<shannon_core::Mark>,
    /// What `:ls` shows of it, counted once per change to its messages
    /// ([`prompts`](Self::prompts)).
    listed: std::cell::RefCell<Option<Listed>>,
    /// **What you have been told of it** — what a pane last drew of its
    /// transcript, and what its alerts held at the last frame
    /// ([`news::Told`]): `unshown` and the rings read it.
    pub(super) told: news::Told,
    /// **How it was made** ([`Made`]) — what a session never written still
    /// has when nothing of its own was given it
    /// ([`has_its_own`](Self::has_its_own)).
    made: Made,
}

/// **What a buffer was given when it was made** — its label (name,
/// description) and marks, and whether it was made as a LOOK: a checkout or
/// a clone, which shares a session's turns as they are
/// ([`SessionBuffer::made_as_a_look`]). Not one by default: every other
/// session a door makes — an edit's prune, a merge, a pick — is a
/// composition, which is what you did.
#[derive(Debug, Clone, Default)]
struct Made {
    label: shannon_core::Label,
    marks: Vec<shannon_core::Mark>,
    look: bool,
}

/// **What a buffer holds**: a session's conversation, its transcript
/// rendered, what the store has of it, and what it has spent — what
/// [`App::hold`](super::App::hold) gives a buffer whole.
pub(super) struct Contents {
    pub(super) conversation: Conversation,
    pub(super) output: Output,
    pub(super) on_disk: OnDisk,
    pub(super) billing: Billing,
}

#[cfg(test)]
thread_local! {
    /// How often [`SessionBuffer::prompts`] scanned rather than read its
    /// memo, on this thread: deleting the memo's hit path left every answer
    /// right and every test green, with the O(history) frame back (full
    /// review 10-21, D 2, mutated).
    static PROMPT_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The scans so far on this thread ([`PROMPT_SCANS`]).
#[cfg(test)]
pub(super) fn prompt_scans() -> usize {
    PROMPT_SCANS.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// How many buffers a lookup by number looked at ([`Buffers::get`]),
    /// on this thread — a count, not a time, so a test holds the bound on
    /// any machine (review stop 10, B 1: the scan was linear, and only a
    /// measurement showed it).
    static LOOKUP_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The lookup steps so far on this thread ([`LOOKUP_STEPS`]).
#[cfg(test)]
pub(super) fn lookup_steps() -> usize {
    LOOKUP_STEPS.with(std::cell::Cell::get)
}

/// [`SessionBuffer::prompts`]' memo: the key it was counted at, the prompts,
/// and the first one's first line.
#[derive(Debug, Clone)]
struct Listed {
    key: (EpisodeId, usize),
    turns: usize,
    first: String,
}

impl SessionBuffer {
    /// A session buffer holding `contents`, with nothing counted yet and
    /// told of as `landing` says ([`news::Told::of`]) — every field named,
    /// so a buffer given a new session ([`App::hold`](super::App::hold))
    /// keeps nothing of the one before.
    pub(super) fn new(id: BufferId, contents: Contents, landing: Landing) -> Self {
        let Contents {
            conversation,
            output,
            on_disk,
            billing,
        } = contents;
        // The marks it saved, placed in the transcript just drawn.
        let marks = super::marks::restored(&conversation, &output);
        let episode = conversation.episode();
        let made = Made {
            label: episode.label.clone(),
            marks: episode.marks.clone().unwrap_or_default(),
            look: false,
        };
        SessionBuffer {
            id,
            conversation,
            output,
            on_disk,
            billing,
            write_failed: None,
            marks: marks.yours,
            agent_marks: marks.agents,
            unplaced_marks: marks.unplaced,
            listed: std::cell::RefCell::new(None),
            told: news::Told::of(landing),
            made,
        }
    }

    /// **Whether it holds something of its own** — a turn, a name, a
    /// description or a mark given it here, or what it is made of: a
    /// session not made as a look ([`made_as_a_look`](Self::made_as_a_look))
    /// is a composition — an edit's prune or reorder, a merge, a pick —
    /// which is what you did. A checkout or a clone only looked at holds
    /// none: it shares every turn, its marks the ones it came with. What a
    /// quit loses of a session never written
    /// ([`App::unwritten`](super::App::unwritten); S21-Q12, review stop 15
    /// F H3: `:qa` refused over a look, and its `:wa` wrote a branch for
    /// good).
    ///
    /// **A look is how it was made, not its kind** (the close verifier,
    /// HIGH 1): read from its origin's kind, every `Branch` was one — an
    /// `e` edit that only deleted a turn is a `Branch` owning nothing, and
    /// `:qa` let it go without a word. And the name alone was compared at
    /// first: `:description` on a look then quit unasked.
    pub(super) fn has_its_own(&self) -> bool {
        let episode = self.conversation.episode();
        let marks = episode.marks.as_deref().unwrap_or_default();
        episode.own_len() > 0
            || !self.made.look
            || episode.label != self.made.label
            || marks != self.made.marks.as_slice()
    }

    /// **This buffer was made as a look** — a checkout of a turn, or a
    /// clone (`:clone`, `:fork`, `:vfork`): it shares a session's turns as
    /// they are, so until something of its own is given it a quit loses
    /// nothing ([`has_its_own`](Self::has_its_own)). Said by the three
    /// doors that make one: `App::checkout_turn`, `App::clone_episode`
    /// (`:clone`) and `App::fork_command` (`:fork`, `:vfork`).
    pub(super) fn made_as_a_look(&mut self) {
        self.made.look = true;
    }

    /// **How many prompts it holds, and its first's first line** — what `:ls`
    /// shows of it, counted once per change rather than every frame `:ls` is
    /// open: the scan was O(history) a frame, 293 µs for 16 sessions of 3000
    /// turns against a 250 µs frame (full review 10-21, B 3, measured).
    /// Keyed by the episode and the message count: messages append, and a
    /// rewrite that changes the prompts makes a new session or changes the
    /// count (one composed over the last reply changes neither, nor its
    /// prompts). The episode half guards a conversation swapped in place —
    /// no production path does that (`App::hold` rebuilds the buffer whole,
    /// and a `Conversation` never reassigns its episode), so deleting it
    /// passes every test; kept as the cheap guard for the class, as one test
    /// fixture does swap one (review stop 10, C 9).
    pub(super) fn prompts(&self) -> (usize, String) {
        let messages = self.conversation.messages();
        let key = (self.conversation.episode().id, messages.len());
        if let Some(listed) = self.listed.borrow().as_ref()
            && listed.key == key
        {
            return (listed.turns, listed.first.clone());
        }
        #[cfg(test)]
        PROMPT_SCANS.with(|n| n.set(n.get() + 1));
        let turns = messages.iter().filter(|m| m.is_prompt()).count();
        let first = messages
            .iter()
            .find(|m| m.is_prompt())
            // Its controls dropped, as the transcript drops them from the
            // same text (`output::sanitize_line`): an imported transcript's
            // prompt can carry escapes, which went into `:ls`'s page raw —
            // its column widths counted them, and `/` matched them (review
            // stop 10, B 5). Once per change, with the memo.
            .map(|m| crate::output::sanitize_line(m.content.lines().next().unwrap_or_default()))
            .unwrap_or_else(|| "(empty)".into());
        *self.listed.borrow_mut() = Some(Listed {
            key,
            turns,
            first: first.clone(),
        });
        (turns, first)
    }

    /// **Where a window with no place of its own in it starts** — at its
    /// end, following what arrives: a transcript is live, and its end is
    /// where the news lands. Shannon's rule, not vim's, and a default
    /// (Steven, 10-26: "keep it at the end for now… We may want to change
    /// this one or allow markers to specify where"): vim's `:b` puts a
    /// window at the buffer's `'"`, as a page and the tree are opened here
    /// (`PageBuffer::returned_to`, `GraphBuffer::left_at`).
    ///
    /// **The one answer, and the buffer's**, so a mark the session kept —
    /// its `'"`, or one set to say where it opens — would be read here
    /// alone. Asked through `App::entry_of` (`rg 'entry_of\('`, 10-26) by
    /// every door a window comes to a session with no place in it by: a
    /// window born on it beside another (`App::session_host`, a split from
    /// a viewer, `:vfork`'s window on the session split from), one shown it
    /// by `:b N`, `:open`, `<CR>`, `:new` (`Window::enter`), a landing put
    /// beneath a view (`Window::repoint_beneath`), a place let go when the
    /// session's rows are rewritten (`App::forget_cursors_on`, after they
    /// are: a delete's fresh session), and the launch's window, once the app
    /// is built on the session a resume loaded (`App::new`; it said "born on
    /// an empty buffer", false under `resume = last` — review of the 10-26
    /// answers, M3).
    ///
    /// **Its cursor at the end too**, on the last line, riding what arrives
    /// as a live window's does (`App::sync_chat_if_dirty`): it was parked
    /// on the first line beside a view of the last, a place nobody put it —
    /// dragged into sight by the first frame, and read by `:graph` as the
    /// turn you were reading when the transcript fit its window (review stop
    /// 15, A H3 / C H3). Where a checkout lands (`App::at_its_end`).
    pub(super) fn entry(&self) -> super::window::Place {
        // Counted, not copied: `lines()` clones every line (the close
        // verifier at review stop 15, LOW 8).
        let last = self.output.line_count().saturating_sub(1);
        super::window::Place {
            cursor: (last, 0),
            ..super::window::Place::at_the_end()
        }
    }
}

/// **What `:q` would lose of the open sessions**
/// ([`App::unwritten`](super::App::unwritten)).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Unwritten {
    /// Each the store does not hold whole, by its label — with why its last
    /// write failed, if it did.
    pub(crate) owed: Vec<(String, Option<String>)>,
    /// Each the store could not say about, with why.
    pub(crate) unsure: Vec<(String, String)>,
}

/// A session's two token numbers, which are always written together.
///
/// They answer different questions and only one of them was being kept. `total`
/// is **what you have paid** — every completed turn summed, growing forever.
/// `context` is **how full the window is** — what the *next* request will
/// carry, which is one prompt, not the sum of all of them. Steven read
/// `↑13226` off the status line and asked what percent of the context was
/// filled; the honest answer was under 4%, and nothing on screen could say so
/// because the cumulative figure was the only one that existed.
///
/// One struct rather than a second field beside the first, because they are
/// written at six sites and a field the compiler does not force you to set is a
/// field five of those sites will forget
/// (`[[feedback_relocating_state_fans_out]]`). Everything goes through
/// [`record`](Self::record).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Billing {
    /// Every completed (or failed) turn's usage, summed.
    pub(crate) total: Usage,
    /// Tokens the next request on this session will carry: the last turn's
    /// whole prompt plus the reply it produced, since a conversation re-sends
    /// both. Zero for a session that has not answered yet.
    pub(crate) context: u32,
    /// **`context` is an estimate, not a measurement** — the last turn's
    /// recorded window measured another history (`shannon_core::window`), so
    /// it was estimated from this one's text. Shown as `~`; the next answer
    /// measures it again.
    pub(crate) estimated: bool,
}

impl Billing {
    /// Fold a completed turn in.
    ///
    /// **One [`TurnCost`], because the two intervals are one fact.**
    ///
    /// The bill accumulates; the window is an assignment, because it holds one
    /// conversation rather than the history of every request made against it.
    ///
    /// This took a single `Usage` and both callers passed the sum, under a doc
    /// that stated the rule correctly over a value that broke it — a two-round
    /// turn read 2.24× too full and the error grew with every round. Splitting
    /// it into two positional `Usage` arguments fixed the callers and left the
    /// *shape* that caused it: `render_session` then passed the same value
    /// twice, one commit later, and compiled. `TurnCost` is that shape closed
    /// (`[[feedback_fix_neighbourhood]]`).
    pub(crate) fn record(&mut self, cost: TurnCost) {
        self.charge(cost);
        self.context = cost
            .final_round
            .prompt_tokens()
            .saturating_add(cost.final_round.output_tokens);
        self.estimated = false;
    }

    /// Add a request's cost to the bill, leaving the window as it was — for
    /// work that is not one of the session's turns (`App::bill`).
    pub(crate) fn charge(&mut self, cost: TurnCost) {
        self.total.add_turn(cost.whole);
    }

    /// The window, ESTIMATED (`tokens`) — for a history no request measured.
    pub(crate) fn estimate(&mut self, tokens: u32) {
        self.context = tokens;
        self.estimated = true;
    }

    /// How full `budget` is, as a percentage, or `None` before the first
    /// answer. Saturates at 100 rather than reporting an impossible number.
    ///
    /// **The budget is a parameter now.** It was `models::CONTEXT_WINDOW`, one
    /// constant asserting the whole family shipped 200k — so an Opus session
    /// 300k tokens in read 100% full when it was 30% used, and the
    /// context-exhausted advice that followed would have had the user squash a
    /// conversation with 700k of room left. It is also the seam the user's own
    /// `[turn] max_context_tokens` needs: "how full am I" is a question about
    /// the budget you are working inside, which is not always the model's.
    pub(crate) fn context_percent(&self, budget: u32) -> Option<u32> {
        (self.context > 0 && budget > 0)
            .then(|| ((self.context as u64 * 100) / u64::from(budget)).min(100) as u32)
    }
}
