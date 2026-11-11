//! **A window's place, carried into a session made from the one it was
//! reading** (S21-Q4; Steven, 11-01: "Carry over whole place by
//! position") — its cursor, the line at its top and its folds, each found
//! again by position (`output::KeptView`) through the maker's turn map, as
//! the session's marks are (`App::marks_carried`, the clone principle).
//! What the new session does not keep, the window takes from its entry.
//!
//! Two shapes, one per kind of landing: a session that lands OVER the one
//! it was made from (a compaction, a squash: `Lands::Over`) keeps every
//! window on it where it was ([`App::views_on`] before the landing,
//! [`App::put_views`] after it); one entered in a window that was reading
//! its source (`:fork`, `:clone`, a checkout, an edit) starts there
//! ([`App::carry_into`]). And within one session, across a change that
//! moves its lines but keeps its blocks — a record closing a call in place
//! — every window kept by its blocks ([`App::anchors_on`]).

use ratatui::layout::Rect;

use shannon_core::EpisodeId;

use super::marks::TurnMap;
use super::tabs::Over;
use super::window::{Away, Place, Window};
use super::{App, BufferId};
use crate::output::{Anchor, KeptView};
use crate::wm::PaneId;

/// **A window's place, kept by its blocks** ([`App::anchors_on`]) — the
/// place, and its cursor, top and Visual selection's anchor as anchors;
/// `None` where it has none (a top while it follows the end; no
/// selection, or a window that set the session aside).
pub(crate) struct Anchored {
    place: Place,
    cursor: Option<Anchor>,
    top: Option<Anchor>,
    visual: Option<Anchor>,
}

impl App {
    /// **The size `pane` was last drawn at**, inside its border — `None`
    /// before a first frame, when no line is at any window's top, and for a
    /// pane this frame has no room for: one in a tab not shown measured
    /// `(0, 0)`, its top found at width 0 was line 0's, and a compaction in
    /// another tab sent it to its end (review stop 15, A M6).
    fn drawn_size(&self, pane: PaneId) -> Option<(u16, u16)> {
        let (w, h) = self.frame?;
        let (w, h) = self.pane_content_size(pane, Rect::new(0, 0, w, h));
        (w > 0 && h > 0).then_some((w, h))
    }

    /// **`place`, a window's in `buffer`'s session, as positions** whose
    /// turns went through `map` — its top the text there (`Viewport::top`),
    /// none while it follows the end. Drawn or not, at any width: a top is
    /// read without a size (it was a row, read at the size it was laid out
    /// at, and a window never drawn carried its rows raw — S21-Q10).
    fn kept_view(&self, buffer: BufferId, place: &Place, map: TurnMap<'_>) -> Option<KeptView> {
        let output = &self.session_of(buffer)?.output;
        let top = place.viewport.top();
        Some(output.keep_view(place.cursor, top, &place.folds, map))
    }

    /// **`kept`, found in `buffer`'s session: where to put a window** — its
    /// cursor, the text at its top, its folds; the session's entry for what
    /// is not found (a top on a turn the session does not keep follows its
    /// end, as the entry does). Frozen it was, frozen it stays — on the
    /// last page too: following is `G`'s to take up, not a landing's.
    fn found_place(&self, buffer: BufferId, kept: &KeptView) -> Place {
        let mut place = self.entry_of(buffer);
        let Some(output) = self.session_of(buffer).map(|session| &session.output) else {
            return place;
        };
        let found = output.find_view(kept);
        place.folds = found.folds;
        if let Some(cursor) = found.cursor {
            place.cursor = cursor;
        }
        if let Some((line, col)) = found.top {
            place.viewport.freeze_at(line, col);
        }
        place
    }

    /// **`pane` put at `place`**, and its cursor on screen when the place
    /// set its view: a line found at the top may not leave the cursor in
    /// the window a rebuilt transcript gives it, and a window keeps its
    /// view, not its cursor (`Output::focused_window`). On screen and no
    /// more (`H`'s margin, none): a view the window had is left as it was.
    fn put_found(&mut self, pane: PaneId, place: Place) {
        let placed = !place.viewport.is_following_tail();
        self.put_place(pane, place);
        if let (true, Some(size)) = (placed, self.drawn_size(pane)) {
            self.keep_pane_cursor_in(pane, size, 0);
        }
    }

    /// **Every window's place in `buffer`'s session, kept** — those showing
    /// it and those that set it aside, a window holding a buffer once —
    /// for a session made from it that lands over it (`Lands::Over`). Read
    /// before the landing, which rebuilds the transcript and puts every one
    /// at its entry (`forget_cursors_on`); [`put_views`](Self::put_views)
    /// after.
    pub(crate) fn views_on(&self, buffer: BufferId, map: TurnMap<'_>) -> Vec<(PaneId, KeptView)> {
        (self.windows())
            .filter_map(|(pane, window)| {
                let place = window_place(window, buffer)?;
                Some((pane, self.kept_view(buffer, &place, map)?))
            })
            .collect()
    }

    /// **The places [`views_on`](Self::views_on) kept, found in what
    /// landed over `buffer`** — each window where it was, if it still
    /// holds the buffer.
    pub(crate) fn put_views(&mut self, buffer: BufferId, views: Vec<(PaneId, KeptView)>) {
        for (pane, kept) in views {
            let place = self.found_place(buffer, &kept);
            self.put_back(pane, buffer, place);
        }
    }

    /// **`pane`'s place in `buffer`'s session, written back** — put if it
    /// shows the session, else its place there replaced where it was set
    /// aside; nothing, if it no longer holds the buffer. The one ending of
    /// [`put_views`](Self::put_views) and [`put_anchored`](Self::put_anchored).
    fn put_back(&mut self, pane: PaneId, buffer: BufferId, place: Place) {
        if self.window_of(pane).and_then(Window::buffer) == Some(buffer) {
            return self.put_found(pane, place);
        }
        let Some(window) = self
            .windows_mut()
            .find(|(at, _)| *at == pane)
            .map(|(_, w)| w)
        else {
            return;
        };
        for away in &mut window.history {
            if let Away::Session(b, aside) = away
                && *b == buffer
            {
                *aside = place.clone();
            }
        }
    }

    /// **Every window's place in `buffer`'s session, kept by its blocks** —
    /// its cursor and the line at its top as [`Anchor`]s, a block's id and a
    /// line in it — across a change inside the transcript that moves lines
    /// and keeps blocks: a record closing a call in its running line's place
    /// (review stop 15, F H2), above a window reading the reply that
    /// streamed below it meanwhile. Kept by position ([`views_on`](Self::views_on)),
    /// a place on another call's running line is none, which no reopen
    /// draws; by its line, it stood on lines the record put above it. As
    /// vim's `mark_adjust` keeps a window's cursor and `topline` past an
    /// edit above them. [`put_anchored`](Self::put_anchored) after it.
    ///
    /// **And a Visual selection's anchor**, a flat line in the window's copy
    /// as the cursor is: kept by its line, it slid up the lines the record
    /// put above it while the cursor stayed, and `y` took them (the close
    /// verifier, MEDIUM 2, probed).
    pub(crate) fn anchors_on(&self, buffer: BufferId) -> Vec<(PaneId, Anchored)> {
        let Some(output) = self.session_of(buffer).map(|session| &session.output) else {
            return Vec::new();
        };
        let at = |(line, col): (usize, usize)| output.anchor_at(line, col);
        (self.windows())
            .filter_map(|(pane, window)| {
                let place = window_place(window, buffer)?;
                let cursor = at(place.cursor);
                let top = place.viewport.top().and_then(at);
                let visual = (window.session_view())
                    .filter(|view| view.buffer == buffer)
                    .and_then(|view| view.chat.anchor())
                    .and_then(at);
                Some((
                    pane,
                    Anchored {
                        place,
                        cursor,
                        top,
                        visual,
                    },
                ))
            })
            .collect()
    }

    /// **The places [`anchors_on`](Self::anchors_on) kept, found again in
    /// `buffer`'s session** — the cursor on its anchor, the top on its, a
    /// selection's anchor on its; a place whose anchor is gone where it was.
    pub(crate) fn put_anchored(&mut self, buffer: BufferId, kept: Vec<(PaneId, Anchored)>) {
        for (pane, kept) in kept {
            let Anchored {
                mut place,
                cursor,
                top,
                visual,
            } = kept;
            let Some(output) = self.session_of(buffer).map(|session| &session.output) else {
                return;
            };
            let found = |anchor: Option<Anchor>| anchor.and_then(|anchor| output.locate(anchor));
            if let Some(at) = found(cursor) {
                place.cursor = at;
            }
            if let Some((line, col)) = found(top) {
                place.viewport.freeze_at(line, col);
            }
            let visual = found(visual);
            self.put_back(pane, buffer, place);
            if let (Some((line, col)), Some(chat)) = (visual, self.pane_chat_mut(pane)) {
                chat.move_anchor(line, col);
            }
        }
    }

    /// **Every window now showing `into`, put where it was reading `from`**
    /// ([`carry_into`](Self::carry_into)) — what `:clone`, a checkout and an
    /// edit end with, each entering what it made where its result is shown.
    /// `from` open in no buffer: no window was reading it.
    pub(crate) fn carry_into_shown(&mut self, from: EpisodeId, into: BufferId, map: TurnMap<'_>) {
        let Some(source) = self.open_buffer_on(from) else {
            return;
        };
        let shown: Vec<PaneId> = self.panes_with_session(into, Over::EveryTab).collect();
        for pane in shown {
            self.carry_into(pane, source, into, map);
        }
    }

    /// **Where `pane`'s window is in `buffer`'s session** — showing it, or
    /// set aside — for a window split from it (`:fork`), which starts there
    /// as `:split`'s does.
    pub(super) fn place_in(&self, pane: PaneId, buffer: BufferId) -> Option<Place> {
        window_place(self.window_of(pane)?, buffer)
    }

    /// **`pane`, which has just entered `into`, put where it was reading
    /// `from`** — its place there, set aside as it entered, through `map`
    /// (`into`'s numbering of `from`'s turns). A pane that never showed
    /// `from` stays at `into`'s entry.
    pub(crate) fn carry_into(
        &mut self,
        pane: PaneId,
        from: BufferId,
        into: BufferId,
        map: TurnMap<'_>,
    ) {
        let Some(place) = self.place_in(pane, from) else {
            return;
        };
        let Some(kept) = self.kept_view(from, &place, map) else {
            return;
        };
        let found = self.found_place(into, &kept);
        self.put_found(pane, found);
    }
}

/// **Where `window` is in `buffer`'s session** — showing it, or set aside
/// in its history; `None` if it holds no place there. Asked of the session
/// it shows, not `Window::place`, which copies a page's whole window to say
/// it is no session's (review stop 15, D L10: every window of every tab, at
/// each landing).
fn window_place(window: &Window, buffer: BufferId) -> Option<Place> {
    match window.session_view() {
        Some(view) if view.buffer == buffer => Some(view.place()),
        _ => window.history.iter().find_map(|away| match away {
            Away::Session(b, place) if *b == buffer => Some(place.clone()),
            _ => None,
        }),
    }
}
