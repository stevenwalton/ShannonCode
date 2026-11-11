//! **Marks in a transcript** — yours, `m{a-z}` set where the cursor is and
//! `'{a-z}` gone back to (N3); the agent's, set by its `mark` tool and gone
//! to with `` `{a-z} `` (N4) — `navigation.md` §7.
//!
//! A mark is the session's: a place in the conversation
//! ([`Anchor`](crate::output::Anchor)) kept on its buffer
//! ([`SessionBuffer::marks`], [`SessionBuffer::agent_marks`]), so every
//! window on the session has it, as vim's lowercase marks are the buffer's.
//! A window's copy of the transcript is told where each one is beside its
//! lines (`App::sync_chat_in`, the copy's one writer), so `'a`, `v'a`, `y'a`
//! and `` `a `` all land through the copy's one motion
//! (`prompt::Buffer::apply_motion`).
//!
//! Transcripts first (`marks.md` Q1, Steven 10-26): in any other pane `m`,
//! `'` and `` ` `` are refused, said.

use super::{App, GenerationId, SessionBuffer, Window};
use crate::keymap::{CursorMotion, EditAction, KeyOutcome, OpTarget, Whose};
use crate::output::{Anchor, BlockRole};
use crate::workspace::PaneKind;

/// Why a mark can be neither set nor gone to in any pane but a transcript.
const ONLY_IN_A_TRANSCRIPT: &str = "marks are a transcript's, for now — `m`, `'` and ` in one";

/// The longest `at.quote` the agent's `mark` is searched for, in chars — its
/// description asks for "a short phrase"; past this, refused unsearched.
const MAX_QUOTE: usize = 200;

/// The longest `note` the agent's `mark` keeps, in chars — "a few words".
const MAX_NOTE: usize = 200;

/// **One of the agent's marks** (its `mark` tool): where, what it said the
/// place is, and when it was made — the oldest is the one a 27th takes the
/// letter of (NQ5).
#[derive(Debug, Clone)]
pub(crate) struct AgentMark {
    pub(crate) anchor: Anchor,
    /// What it said the place is — `:marks` shows it, as the call's record
    /// in the transcript does.
    pub(crate) note: String,
    /// When it was made, in this session's order — what a 27th reuses the
    /// oldest by, and what its saved list keeps.
    pub(crate) made: u64,
}

impl App {
    /// `m{name}`: set mark `name` where the focused transcript's cursor is,
    /// and say so — vim says nothing, but nothing on screen shows a mark
    /// until the gutter does (N6).
    pub(super) fn set_mark(&mut self, name: char) {
        let pane = self.workspace.focused();
        let buffer = (self.kind_of(pane) == Some(PaneKind::Output))
            .then(|| self.window_of(pane).and_then(Window::session))
            .flatten();
        let Some(buffer) = buffer else {
            return self.refuse(ONLY_IN_A_TRANSCRIPT);
        };
        let (line, col) = self.pane_chat(pane).cursor();
        let Some(output) = self.session_for_pane(pane).map(|b| &b.output) else {
            return self.refuse(ONLY_IN_A_TRANSCRIPT);
        };
        let Some(anchor) = output.anchor_at(line, col) else {
            return self.refuse("nothing here to mark yet");
        };
        let turn = (output.spans())
            .find(|span| span.id == anchor.block)
            .and_then(|span| span.turn);
        // A place no reopened transcript draws is set for this run, said.
        let unkept = output.position_of(anchor).err();
        let Some(session) = self.session_of_mut(buffer) else {
            return self.refuse(ONLY_IN_A_TRANSCRIPT);
        };
        session.marks.insert(name, anchor);
        // This window's copy at once: the next key may be `'a`.
        self.sync_chat_in(pane);
        if let Err(why) = self.save_marks(buffer) {
            return self.refuse(format!(
                "mark `{name}` set, but not saved with the session — {why}"
            ));
        }
        let set = match turn {
            Some(turn) => format!("mark `{name}` — turn {}", turn + 1),
            None => format!("mark `{name}` set"),
        };
        self.notify(match unkept {
            Some(unkept) => format!("{set}, not saved: {}", unkept.why()),
            None => set,
        });
    }

    /// **The agent marks a place** (the `mark` tool, `ToolDef::mark`) —
    /// tracked under the work that asked, so the answer reaches that
    /// session's round wherever it is; recorded as any call is; answered at
    /// once. No gate: it runs nothing.
    ///
    /// Here is the call's own record, so `` `a `` lands on the line that says
    /// the agent marked it, under what it wrote first; `at` is the first line
    /// of a turn that holds a quoted phrase
    /// ([`find_quote`](crate::output::Output::find_quote)). Shannon names it
    /// — the next free letter, or the oldest's past 26 (NQ5) — and the
    /// result says which, so
    /// the reply names a mark that exists. A refusal says why, to the model
    /// and on the record.
    pub(crate) fn answer_mark_call(
        &mut self,
        owner: GenerationId,
        call: &str,
        input: &serde_json::Value,
    ) {
        let buffer = self.track_tool_call(call, Some(owner));
        let placed = self
            .session_of(buffer)
            .ok_or_else(|| "no session to mark".to_string())
            .and_then(|session| agent_mark_at(session, input, &self.aliases));
        let text = match &placed {
            Ok((name, at, _)) => {
                let place = match (at, self.session_of(buffer)) {
                    (Some(anchor), Some(session)) => format!(
                        "at \"{}\"{}",
                        input["at"]["quote"].as_str().unwrap_or_default(),
                        found_in(&session.output, *anchor)
                    ),
                    _ => "here".to_string(),
                };
                format!("mark `{name}` set {place}.")
            }
            Err(why) => why.clone(),
        };
        let refused = placed.is_err();
        // As a reopened turn records it too (`own_tool_verb`).
        let verb = super::own_tool_verb(shannon_core::MARK_TOOL, refused).unwrap_or("marked");
        let shape = self.record_shape();
        let record = crate::tool_record::Record::call(verb, shannon_core::MARK_TOOL, input, shape);
        self.record_tool_activity(buffer, call, record, &text);
        if let Ok((name, at, note)) = placed
            && let Some(session) = self.session_of_mut(buffer)
        {
            // Here is the record just written: the last tool block.
            let here = || {
                (session.output.spans())
                    .filter(|span| span.role == BlockRole::Tool)
                    .last()
                    .map(|span| Anchor {
                        block: span.id,
                        line: 0,
                        col: 0,
                    })
            };
            if let Some(anchor) = at.or_else(here) {
                let made = (session.agent_marks.values())
                    .map(|mark| mark.made + 1)
                    .max()
                    .unwrap_or(0);
                let mark = AgentMark { anchor, note, made };
                session.agent_marks.insert(name, mark);
            }
            if let Err(why) = self.save_marks(buffer) {
                let said =
                    format!("the agent's mark `{name}` is not saved with the session — {why}");
                self.notify_for(buffer, said);
            }
        }
        self.finish_tool_call(call, text, refused);
    }

    /// **Why a motion to a mark cannot go**, or `None` when it can (or the
    /// outcome goes to no mark) — asked before any outcome is applied
    /// (`apply_visible_refusal`), as vim's E20 answers `'a` with no `a`.
    /// Read from the session, as the copy's table is derived from it.
    pub(super) fn mark_refusal(&self, outcome: &KeyOutcome) -> Option<String> {
        let (whose, name) = match outcome {
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Mark(whose, name)),
                ..
            }
            | KeyOutcome::Operator {
                target:
                    OpTarget::Motion(CursorMotion::Mark(whose, name))
                    | OpTarget::LineMotion(CursorMotion::Mark(whose, name)),
                ..
            } => (*whose, *name),
            _ => return None,
        };
        let pane = self.workspace.focused();
        if self.kind_of(pane) != Some(PaneKind::Output) {
            return Some(ONLY_IN_A_TRANSCRIPT.into());
        }
        let Some(session) = self.session_for_pane(pane) else {
            return Some(ONLY_IN_A_TRANSCRIPT.into());
        };
        let anchor = match whose {
            Whose::Yours => session.marks.get(&name).copied(),
            Whose::Agents => session.agent_marks.get(&name).map(|mark| mark.anchor),
        };
        let set = anchor.is_some_and(|a| session.output.locate(a).is_some());
        (!set).then(|| match whose {
            Whose::Yours => format!("mark `{name}` is not set"),
            Whose::Agents => format!("the agent has set no mark `{name}`"),
        })
    }

    /// **Where each of `session`'s marks is, as lines of its transcript** —
    /// yours and the agent's — what a window's copy is told beside its
    /// lines. A mark whose block is gone is left out: not set. One walk of
    /// the blocks for all of them ([`placed`]).
    pub(super) fn marks_as_lines(session: &SessionBuffer) -> Vec<crate::prompt::MarkAt> {
        (placed(session).into_iter())
            .map(|mark| ((mark.whose, mark.name), (mark.found.line, mark.found.col)))
            .collect()
    }

    /// **`:marks`' page** — a table per session (Steven's draft, 10-28: "humans
    /// need whitespace… draw the eye's attention to what they care about"):
    /// the session in `ui.secret`, a header row, then one row per mark —
    /// mark, turn, line, column, the WORD it stands on, and the line's
    /// context — yours, a soft rule, the agent's, each in turn order. The
    /// mark in one style for both owners ([`MARK_STYLE`]), the word bold, the
    /// coordinates plain, the context faint and **cut to `width`, never
    /// wrapped** ("if we have space, might as well use it"); the agent's
    /// context begins with its note. A blank row between sessions. A mark
    /// whose block is gone is not listed: not set. `<CR>` on a mark goes to
    /// it ([`go_to_mark`](Self::go_to_mark)). Derived every frame it is
    /// shown, so one walk of a session's blocks for all its marks.
    ///
    /// **Every line cut to `width` by cells** (`PageLine::clip`), the word
    /// column measured and padded by cells: only the context was cut, by
    /// chars, so the fixed columns wrapped a row in two below ~49 columns,
    /// and a CJK line drew 131 cells in 80 (review stop 14, B F7 / C 7).
    pub(crate) fn marks_lines(&self, width: usize) -> crate::page::Lines {
        use crate::page::Row;
        use crate::paint::{PageLine, Run};
        use crate::style::{CellStyle, Ink};
        use crate::wrap::{clip, str_cells};
        let line = |runs: Vec<Run>| {
            PageLine {
                runs,
                ..PageLine::default()
            }
            .clip(width)
        };
        let (plain, faint) = (CellStyle::ink(Ink::Text), CellStyle::ink(Ink::TextFaint));
        let mut out = crate::page::Lines::default();
        for session in self.buffers.sessions() {
            let mut marks = placed(session);
            let unread = session.conversation.episode().unread_marks.len();
            if marks.is_empty() && session.unplaced_marks.is_empty() && unread == 0 {
                continue;
            }
            marks.sort_by_key(|mark| (mark.whose, mark.found.line, mark.found.col));
            if !out.lines().is_empty() {
                out.push(None, PageLine::default());
            }
            let handle = session.conversation.episode().handle();
            let heading = format!("Session b{} {handle}", session.id.number());
            out.push(
                None,
                line(vec![Run::role(heading, CellStyle::ink(Ink::Secret).bold())]),
            );
            // The word column as wide as its widest word, within reason.
            let words: Vec<String> = (marks.iter())
                .map(|m| word_at(m.found.text, m.found.col))
                .collect();
            let wide = (words.iter().map(|w| str_cells(w)).max().unwrap_or(4)).clamp(4, 24);
            let header = format!(
                "mark  turn  {:>5}  {:>4}  {:<wide$}  context",
                "line", "col", "word"
            );
            out.push(None, line(vec![Run::role(header, faint)]));
            let mut group = None;
            for (mark, word) in marks.into_iter().zip(words) {
                if group.is_some_and(|g| g != mark.whose) {
                    out.push(
                        None,
                        line(vec![Run::role("─".repeat(width.min(80)), faint)]),
                    );
                }
                group = Some(mark.whose);
                let key = if mark.whose == Whose::Yours {
                    '\''
                } else {
                    '`'
                };
                let turn = mark
                    .found
                    .turn
                    .map_or(String::new(), |t| (t + 1).to_string());
                let coords = format!(
                    "  {turn:<4}  {:>5}  {:>4}  ",
                    mark.found.line + 1,
                    mark.found.col
                );
                let word = pad(&clip(&word, wide), wide);
                let used = 4 + str_cells(&coords) + wide + 2;
                let rest = mark.found.text.trim();
                let context = match mark.whose {
                    Whose::Yours => rest.to_string(),
                    Whose::Agents => format!("{} · {rest}", mark.note),
                };
                // As wide as the header's `mark`, so the columns line up.
                let mark_cell = format!("{:<4}", format!("{key}{}", mark.name));
                let mut runs = vec![
                    Run::role(mark_cell, MARK_STYLE),
                    Run::role(coords, plain),
                    Run::role(word, CellStyle::ink(Ink::Operand).bold()),
                ];
                if width > used + 4 {
                    runs.push(Run::role(
                        format!("  {}", clip(&context, width - used)),
                        faint,
                    ));
                }
                let row = Row::Mark {
                    buffer: session.id,
                    whose: mark.whose,
                    name: mark.name,
                };
                out.push(Some(row), line(runs));
            }
            // Saved, and not drawn by this transcript: said, never dropped.
            if !session.unplaced_marks.is_empty() {
                let names: Vec<String> = (session.unplaced_marks.iter())
                    .map(|m| {
                        let key = if m.whose == Whose::Yours { '\'' } else { '`' };
                        // `turn` is read from disk: no `+ 1` can overflow
                        // it (review stop 14, B F2).
                        format!("{key}{} (turn {})", m.name, m.turn.saturating_add(1))
                    })
                    .collect();
                let said = format!(
                    "saved, but not found in this transcript: {}",
                    names.join(", ")
                );
                out.push(None, line(vec![Run::role(said, faint)]));
            }
            // Saved in a shape this build does not read: kept as they were,
            // and said (review stop 14, B F4 — they made the whole list go).
            if unread > 0 {
                let said =
                    format!("saved, but not read by this build: {unread}, kept as they were");
                out.push(None, line(vec![Run::role(said, faint)]));
            }
        }
        if out.lines().is_empty() {
            out.text(
                "no marks — `m{a-z}` sets one in a transcript ('a goes back), and the agent sets \
                 its own with its `mark` tool (`a goes there)",
            );
        }
        out
    }

    /// **`<CR>` on `:marks`' row: that session shown here, as `:b N` shows it,
    /// and the cursor on the mark** — through the one motion `'a` and `` `a ``
    /// take, so the window follows the cursor as after any jump. Said when
    /// it cannot: the mark gone since the page was drawn, or the session
    /// shown somewhere else than the pane you stand in.
    pub(super) fn go_to_mark(&mut self, buffer: crate::app::BufferId, whose: Whose, name: char) {
        self.show_buffer(buffer);
        let pane = self.workspace.focused();
        let here = self.kind_of(pane) == Some(PaneKind::Output)
            && self.window_of(pane).and_then(Window::session) == Some(buffer);
        if !here {
            let key = match whose {
                Whose::Yours => '\'',
                Whose::Agents => '`',
            };
            return self.refuse(format!(
                "b{} is shown in another pane — `{key}{name}` there goes to the mark",
                buffer.number()
            ));
        }
        self.sync_chat_in(pane);
        let outcome = KeyOutcome::Edit {
            action: EditAction::Move(CursorMotion::Mark(whose, name)),
            count: 1,
        };
        if let Some(why) = self.mark_refusal(&outcome) {
            return self.refuse(why);
        }
        let area = self
            .frame
            .map(|(w, h)| ratatui::layout::Rect::new(0, 0, w, h));
        let size = area.map_or((0, 0), |area| self.pane_content_size(pane, area));
        self.apply_edit_outcome(EditAction::Move(CursorMotion::Mark(whose, name)), 1, size);
    }
}

/// **A session's marks, as a buffer holds them** — yours, the agent's, and
/// any it saved that this transcript does not draw ([`restored`]).
pub(super) struct Restored {
    pub(super) yours: std::collections::BTreeMap<char, Anchor>,
    pub(super) agents: std::collections::BTreeMap<char, AgentMark>,
    pub(super) unplaced: Vec<shannon_core::Mark>,
}

/// **The marks `conversation` saved, placed in `output`** — what a buffer
/// opens with (`SessionBuffer::new`, the door every buffer is made by:
/// a launch's resume, `:open`, an import), so marks outlive the run
/// (Steven, 10-28: "We should store marks along with the conversation").
/// The agent's in the order they were made, which their saved list keeps.
/// One whose place this transcript does not draw is kept aside, unplaced —
/// listed by `:marks`, and saved again as it was, never dropped unsaid.
pub(super) fn restored(
    conversation: &shannon_core::Conversation,
    output: &crate::output::Output,
) -> Restored {
    let mut out = Restored {
        yours: Default::default(),
        agents: Default::default(),
        unplaced: Vec::new(),
    };
    let saved = conversation.episode().marks.as_deref().unwrap_or_default();
    for (made, mark) in saved.iter().enumerate() {
        let position = BlockRole::from_name(&mark.block).map(|role| crate::output::Position {
            turn: mark.turn,
            role,
            nth: mark.nth,
            line: mark.line,
            col: mark.col,
        });
        let Some(anchor) = position.and_then(|p| output.anchor_of(p)) else {
            out.unplaced.push(mark.clone());
            continue;
        };
        match mark.whose {
            Whose::Yours => {
                out.yours.insert(mark.name, anchor);
            }
            Whose::Agents => {
                let note = mark.note.clone();
                let made = made as u64;
                out.agents
                    .insert(mark.name, AgentMark { anchor, note, made });
            }
        }
    }
    out
}

/// **`session`'s marks as it keeps them** — yours by name, the agent's in
/// the order they were made, then those it holds unplaced (not drawn by
/// this transcript; a mark set since under the same name replaces one).
fn kept(session: &SessionBuffer) -> Vec<shannon_core::Mark> {
    let output = &session.output;
    let mark = |whose, name, anchor: Anchor, note: &str| {
        let p = output.position_of(anchor).ok()?;
        Some(shannon_core::Mark {
            name,
            whose,
            turn: p.turn,
            block: p.role.name().to_string(),
            nth: p.nth,
            line: p.line,
            col: p.col,
            note: note.to_string(),
        })
    };
    let yours = (session.marks.iter()).filter_map(|(&n, &a)| mark(Whose::Yours, n, a, ""));
    let mut agents: Vec<_> = session.agent_marks.iter().collect();
    agents.sort_by_key(|(_, m)| m.made);
    let agents =
        (agents.into_iter()).filter_map(|(&n, m)| mark(Whose::Agents, n, m.anchor, &m.note));
    let mut out: Vec<shannon_core::Mark> = yours.chain(agents).collect();
    let held = |m: &shannon_core::Mark| out.iter().any(|o| (o.whose, o.name) == (m.whose, m.name));
    let unplaced: Vec<_> = session
        .unplaced_marks
        .iter()
        .filter(|m| !held(m))
        .cloned()
        .collect();
    out.extend(unplaced);
    out
}

impl App {
    /// **Keep `buffer`'s marks with its session** — in its header, re-put
    /// now if it is on disk (`write_preferences`, the door a rename and
    /// `:model` take), and written with its first header if not. `Err` says
    /// why it could not be, for the caller to say.
    pub(super) fn save_marks(&mut self, buffer: crate::app::BufferId) -> Result<(), String> {
        let Some(session) = self.session_of(buffer) else {
            return Err("this session is gone".into());
        };
        let marks = kept(session);
        if let Some(session) = self.session_of_mut(buffer) {
            session.conversation.set_marks(marks);
        }
        self.write_preferences(buffer)
    }

    /// **`buffer`'s marks kept again if what they mark has moved** — asked
    /// at the end of a turn or a job (`resolve`), when blocks leave a turn:
    /// dropped with it or taken out of every turn
    /// ([`abandon_turn`](crate::output::Output::abandon_turn)), or an empty
    /// open block popped (`end_turn`). And when one joins the count a kept
    /// place makes: a call's running line, which no place counts, closes as
    /// a record that does (`Output::push_tool_record`), so a mark saved
    /// while a sibling call ran names one record short until this keeps it
    /// again at the turn's end (the close verifier at review stop 15, LOW
    /// 5). Kept only at the next save, a mark
    /// in a turn you stopped stayed in the header under that turn's index,
    /// and reopened on the NEXT turn, which took it (review stop 14, A 3).
    /// Nothing is written while what is kept is what was. A mark that
    /// leaves what is kept is said, with why: live it may still go there.
    pub(super) fn keep_moved_marks(&mut self, buffer: crate::app::BufferId) {
        let Some(session) = self.session_of(buffer) else {
            return;
        };
        let now = kept(session);
        let saved = (session.conversation.episode().marks.as_deref()).unwrap_or_default();
        if saved == now.as_slice() {
            return;
        }
        let left: Vec<String> = (saved.iter())
            .filter(|m| !now.iter().any(|n| (n.whose, n.name) == (m.whose, m.name)))
            .map(|m| {
                let anchor = match m.whose {
                    Whose::Yours => session.marks.get(&m.name).copied(),
                    Whose::Agents => session.agent_marks.get(&m.name).map(|mark| mark.anchor),
                };
                let why = anchor
                    .and_then(|anchor| session.output.position_of(anchor).err())
                    .unwrap_or(crate::output::Unkept::Gone);
                let whose = match m.whose {
                    Whose::Yours => "",
                    Whose::Agents => "the agent's ",
                };
                format!("{whose}mark `{}`: {}", m.name, why.why())
            })
            .collect();
        match self.save_marks(buffer) {
            Err(why) => {
                self.notify_for(buffer, format!("marks not saved with the session — {why}"))
            }
            Ok(()) if !left.is_empty() => self.notify_for(
                buffer,
                format!("no longer saved with the session — {}", left.join("; ")),
            ),
            Ok(()) => {}
        }
    }
}

/// **Where each turn of a session goes in one made from it** — its index
/// there, or `None` for a turn not kept as it was ([`App::marks_carried`]).
pub(crate) type TurnMap<'a> = &'a dyn Fn(usize) -> Option<usize>;

/// **The marks a session made from another starts with** — the clone
/// principle (Steven, 10-30, S16-Q2: "like cloning a person… share all the
/// same history and memories *up to the point of cloning*. But at that
/// point in time they diverge"): `from`'s marks on the turns the new one
/// keeps, at their turns there, as its own; from there each session's are
/// its own. What `from` saved unread goes too, as it was.
pub(crate) struct Carried {
    pub(crate) marks: Vec<shannon_core::Mark>,
    pub(crate) unread: Vec<serde_json::Value>,
    /// The marks left behind — their turn is not kept as it was — named
    /// as a notice names one ("mark `a`", "the agent's mark `b`").
    pub(crate) left: Vec<String>,
    /// The marks on `from`'s turn still running — no maker has that turn,
    /// stored only once it ends; carried, a mark there stood past the new
    /// session's end, where its next turn took the index (the close's
    /// verifier, V 4).
    pub(crate) running: Vec<String>,
}

impl Carried {
    /// **What a maker says of the marks it did not carry**, once it has
    /// landed (said before, a landing that then refused had spoken of a
    /// session that never was — V 8): those `left`, with `why` when the
    /// maker leaves them for a reason worth saying (a checkout's later turns
    /// are not missed), and those on a turn still running.
    pub(crate) fn said(&self, into: &str, why: Option<&str>) -> Option<String> {
        let mut parts = Vec::new();
        if let (Some(why), false) = (why, self.left.is_empty()) {
            parts.push(format!("{}: {why}", self.left.join(", ")));
        }
        if !self.running.is_empty() {
            parts.push(format!(
                "{}: their turn is still running",
                self.running.join(", ")
            ));
        }
        (!parts.is_empty()).then(|| format!("not carried into {into} — {}", parts.join("; ")))
    }
    /// **Onto `episode`**, minted just now: its marks, unread and all.
    /// Nothing carried leaves it with none, its header the bytes it was.
    pub(crate) fn onto(self, episode: &mut shannon_core::Episode) {
        if !self.marks.is_empty() || !self.unread.is_empty() {
            episode.marks = Some(self.marks);
            episode.unread_marks = self.unread;
        }
    }
}

impl App {
    /// **What a session made from `from` carries of its marks** ([`Carried`]),
    /// each mark's turn put through `map` — `None` where the new session
    /// does not keep that turn as it was. Read as `from` keeps them now
    /// (`kept`, its buffer's), else as it saved them. Read BEFORE the new
    /// session lands: one that replaces `from` (`:continue`) rebuilds its
    /// buffer.
    pub(crate) fn marks_carried(&self, from: shannon_core::EpisodeId, map: TurnMap<'_>) -> Carried {
        // The turn still running, if `from` has one: the transcript numbers
        // its turns as the store does, the running one last.
        let (marks, unread, running) =
            match (self.buffers.sessions()).find(|b| b.conversation.episode().id == from) {
                Some(session) => (
                    kept(session),
                    session.conversation.episode().unread_marks.clone(),
                    (session.conversation.turn_in_progress())
                        .then(|| session.output.last_turn())
                        .flatten(),
                ),
                None => match self.store.load_episode(&from) {
                    Ok(loaded) => (
                        loaded.episode.marks.unwrap_or_default(),
                        loaded.episode.unread_marks,
                        None,
                    ),
                    Err(_) => (Vec::new(), Vec::new(), None),
                },
            };
        let mut carried = Carried {
            marks: Vec::new(),
            unread,
            left: Vec::new(),
            running: Vec::new(),
        };
        for mut mark in marks {
            let named = match mark.whose {
                Whose::Yours => format!("mark `{}`", mark.name),
                Whose::Agents => format!("the agent's mark `{}`", mark.name),
            };
            if running.is_some_and(|running| mark.turn >= running) {
                carried.running.push(named);
                continue;
            }
            match map(mark.turn) {
                Some(turn) => {
                    mark.turn = turn;
                    carried.marks.push(mark);
                }
                None => carried.left.push(named),
            }
        }
        carried
    }
}

/// **How every mark is drawn in `:marks`** — one style, yours and the
/// agent's alike, in `ui.tool`, bold (Steven, 10-30: "All marks should
/// share the same color", then "I still wanted the marks colored"): the
/// key (`'` or `` ` ``) and the soft rule between the groups already tell
/// them apart.
const MARK_STYLE: crate::style::CellStyle =
    crate::style::CellStyle::ink(crate::style::Ink::Tool).bold();

/// `text` padded with blanks to `cells` cells — by cell, as it was cut
/// (`wrap::clip`), so a wide character's column lines up.
fn pad(text: &str, cells: usize) -> String {
    let blanks = cells.saturating_sub(crate::wrap::str_cells(text));
    format!("{text}{}", " ".repeat(blanks))
}

/// **The WORD a mark stands on** — the run of non-blanks at its column, or
/// the next one when it stands on a blank (vim's WORD): what the eye looks
/// for in `:marks`.
fn word_at(text: &str, col: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = (col..chars.len()).find(|&i| !chars[i].is_whitespace());
    let Some(start) = start else {
        return String::new();
    };
    let back = (0..start)
        .rev()
        .take_while(|&i| !chars[i].is_whitespace())
        .count();
    let from = start - back;
    chars[from..]
        .iter()
        .take_while(|c| !c.is_whitespace())
        .collect()
}

/// **`text`, drawn, as the model was sent it** — every held value masked as
/// its handle, in the form it was sent AND the form the transcript draws it
/// in: `sanitize_line` turns a tab to a space and drops a `\r`, so a value
/// holding either was drawn as text no exact match found, and "found" or
/// "not" over it told the model about the value again — B F1's oracle and
/// E F2's leak, for a pasted value with a tab or a CRLF key lent by
/// `[secrets] env` (the close's verifier, V 2). Longest first, as masking
/// is. `view::row_secrets` and `Copied::touched` share the gap (docketed).
fn as_sent(aliases: &shannon_core::Aliases, text: &str) -> String {
    let mut out = aliases.mask_secrets(text).0;
    let mut drawn: Vec<(String, &str)> = (aliases.secrets())
        .filter_map(|a| {
            let value = a.shown_to_user();
            let form = crate::output::sanitize_lines(value).join("\n");
            let differs = !form.is_empty() && form != value;
            differs.then_some((form, a.handle.as_str()))
        })
        .collect();
    drawn.sort_by_key(|(form, _)| std::cmp::Reverse(form.len()));
    for (form, handle) in drawn {
        out = out.replace(&form, handle);
    }
    out
}

/// **Where a quote landed, told to the model** (review stop 14, A 6): the
/// kind of line and its turn, in `turns_ago`'s terms — ", in your reply 2
/// turns ago" — so a mark on a line the model did not mean can be noticed
/// and set again. Nothing, for a place in no turn.
fn found_in(output: &crate::output::Output, anchor: Anchor) -> String {
    let span = output.spans().find(|span| span.id == anchor.block);
    let (Some(span), Some(last)) = (span, output.last_turn()) else {
        return String::new();
    };
    let Some(turn) = span.turn else {
        return String::new();
    };
    let line = match span.role {
        BlockRole::User => "the user's message",
        BlockRole::Assistant => "your reply",
        BlockRole::Tool => "a tool's record",
        BlockRole::Rule => "the frame",
        BlockRole::Thinking => "your thinking",
    };
    let when = match last.saturating_sub(turn) {
        0 => "this turn".to_string(),
        1 => "1 turn ago".to_string(),
        ago => format!("{ago} turns ago"),
    };
    format!(", in {line} {when}")
}

/// **A mark, found** — whose, its name, where it is, and the agent's note.
struct Placed<'a> {
    whose: Whose,
    name: char,
    found: crate::output::Found<'a>,
    note: &'a str,
}

/// **Every mark of `session` that is set, found in one walk** — yours and
/// the agent's ([`Output::find_each`](crate::output::Output::find_each)).
fn placed(session: &SessionBuffer) -> Vec<Placed<'_>> {
    let yours = (session.marks.iter()).map(|(&name, &anchor)| (Whose::Yours, name, anchor, ""));
    let agents = (session.agent_marks.iter())
        .map(|(&name, mark)| (Whose::Agents, name, mark.anchor, mark.note.as_str()));
    let marks: Vec<_> = yours.chain(agents).collect();
    let anchors: Vec<Anchor> = marks.iter().map(|&(_, _, anchor, _)| anchor).collect();
    let found = session.output.find_each(&anchors);
    (marks.into_iter().zip(found))
        .filter_map(|((whose, name, _, note), found)| {
            Some(Placed {
                whose,
                name,
                found: found?,
                note,
            })
        })
        .collect()
}

/// **Where the agent's `mark` call puts its mark, and its name** — the name
/// the next free letter (the oldest's past 26), the place `None` for here
/// (the call's own record, written after this), else the quoted line; and
/// the note. `Err` says why not, in words for the model.
///
/// A quote is searched in the transcript **as the model was sent it**,
/// `aliases`' secrets masked: drawn, a held value stands where the model
/// wrote its handle, and found-or-not over that text was an oracle on the
/// value (review stop 14, B F1).
fn agent_mark_at(
    session: &SessionBuffer,
    input: &serde_json::Value,
    aliases: &shannon_core::Aliases,
) -> Result<(char, Option<Anchor>, String), String> {
    let note = input["note"].as_str().unwrap_or_default().trim();
    if note.is_empty() {
        return Err("a mark needs a `note`: what the place is, in a few words".into());
    }
    // Every save writes every note again (the header is appended whole),
    // and `:marks` draws it: a few words, said so (review stop 14, B F6).
    let long = note.chars().count();
    if long > MAX_NOTE {
        return Err(format!(
            "the `note` is {long} characters — say what the place is in a few words, \
             {MAX_NOTE} at most"
        ));
    }
    let at = match input.get("at").filter(|at| !at.is_null()) {
        None => None,
        Some(at) => {
            let quote = at["quote"].as_str().unwrap_or_default();
            if quote.trim().is_empty() {
                return Err("`at` needs a `quote`: a short phrase copied from one line".into());
            }
            // Before any search: its cost is the transcript's lines times
            // the quote's length, on the event loop, and the refusal below
            // echoes it (review stop 14, B F5).
            let long = quote.chars().count();
            if long > MAX_QUOTE {
                return Err(format!(
                    "`at.quote` is {long} characters — quote a short phrase, {MAX_QUOTE} at most"
                ));
            }
            let output = &session.output;
            // Present but not a whole number is refused, not read as "any
            // turn": the model asked for one, and would be told it was set
            // (review stop 14, B F9).
            let ago = match at.get("turns_ago").filter(|ago| !ago.is_null()) {
                None => None,
                Some(ago) => Some(
                    ago.as_u64()
                        .ok_or("`turns_ago` is a whole number, 0 or more — 0 is this turn")?,
                ),
            };
            let turn = match ago {
                None => None,
                Some(ago) => {
                    let last = output.last_turn().unwrap_or(0);
                    let ago = usize::try_from(ago).unwrap_or(usize::MAX);
                    Some(last.checked_sub(ago).ok_or_else(|| {
                        format!(
                            "`turns_ago` {ago} is before the first turn: there are {}",
                            last + 1
                        )
                    })?)
                }
            };
            let as_sent = |text: &str| as_sent(aliases, text);
            let found = output.find_quote(quote, turn, as_sent).ok_or_else(|| {
                let place = turn.map_or("in this conversation", |_| "in that turn");
                format!(
                    "no line {place} holds {quote:?} — quote a short phrase exactly as one \
                     line has it"
                )
            })?;
            Some(found)
        }
    };
    // A letter is free, or all 26 are held and one of them is the oldest:
    // the `'a'` is never reached.
    let marks = &session.agent_marks;
    let name = ('a'..='z')
        .find(|name| !marks.contains_key(name))
        .or_else(|| (marks.iter()).min_by_key(|(_, m)| m.made).map(|(&n, _)| n))
        .unwrap_or('a');
    Ok((name, at, note.to_string()))
}
