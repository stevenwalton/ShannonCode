//! TUI rendering: layout and per-pane widgets.
//!
//! `draw` is the only entry point. It takes the immutable application
//! state plus the ratatui `Frame`, computes the three-region layout,
//! and dispatches to a focused sub-fn per region. Layout constants
//! live here because they're rendering concerns, not application
//! logic; they'll graduate to a config struct when one exists.

use std::borrow::Cow;

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use shannon_core::{
    EdgeKind, EpisodeId, Graph, GraphChars, GraphIds, GraphStyle, Node, Role, TokenCounts, Usage,
};

use crate::app::{AnswerKeys, App};
use crate::graph_view::{GraphView, PALETTE_LEN};
use crate::layout::{PaneRect, content_size};
use crate::output::{BlockRole, FocusedWindow, Output};
use crate::prompt::{Prompt, PromptWindow};
use crate::style::{CellStyle, Ink};
use crate::theme::{Context, Theme};
use crate::wm::BufferId;
use crate::workspace::{PageKind, PaneKind};

pub fn draw(frame: &mut Frame, app: &App) {
    // The theme's ground under everything (`ui.background`): what every
    // widget draws over keeps its background unless it says one.
    ground(frame, frame.area(), app.theme());
    // A decision behind what takes the keys — said by both, or a turn waits
    // on a question nobody can see (Reviewer A, round 28: `:memories` is a
    // screen people stay in, and only the panel said so).
    let waiting = || app.decision_in_a_pane() || app.confirm_prompt().is_some();
    // **What the frame draws is the app's answer** (`App::frame_draws`),
    // which what the frame counts as seen asks too — asked here on its own,
    // news that ended behind setup was counted seen (review stop 9, A F2).
    use crate::app::FrameDraws;
    let draws = app.frame_draws();
    // No session aimed at (`shannon tutor`, before one is called) draws the
    // panes as any frame does: each reads its session as an `Option`, and
    // the row says "no agent" in red (Steven, 10-20 and 11-11). It drew the
    // row alone while every pane assumed a session (full review 10-21, B 5
    // and C 5).
    match draws {
        // Setup, while open, is the whole screen: it takes every key, so
        // nothing behind it may look answerable
        // (`[[project_shannon_two_predicates]]` — what is drawn and what takes
        // input ask one question: is setup open).
        FrameDraws::Whole(setup) => {
            crate::setup::draw(frame, setup, waiting(), app.theme());
            return;
        }
        FrameDraws::Panes | FrameDraws::Panel(_) | FrameDraws::Float(_) => {}
    }
    // A PANEL (`:effort`'s, vim's command-line window) takes every key too,
    // but leaves the session readable above it — so the panes are drawn with
    // no focus at all: no focused border, no terminal cursor, nothing that
    // reads as taking keys while the panel does.
    //
    // A FLOAT (`:memories`, `:skills`) is drawn the panel's way: the panes
    // unfocused behind it, its keys on the status row.
    let waiting = waiting();
    let mut layout = app.workspace.frame_layout(frame.area());
    if layout.tabline.height > 0 {
        draw_tabline(frame, layout.tabline, app);
    }
    if !matches!(draws, FrameDraws::Panes) {
        for pane in &mut layout.panes {
            pane.focused = false;
        }
    }

    // **Draw the panes there are.** Each is handed `pane.focused`, computed once
    // by the layer that owns focus, and dispatches on *what it shows* — so this
    // loop is indifferent to how many panes exist, what order they are in, and
    // whether two of them show the same thing.
    //
    // The version this replaces named three panes and derived two of their
    // focus states from the third's (`!prompt_focused`). Correct at n = 2, and
    // a bug the instant there was a third: two panes drew the focused border
    // and the terminal cursor sat in one that wasn't taking keys.
    // What each session's decision pane drew this frame (`decide_value`).
    let mut decision_shown: Vec<(BufferId, DecisionShown)> = Vec::new();
    // Where the terminal's cursor goes: each draw says where its cursor is,
    // and the focused pane's is the one placed — one focus gate for every
    // pane, where three draws each kept their own ([`place`]).
    let mut cursor: Option<Position> = None;
    let worn = app.theme();
    for pane in &layout.panes {
        // **The preview wears the theme it shows** — its ground, its border,
        // its number as well as its page (Steven, 10-14: "even the borders
        // … should be shown as the new color of the theme").
        let preview = app.pane_preview(pane.id);
        let theme = preview.map_or(worn, |p| &p.theme);
        if preview.is_some() {
            reground(frame, pane.rect, theme);
        }
        // The one place a pane gets its colour map (`painter`).
        let paint = painter(pane.kind, theme);
        let at = match pane.kind {
            // The title is asked of the PANE, not of the app: it names the
            // session that pane shows and flags it if that session is a branch
            // or has a child mid-turn (tmux/weechat-style context in the
            // title, not a banner that eats transcript rows).
            PaneKind::Output => {
                let title = app.transcript_title(pane.id);
                draw_transcript(frame, pane, app, &title)
            }
            // Each prompt window its own draft and its own title (10-26).
            PaneKind::Prompt => match app.prompt_of(pane.id) {
                Some(prompt) => {
                    let title = app.prompt_title(pane.id);
                    draw_prompt(frame, pane, &prompt.draft, &title, app.aliases(), theme)
                }
                None => draw_empty_pane(frame, pane, theme),
            },
            // A band across the conversation, not a takeover of it: the
            // transcript stays readable and scrollable while the decision waits,
            // because the context that produced a command is often what tells
            // you whether to run it.
            PaneKind::Tool => match (app.on_pane_in(pane.id), app.decides_for(pane.id)) {
                // A call's report or a summary's draft: the one page
                // (`DecisionPage`), each with its own title and keys.
                (Some(on), Some(buffer)) => {
                    let keys = app.answer_keys(pane.id);
                    let (drew, at) =
                        draw_decision(frame, pane, on.page(), keys, paint, app.aliases());
                    decision_shown.push((buffer, drew));
                    at
                }
                _ => match app.decision_wait_text(pane.id) {
                    Some((title, body)) => {
                        let widget = Paragraph::new(body)
                            .style(theme.ui(Ink::TextQuiet))
                            .block(pane_block(&title, pane, theme));
                        frame.render_widget(widget, pane.rect);
                        None
                    }
                    None => draw_empty_pane(frame, pane, theme),
                },
            },
            // The viewers. They used to `return` early out of `draw`, covering
            // the frame — which is why "open" and "focused" were the same thing
            // for them, and why a `q` meant for the graph could answer a tool
            // call nobody could see. As panes they are ordinary: they draw where
            // the tree puts them and take keys only when focused.
            PaneKind::Graph => match app.pane_graph(pane.id) {
                Some(view) => draw_graph(frame, pane, view, GraphLook::of(&app.config.tui), theme),
                None => draw_empty_pane(frame, pane, theme),
            },
            // Drawn by the same function as `:help`, `:analyze` and
            // `:messages` — which is what gives it their scroll, their cursor
            // row and their legend, rather than three near-copies of each.
            PaneKind::Inspector => match app.pane_inspector(pane.id) {
                Some(text) => draw_text_pane(frame, pane, " inspect ", text, paint),
                None => draw_empty_pane(frame, pane, theme),
            },
            // Every page, the listings' since they became buffers (V5.1):
            // one draw, titled by the buffer (` ls! ` while `:ls` names the
            // unlisted — another pane's `:ls` can change it).
            PaneKind::Page(_) => match app.pane_page(pane.id) {
                Some(view) => draw_page(frame, pane, &app.page_title(view), view, paint),
                None => draw_empty_pane(frame, pane, theme),
            },
            PaneKind::ThemePreview => match preview {
                Some(preview) => draw_text_pane(frame, pane, " preview ", &preview.text, paint),
                None => draw_empty_pane(frame, pane, theme),
            },
        };
        if pane.focused {
            cursor = at;
        }
        // Number every pane from the loop that KNOWS the numbers, rather than
        // handing a number to each of nine draw functions and hoping they all
        // render it the same way. This is also the number `{count}<C-w>w`
        // takes, read off the same ordered list.
        draw_pane_number(frame, pane, theme);
    }
    // The panel's bar, or the float's, takes the status row: its account of
    // keys would be the open screen's keys' opposite (`setup::draw_panel`).
    match draws {
        FrameDraws::Panel(panel) => {
            crate::setup::draw_panel(frame, panel, layout.status, waiting, worn)
        }
        FrameDraws::Float(screen) => {
            crate::setup::draw_float(frame, screen, layout.status, waiting, worn)
        }
        // The `:` line, while open, takes the keys and so the cursor — asked
        // as the keys ask it (`cmdline.is_open()`), where draw order decided.
        FrameDraws::Panes | FrameDraws::Whole(_) => {
            cursor = draw_status(frame, layout.status, app, &decision_shown).or(cursor);
        }
    }
    place(frame, cursor);
}

/// **The theme's ground over `area`** — `ui.background`'s colour and
/// background on every cell, so text with no role of its own, and every
/// widget drawn over it, wear the theme's rather than the terminal's.
/// Nothing at all where the theme leaves it unset (the shipped one does):
/// the terminal's own, untouched. Drawn first, and again after a `Clear`,
/// which resets cells to the terminal's.
pub(crate) fn ground(frame: &mut Frame, area: Rect, theme: &Theme) {
    let style = theme.ui(Ink::Background);
    if style != Style::default() {
        frame.render_widget(Block::default().style(style), area);
    }
}

/// [`ground`], for a pane wearing a theme of its own (`:themes`' preview)
/// over a frame the worn theme grounded: what the theme leaves unset is the
/// terminal's own, PUT BACK — `terminal` previewed under a dark theme would
/// otherwise be drawn on the dark theme's ground.
fn reground(frame: &mut Frame, area: Rect, theme: &Theme) {
    let style = Style::reset().patch(theme.ui(Ink::Background));
    frame.render_widget(Block::default().style(style), area);
}

/// **`(row, cell)` of `inner`'s content, on screen** — kept inside it, so a
/// window's cursor never lands on its border. `None` for content with no
/// room: no cell, no cursor. Every draw that has a cursor says where it is
/// through this, for [`place`] to put the terminal's there.
fn cell_in(inner: Rect, (row, cell): (usize, usize)) -> Option<Position> {
    if inner.is_empty() {
        return None;
    }
    let x = inner.x.saturating_add(saturating_u16(cell));
    let y = inner.y.saturating_add(saturating_u16(row));
    Some(Position::new(
        x.min(inner.right() - 1),
        y.min(inner.bottom() - 1),
    ))
}

/// **The terminal's cursor, placed once a frame** — the one call to
/// `set_cursor_position` (`rg` counts it), at what [`draw`] chose: the
/// focused pane's cursor, or the `:` line's while it is open — whoever takes
/// the keys owns the cursor. `None` places nothing, and ratatui hides it.
///
/// One cursor for every pane (S21-Q23, Steven 11-02: "If it is elegant and
/// saves us from more future errors then that's how I want it"; Reviewer
/// D). A page used to paint its own, a reversed cell: no shape, no blink,
/// not the theme's `ui.cursor`, no anchor for an input method — and four
/// defects where the two mechanisms disagreed (two cursors with `:` open
/// over a page; one erased by a secret's colouring; one clipped off a
/// full row; one the selection's colour).
///
/// **The cell under it is never drawn reversed.** Inside a Visual range the
/// selection reverses it, and the cursor — on a grounded theme, the
/// ground's text colour — would be the selection's own colour there
/// (Reviewer D, F1, a regression of `ui.cursor`'s derivation). Un-reversed
/// in the frame's cells, whoever reversed them, it is a cursor on a plain
/// cell: the rule a page's painted cursor kept, now everyone's. One cell,
/// even under a wide character: ratatui never sends the cell a wide one
/// covers (`Buffer::diff`), so the glyph wears its first cell's style.
fn place(frame: &mut Frame, at: Option<Position>) {
    let Some(at) = at else {
        return;
    };
    if let Some(cell) = frame.buffer_mut().cell_mut(at) {
        cell.modifier.remove(Modifier::REVERSED);
    }
    frame.set_cursor_position(at);
}

/// The pane's ring number, in its top-right border corner.
///
/// Shorten `title` so the pane NUMBER cannot land on top of it, and make any
/// shortening **visible**.
///
/// Both live on the top border row: the title starts at `x + 1` and the number
/// is drawn at `width - digits - 2`. A title long enough to reach that cell was
/// silently overwritten, and because a session id is the tail of the title the
/// result was a *well-formed wrong id* — pane 1 rendering `7d7f16e1` for a
/// session that is actually `7d7f16e4` (Reviewer F, read off the cell grid).
///
/// That is the dangerous kind of wrong: the manual promises "what you read off
/// a border is what you can type", so the id goes into `:open` or `:delete`,
/// where a prefix match can land on a DIFFERENT session. Truncating with `…`
/// keeps the rule honest — a shortened id now looks shortened.
fn elide_title(title: &str, pane: &PaneRect) -> String {
    // **Display cells, not chars.** A terminal spends two columns on a wide
    // glyph, so counting `chars()` under-measures and the title creeps back
    // under the number — and this is reachable, because `:model` passes any
    // name through verbatim and it is rendered in the title.
    //
    // `wrap.rs` has measured in display width since it was written, and this
    // comment used to *claim* it must have the same answer while quietly
    // computing its own (Reviewer B). Now it asks: if `char_cells` is ever
    // refined for a variation selector or an East-Asian-width edge case, the
    // pane title moves with it instead of silently keeping the old answer.
    let cells = crate::wrap::str_cells;
    // The corner, a border cell either side of the number, and the title's own
    // leading cell. Mirrors `draw_pane_number`'s own arithmetic.
    let digits = pane.number.to_string().chars().count();
    let Some(avail) = (pane.rect.width as usize).checked_sub(digits + 4) else {
        return String::new();
    };
    if cells(title) <= avail {
        return title.to_string();
    }
    // Take whole characters while they fit, leaving room for the ellipsis and
    // the trailing space the title style keeps. Never splits a character, and
    // never counts a wide one as a single cell.
    // Below two cells there is no room for the ellipsis and its trailing
    // space, and appending them anyway wrote 2 cells into a 0-cell budget —
    // straight over the number the elision exists to protect. The same overlap,
    // one boundary earlier, and outside the swept range of the property test
    // that was supposed to cover it (Reviewers A and B, both probed it).
    if avail < 2 {
        return String::new();
    }
    let budget = avail - 2;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in title.chars() {
        let w = crate::wrap::char_cells(ch);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out.push(' ');
    out
}

/// vim numbers windows and `{count}<C-w>w` jumps to one; a number you cannot
/// see is a number you cannot type, so it is drawn rather than left to be
/// counted. Skipped on a pane too small to hold it without eating the border.
fn draw_pane_number(frame: &mut Frame, pane: &PaneRect, theme: &Theme) {
    let label = format!("{}", pane.number);
    let w = label.chars().count() as u16;
    // Needs the corner plus a border cell either side; anything smaller would
    // overwrite the frame the number is supposed to be sitting in.
    if pane.rect.width < w + 4 || pane.rect.height < 2 {
        return;
    }
    let style = if pane.focused {
        theme.ui(Ink::BorderFocused).add_modifier(Modifier::BOLD)
    } else {
        // The unfocused border's colour, which it sits in. It was dimmer than
        // the frame around it, while this function's own doc says "a number
        // you cannot see is a number you cannot type" and the manual tells
        // you to read it (Reviewer F).
        theme.ui(Ink::Border)
    };
    let at = Rect {
        x: pane.rect.x + pane.rect.width - w - 2,
        y: pane.rect.y,
        width: w,
        height: 1,
    };
    frame.render_widget(Paragraph::new(Span::styled(label, style)), at);
}

/// A pane whose content is gone — **still a pane**, with its border, its title,
/// and a way out.
///
/// Every content-bearing arm used to be `if let Some(view) = …` with no `else`,
/// and ratatui clears the frame each draw: a pane with no content rendered
/// *nothing at all*, not even a border. The screen grew a black hole you could
/// focus, whose caret vanished, and which the status line still named. A pane
/// that exists must always look like one. It has nothing to put a cursor on:
/// `None`, for [`place`].
fn draw_empty_pane(frame: &mut Frame, pane: &PaneRect, theme: &Theme) -> Option<Position> {
    let title = format!(" {} ", pane.kind.title());
    let block = pane_block(&title, pane, theme);
    let widget = Paragraph::new("(nothing to show — `:close` to dismiss)")
        .style(theme.ui(Ink::Border))
        .block(block);
    frame.render_widget(widget, pane.rect);
    None
}

/// The transcript pane: one `Output` projection, two renderings.
///
/// Focused, it gains a navigable cursor and the Visual selection; unfocused it
/// is the streaming view. Same word-wrap, same styling, same geometry — an
/// unfocused pane simply shows no caret, which is the *only* difference between
/// the two paths. Where the caret is, for [`place`].
fn draw_transcript(frame: &mut Frame, pane: &PaneRect, app: &App, title: &str) -> Option<Position> {
    // THIS pane's session, not "the current one" — the two are the same only
    // until `:open` points a pane somewhere else.
    // A transcript's window is on a session; with none there is nothing to
    // draw in it, not a frame to take down.
    let session = app.session_for_pane(pane.id)?;
    let output = &session.output;
    // An alert holding for it marks its border (B2.1).
    let alert = (app.status_alerts()).border(&mut crate::app::OfSession::new(app, session.id));
    if !pane.focused || output.is_empty() {
        // Not focused, or focused with nothing to navigate yet — either way the
        // placeholder/streaming path, with the border telling the truth.
        // Is this the session the prompt commits to? Drawn, not remembered.
        // Asked only here: a FOCUSED pane already wears the focus colour, so
        // the accent has nothing to add and the answer was being computed and
        // discarded.
        let aimed = app.is_prompt_target(pane.id);
        let vp = app.pane_viewport(pane.id);
        let edge = Edge { aimed, alert };
        draw_output(
            frame,
            pane,
            output,
            (&vp, &app.pane_shut(pane.id)),
            edge,
            title,
            app.aliases(),
            app.theme(),
        );
        return None;
    }
    let (w, h) = pane.content_size();
    // THIS pane's cursor, not "the focused one's". They were the same object
    // while the cursor was a field on `App`, which is why two transcript panes
    // could not disagree about where you were looking.
    let chat = app.pane_chat(pane.id);
    let (cursor_row, cursor_col) = chat.cursor();
    let vp = app.pane_viewport(pane.id);
    let shut = app.pane_shut(pane.id);
    let window = output.focused_window(&vp, &shut, w, h, cursor_row, cursor_col, |line, len| {
        chat.selected_char_range(line, len)
    });
    let secrets = transcript_secrets(
        app.aliases(),
        output,
        &window.rows,
        &window.origins,
        &window.folds,
    );
    let edge = Edge {
        aimed: false,
        alert,
    };
    draw_focused_chat(frame, pane, window, title, edge, &secrets, app.theme())
}

/// **What a transcript's border says beyond focus**: whether the prompt is
/// aimed at it, and an alert holding for its session (B2.1).
#[derive(Clone, Copy, Default)]
struct Edge {
    aimed: bool,
    alert: Option<Style>,
}

/// A logical line longer than this (bytes) is matched row by row instead of
/// whole: matching is linear in the line, the render path must stay
/// O(viewport), and one line of a tool's minified JSON can be megabytes.
const WHOLE_LINE_MATCH: usize = 64 * 1024;

/// **The one rule for where a secret is drawn**: matched on each row's
/// LOGICAL line and sliced to the row, so a marked value long enough to wrap
/// is still one secret. Asked by the four panes that draw user and model
/// text as they were written — the prompt, the transcript, the focused chat
/// and the decision pane (`page_secrets`; review stop 8, E2 5). Each matched its own visual row until 2026-10-02, and a pasted API
/// key wrapped across two rows matched neither: drawn as ordinary text, while
/// the wire was masked. Not yet wired, and what each shows now: `:ls`'s
/// first-prompt column, the stored form, by its handle; `:messages` and the
/// status row, what a notice said — the value in clear, unstyled (both
/// measured, review stop 10, E 1); the inspector and `:graph` labels, the
/// stored (masked) form (by reading; not probed); `:marks`' context column,
/// the drawn line its mark is on — a value in clear, unstyled, and a mark
/// can sit in a thought or an opened record now (by reading, review stop
/// 15, E L7). The pages wrap by
/// `TextView::visible_rows`, which gives no row origins yet (Reviewers B
/// and D).
///
/// `origins[i]` is where `rows[i]` begins, `(line, char offset)`, the line in
/// whatever key the pane can look up in O(1); `line(k)` is that line's text,
/// and `shown(k)` where in it a held secret was put back from its handle when
/// the model wrote it (`Output::shown_at`; `None` for text it did not
/// write). One match per distinct line, in row order. A line past
/// `WHOLE_LINE_MATCH` falls back to the row alone, unread for leaks.
fn row_secrets<'a, K: Copy + PartialEq>(
    aliases: &shannon_core::Aliases,
    rows: &[String],
    origins: &[(K, usize)],
    line: impl Fn(K) -> Option<Cow<'a, str>>,
    shown: impl Fn(K) -> Option<Vec<(usize, usize)>>,
) -> Vec<Vec<SecretSpan>> {
    if aliases.secrets().next().is_none() {
        return vec![Vec::new(); rows.len()];
    }
    /// One line's secrets, in LINE coordinates; `None` = too long to match
    /// whole.
    type LineSpans = Option<Vec<SecretSpan>>;
    let mut cached: Option<(K, LineSpans)> = None;
    rows.iter()
        .zip(origins)
        .map(|(row, &(n, offset))| {
            if cached.as_ref().is_none_or(|(at, _)| *at != n) {
                let spans = line(n)
                    .filter(|text| text.len() <= WHOLE_LINE_MATCH)
                    .map(|text| line_secrets(aliases, &text, shown(n).as_deref()));
                cached = Some((n, spans));
            }
            let Some((_, Some(spans))) = &cached else {
                return secrets_on_row(aliases, row);
            };
            let len = row.chars().count();
            spans
                .iter()
                .filter_map(|s| {
                    let (lo, hi) = (s.from.max(offset), s.to.min(offset + len));
                    (lo < hi).then(|| SecretSpan {
                        from: lo - offset,
                        to: hi - offset,
                        leaked: s.leaked,
                    })
                })
                .collect()
        })
        .collect()
}

/// **A line's secrets, and which the model wrote itself** — the one rule a
/// row's ([`row_secrets`]) and a fold's ([`hides_a_leak`]) ask: `put_back`
/// is where in it a held value was put back from its handle, `None` for
/// text the model did not write, where nothing is a leak; a held value in
/// the model's text and not put back there is one it had.
fn line_secrets(
    aliases: &shannon_core::Aliases,
    text: &str,
    put_back: Option<&[(usize, usize)]>,
) -> Vec<SecretSpan> {
    (aliases.secret_ranges(text).into_iter())
        .map(|(from, to)| SecretSpan {
            from,
            to,
            leaked: put_back.is_some_and(|put| !put.iter().any(|&(lo, hi)| lo < to && from < hi)),
        })
        .collect()
}

/// **Whether a shut block holds a value the model wrote itself** — on any
/// line the fold stands for, by [`line_secrets`]: drawn open, the value is
/// underlined in `ui.secret.leaked`; shut, nothing said it was there
/// (S21-Q13; review stop 15, E M3, probed: 0 such cells shut, 14 open).
/// Only the model's text holds one; a line past `WHOLE_LINE_MATCH` is
/// unread, as a row's is. Remembered by the block (`Output::fold_holds`)
/// under `table`, the secret table's `fingerprint`.
fn hides_a_leak(
    aliases: &shannon_core::Aliases,
    output: &Output,
    (block, _): crate::output::LineAt,
    table: u64,
) -> bool {
    output.fold_holds(block, table, |text, put_back| {
        text.len() <= WHOLE_LINE_MATCH
            && (line_secrets(aliases, text, Some(put_back)).iter()).any(|s| s.leaked)
    })
}

/// **The secrets on a titled fold row** — matched on its whole `title` and
/// cut to the `row` drawn of it, a prefix, as a line's are matched whole
/// and sliced to its rows ([`row_secrets`]): read on the cut row alone, a
/// value the `…` cut through lost its styling (the close verifier at review
/// stop 15, LOW 4). None said leaked: a record is not the model's text.
fn title_secrets(
    aliases: &shannon_core::Aliases,
    title: Option<&str>,
    row: &str,
) -> Vec<SecretSpan> {
    let Some(title) = title else {
        return secrets_on_row(aliases, row);
    };
    let len = row.chars().count();
    (line_secrets(aliases, title, None).into_iter())
        .filter(|s| s.from < len)
        .map(|s| SecretSpan {
            to: s.to.min(len),
            ..s
        })
        .collect()
}

/// The secrets on `row` read alone, no line behind it — none said leaked:
/// that is a line's question ([`row_secrets`]).
fn secrets_on_row(aliases: &shannon_core::Aliases, row: &str) -> Vec<SecretSpan> {
    (aliases.secret_ranges(row).into_iter())
        .map(|(from, to)| SecretSpan {
            from,
            to,
            leaked: false,
        })
        .collect()
}

/// **What the secret table says of a transcript window's rows** — for the
/// two windows that draw a transcript (the focused chat's and any
/// other's): each row's secrets ([`row_secrets`] over its line; on a
/// titled fold row, whose text is its block's title and no line's,
/// `output::FoldRow::titled`, read alone), and whether a fold row stands
/// for a value the model wrote itself ([`hides_a_leak`]).
struct TranscriptSecrets {
    rows: Vec<Vec<SecretSpan>>,
    leaks: Vec<bool>,
}

/// [`TranscriptSecrets`] for a window's `rows`, their `origins` and `folds`.
fn transcript_secrets(
    aliases: &shannon_core::Aliases,
    output: &Output,
    rows: &[String],
    origins: &[(crate::output::LineAt, usize)],
    folds: &[Option<crate::output::FoldRow>],
) -> TranscriptSecrets {
    let mut secrets = row_secrets(
        aliases,
        rows,
        origins,
        |n| output.line_at(n).map(Cow::Borrowed),
        |n| output.shown_at(n),
    );
    let mut leaks = vec![false; rows.len()];
    if aliases.secrets().next().is_none() {
        return TranscriptSecrets {
            rows: secrets,
            leaks,
        };
    }
    let table = aliases.fingerprint();
    for (i, fold) in folds.iter().enumerate() {
        let Some(fold) = fold else { continue };
        if fold.titled {
            secrets[i] = title_secrets(aliases, output.title_of(origins[i].0.0), &rows[i]);
        }
        leaks[i] = hides_a_leak(aliases, output, origins[i].0, table);
    }
    TranscriptSecrets {
        rows: secrets,
        leaks,
    }
}

/// **A secret drawn on a row** — chars `from..to` — and whether the model
/// wrote it itself: a held value in its text that was not put back from a
/// handle, so it had the value (review stop 6, F 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SecretSpan {
    from: usize,
    to: usize,
    leaked: bool,
}

/// **The style of char `i`, if a secret covers it** — the one answer the
/// three renderers ask ([`overlay_secrets`], [`compose_row`],
/// [`focused_chat_line`]): [`secret_style`], or for one the model wrote
/// itself `ui.secret.leaked`, underlined.
fn secret_style_at(secrets: &[SecretSpan], i: usize, theme: &Theme) -> Option<Style> {
    secrets
        .iter()
        .find(|s| s.from <= i && i < s.to)
        .map(|s| match s.leaked {
            true => theme
                .ui(Ink::SecretLeaked)
                .add_modifier(Modifier::UNDERLINED),
            false => secret_style(theme),
        })
}

/// A pane border whose colour signals focus — `ui.border.focused` for the
/// focused pane, `ui.border` for the rest (cyan and gray, shipped), so the
/// active pane is obvious at a glance (vim/tmux convention).
fn pane_block(title: &str, pane: &PaneRect, theme: &Theme) -> Block<'static> {
    aimed_pane_block(title, pane, Edge::default(), theme)
}

/// [`pane_block`], plus the **prompt-target accent**.
///
/// The transcript the prompt commits to wears an accent, so "where does this
/// message go" is answered by looking rather than by remembering. That is the
/// whole point of P4: aiming a prompt somewhere is only safe if the aim is
/// visible, and a target you cannot see is how a message reaches the wrong
/// conversation.
///
/// **Only the transcript side.** An earlier version of this comment said the
/// prompt and its target "share a colour"; the prompt bar draws through plain
/// `pane_block` and never wore it, so the pairing described did not exist. The
/// pair is really **accent on the transcript, id in the prompt's title** — two
/// channels rather than one shade, which is the better answer anyway: colour
/// is the one channel a colourblind user may not have.
///
/// Focus still wins the border outright — you must always be able to see where
/// you are typing — so the accent shows on the *unfocused* target, which is the
/// one you would otherwise have to remember.
///
/// **And an alert holding for the session** (`Edge::alert`, B2.1): its colour
/// last — focus, then the aim, then the alert, for the aim's accent is a
/// safety channel an alert must not paint over (DR-B #13) — and a heavy line,
/// a channel neither of the others uses, so a focused or aimed window still
/// shows its alert.
fn aimed_pane_block(title: &str, pane: &PaneRect, edge: Edge, theme: &Theme) -> Block<'static> {
    // **Every title is elided here, because every title shares its row with the
    // pane number.** This used to be done by the two call sites where the
    // overwrite had been reported — the transcript and the prompt — and six
    // others went straight through. Two of those six carry the COMMAND BEING
    // APPROVED, so `run this? — grep … 'a-very-long-needle-st2i` put the pane
    // number inside the text a human was authorising: a garbled string
    // presented as the thing you are about to run (Reviewer F, off the cell
    // grid). A check placed as an override answers for the members somebody
    // reported; the door answers for all of them
    // (`[[feedback_placement_not_predicate]]`).
    let title = elide_title(title, pane);
    let style = match (pane.focused, edge.aimed, edge.alert) {
        (true, ..) => theme.ui(Ink::BorderFocused),
        (false, true, _) => theme.ui(Ink::BorderAimed),
        (false, false, Some(alert)) => theme.ui(Ink::Border).patch(alert),
        (false, false, None) => theme.ui(Ink::Border),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(title);
    // **Focus on a channel nothing else uses** (S21-Q11, Steven 11-01: "Let's
    // give it a look"; review stop 15, F H1): the focused pane's title in
    // reverse video, as vim's current window's status line is (StatusLine
    // over StatusLineNC). A hue alone said it: two windows on one session
    // drew `ui.border.focused` (cyan) beside `ui.border.aimed` (light cyan,
    // the brighter), and in Steven's palette the two drew alike.
    let block = match pane.focused {
        true => block.title_style(Style::default().add_modifier(Modifier::REVERSED)),
        false => block,
    };
    match edge.alert {
        Some(_) => block.border_type(ratatui::widgets::BorderType::Thick),
        None => block,
    }
}

/// The **startup page** — vim's, in shape and in spirit.
///
/// Vim shows it on an empty buffer, centred, and it vanishes the moment you
/// type because it is *drawn*, never inserted: it is not content, it is what
/// the window says when it has no content. Ours works the same way — this
/// renders only when the transcript is empty, so the first message replaces it
/// with no state to clear.
///
/// What it says is chosen the way vim chooses: the name, the version, and the
/// two or three things a person who has never seen this program needs in order
/// to not be stuck. `:help` and `:q` are vim's own picks and they are the right
/// ones. The rest of the list is what makes THIS program different from a chat
/// box, because a newcomer's real question is not "how do I type" — it is "why
/// is this not a web page".
///
/// Degrades rather than clipping: too short and it drops to the essentials,
/// too narrow and it falls back to a single line. A splash that overflows its
/// pane is worse than no splash.
fn splash(width: u16, height: u16) -> String {
    let w = width as usize;
    let h = height as usize;
    if h == 0 || w < 5 {
        return String::new();
    }
    let version = env!("CARGO_PKG_VERSION");
    // Three tiers, widest first. Each is gated on the width IT needs, measured
    // from its own text rather than a constant that has to be kept in step —
    // the first version guessed 44 and the pitch lines were 60
    // (`[[feedback_rules_rot_by_retyping]]`).
    let essentials = vec![
        "SHANNON CODE".to_string(),
        String::new(),
        format!("version {version}"),
        "a terminal-native coding agent".to_string(),
        String::new(),
        "press i                   to start typing, <Esc><Enter> to send".to_string(),
        "type  :help<Enter>        to read the manual".to_string(),
        // `:qa`, NOT `:q`. Launch is TWO panes with focus in the prompt, and
        // `:q` is vim's close-window — so the documented exit command silently
        // deleted the newcomer's input box while the screen went on telling
        // them to press it (Reviewer F, executed). Vim can say `:q` because vim
        // launches with one window; we cannot.
        "type  :qa<Enter>          to exit".to_string(),
    ];
    // What makes this not a chat box. A newcomer's real question is not "how do
    // I type" — it is "why is this not a web page" — but a STUCK one needs
    // `:help` and `:q` first, so this tier goes when there is no room.
    let pitch = vec![
        String::new(),
        // Steven's tutor draft, 11-03, as `--resume` below: the lessons, the
        // stub answering, from the shell.
        "run   shannon tutor       to learn Shannon, nothing spent".to_string(),
        // The WHOLE recipe. `:vs` alone splits whatever pane you are in — from
        // the launch state that is the prompt, giving two prompt bars (each
        // its own draft since 10-26) and zero second sessions. That is the exact CRITICAL
        // fixed two commits earlier, and this line taught it again from a more
        // prominent place than the manual that warns about it (Reviewer F).
        "type  <C-w>k :vs :new     for a second session, side by side".to_string(),
        "type  :model opus<Enter>  to change who answers".to_string(),
        "type  :graph<Enter>       to see the shape of the conversation".to_string(),
        // Steven's tutor draft, 11-03: "We should also modify Shannon's
        // startup screen to mention this. And we should also mention
        // `shannon --resume` there too." A command run from the shell, not
        // typed here: said as one.
        "run   shannon --resume    to pick a session up again".to_string(),
    ];
    let widest = |ls: &[String]| ls.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    let mut lines = if w >= widest(&essentials) && h >= essentials.len() {
        essentials
    } else {
        // `:qa`, like the essentials tier. The `:q` CRITICAL — launch is two
        // panes, so `:q` closes the newcomer's PROMPT — was fixed above and
        // left standing here, and this tier is not exotic: widest-essentials
        // is 62 columns, so `:vs` on an 80-column terminal puts BOTH panes in
        // it, which is the workflow the pitch line recommends (Reviewer A).
        let hint = if w >= 13 { ":help  ·  :qa" } else { ":help" };
        return hint.to_string();
    };
    if w >= widest(&pitch) && h >= lines.len() + pitch.len() + 2 {
        lines.extend(pitch);
    }

    let indent = w.saturating_sub(widest(&lines)) / 2;
    let pad = " ".repeat(indent);
    let body = lines.into_iter().map(|l| {
        if l.is_empty() {
            String::new()
        } else {
            format!("{pad}{l}")
        }
    });
    // Centre vertically the way vim does, then clamp STRUCTURALLY: the padding
    // and the content are computed from different things, and a splash that
    // overflows pushes the pane border off the screen.
    let content: Vec<String> = body.collect();
    let pad_top = h.saturating_sub(content.len()) / 2;
    let mut out: Vec<String> = std::iter::repeat_n(String::new(), pad_top)
        .chain(content)
        .collect();
    out.truncate(h);
    out.join("\n")
}

#[allow(clippy::too_many_arguments)] // past the limit; a struct here would be a
// parameter object for a private draw helper
fn draw_output(
    frame: &mut Frame,
    pane: &PaneRect,
    output: &Output,
    (viewport, shut): (&crate::viewport::Viewport, &crate::output::Shut<'_>),
    edge: Edge,
    title: &str,
    aliases: &shannon_core::Aliases,
    theme: &Theme,
) {
    let area = pane.rect;
    let block = aimed_pane_block(title, pane, edge, theme);
    let (content_width, content_height) = content_size(area);

    let widget = if output.is_empty() {
        Paragraph::new(splash(content_width, content_height))
            .style(theme.ui(Ink::Border))
            .block(block)
    } else {
        let window = output.visible_window(viewport, shut, content_width, content_height);
        let secrets = transcript_secrets(
            aliases,
            output,
            &window.rows,
            &window.origins,
            &window.folds,
        );
        let lines: Vec<Line> = (0..window.rows.len())
            .map(|i| {
                let line = bubble_line(
                    &window.rows[i],
                    &window.styles[i],
                    Bubble {
                        role: window.roles[i],
                        dropped: window.dropped[i],
                    },
                    &secrets.rows[i],
                    usize::from(content_width),
                    theme,
                );
                let width = usize::from(content_width);
                folded(line, window.folds[i], secrets.leaks[i], width, theme)
            })
            .collect();
        Paragraph::new(lines).block(block)
    };
    frame.render_widget(widget, area);
}

/// A [`BlockRole::Rule`] row: the pane's width of `─` in `ui.border.user`.
/// Its line is empty, so the width is the pane's, not the text's.
fn rule_line(width: usize, dropped: bool, theme: &Theme) -> Line<'static> {
    let ink = if dropped {
        Ink::Danger
    } else {
        Ink::BorderUser
    };
    Line::from(Span::styled("─".repeat(width), theme.ui(ink)))
}

/// **A shut block's row, drawn whole** (N5a): `+ `, its text as its role
/// paints it, `…` where the text was cut, then how many lines it holds —
/// those three in `ui.text.faint`, so the text reads first. Its text is its
/// first line's start, so a secret or a selection on it lands where it does
/// on the line — or its block's title (`FoldRow::titled`), read alone
/// ([`transcript_secrets`]) and selected whole or not at all
/// (`Output::focused_window`). Any other row as it is.
///
/// **A value the model wrote itself under it** (`leak`, [`hides_a_leak`])
/// is said after the count, `FoldRow::LEAKED` in the value's own style —
/// the row's text cut again to leave it room in `width`, as the fold cut
/// it to leave room for the rest.
fn folded(
    mut line: Line<'static>,
    fold: Option<crate::output::FoldRow>,
    leak: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    use crate::output::FoldRow;
    use crate::wrap::str_cells;
    let Some(fold) = fold else {
        return line;
    };
    let faint = theme.ui(Ink::TextFaint);
    let mut cut = fold.cut;
    if leak {
        let text: usize = (line.spans.iter()).map(|s| str_cells(&s.content)).sum();
        let room = width
            .saturating_sub(str_cells(&FoldRow::lead()))
            .saturating_sub(str_cells(&fold.suffix()))
            .saturating_sub(str_cells(FoldRow::LEAKED));
        if text + usize::from(cut) > room {
            // One cell for the `…`, as the fold's own cut leaves.
            line.spans = leading_spans(line.spans, room.saturating_sub(1));
            cut = true;
        }
    }
    line.spans.insert(0, Span::styled(FoldRow::lead(), faint));
    if cut {
        line.spans.push(Span::styled("…", faint));
    }
    line.spans.push(Span::styled(fold.suffix(), faint));
    if leak {
        let worn = theme
            .ui(Ink::SecretLeaked)
            .add_modifier(Modifier::UNDERLINED);
        line.spans.push(Span::styled(FoldRow::LEAKED, worn));
    }
    line
}

/// `spans` cut to their first `cells` cells (`wrap::leading`), each keeping
/// its style.
fn leading_spans(spans: Vec<Span<'static>>, mut cells: usize) -> Vec<Span<'static>> {
    let mut kept = Vec::with_capacity(spans.len());
    for span in spans {
        let lead = crate::wrap::leading(&span.content, cells);
        let whole = lead.len() == span.content.len();
        cells -= crate::wrap::str_cells(lead);
        kept.push(Span::styled(lead.to_string(), span.style));
        if !whole {
            break;
        }
    }
    kept
}

/// **What a transcript row wears**: its block's role, and whether it is of a
/// turn not in the conversation (`output::Span::dropped`) — painted in
/// `ui.danger` over the role, whatever the role (Steven, 10-31: "making it
/// clear in the conversation that this message was not sent and not
/// included"). A held secret's style still outranks it.
#[derive(Debug, Clone, Copy)]
struct Bubble {
    role: BlockRole,
    dropped: bool,
}

impl From<BlockRole> for Bubble {
    fn from(role: BlockRole) -> Self {
        Bubble {
            role,
            dropped: false,
        }
    }
}

/// Render one transcript row with its per-role "bubble" treatment. The **user
/// echo** is dimmed — it reflects what you typed, so the assistant's reply is
/// what stands out; the **assistant** reply carries the markdown/syntax styling;
/// a **rule** frames your message. `width` is the pane's, which a rule spans.
/// (Configurable per-role padding/alignment is a later refinement.)
fn bubble_line(
    row: &str,
    styles: &[CellStyle],
    bubble: impl Into<Bubble>,
    secrets: &[SecretSpan],
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let bubble = bubble.into();
    // A withheld secret outranks every other reason to colour a run. The user
    // echo is DIM and an assistant reply may have markdown styling — which is
    // exactly how a password ended up looking like ordinary text in one pane and
    // like a *code span* in another. "This did not leave your machine" is the
    // more important fact, so it wins. `secrets` comes from `row_secrets`.
    if !secrets.is_empty() {
        return overlay_secrets(
            row,
            secrets,
            |i| base_style(styles, bubble, i, theme),
            theme,
        );
    }
    let Bubble { role, dropped } = bubble;
    if dropped && role != BlockRole::Rule {
        return Line::from(Span::styled(row.to_string(), theme.ui(Ink::Danger)));
    }
    match role {
        BlockRole::User => Line::from(Span::styled(
            row.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        )),
        BlockRole::Assistant => styled_line(row, styles, theme, Context::Code),
        // Machinery, not either party talking — cyan marks it as the app's own
        // voice (the same colour the action lines use), so a tool call is
        // visually separable from the conversation without shouting. Its
        // record says which part is which (`tool_record`): what ran in
        // `TOOL`, what came of it in `ui.tool.output` and dim
        // (`tool_record::OUTPUT`), a write's lines marked.
        BlockRole::Tool if styles.len() == row.chars().count() => {
            styled_line(row, styles, theme, Context::Code)
        }
        BlockRole::Tool => Line::from(Span::styled(
            row.to_string(),
            theme.style(Context::Code, CellStyle::ink(Ink::Tool)),
        )),
        BlockRole::Rule => rule_line(width, dropped, theme),
        // Per char, as the focused pane paints it (`base_style`): its
        // markdown over `THINKING`.
        BlockRole::Thinking => {
            let chars: Vec<char> = row.chars().collect();
            let worn: Vec<Style> = (0..chars.len())
                .map(|i| base_style(styles, bubble, i, theme))
                .collect();
            runs_to_line(&chars, &worn)
        }
    }
}

/// Build a styled `Line` from a row and its per-char styles (markdown / syntax
/// highlighting, computed in the output cache), as `theme` paints them in
/// `context`. Consecutive equal styles fold into one `Span`; an all-plain row
/// (the common case) is a single raw span.
pub(crate) fn styled_line(
    row: &str,
    styles: &[CellStyle],
    theme: &Theme,
    context: Context,
) -> Line<'static> {
    let chars: Vec<char> = row.chars().collect();
    // Fast path / defensive: all-plain, or a length mismatch (shouldn't happen
    // — `ensure` produces one style per visible char) → render plain.
    if styles.len() != chars.len() || styles.iter().all(CellStyle::is_plain) {
        return Line::from(row.to_string());
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let style = styles[i];
        let mut j = i + 1;
        while j < chars.len() && styles[j] == style {
            j += 1;
        }
        let text: String = chars[i..j].iter().collect();
        spans.push(if style.is_plain() {
            Span::raw(text)
        } else {
            Span::styled(text, theme.style(context, style))
        });
        i = j;
    }
    Line::from(spans)
}

fn draw_prompt(
    frame: &mut Frame,
    pane: &PaneRect,
    prompt: &Prompt,
    title: &str,
    aliases: &shannon_core::Aliases,
    theme: &Theme,
) -> Option<Position> {
    let (content_width, content_height) = content_size(pane.rect);
    // Soft-wrap window: long lines wrap into multiple visual rows, the cursor
    // is reported in visual (within-window) coordinates, with a phantom
    // continuation row at an end-of-line wrap boundary.
    let window = prompt.visible_window(content_width as usize, content_height as usize);
    // Your own text: never the model's, so never a leak.
    let secrets = row_secrets(
        aliases,
        &window.rows,
        &window.origins,
        |n| prompt.line_text(n).map(Cow::Owned),
        |_| None,
    );
    draw_buffer_pane(frame, pane, window, title, &secrets, theme)
}

/// Render a [`Buffer`](crate::prompt::Buffer)-backed pane from its
/// `visible_window`: the soft-wrapped rows, the reverse-video Visual selection,
/// marked secrets (`secrets`, per row, from `row_secrets`) — and where its
/// cursor is, for [`place`]. Used by the prompt; the focused chat renders
/// through `draw_focused_chat`, which keeps its role styling.
fn draw_buffer_pane(
    frame: &mut Frame,
    pane: &PaneRect,
    window: PromptWindow,
    title: &str,
    secrets: &[Vec<SecretSpan>],
    theme: &Theme,
) -> Option<Position> {
    let area = pane.rect;
    let block = pane_block(title, pane, theme);
    let inner = block.inner(area);
    // Reverse-video the Visual-mode selection — theme-agnostic and the
    // classic terminal "selected" look (weechat/vim lineage), so it reads
    // correctly whatever the eventual color scheme.
    let highlight = Style::default().add_modifier(Modifier::REVERSED);
    let lines: Vec<Line> = window
        .rows
        .iter()
        .zip(window.selection.iter())
        .zip(secrets)
        .map(|((row, sel), secrets)| selected_line(row, *sel, highlight, secrets, theme))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
    // `cursor_visual` is within the window already, in cells.
    cell_in(inner, window.cursor_visual)
}

/// Build one prompt row, reverse-highlighting the selected `[lo, hi)`
/// char range (Visual mode). No selection → the row verbatim. `lo`/`hi`
/// are char offsets into the row (clamped defensively).
fn selected_line(
    row: &str,
    sel: Option<(usize, usize)>,
    highlight: Style,
    secrets: &[SecretSpan],
    theme: &Theme,
) -> Line<'static> {
    if secrets.is_empty() {
        let Some((lo, hi)) = sel else {
            return Line::from(row.to_string());
        };
        let chars: Vec<char> = row.chars().collect();
        let hi = hi.min(chars.len());
        // Clamp `lo <= hi <= len` so the three slices below can never panic,
        // whatever a future producer emits (honoring the doc's "defensively").
        let lo = lo.min(hi);
        return Line::from(vec![
            Span::raw(chars[..lo].iter().collect::<String>()),
            Span::styled(chars[lo..hi].iter().collect::<String>(), highlight),
            Span::raw(chars[hi..].iter().collect::<String>()),
        ]);
    }
    compose_row(row, sel, highlight, secrets, theme)
}

/// The base style a row character wears before secrets are considered — the
/// role bubble plus any markdown/syntax styling, as `theme` paints it.
fn base_style(styles: &[CellStyle], bubble: Bubble, i: usize, theme: &Theme) -> Style {
    let cell = |or: CellStyle| theme.style(Context::Code, styles.get(i).copied().unwrap_or(or));
    if bubble.dropped {
        return theme.ui(Ink::Danger);
    }
    match bubble.role {
        BlockRole::User => Style::default().add_modifier(Modifier::DIM),
        BlockRole::Assistant => cell(CellStyle::PLAIN),
        BlockRole::Tool => cell(CellStyle::ink(Ink::Tool)),
        BlockRole::Rule => theme.ui(Ink::BorderUser),
        // Quieter than the reply; a heading or a code span in it keeps its
        // own colour, in the thinking's italic and dim.
        BlockRole::Thinking => theme
            .style(Context::Code, THINKING)
            .patch(cell(CellStyle::PLAIN)),
    }
}

/// **What a thought wears**: quiet text, italic and dim — told from the
/// reply by more than its slant (Steven, 11-01: "Italic helps but is not
/// enough"), and from a tool's output — dim too, but in the record's
/// colour and upright — by its colour and its slant. The emphasis is
/// the cell's, so every theme's quiet colour carries it.
const THINKING: CellStyle = CellStyle::ink(Ink::Thinking).italic().dim();

/// Build a row where `secrets` wear [`secret_style`] and everything else keeps
/// whatever `base` gives it.
///
/// The secret style **replaces** rather than patches. A withheld value inside a
/// markdown code span was rendering as *code*, which says "this is a literal to
/// copy" — the opposite of what we mean. One fact wins here, and it is ours —
/// save a selection's reverse, which says where you are, not what the text
/// is: the prompt and the transcript patch theirs over a secret, and a
/// decision page's range lost its reverse on one (Reviewer D, F3).
fn overlay_secrets(
    row: &str,
    secrets: &[SecretSpan],
    base: impl Fn(usize) -> Style,
    theme: &Theme,
) -> Line<'static> {
    let chars: Vec<char> = row.chars().collect();
    let styles: Vec<Style> = (0..chars.len())
        .map(|i| match secret_style_at(secrets, i, theme) {
            Some(secret) => secret.add_modifier(base(i).add_modifier & Modifier::REVERSED),
            None => base(i),
        })
        .collect();
    runs_to_line(&chars, &styles)
}

/// Group equal-styled runs into spans — one `Span` per run rather than per
/// character, which matters at our frame budget.
fn runs_to_line(chars: &[char], styles: &[Style]) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut j = i + 1;
        while j < chars.len() && styles[j] == styles[i] {
            j += 1;
        }
        spans.push(Span::styled(
            chars[i..j].iter().collect::<String>(),
            styles[i],
        ));
        i = j;
    }
    Line::from(spans)
}

/// The style a withheld secret wears.
///
/// **Deliberately outside the danger palette.** `Tone` is green/yellow/red and
/// every value on it claims *how dangerous this is*; a withheld secret is not a
/// point on that scale. Green would assert the surrounding text is safe, and
/// yellow/red would say "worry" when the case worth worrying about is a message
/// with *nothing* marked. So: its own colour, able to co-occur with a `Tone`
/// rather than competing with one. Bold because the shipped `ui.secret` is.
fn secret_style(theme: &Theme) -> Style {
    theme.ui(Ink::Secret)
}

/// Build a row where withheld secrets carry their own style and the Visual
/// selection still reads as selected — the two compose rather than overriding,
/// because a selected secret is both things at once.
fn compose_row(
    row: &str,
    sel: Option<(usize, usize)>,
    highlight: Style,
    secrets: &[SecretSpan],
    theme: &Theme,
) -> Line<'static> {
    let chars: Vec<char> = row.chars().collect();
    let styles: Vec<Style> = (0..chars.len())
        .map(|i| {
            let mut style = secret_style_at(secrets, i, theme).unwrap_or_default();
            if sel.is_some_and(|(lo, hi)| i >= lo && i < hi.min(chars.len())) {
                style = style.patch(highlight);
            }
            style
        })
        .collect();
    runs_to_line(&chars, &styles)
}

/// Render the **focused chat** from an [`Output`]-built [`FocusedWindow`]: the
/// same word-wrapped, role-bubbled, markdown/syntax-styled rows as the streaming
/// view, with the Visual selection reverse-highlighted — and where the cursor
/// is, for [`place`]. One render model — focusing the chat changes nothing but
/// adding a caret.
fn draw_focused_chat(
    frame: &mut Frame,
    pane: &PaneRect,
    window: FocusedWindow,
    title: &str,
    edge: Edge,
    secrets: &TranscriptSecrets,
    theme: &Theme,
) -> Option<Position> {
    let area = pane.rect;
    let block = aimed_pane_block(title, pane, edge, theme);
    let inner = block.inner(area);
    let width = usize::from(content_size(area).0);
    let lines: Vec<Line> = (0..window.rows.len())
        .map(|i| {
            let line = focused_chat_line(
                &secrets.rows[i],
                &window.rows[i],
                &window.styles[i],
                Bubble {
                    role: window.roles[i],
                    dropped: window.dropped[i],
                },
                window.selection[i],
                width,
                theme,
            );
            folded(line, window.folds[i], secrets.leaks[i], width, theme)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
    // `cursor` is within the window already, in cells.
    window.cursor.and_then(|at| cell_in(inner, at))
}

/// One focused-chat row: the per-role bubble + markdown/syntax styling **and** the
/// Visual selection, composed. Each char's style is its base (dimmed for the user
/// echo, the cached `CellStyle` for the assistant) plus reverse-video if it falls
/// in the selected `[lo, hi)` char span; equal-styled runs fold into spans.
fn focused_chat_line(
    secrets: &[SecretSpan],
    row: &str,
    styles: &[CellStyle],
    bubble: impl Into<Bubble>,
    sel: Option<(usize, usize)>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let bubble = bubble.into();
    if bubble.role == BlockRole::Rule {
        return rule_line(width, bubble.dropped, theme);
    }
    let chars: Vec<char> = row.chars().collect();
    if chars.is_empty() {
        return Line::from(String::new());
    }
    let (slo, shi) = match sel {
        Some((lo, hi)) => (lo.min(chars.len()), hi.min(chars.len())),
        None => (0, 0),
    };
    let style_at = |i: usize| -> Style {
        // A withheld secret outranks the role bubble and any markdown styling.
        let base = secret_style_at(secrets, i, theme)
            .unwrap_or_else(|| base_style(styles, bubble, i, theme));
        if slo <= i && i < shi {
            base.add_modifier(Modifier::REVERSED)
        } else {
            base
        }
    };
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let style = style_at(i);
        let mut j = i + 1;
        while j < chars.len() && style_at(j) == style {
            j += 1;
        }
        spans.push(Span::styled(chars[i..j].iter().collect::<String>(), style));
        i = j;
    }
    Line::from(spans)
}

/// Render the `:graph` viewer: the visible slice of the collapsible tree, one
/// node per row, indented by depth with a fold disclosure marker, the selected
/// row reverse-highlighted. Only the `O(viewport)` rows the window shows are
/// built — navigation/folding update the model, not history (the perf rule).
/// Over-long rows are clipped by the (no-wrap) paragraph, so each node stays on
/// its own line.
/// **How `:graph` is drawn**, as `[tui]` says — read in one place for the two
/// readers that must agree: the renderer, and `/`, which searches the text
/// the renderer draws ([`graph_row_texts`]). Two reads of three settings
/// could draw one style and search another.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GraphLook {
    style: GraphStyle,
    chars: GraphChars,
    ids: GraphIds,
}

impl GraphLook {
    /// The look `tui` configures.
    pub(crate) fn of(tui: &shannon_core::config::TuiConfig) -> Self {
        Self {
            style: tui.graph_style,
            chars: tui.graph_chars,
            ids: tui.graph_ids,
        }
    }
}

fn draw_graph(
    frame: &mut Frame,
    pane: &PaneRect,
    view: &GraphView,
    look: GraphLook,
    theme: &Theme,
) -> Option<Position> {
    let area = pane.rect;
    let block = pane_block(" graph ", pane, theme);
    let inner = block.inner(area);
    let (w, h) = content_size(area);

    // No sessions yet is exactly when Claude Code's conversations are worth
    // showing, so only an empty tree with no section says so.
    if view.is_empty() && view.claude().is_none() {
        let widget = Paragraph::new("(no episodes yet — start a conversation)")
            .style(theme.ui(Ink::Border))
            .block(block);
        frame.render_widget(widget, area);
        return None;
    }

    // `window_top` slides a remembered window.
    let top = view.window_top(h as usize);
    let now = std::time::SystemTime::now();
    let rows = view.tree_rows(top, h as usize);
    // A list's cursor is its selected row, reversed whole; inside a Visual
    // range every row is, and the cursor is the row's first cell, which
    // `place` draws plain.
    let at = (rows.iter())
        .position(|row| row.selected && row.visual)
        .and_then(|row| cell_in(inner, (row, 0)));
    let lines: Vec<Line> = rows
        .iter()
        .map(|row| graph_row_line(view, row, look, now, w as usize, theme))
        .collect();
    // `~` below the last row — a `zt` near the end, or a tree shorter than
    // its pane — as in every other window that keeps a top.
    let below = (h as usize).saturating_sub(lines.len());
    let lines: Vec<Line> = lines
        .into_iter()
        .chain(past_the_end(below, theme))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
    at
}

/// **What `/` searches in `:graph`: the rows, as drawn** — every one
/// `GraphView::searched_tree_rows` names (the drawn ones, and the sessions
/// and turns a fold hides; Claude Code's with them), one string each, by the
/// function [`draw_graph`] draws them with (Steven, 10-14: "We should be able
/// to search what we see on the screen"). So a search finds no word a row
/// would not show and misses none it does: a name, an id, a prompt's opening
/// words, `held`. A FOLDED session or turn is searched as it would draw once
/// opened, and a match opens its folds, as vim's search opens one
/// (`'foldopen'`): a session forked from a turn hangs in that turn's fold,
/// and could not be found (Steven, 10-15). A row scrolled out of the window
/// is searched, as vim searches past the window. The Claude Code heading's
/// rule is drawn to the pane's width and searched without it — it holds no
/// words. `theme` paints what is kept as text alone; a colour changes no
/// word.
pub(crate) fn graph_row_texts(view: &GraphView, look: GraphLook, theme: &Theme) -> Vec<String> {
    let now = std::time::SystemTime::now();
    let text =
        |line: Line<'static>| -> String { line.spans.iter().map(|s| s.content.as_ref()).collect() };
    view.searched_tree_rows()
        .iter()
        .map(|row| text(graph_row_line(view, row, look, now, 0, theme)))
        .collect()
}

/// **One `:graph` row, whatever it stands for** — a node in the configured
/// style ([`tree_row_line`]), a Claude Code entry as its own
/// ([`claude_row_line`]). The one door both the drawing and the search take.
/// `width` is the pane's content width, which the section's rule fills.
fn graph_row_line(
    view: &GraphView,
    row: &crate::graph_view::DisplayRow<'_>,
    look: GraphLook,
    now: std::time::SystemTime,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    match view.claude_row(row) {
        Some(claude) => {
            // What it hangs from, in the style's terms — nothing at the top;
            // a conversation resumed from another hangs under that one's
            // turn, as a session does.
            let indent: Vec<Span<'static>> = match look.style {
                GraphStyle::Tree => vec![Span::raw(row.prefix.clone())],
                GraphStyle::Git => row
                    .lane
                    .iter()
                    .map(|c| {
                        let ch = graph_glyph(c.ch, look.chars).to_string();
                        Span::styled(ch, lane(c.color, theme))
                    })
                    .collect(),
            };
            claude_row_line(claude, indent, now, width, view.glyphs(), look.chars, theme)
        }
        None => tree_row_line(view, row, look, theme),
    }
}

/// **The mark a folded row trails** — it holds more than it shows: a
/// session's, a conversation's.
fn more_mark(chars: GraphChars) -> &'static str {
    match chars {
        GraphChars::Ascii => "  ~",
        GraphChars::Unicode => "  ⋯",
    }
}

/// One tree row as `:graph` draws it: the HEAD gutter, the style's
/// connectors or lanes, the label, and any background work — styled for
/// HEAD, the selection, a range, a borrowed turn and a mark.
fn tree_row_line(
    view: &GraphView,
    row: &crate::graph_view::DisplayRow<'_>,
    look: GraphLook,
    theme: &Theme,
) -> Line<'static> {
    // A fixed 2-col gutter for the active (HEAD) flag, so marking a row
    // never shifts its content — siblings stay column-aligned. `tree`
    // then draws depth-indented connectors; `git` draws the branch-lane
    // graph. Only the node marker + label differ after that.
    let gutter = if row.active { "▶ " } else { "  " };
    let mut spans = vec![Span::raw(gutter.to_string())];
    match look.style {
        GraphStyle::Tree => {
            spans.push(Span::raw(row.prefix.clone()));
            spans.extend(tree_row_spans(row));
        }
        GraphStyle::Git => spans.extend(git_row_spans(view, row, look.chars, look.ids, theme)),
    }
    // The active session is bold; the selection reverses (and wins when
    // both apply). Applied as the line's base style; per-span styles (the
    // dim decoration, the per-branch dot color) patch over it. In `tree`
    // the whole active row goes cyan; in `git` the cyan would fight the
    // per-branch dot colors, so active is bold-only there.
    let mut line_style = Style::default();
    if row.active {
        line_style = line_style.add_modifier(Modifier::BOLD);
        if matches!(look.style, GraphStyle::Tree) {
            line_style = line_style.patch(theme.ui(Ink::GraphTree));
        }
    }
    // A Visual range reverses every row in it, as a selection does in
    // the prompt and the transcript.
    if row.selected || row.visual {
        line_style = line_style.add_modifier(Modifier::REVERSED);
    }
    // A turn this session only *shares* (borrowed by reference — a clone's
    // memory of another session) renders italic AND faded (`ui.graph.shared`,
    // shipped): a light gray between the annotation gray and normal text, so
    // shared vs owned history reads at a glance. `shared` is precomputed at
    // build — no per-frame derivation.
    if row.shared {
        line_style = line_style.patch(theme.ui(Ink::GraphShared));
    }
    // A row marked (`<Space>`) for a `pick` gets a background
    // (`ui.graph.mark`) — it persists as the cursor moves on, so a
    // multi-select reads at a glance.
    if row.marked {
        line_style = line_style.patch(theme.ui(Ink::GraphMark));
    }
    // A child session with background work announces itself on its
    // row (the border highlight is the M4 upgrade; this is the honest
    // minimum). Both the marker and the WORD come from the activity, so
    // a child that is stopped waiting for you cannot read as one that
    // is working — which is what `▶ running` said for a held job.
    if let Some(activity) = row.activity {
        // The word comes from `Activity`, shared with the inspector;
        // only the colour is this renderer's own. Cyan for held, not
        // yellow: that one is addressed to the reader.
        let word = activity.word();
        let role = match activity {
            crate::graph_view::Activity::Running => Ink::GraphRunning,
            crate::graph_view::Activity::Held => Ink::GraphHeld,
        };
        spans.push(Span::styled(
            format!("  {} {word}", view.glyphs().for_activity(activity)),
            theme.ui(role),
        ));
    }
    // Inside a range the reversed row no longer says where the cursor is;
    // the terminal's cursor on its first cell does (`draw_graph`, `place`).
    Line::from(spans).style(line_style)
}

/// One row of `:graph`'s Claude Code section: the heading — a rule across
/// the pane, so the section reads as apart from Shannon's tree (Steven,
/// 10-04: "a separator line… for added visual clarity"), saying how many and
/// from where — or a conversation: its id, what Claude Code calls it, and
/// how long ago it was written. Selected and Visual rows reverse as the
/// tree's do; inside a range the cursor is the terminal's, on the row's
/// first cell (`draw_graph`). `width` is the pane's content width, which the
/// rule fills.
fn claude_row_line(
    row: crate::graph_view::ClaudeRow,
    indent: Vec<Span<'static>>,
    now: std::time::SystemTime,
    width: usize,
    glyphs: crate::graph_view::NodeGlyphs,
    chars: GraphChars,
    theme: &Theme,
) -> Line<'static> {
    use crate::graph_view::ClaudeRow;
    let dim = theme.ui(Ink::TextQuiet);
    let (spans, selected, visual) = match row {
        ClaudeRow::Heading {
            count,
            selected,
            visual,
        } => {
            // Short, so a graph pane beside the inspector still has room for
            // the rule; the keys are the legend's, the inspector's and
            // `:help`'s to say.
            let rule = theme.ui(Ink::Border);
            let said = match count {
                Some(count) => format!(" · {count} here "),
                None => format!(" · here: {} ", crate::graph_view::NOT_READ),
            };
            let mut spans = vec![
                Span::styled("── ", rule),
                Span::styled("Claude Code", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(said, dim),
            ];
            let used: usize = spans
                .iter()
                .map(|s| crate::wrap::str_cells(&s.content))
                .sum();
            spans.push(Span::styled("─".repeat(width.saturating_sub(used)), rule));
            (spans, selected, visual)
        }
        ClaudeRow::Conversation {
            found,
            continued,
            folded,
            awaiting,
            resumes,
            selected,
            visual,
        } => {
            let (id, title) = crate::graph_view::conversation_name(found);
            // Continued in Shannon: its own mark and colour, so a fold or a
            // long list cannot hide that it went on (Steven, 10-16). Both
            // are the user's (`[tui.graph_glyphs]`, `ui.graph.continued`).
            let ink = match continued {
                true => Ink::GraphContinued,
                false => Ink::Tool,
            };
            let mut spans = vec![Span::raw("  ")];
            spans.extend(indent);
            spans.extend([
                Span::styled(format!("{} ", glyphs.for_claude(continued)), theme.ui(ink)),
                Span::raw(id.to_string()),
                Span::raw("  "),
                Span::raw(title.to_string()),
            ]);
            // A resume hangs flat from its lineage's top: where it was
            // picked up is said here, where its place used to say it.
            if let Some(resumes) = resumes {
                spans.push(Span::styled(format!("  · {resumes}"), dim));
            }
            if let Some(then) = found.modified {
                spans.push(Span::styled(
                    format!("  · {}", crate::graph_view::ago(now, then)),
                    dim,
                ));
            }
            // Folded — unread, or closed — it says it holds more, as a
            // folded session's row does: `l` opening it was otherwise a key
            // nothing on screen offered (review stop 3, A F10, C 9).
            // Being read off the UI thread: its turns, and the sessions that
            // hang at one, come when it is (Steven, 10-17: "the user will
            // expect that action to take time").
            if awaiting {
                spans.push(Span::styled("  · reading…", dim));
            }
            if folded {
                spans.push(Span::styled(more_mark(chars), dim));
            }
            (spans, selected, visual)
        }
    };
    let style = if selected || visual {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    Line::from(spans).style(style)
}

/// **A window on a page** — the `:help` manual, `:analyze`'s report (the
/// verdict's colours the app's own, the shell's the theme's, painted by the
/// decision pane's map: `painter`) or the notice history — titled by the
/// window ([`PageWindow::title`](crate::page::PageWindow::title)).
///
/// **Through the shared renderer**, like every text page. `:analyze` built
/// its own `Paragraph` and so never drew the cursor row — while taking the
/// same motions as every page (`TextView::apply`), so `j` moved an invisible
/// cursor for a screenful before the page appeared to respond (Reviewer A).
/// Wrapped rather than clipped: the manual is written to 78 columns, and a
/// help pane is very often narrower; clipping would drop the right-hand end
/// of exactly the lines that carry the `*tags*`.
fn draw_page(
    frame: &mut Frame,
    pane: &PaneRect,
    title: &str,
    view: &crate::page::PageWindow,
    paint: crate::paint::Painter<'_>,
) -> Option<Position> {
    draw_text_pane(frame, pane, title, view.text(), paint)
}

/// **The one place a pane is given its colour map** — the theme's
/// `decision.*` entries for the decision pane and `:analyze` (where green,
/// yellow and red are the verdict's), its code entries everywhere else.
/// Exhaustive, no wildcard: a new kind of pane is a decision here, never a
/// default. A leaf naming a `Context` itself is how a pane could paint with
/// the wrong map, and no leaf does (`crate::paint`).
pub(crate) fn painter(kind: PaneKind, theme: &crate::theme::Theme) -> crate::paint::Painter<'_> {
    use crate::theme::Context;
    let context = match kind {
        // `:analyze`'s report is a decision's, drawn as the pane draws one.
        PaneKind::Tool | PaneKind::Page(PageKind::Analyze) => Context::Decision,
        PaneKind::Output
        | PaneKind::Prompt
        | PaneKind::Graph
        | PaneKind::Inspector
        | PaneKind::ThemePreview
        | PaneKind::Page(_) => Context::Code,
    };
    crate::paint::Painter::new(theme, context)
}

/// A bordered, wrapped, scrolled [`TextView`] — what every static page is
/// once its title is decided — and where its cursor is, for [`place`].
///
/// Three call sites over three holders, and the count is checkable: `rg
/// 'draw_text_pane\('` — the inspector, `:themes`' preview, and every page
/// window through [`draw_page`]: `:help`, `:analyze`, `:messages` and the
/// listings (`:ls`, `:alias`, `:rules`, `:scriptnames`, `:themes`), one
/// holder since the listings became pages (V5.1). `:analyze` was the one
/// that built its own `Paragraph` and so drew no cursor. The DECISION pane
/// is the fourth holder and draws its own frame (a pinned legend and a fold
/// marker), but its rows come from the same [`page_rows`] and its cursor
/// from the same [`page_cursor`] — one wrap and one cursor for both (S4).
fn draw_text_pane(
    frame: &mut Frame,
    pane: &PaneRect,
    title: &str,
    text: &crate::text_view::TextView,
    paint: crate::paint::Painter<'_>,
) -> Option<Position> {
    let block = pane_block(title, pane, paint.theme());
    let inner = block.inner(pane.rect);
    frame.render_widget(block, pane.rect);
    let rows = page_rows(text, paint, inner.width, inner.height);
    frame.render_widget(Paragraph::new(rows), inner);
    page_cursor(text, inner)
}

/// **Where a page's cursor is**, drawn over `inner`: the cell
/// `TextView::cursor_cell` measures by the rows `visible_rows` draws.
///
/// **Show where the cursor is.** A position only the code knows about is
/// indistinguishable from no position at all: it is what `j`/`k` move and
/// what a yank starts at. The decision pane once inherited no cursor, and
/// with `scrolloff = 5` its `j` moved one nobody could see for five presses
/// (Reviewer A, round 3); a page's own painted cell later fell off a full
/// row's end and under a secret's colours (Reviewer D, F3, F4).
fn page_cursor(text: &crate::text_view::TextView, inner: Rect) -> Option<Position> {
    let at = text.cursor_cell(inner.width, usize::from(inner.height))?;
    cell_in(inner, at)
}

/// **A text page's rows on screen, painted** — `height` of them at `width`,
/// cut by `TextView::visible_rows` (the wrap its measures count by, so no
/// row it counts is undrawn and none it draws uncounted), each row's part
/// of a Visual range reversed (`v`, `V` or `<C-v>`, by the buffer's own
/// projection, `TextView::selected_char_range`), and `~` past the end. Drawn
/// with no `Wrap` and no scroll: ratatui's word wrapper was the second ruler
/// (TODO.md's CRITICAL). The cursor is the terminal's ([`page_cursor`]).
fn page_rows(
    text: &crate::text_view::TextView,
    paint: crate::paint::Painter<'_>,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let selected = text.selected_lines();
    let mut rows: Vec<Line<'static>> = text
        .visible_rows(width, height as usize)
        .into_iter()
        .map(|crate::text_view::PageRow { line, start, row }| {
            let shown = row.text().chars().count();
            let mut painted = paint.line(&row);
            // The line's length only inside the range: a model-written
            // command is one line, and this runs per row.
            if selected.as_ref().is_some_and(|lines| lines.contains(&line)) {
                let len = text.lines()[line].text().chars().count();
                if len == 0 {
                    // An empty line in the range: one cell, as vim draws it.
                    painted = reversed_blank(painted);
                } else if let Some((lo, hi)) = text.selected_char_range(line, len) {
                    let (lo, hi) = (lo.max(start), hi.min(start + shown));
                    if lo < hi {
                        painted = reversed_chars(painted, lo - start, hi - start);
                    }
                }
            }
            painted
        })
        .collect();
    let drawn = rows.len();
    rows.extend(past_the_end(
        (height as usize).saturating_sub(drawn),
        paint.theme(),
    ));
    rows
}

/// **Rows past the end read `~`**, as in vim — so a window scrolled past its
/// end (`zt` on the last lines) or longer than what it shows cannot be mistaken
/// for one whose content simply has blank lines. `rows` of them, appended after
/// the content; a caller that scrolls draws only the ones that land on screen.
///
/// One helper for every pane that keeps a window top — the text pages, the
/// decision pane and `:graph` — so the marker cannot differ between them.
fn past_the_end(rows: usize, theme: &Theme) -> impl Iterator<Item = Line<'static>> {
    let tilde = Line::styled("~", theme.ui(Ink::NonText));
    std::iter::repeat_n(tilde, rows)
}

/// Test seam: [`page_rows`] at `width` × `height`, painted as `:help` is.
#[cfg(test)]
pub(crate) fn page_rows_for_test(
    text: &crate::text_view::TextView,
    width: u16,
    height: u16,
) -> Vec<ratatui::text::Line<'static>> {
    let theme = crate::theme::Theme::terminal();
    page_rows(
        text,
        painter(PaneKind::Page(PageKind::Help), &theme),
        width,
        height,
    )
}

/// `line` with chars `[lo, hi)` reversed — a row's part of a Visual range.
fn reversed_chars(
    line: ratatui::text::Line<'static>,
    lo: usize,
    hi: usize,
) -> ratatui::text::Line<'static> {
    restyle_chars(line, lo, hi, |span| span.add_modifier(Modifier::REVERSED))
}

/// An empty line inside a Visual range: one reversed cell, as vim draws it.
fn reversed_blank(mut line: ratatui::text::Line<'static>) -> ratatui::text::Line<'static> {
    let style = line.style.add_modifier(Modifier::REVERSED);
    line.spans.push(Span::styled(" ".to_string(), style));
    line
}

/// **`line` with chars `[lo, hi)` restyled by `restyle`**, every other char
/// as it was — the one place a painted row is cut by chars, for a selection
/// (the cursor was cut here too, while a page painted its own).
///
/// Split on CHAR boundaries, never byte offsets — `s[..1]` panics the moment
/// a line starts with anything non-ASCII, and these lines carry `↑`, `·` and
/// box-drawing already (`[[feedback_char_boundary_slicing]]`). Chars rather
/// than grapheme clusters: the crate that knows clusters is not a dependency
/// here, and the difference shows only as a base character styled with its
/// combining mark left plain — cosmetic on rare input, against a dependency
/// for a cursor.
fn restyle_chars(
    line: ratatui::text::Line<'static>,
    lo: usize,
    hi: usize,
    restyle: impl Fn(Style) -> Style,
) -> ratatui::text::Line<'static> {
    let ratatui::text::Line {
        spans,
        style,
        alignment,
    } = line;
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + 2);
    let mut at = 0;
    for span in spans {
        let n = span.content.chars().count();
        let (from, to) = (lo.clamp(at, at + n) - at, hi.clamp(at, at + n) - at);
        at += n;
        if from == to {
            out.push(span);
            continue;
        }
        let text: Vec<char> = span.content.chars().collect();
        for (piece, inside) in [
            (&text[..from], false),
            (&text[from..to], true),
            (&text[to..], false),
        ] {
            if !piece.is_empty() {
                let look = if inside {
                    restyle(span.style)
                } else {
                    span.style
                };
                out.push(Span::styled(piece.iter().collect::<String>(), look));
            }
        }
    }
    ratatui::text::Line {
        spans: out,
        style,
        alignment,
    }
}

/// **What a drawn decision pane showed in its body** that the status row
/// would otherwise say: its answer keys (the pinned legend). The command it
/// could not show, its own title carries (`ToolPrompt::question`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecisionShown {
    keys: bool,
}

/// **What the decision's keys are while a selection is up over its report**
/// — said by its legend, or by the status row when the pane has no room for
/// one (`decide_value`).
const SELECTING: &str = "selecting — y copies · o the other end · Esc back to the answers";

/// **What the decision's keys are while a chord is pending on them** (`y`
/// waiting for its object, `z` or `<C-w>` for its command) — said as
/// [`SELECTING`] is.
const CHORD: &str = "finishing a command — Esc back to the answers";

/// **A decision pane, as rows of text in the colours it is drawn in** — what
/// the tutor pictures (`tutor::Pictures`): `page` drawn by [`draw_decision`]
/// itself into a buffer `width` wide and as tall as the pane opens (its
/// content rows, capped at `max_rows` as `tui.approval_max_height` caps it,
/// and its border), then read back cell by cell. Not focused — the pane
/// takes no cursor when it opens — its number `number`, its keys live.
/// The picture IS the pane: a picture typed beside it is a second statement
/// of it, wrong the day either changes.
pub(crate) fn decision_picture(
    page: &dyn crate::decision_page::DecisionPage,
    width: u16,
    max_rows: u16,
    number: usize,
    theme: &crate::theme::Theme,
    aliases: &shannon_core::Aliases,
) -> Vec<crate::paint::PageLine> {
    let rows = page
        .content_rows(width.saturating_sub(2))
        .min(max_rows)
        .saturating_add(2);
    // A buffer, not a terminal: its backend cannot fail.
    let backend = ratatui::backend::TestBackend::new(width, rows);
    let Ok(mut terminal) = ratatui::Terminal::new(backend);
    let Ok(_) = terminal.draw(|frame| {
        let pane = PaneRect {
            id: crate::wm::PaneId(0),
            kind: PaneKind::Tool,
            rect: frame.area(),
            focused: false,
            number,
        };
        let paint = painter(PaneKind::Tool, theme);
        draw_decision(frame, &pane, page, AnswerKeys::Live, paint, aliases);
        // And numbered, as the frame numbers every pane after drawing it.
        draw_pane_number(frame, &pane, theme);
    });
    let buffer = terminal.backend().buffer();
    (0..rows)
        .map(|y| {
            let mut runs: Vec<crate::paint::Run> = Vec::new();
            for x in 0..width {
                let cell = &buffer[(x, y)];
                match runs.last_mut() {
                    Some(run) if run.look == crate::paint::Look::Own(cell.style()) => {
                        run.text.push_str(cell.symbol());
                    }
                    _ => runs.push(crate::paint::Run::own(cell.symbol(), cell.style())),
                }
            }
            crate::paint::PageLine {
                runs,
                style: Style::default(),
            }
        })
        .collect()
}

/// **[`SELECTING`] or [`CHORD`] as the legend draws it**: quiet, as keys
/// that answer nothing are, but the keys that DO work there drawn as keys
/// (`paint::hint`) — what precedes the ` — ` is the lead-in.
fn keys_said(said: &str) -> crate::paint::PageLine {
    let quiet = CellStyle::ink(Ink::TextQuiet);
    let (lead, items) = said.split_at(keys_start(said));
    let mut runs = vec![crate::paint::Run::role(lead, quiet)];
    runs.retain(|run| !run.text.is_empty());
    runs.extend(crate::paint::hint(items, quiet));
    crate::paint::PageLine {
        runs,
        style: Style::default(),
    }
}

/// Where the keys start in what a decision says of them: past its lead-in,
/// the ` — `, where it has one.
fn keys_start(said: &str) -> usize {
    said.find(" — ").map_or(0, |at| at + " — ".len())
}

/// **A decision's page, drawn** (`decision_page::DecisionPage`) — its body,
/// its legend pinned beneath it, the fold marker on a row of its own —
/// whichever kind of decision it is. Its title is the kind's own, asked of
/// the body's area once it is laid out: a call's carries its command only
/// when the body cannot show it.
///
/// `keys`: what its answer keys are right now (`App::answer_keys`). Under a
/// selection or a chord they are dead, and the legend says what they are
/// instead of offering them; during the approval wait they are the legend's
/// own, dimmed — a key that looks live and does not answer is the door
/// (review stop 6, F 4; Firefox greys its dialog's buttons for the same
/// wait).
fn draw_decision(
    frame: &mut Frame,
    pane: &PaneRect,
    page: &dyn crate::decision_page::DecisionPage,
    keys: AnswerKeys,
    paint: crate::paint::Painter<'_>,
    aliases: &shannon_core::Aliases,
) -> (DecisionShown, Option<Position>) {
    let leaked = page.leaked();
    let area = pane.rect;
    // The body is laid out before the title is chosen: the title carries the
    // command only when the body cannot (`ToolPrompt::question`). The title
    // sits on the border, so the inside does not wait on it — read off the
    // pane's own block, not a second statement of its border (review of
    // 716f4f9, C-L8).
    let inner = pane_block("", pane, paint.theme()).inner(area);

    // The action line is a LEGEND, not content: render it as a fixed last row so
    // a long report can scroll underneath it without ever hiding the keys that
    // resolve the decision.
    let (body_area, legend_area) = match page.legend() {
        Some(_) if inner.height > 1 => (
            Rect {
                height: inner.height - 1,
                ..inner
            },
            Some(Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            }),
        ),
        _ => (inner, None),
    };
    // **The fold marker gets its own row, reserved BEFORE the body is drawn.**
    // It used to be painted over the right-hand end of the body's last row, so
    // on a command that wrapped, ` | head -50` was replaced *in place* by
    // `↓ 3 more` — no ellipsis, no seam — and the line rendered as a
    // syntactically complete pipeline ending at `sort -u`. On the one surface
    // whose entire purpose is that you read the exact text before it runs
    // (Reviewer F, read off the cell grid).
    //
    // Whether a row is reserved is `body_rows`' answer — the same one every
    // clamp sizes the report by — so the view and the draw agree on how many
    // rows the report has.
    let reserved = page.body_rows(inner.height, inner.width) < body_area.height;
    let (body_area, fold_area) = split_fold_row(body_area, page.text(), reserved);
    // **Its keys are shown when every answer is** — a legend row as wide as
    // its answers run (`DecisionPage::answers_width`); one cut before an
    // answer hands the keys to the status row (review stop 8, F 3: the
    // review's `d drop`, cut in the `:graph` layout, was on no screen).
    let shown = DecisionShown {
        keys: legend_area.is_some_and(|rect| usize::from(rect.width) >= page.answers_width()),
    };
    let title = page.title(body_area.height, body_area.width);
    frame.render_widget(pane_block(&title, pane, paint.theme()), area);

    let rows = page_rows(page.text(), paint, body_area.width, body_area.height);
    let rows = page_secrets(rows, page.text(), body_area, aliases, leaked, paint.theme());
    frame.render_widget(Paragraph::new(rows), body_area);
    if let (Some(rect), Some(line)) = (legend_area, page.legend()) {
        let quiet = paint.theme().ui(Ink::TextQuiet);
        let legend = match keys {
            AnswerKeys::Live => paint.line(line),
            // Each key's own style kept under the dimming: the strike through
            // an `e edit` that cannot apply is part of what the legend says.
            AnswerKeys::Waiting => {
                let mut dimmed = paint.line(line);
                dimmed.style = dimmed.style.patch(quiet);
                for span in &mut dimmed.spans {
                    span.style = span.style.patch(quiet);
                }
                dimmed
            }
            AnswerKeys::Selecting => paint.line(&keys_said(SELECTING)),
            AnswerKeys::Chord => paint.line(&keys_said(CHORD)),
        };
        frame.render_widget(Paragraph::new(legend), rect);
    }
    // **Say when the report does not all fit.** Sizing it correctly is not
    // enough: `tui.approval_max_height` can still cap a genuinely long report,
    // and a clipped one is otherwise indistinguishable from a complete one —
    // full border, pinned legend, no marker. What falls off the bottom is
    // *which stage is dangerous*, because the body is ordered verdict-headline
    // first (Reviewer F). The border must stop being able to say "complete"
    // about something that isn't.
    if let Some(rect) = fold_area {
        fold_marker(frame, rect, body_area, page.text());
    }
    (shown, page_cursor(page.text(), body_area))
}

/// **A decision page's rows with every value you marked secret drawn as
/// one** — [`row_secrets`]' rule, its fourth consumer: the call's pane drew a
/// marked value as plain text while the prompt drew it `ui.secret`, on the
/// one surface where a line holding it is approved (review stop 8, E2 5).
/// `leaked`: every held value counts as one the model wrote itself (a Claude
/// Code call's line: no handle of it was put back). Over the visible rows
/// only, as `page_rows` drew them — their own styles kept beneath.
fn page_secrets(
    rows: Vec<Line<'static>>,
    text: &crate::text_view::TextView,
    area: Rect,
    aliases: &shannon_core::Aliases,
    leaked: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if aliases.secrets().next().is_none() {
        return rows;
    }
    let visible = text.visible_rows(area.width, area.height as usize);
    let texts: Vec<String> = visible.iter().map(|r| r.row.text()).collect();
    let origins: Vec<(usize, usize)> = visible.iter().map(|r| (r.line, r.start)).collect();
    let spans = row_secrets(
        aliases,
        &texts,
        &origins,
        |n| text.lines().get(n).map(|l| Cow::Owned(l.text())),
        |_| leaked.then(Vec::new),
    );
    (rows.into_iter().enumerate())
        .map(|(i, line)| match spans.get(i) {
            Some(secrets) if !secrets.is_empty() => {
                let base = line.style;
                let styles: Vec<Style> = (line.spans.iter())
                    .flat_map(|s| s.content.chars().map(move |_| base.patch(s.style)))
                    .collect();
                let painted: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                overlay_secrets(
                    &painted,
                    secrets,
                    |c| styles.get(c).copied().unwrap_or(base),
                    theme,
                )
            }
            _ => line,
        })
        .collect()
}

/// Split a scrollable pane's area into `(body, the fold marker's own row)`.
///
/// `reserved` is whether the report does not fit — `DecisionPage::body_rows`'
/// answer, which is what the report's `TextView` is sized by. Returns `None`
/// for the marker when everything fits and nothing is scrolled off the top —
/// the common case, where the body keeps every row.
///
/// **A row, not an overlay.** The marker is chrome and the body is the command
/// a human is about to authorise; chrome that overwrites content on the
/// decision pane deletes part of what is being approved, and does it invisibly
/// because the remaining text is still valid shell. Shrinking the body can only
/// push MORE content below the fold, never less, so the marker cannot talk
/// itself out of existence.
fn split_fold_row(
    area: Rect,
    text: &crate::text_view::TextView,
    reserved: bool,
) -> (Rect, Option<Rect>) {
    let (min_rows, min_cols) = crate::decision_page::FOLD_MIN;
    let top = text.top_row(area.width);
    // A report that fits but was scrolled past its end (`zt`) has lines above
    // and none below: it gets the marker too, drawn at the top. Its view was
    // sized WITHOUT the row, which costs nothing — every line it still has is
    // above the rows the marker takes.
    if !(reserved || top > 0) || area.height < min_rows || area.width < min_cols {
        return (area, None);
    }
    // Below the fold: the marker sits at the bottom, where the eye leaves the
    // body. Nothing below but something above: it sits at the top, for the
    // same reason. "Below" is measured against the body that REMAINS once the
    // marker has its row — against the whole area, one row left below read as
    // none, so "↓ 1 more" (`fold_marker` counts correctly) was drawn ABOVE the
    // text it points past.
    let at_bottom = text.rows_at(area.width) > top + (area.height - 1) as usize;
    let body = Rect {
        y: if at_bottom { area.y } else { area.y + 1 },
        height: area.height - 1,
        ..area
    };
    let row = Rect {
        y: if at_bottom {
            area.y + area.height - 1
        } else {
            area.y
        },
        height: 1,
        ..area
    };
    (body, Some(row))
}

/// Draw `↓ N more` / `↑ N above` at the right-hand end of a scrollable pane's
/// last/first row, when there is content outside it.
///
/// Right-aligned and dim so it reads as chrome rather than as report content —
/// the one thing it must not do is look like part of the command's analysis.
fn fold_marker(frame: &mut Frame, row: Rect, body: Rect, text: &crate::text_view::TextView) {
    // Counted against the BODY, which is what is actually shown — the row this
    // draws into is no longer part of it.
    let total = text.rows_at(body.width);
    let top = text.top_row(body.width);
    let below = total.saturating_sub(top + body.height as usize);
    let marker = if below > 0 {
        format!("↓ {below} more ")
    } else if top > 0 {
        format!("↑ {top} above ")
    } else {
        return;
    };
    let w = saturating_u16(crate::wrap::str_cells(&marker)).min(row.width);
    let rect = Rect {
        x: row.x + row.width - w,
        y: row.y,
        width: w,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(marker).style(Style::default().add_modifier(Modifier::DIM)),
        rect,
    );
}

/// `tree` style: a fold triangle (▾ expanded / ▸ collapsed / ▹ leaf) then the
/// precomputed tree label.
fn tree_row_spans(row: &crate::graph_view::DisplayRow<'_>) -> Vec<Span<'static>> {
    let marker = match row.fold {
        Some(true) => "▾ ",
        Some(false) => "▸ ",
        None => "▹ ",
    };
    vec![Span::raw(format!("{marker}{}", row.label))]
}

/// `git` style: the branch-lane graph (persistent per-branch columns with fork
/// connectors, `●` commit dots, colored per lane) then a `git log --oneline
/// --decorate`-flavored label — short id, `(HEAD …)`/branch decoration, a
/// one-line content summary. A collapsed node trails a dim `⋯`.
fn git_row_spans(
    view: &GraphView,
    row: &crate::graph_view::DisplayRow<'_>,
    chars: GraphChars,
    ids: GraphIds,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut spans: Vec<Span> = row
        .lane
        .iter()
        .map(|c| Span::styled(graph_glyph(c.ch, chars).to_string(), lane(c.color, theme)))
        .collect();
    spans.push(Span::raw(" "));
    if let Some(node) = row.entry.node() {
        spans.extend(git_label_spans(view.graph(), view.active(), node, ids));
    } else if let Some(turn) = view.claude_turn(row.entry) {
        // A Claude Code turn: its prompt's opening words, and the model
        // that answered, dim — as a Shannon turn's row reads. Its dot is
        // on the lane.
        spans.push(Span::raw(snippet(&turn.prompt, 48)));
        if let Some(model) = &turn.model {
            spans.push(Span::styled(format!(" · {model}"), dim));
        }
    }
    if row.fold == Some(false) {
        spans.push(Span::styled(more_mark(chars), dim));
    }
    spans
}

/// Map a lane glyph to the configured character set. `Unicode` passes through;
/// `Ascii` falls back to `git log --graph`-style plain characters for terminals
/// or fonts without dependable box-drawing.
fn graph_glyph(c: char, chars: GraphChars) -> char {
    match chars {
        GraphChars::Unicode => c,
        GraphChars::Ascii => match c {
            '●' | '○' => '*',
            '│' => '|',
            '─' => '-',
            '├' => '+',
            '╰' | '└' => '\\',
            _ => c,
        },
    }
}

/// A lane's style: its slot's role (`ui.graph.lane.N`), cycled. The shipped
/// lanes avoid gray (the unfocused border colour) and stay readable on a
/// dark background.
fn lane(slot: usize, theme: &Theme) -> Style {
    theme.ui(Ink::LANES[slot % PALETTE_LEN])
}

/// The `git`-style label spans for one node: the session's name (else its
/// short id — [`session_heading`](crate::graph_view::session_heading)) + dim
/// decoration for an episode, short id + content summary for a turn, who
/// spoke ([`speaker`](crate::graph_view::speaker)) + preview for a message.
fn git_label_spans(
    graph: &Graph,
    active: Option<EpisodeId>,
    node: Node,
    ids: GraphIds,
) -> Vec<Span<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    match node {
        Node::Episode(id) => {
            // The name leads, the id after it, dim (Steven, 10-14: "show the
            // human readable name first"). It was a ref in the decoration,
            // `(HEAD -> name)`, as git names a branch — read last, after an id
            // nobody remembers.
            let (lead, after) = crate::graph_view::session_heading(graph, id, ids);
            let mut spans = vec![Span::raw(lead)];
            if let Some(hex) = after {
                spans.push(Span::styled(format!(" {hex}"), dim));
            }
            // git-style decoration: refs in parens — HEAD, then the branch
            // origin.
            let mut refs: Vec<String> = Vec::new();
            if active == Some(id) {
                refs.push("HEAD".to_string());
            }
            match graph.episode(&id).and_then(|ep| ep.origin) {
                None => refs.push(crate::graph_view::root_word(graph.episode(&id))),
                // The decoration names the RELATIONSHIP for the notable kinds;
                // a plain branch stays bare `←` (it's the default relation, and
                // git decorations reward brevity). A child must be tellable
                // from a branch in this style too — the head dot plus this.
                Some(p) => {
                    // A plain branch stays bare `←` (the default relation; git
                    // decorations reward brevity); the notable kinds name
                    // themselves via the shared label.
                    let kind = if p.kind == EdgeKind::Branch {
                        String::new()
                    } else {
                        format!("{} ", p.kind.label())
                    };
                    refs.push(format!("{kind}← {}", p.turn.short_hex()));
                }
            }
            spans.push(Span::styled(format!("  ({})", refs.join(", ")), dim));
            spans
        }
        // Labels read the outline, never the body: a graph pane draws from the
        // structure alone, so `:graph` over a long history loads no messages.
        Node::Turn { turn, .. } => {
            let mut spans = vec![Span::raw(turn.short_hex())];
            if let Some(text) = graph.outline(&turn).and_then(first_user_snippet) {
                spans.push(Span::raw(format!("  {text}")));
            }
            spans
        }
        Node::Message { turn, index, .. } => {
            let msg = graph.outline(&turn).and_then(|m| m.get(index));
            let label = match msg {
                Some(m) => match crate::graph_view::speaker(m) {
                    Some(who) => format!("{who}: {}", snippet(&m.first_line, 56)),
                    None => snippet(&m.first_line, 56),
                },
                None => "?".to_string(),
            };
            vec![Span::styled(label, dim)]
        }
    }
}

/// One-line summary of a turn for the `git` style: its first user message,
/// trimmed to a readable width.
fn first_user_snippet(outline: &[shannon_core::MessageOutline]) -> Option<String> {
    outline
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| snippet(&m.first_line, 48))
}

/// First non-empty line of `s`, collapsed to a single line and truncated to
/// `max` display chars with an ellipsis. Empty input yields `""`. The one
/// rule for what a row shows of a text, in either style.
pub(crate) fn snippet(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Test seam: [`inspector_body_with`] painting afresh, for the tests that
/// read one page.
#[cfg(test)]
pub(crate) fn inspector_body(
    view: &GraphView,
    stub_only: bool,
    time_format: &str,
) -> Vec<crate::paint::PageLine> {
    inspector_body_with(view, stub_only, time_format, &mut PaintedMessage::default())
}

/// **A message's painted words, and which message they are** — kept by the
/// inspector's pane, so a selected message is painted once, not each frame.
///
/// The key is complete, which is what earns it a cache where the rest of the
/// page is re-derived and compared (`App::refresh_inspector`): a turn is
/// content-addressed, so `(turn, index)` IS the message's words, and the
/// paint is roles, themed at the draw — nothing else goes into it. Painting
/// runs syntect over every fence, ~120 ms on a long reply; done per frame it
/// took every pane's frame rate with it (review of 34033e8, Reviewer B).
#[derive(Debug, Default)]
pub(crate) struct PaintedMessage {
    key: Option<PaintKey>,
    lines: Vec<crate::paint::PageLine>,
}

impl PaintedMessage {
    /// **Let go of a Claude Code turn's words, if held** — `:unread` takes
    /// back the leave they were read under, and the page kept them, unshown
    /// (review stop 5, E1 F5). Whether it held one. A page still allowed
    /// paints them again on its next frame.
    pub(crate) fn forget_claude(&mut self) -> bool {
        let held = self.holds_claude();
        if held {
            *self = PaintedMessage::default();
        }
        held
    }

    /// Whether it holds a Claude Code turn's words.
    pub(crate) fn holds_claude(&self) -> bool {
        matches!(self.key, Some(PaintKey::Claude { .. }))
    }
}

/// **Which words a [`PaintedMessage`] holds** — a Shannon message by turn and
/// place, which IS its words (a turn is content-addressed); a Claude Code
/// turn by the words themselves, its prompt and reply. Its place was no
/// key: a conversation still being written, read again, has new words at
/// the same place, and a secret marked since re-scrubs them — the page
/// went on showing the old, the masked value with it (review stop 3, A F6,
/// B 3, C 2). Comparing the words is a memcmp a frame; painting them,
/// syntect.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PaintKey {
    Message(shannon_core::TurnId, usize),
    Claude { prompt: String, reply: String },
}

/// The internals inspector's body for a graph view's current selection — the
/// "what produced this / what's in scope" panel — a message's words painted
/// through `painted`, the pane's own ([`PaintedMessage`]).
///
/// Split out from the draw so the app can hold it in a
/// [`TextView`](crate::text_view::TextView) and page it. It used to be built
/// inside the renderer and handed straight to a `Paragraph` with **no scroll
/// offset**, so a selected `Node::Message` whose body ran past the last row was
/// silently cut — with `j`, `k`, `gg`, `G` and `q` all dead, since
/// `focused_viewer` did not know the pane existed. A pane that looks like its
/// three scrollable siblings, draws a border like them, and answers none of
/// their keys is the calibration set's Norman door exactly (Reviewer F).
/// `stub_only` is `App::only_stub` — whether this process has nothing but the
/// canned stub to answer with. Passed in rather than read from an `App`,
/// because that is all this function needs of one, and it is the same fact
/// `App::answering_model` consults: the inspector sits BESIDE `:graph` and was
/// naming a model that could not answer (Reviewers A and C).
pub(crate) fn inspector_body_with(
    view: &GraphView,
    stub_only: bool,
    time_format: &str,
    painted: &mut PaintedMessage,
) -> Vec<crate::paint::PageLine> {
    // The Claude Code section's rows are not nodes: their own page — a
    // turn's (its words scrubbed as they were read, `Scrubber::imported`),
    // or the conversation's.
    if view.on_claude() {
        if let Some(turn) = view.selected_claude_turn() {
            return claude_turn_page(turn, painted);
        }
        let listed = view.claude().is_none_or(|c| c.listed);
        return claude_inspector(view.selected_claude(), listed)
            .into_iter()
            .map(Into::into)
            .collect();
    }
    // Activity is job state (the view's), not graph data — resolved here so
    // `inspector_lines` stays a pure function of the snapshot.
    //
    // The ACTIVITY, not just its glyph. `inspector_lines` writes the word as
    // well as the marker and wrote "running" unconditionally, so a held child
    // read `‖ running` — the held marker beside the working word. The tree row
    // was fixed for exactly this and the inspector got only the glyph half
    // (Reviewer A).
    let activity = match view.selected_node() {
        Some(Node::Episode(id)) => view.activity_of(&id),
        _ => None,
    };
    inspector_lines(
        view.graph(),
        view.selected_node(),
        activity.map(|a| (view.glyphs().for_activity(a), a.word())),
        stub_only,
        time_format,
        painted,
    )
}

/// **The inspector on one of a Claude Code conversation's turns** — which
/// conversation and where in it, the model that answered, the calls it made,
/// whether a compaction's summary opened it; then its prompt and its reply,
/// painted once ([`PaintKey::Claude`]) — scrubbed already, as every reading
/// a view holds is (`Scrubber::imported`).
fn claude_turn_page(
    (found, at, total, turn): (
        &shannon_core::claude_import::Found,
        usize,
        usize,
        &shannon_core::claude_import::ImportedTurn,
    ),
    painted: &mut PaintedMessage,
) -> Vec<crate::paint::PageLine> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let (id, title) = crate::graph_view::conversation_name(found);
    let mut lines = vec![
        insp_heading("Claude Code turn"),
        insp_field("conversation", format!("{id}  {title}")),
        insp_field("turn", format!("{}/{total}", at + 1)),
    ];
    if let Some(model) = &turn.model {
        lines.push(insp_field("model", model.clone()));
    }
    let calls: usize = turn.rounds.iter().map(|r| r.calls.len()).sum();
    lines.push(insp_field(
        "tools",
        format!(
            "{} in {}",
            crate::view::counted(calls, "call", "calls"),
            crate::view::counted(turn.rounds.len(), "round", "rounds")
        ),
    ));
    if turn.summary {
        lines.push(insp_field(
            "opened by",
            "Claude Code's summary, where it compacted",
        ));
    }
    if let Some(uuid) = &turn.uuid {
        lines.push(insp_field("entry", uuid.clone()));
    }
    lines.push(Line::from(Span::styled(
        "Claude Code's, read and never changed — <Enter> imports it up to this turn.",
        dim,
    )));
    lines.push(Line::from(""));
    let mut out: Vec<crate::paint::PageLine> = lines.into_iter().map(Into::into).collect();
    let painted_now = matches!(
        &painted.key,
        Some(PaintKey::Claude { prompt, reply }) if *prompt == turn.prompt && *reply == turn.reply
    );
    if !painted_now {
        let mut words = vec![crate::paint::PageLine::from(insp_heading("Prompt"))];
        words.extend(message_body(Role::User, &turn.prompt));
        words.push(Line::from("").into());
        words.push(insp_heading("Reply").into());
        words.extend(message_body(Role::Assistant, &turn.reply));
        painted.lines = words;
        painted.key = Some(PaintKey::Claude {
            prompt: turn.prompt.clone(),
            reply: turn.reply.clone(),
        });
    }
    out.extend(painted.lines.iter().cloned());
    out
}

/// The inspector on `:graph`'s Claude Code section: one conversation — what
/// it is, where it lives, and what `<Enter>` does with it — or, on the
/// heading, what the section is.
fn claude_inspector(
    found: Option<&shannon_core::claude_import::Found>,
    listed: bool,
) -> Vec<Line<'static>> {
    // Not read yet: what it is, and what each answer to `l`'s question does.
    if !listed {
        let mut out = vec![insp_heading("Claude Code"), Line::from("")];
        out.extend(
            [
                "Claude Code's own conversations are another program's records:",
                "Shannon reads them only where you say it may. `l` here asks —",
                "`y` reads them this run, `a` always (for this directory), `n`",
                "not. `:import <id>` reads the one you name.",
            ]
            .map(Line::from),
        );
        return out;
    }
    let what = [
        "Claude Code's own conversations, read and never changed: Shannon",
        "does not delete them. <Enter> imports one as a Shannon session — a",
        "copy, from its latest compaction (`:import --all <id>` reads all of",
        "it); on one a session goes on with (⧓), it opens that session; on",
        "a turn, it imports up to that turn. Only this directory's are",
        "listed; `:import <id>` reads another's. One Shannon itself ran in",
        "Claude Code is not listed: it is in the tree above.",
    ];
    let mut out = Vec::new();
    match found {
        Some(found) => {
            out.push(insp_heading("Claude Code conversation"));
            out.push(insp_field("id", found.id.clone()));
            if let Some(title) = &found.title {
                out.push(insp_field("title", title.clone()));
            }
            if let Some(dir) = found.path.parent().and_then(|p| p.file_name()) {
                out.push(insp_field("from", dir.to_string_lossy().into_owned()));
            }
            if let Some(then) = found.modified {
                out.push(insp_field(
                    "written",
                    crate::graph_view::ago(std::time::SystemTime::now(), then),
                ));
            }
            out.push(insp_field(
                "size",
                format!("{} KB", found.bytes.div_ceil(1024)),
            ));
        }
        None => out.push(insp_heading("Claude Code")),
    }
    out.push(Line::from(""));
    out.extend(what.into_iter().map(Line::from));
    out
}

/// **What a session's page says of Claude Code** — the conversation an
/// import continues and how much came in; an import from before imports
/// said; or, for one born here from such a session (a clone, a branch), the
/// turns it holds that came in, and through which session — the one answer
/// `:delete all` asks by (`app::from_claude`), so the page says what
/// `shannon-only` does with it. `None` for a session with nothing of
/// Claude Code's.
fn claude_field(graph: &Graph, id: EpisodeId) -> Option<String> {
    use crate::app::FromClaude;
    let turns = |n: usize| counted(n, "turn", "turns");
    Some(match crate::app::from_claude(graph, &id)? {
        FromClaude::Continues(record) => format!(
            "continues {} — {} came in",
            shannon_core::short_hex(&record.session),
            turns(record.turns)
        ),
        FromClaude::Unrecorded => "continues one — imported before imports said which".to_string(),
        FromClaude::Holds { imported, through } => format!(
            "born here (`shannon-only` deletes it), holding {} that came in {}",
            turns(imported),
            match through {
                Some(from) => format!("through {}", session_tag_in(graph, from)),
                None => "through a session now gone".to_string(),
            }
        ),
    })
}

/// `episode` as a sentence names it ([`Label::tag`](shannon_core::Label::tag))
/// from the snapshot the page is drawn from — the name the tree row shows,
/// and the id.
pub(crate) fn session_tag_in(graph: &Graph, episode: EpisodeId) -> String {
    graph
        .episode(&episode)
        .map_or_else(|| episode.short_hex(), |e| e.label.tag(episode))
}

/// A bold section heading line for the inspector.
fn insp_heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// A `key  value` line: the key right-aligned and dimmed, the value plain.
fn insp_field(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:>9}  "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw(value.into()),
    ])
}

/// Build the inspector body for `node` (or a placeholder when nothing is
/// selected). Episodes show id/origin/config/turn-count; turns show
/// id/provenance/message roster; messages show role/index/length then content.
/// The tools / MCPs rows are an **honest placeholder** until the tool surface
/// (§7) lands — they report "none in scope yet" rather than inventing data.
fn inspector_lines(
    graph: &Graph,
    node: Option<Node>,
    activity: Option<(char, &'static str)>,
    stub_only: bool,
    time_format: &str,
    painted: &mut PaintedMessage,
) -> Vec<crate::paint::PageLine> {
    let Some(node) = node else {
        return vec![Line::from("(nothing selected)").into()];
    };
    // The fields are the app's own look; a message's words are painted as the
    // transcript paints them (`message_body`).
    let page = |lines: Vec<Line<'static>>| -> Vec<crate::paint::PageLine> {
        lines.into_iter().map(Into::into).collect()
    };
    match node {
        Node::Episode(id) => {
            let mut lines = vec![insp_heading("Episode")];
            // What a person called it, first — above the id, which nobody
            // reads first (Steven, 10-14: "put the name and description at the
            // top, above ID") — and only when they did ("only shown when they
            // have values").
            let label = graph.episode(&id).map(|ep| &ep.label);
            if let Some(name) = label.and_then(|l| l.name.as_ref()) {
                lines.push(insp_field("name", name.clone()));
            }
            if let Some(about) = label.and_then(|l| l.description.as_ref()) {
                lines.push(insp_field("about", about.clone()));
            }
            lines.push(insp_field("id", id.to_hex()));
            match graph.episode(&id) {
                Some(ep) => {
                    let origin = ep.origin_clause().unwrap_or_else(|| "root".to_string());
                    lines.push(insp_field("origin", origin));
                    if let Some(from) = claude_field(graph, id) {
                        lines.push(insp_field("Claude Code", from));
                    }
                    if let Some((mark, word)) = activity {
                        lines.push(insp_field("state", format!("{mark} {word}")));
                    }
                    // The ancestry-tree facts child sessions live by ([agents]
                    // guardrails count exactly these) — shown only when nonzero
                    // so ordinary sessions stay uncluttered.
                    let spawns = graph.child_depth(&id);
                    if spawns > 0 {
                        lines.push(insp_field("depth", format!("{spawns} spawns deep")));
                    }
                    let kids = graph.child_sessions(&id).len();
                    if kids > 0 {
                        lines.push(insp_field(
                            "children",
                            format!("{kids} child session{}", if kids == 1 { "" } else { "s" }),
                        ));
                    }
                    // `answering_model`, not the config: this pane sits BESIDE
                    // `:graph` and was saying `haiku` while the stub answered.
                    lines.push(insp_field(
                        "model",
                        crate::app::model_answering(&ep.config.model, stub_only).to_string(),
                    ));
                    lines.push(insp_field("max tok", opt_num(ep.config.max_tokens)));
                    // The session's own effort, which `:effort` sets and its
                    // header keeps — `default` follows `[turn] effort`.
                    let effort = ep.config.effort.map_or("default", |e| e.as_str());
                    lines.push(insp_field("effort", effort.to_string()));
                    lines.push(insp_field("turns", graph.full_turns(&id).len().to_string()));
                    // What this session and everything it spawned cost — real
                    // recorded tokens (own turns only; shared history counts
                    // once, in its owner). Shown when anything was recorded.
                    let spent = subtree_usage(graph, id);
                    if spent != Usage::default() {
                        lines.push(insp_field("usage", usage_str(Some(spent))));
                    }
                }
                None => lines.push(insp_field("", "(not in graph)")),
            }
            lines.push(Line::from(""));
            lines.extend(capability_lines());
            page(lines)
        }
        Node::Turn {
            episode,
            turn,
            index,
        } => {
            let mut lines = vec![insp_heading("Turn")];
            lines.push(insp_field("session", session_tag_in(graph, episode)));
            // The node's own occurrence index (not a content search), so a repeated
            // turn shows its true position in the full history.
            let placed = graph.placed(&episode);
            let n = placed.len();
            if index < n {
                lines.push(insp_field("turn", format!("{}/{}", index + 1, n)));
            }
            // Content-addressed: an identical turn is one turn, shared across
            // sessions. Surface that so its recurrence in the tree isn't a mystery.
            let shared = graph.episodes_with_turn(&turn);
            if shared > 1 {
                lines.push(insp_field("shared", format!("in {shared} sessions")));
            }
            lines.push(insp_field("id", turn.to_hex()));
            // THIS occurrence's facts: identical turns share content, not when
            // they ran or what they cost. Read from the placement and the
            // outline, so inspecting a turn loads no words.
            let here = placed.get(index).filter(|p| p.turn == turn);
            match here.zip(graph.outline(&turn)) {
                Some((placement, outline)) => {
                    let p = placement.provenance;
                    lines.push(insp_field("model", p.config.model.clone()));
                    lines.push(insp_field("max tok", opt_num(p.config.max_tokens)));
                    // The effort it was SENT — `—` for none, the model's own.
                    let effort = p.config.effort.map_or("—".to_string(), |e| e.to_string());
                    lines.push(insp_field("effort", effort));
                    lines.push(insp_field("tokens", usage_str(p.usage)));
                    // Why the turn ended — a truncated (`max_tokens`) or
                    // interrupted turn must be tellable from a complete one;
                    // `:continue` picks those up. Old turns recorded nothing.
                    if let Some(reason) = &p.stop_reason {
                        let label = if reason.is_continuable() {
                            format!("{reason} (:continue resumes)")
                        } else {
                            reason.to_string()
                        };
                        lines.push(insp_field("stopped", label));
                    }
                    // As `tui.time_format` says — the locale's, by default;
                    // what is saved is the unix time (Steven, 10-14).
                    lines.push(insp_field(
                        "time",
                        shannon_core::when::shown(p.at, time_format),
                    ));
                    lines.push(insp_field(
                        "account",
                        p.account.clone().unwrap_or_else(|| "—".into()),
                    ));
                    lines.push(insp_field("msgs", roster(outline.iter().map(|m| m.role))));
                    // A squash summary names what it stands in for — the
                    // distilled turns stay resolvable (squash preserves the
                    // originals), so their short ids are a working trailhead.
                    if !p.distilled_from.is_empty() {
                        let ids: Vec<String> =
                            p.distilled_from.iter().map(|id| id.short_hex()).collect();
                        lines.push(insp_field("distills", distilled(&ids)));
                    }
                }
                None => lines.push(insp_field("", "(not in graph)")),
            }
            lines.push(Line::from(""));
            lines.extend(capability_lines());
            page(lines)
        }
        Node::Message {
            episode,
            turn,
            turn_index,
            index,
        } => {
            let mut lines = vec![insp_heading("Message")];
            lines.push(insp_field("session", session_tag_in(graph, episode)));
            // The parent turn's own occurrence index (not a content search).
            let n = graph.full_turns(&episode).len();
            if turn_index < n {
                lines.push(insp_field("turn", format!("{}/{}", turn_index + 1, n)));
            }
            // The fields from the outline; the words from the body, which the
            // app loads for this selection (`App::load_inspected_body`).
            match graph.outline(&turn).and_then(|m| m.get(index)) {
                Some(m) => {
                    let role = match m.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    };
                    lines.push(insp_field("role", role));
                    lines.push(insp_field("index", index.to_string()));
                    lines.push(insp_field(
                        "length",
                        format!("{} chars · ~{} tok", m.chars, est_tokens(m.chars)),
                    ));
                    lines.push(Line::from(""));
                    let dim = Style::default().add_modifier(Modifier::DIM);
                    match graph.messages(&turn).and_then(|body| body.get(index)) {
                        Some(body) if body.content.is_empty() => {
                            lines.push(Line::from(Span::styled("(empty)", dim)));
                        }
                        // The content itself, painted as the transcript paints
                        // it (the page wraps long lines).
                        Some(body) => {
                            let mut out = page(lines);
                            // Painted once per message (`PaintedMessage`).
                            let key = PaintKey::Message(turn, index);
                            if painted.key.as_ref() != Some(&key) {
                                painted.lines = message_body(m.role, &body.content);
                                painted.key = Some(key);
                            }
                            out.extend(painted.lines.iter().cloned());
                            return out;
                        }
                        None => lines.push(Line::from(Span::styled(
                            "(this message's text could not be loaded from the store)",
                            dim,
                        ))),
                    }
                }
                None => lines.push(insp_field("", "(not in graph)")),
            }
            page(lines)
        }
    }
}

/// **A message's words, painted as the transcript paints them** (Steven,
/// 10-14: "add coloring to the messages in inspect. Same as the
/// conversation, so they can have the same primitives"): an assistant's
/// through the markdown and syntax styles (`markdown::style_lines_with`),
/// themed at draw time; a user's dim, as their echo is. Sanitized as the
/// transcript's are (`output::sanitize_lines`): the words are the model's
/// and the store's, and a control byte in them is not the terminal's.
fn message_body(role: Role, content: &str) -> Vec<crate::paint::PageLine> {
    let lines = crate::output::sanitize_lines(content);
    match role {
        Role::Assistant => {
            let styles =
                crate::markdown::style_lines_with(&lines, crate::markdown::Highlight::Syntax);
            lines
                .iter()
                .zip(styles)
                .map(|(line, styles)| crate::paint::PageLine::of_cells(line, &styles))
                .collect()
        }
        Role::User => lines
            .into_iter()
            .map(|line| {
                Line::from(Span::styled(
                    line,
                    Style::default().add_modifier(Modifier::DIM),
                ))
                .into()
            })
            .collect(),
    }
}

/// The capability rows — tools / MCPs in scope.
///
/// Sourced from the tool sets `App::tool_request` chooses from
/// (`ToolDef::offered_names`, which says `skill`'s condition rather than
/// whether a given turn met it — not recorded per turn), not a stub.
/// "none in scope yet" was honest while nothing was offered; once main
/// turns began carrying a `bash` tool it became an overclaim in the reassuring
/// direction — the one panel whose job is "what was in scope" telling you a
/// session could not have touched your machine, about a turn that ran a shell
/// command. (Reviewer F.)
fn capability_lines() -> Vec<Line<'static>> {
    // **Resolved once, not per frame.** `offered_names` builds a full `ToolDef`
    // — a `serde_json::json!` schema and three `String`s — to hand back the
    // constant `"bash"`. Harmless where it was written (an inspector query at
    // selection time); the inspector became a per-frame rebuild, and this went
    // with it (Reviewer B). The `Line`s themselves must still be cloned per
    // call, because a `Line<'static>` is owned and the caller consumes it.
    static TOOLS: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(shannon_core::ToolDef::offered_names);
    vec![
        insp_field("tools", TOOLS.as_str()),
        insp_field("MCPs", "none in scope yet"),
    ]
}

/// `"2 (user, assistant)"` — count plus the ordered role roster.
/// **A turn's messages, counted by role** — "2 (1 user, 1 assistant)".
/// Counted, not listed: an imported turn holds a Claude Code conversation's
/// every tool round, and the list of 378 roles filled the inspector
/// (Steven's screenshot, 10-16).
fn roster(roles: impl Iterator<Item = Role>) -> String {
    let (mut user, mut assistant) = (0, 0);
    for role in roles {
        match role {
            Role::User => user += 1,
            Role::Assistant => assistant += 1,
        }
    }
    let counted: Vec<String> = [(user, "user"), (assistant, "assistant")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, role)| format!("{n} {role}"))
        .collect();
    match counted.is_empty() {
        true => "0".into(),
        false => format!("{} ({})", user + assistant, counted.join(", ")),
    }
}

/// **The turns a summary stands in for** — "1 turn: 5b77d58f", "19 turns:
/// a, b, c, d and 15 more". The first few short ids are a trailhead (squash
/// keeps the originals, so each resolves); every id of a `:compact` of a
/// long session would fill the pane, as the roster did.
fn distilled(ids: &[String]) -> String {
    format!(
        "{}: {}",
        counted(ids.len(), "turn", "turns"),
        first_and_more(ids, 4)
    )
}

/// **`n` of a thing, in the number it is** — "1 entry", "2 entries". Both
/// forms given, since English plurals are not a suffix ("copy"/"copies",
/// "a call without a result"/"calls without results"); "1 Claude Code's own
/// entries" is what a fixed noun said (Steven's `:messages`, 10-16).
pub(crate) fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// **The first `shown`, then how many more** — "a, b, c and 15 more": a
/// trailhead rather than a listing, wherever a list of ids could fill the
/// line it is said on (the inspector's summary row, a notice).
pub(crate) fn first_and_more(items: &[String], shown: usize) -> String {
    let mut said = items[..items.len().min(shown)].join(", ");
    if items.len() > shown {
        said.push_str(&format!(" and {} more", items.len() - shown));
    }
    said
}

#[cfg(test)]
mod keys_said_tests {
    use super::*;

    /// **While a selection is up, the keys that work are drawn as keys** —
    /// `y`, `o` and `Esc` in what-you-type's look, the rest quiet: the
    /// answers are dead, these are not.
    #[test]
    fn the_keys_a_selection_takes_are_drawn_as_keys() {
        let key = crate::paint::Look::Role(CellStyle::ink(Ink::HelpCode));
        let keys: Vec<String> = (keys_said(SELECTING).runs.into_iter())
            .filter(|run| run.look == key)
            .map(|run| run.text)
            .collect();
        assert_eq!(keys, ["y", "o", "Esc"]);
        let chord = keys_said(CHORD);
        assert_eq!(chord.text(), CHORD, "the text as written");
        assert!(
            chord
                .runs
                .iter()
                .any(|run| run.text == "Esc" && run.look == key)
        );
    }
}

#[cfg(test)]
mod inspector_field_tests {
    use super::*;

    /// Counted by role, whatever the length — the list of every role is
    /// what flooded the pane.
    #[test]
    fn the_roster_counts_roles() {
        let turn = [Role::User, Role::Assistant];
        assert_eq!(roster(turn.into_iter()), "2 (1 user, 1 assistant)");
        let imported = turn.into_iter().cycle().take(378);
        assert_eq!(roster(imported), "378 (189 user, 189 assistant)");
        assert_eq!(roster([Role::User].into_iter()), "1 (1 user)");
        assert_eq!(roster(std::iter::empty()), "0");
    }

    /// One turn is "turn"; past the first few ids, a count.
    #[test]
    fn a_summary_names_the_first_turns_it_distills() {
        let ids: Vec<String> = (1..=6).map(|n| format!("t{n}")).collect();
        assert_eq!(distilled(&ids[..1]), "1 turn: t1");
        assert_eq!(distilled(&ids[..4]), "4 turns: t1, t2, t3, t4");
        assert_eq!(distilled(&ids), "6 turns: t1, t2, t3, t4 and 2 more");
    }
}

/// A token cap, or an em-dash when unset.
fn opt_num(n: Option<u32>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "—".into())
}

/// Sum the recorded token usage of an episode's OWN turns plus those of every
/// episode spawned beneath it in the ancestry tree — "what did this session,
/// and everything it started, cost." Own turns only, so shared (sliced)
/// history is never double-counted: a clone or merge owns nothing and adds
/// nothing. Depth-first over `spawn_children`; the seen set guards a corrupt
/// origin cycle. An inspector query (selection-time), not a render hot path.
fn subtree_usage(graph: &Graph, id: EpisodeId) -> Usage {
    let mut total = Usage::default();
    let mut stack = vec![id];
    let mut seen = std::collections::HashSet::new();
    while let Some(ep) = stack.pop() {
        if !seen.insert(ep) {
            continue;
        }
        if let Some(episode) = graph.episode(&ep) {
            // Per occurrence: two identical own turns are two bills.
            for occurrence in episode.own_occurrences() {
                if let Some(u) = occurrence.provenance.usage {
                    total.add_turn(u);
                }
            }
        }
        stack.extend(graph.spawn_children(&ep));
    }
    total
}

/// The status row's token values — `$tokens_in`, `$tokens_out`, `$cache` —
/// all empty until a turn has reported something, so a fresh session's
/// `tok` group hides rather than reading a noisy `0/0`.
///
/// `prompt_tokens`, not `input_tokens`: with caching on, the wire's
/// `input_tokens` counts only what came after the breakpoint, so a session
/// answered entirely from cache reads ↑0 — and, since the emptiness test asks
/// the same field, the whole group would vanish as though no turn had ever
/// run. The cache's note is the one number worth a glance here: it says
/// whether caching is actually engaging, which below a model's minimum
/// cacheable length it will not. The exact counts live in the inspector
/// ([`usage_str`]); here they are written as `style` says (`[tui.status]
/// tokens`, short by default: `↑10.4M`).
fn token_values(u: Usage, style: TokenCounts) -> [String; 3] {
    let prompt = u.prompt_tokens();
    if prompt == 0 && u.output_tokens == 0 {
        return Default::default();
    }
    let count = |n: u32| crate::status_line::token_count(u64::from(n), style);
    [count(prompt), count(u.output_tokens), cache_note(u, style)]
}

/// How full the context window is (`43%`), or nothing before the session's
/// first answer — the status row's `$ctx`.
///
/// **A different question from `tok ↑`, which is why both are shown.** The
/// arrow is cumulative — everything this session has ever sent, growing with
/// every turn and bounded by nothing. The window holds one conversation, so
/// what matters for "am I about to run out of room" is the last turn alone.
/// Steven read `↑13226` and asked what percent of the context was filled; the
/// answer was under 4%, and the two numbers differ by more the longer you talk.
///
/// **`~43%` when no request measured it** (`Billing::estimated`) — after a
/// `:compact`, the window is this history's text, estimated, until the next
/// answer measures it; the `~` the transcript's own estimates wear.
fn context_value(billing: crate::app::Billing, budget: u32, yours: bool) -> String {
    match billing.context_percent(budget) {
        // `43%*` when the ceiling is one the user chose: "83% full" means a
        // different thing against a budget you set than against the model's
        // hard limit, and the gauge should not make them look alike.
        Some(pct) => format!(
            "{}{pct}%{}",
            if billing.estimated { "~" } else { "" },
            if yours { "*" } else { "" }
        ),
        None => String::new(),
    }
}

/// **The colour `ctx` takes** at `pct` percent of the window: the last of
/// `stops` (in percent order, `theme::ctx_stops`) at or below it, the row's
/// own below the first (Steven, 10-14: "a visual indicator of the limit",
/// "an array with `percentage:#hex`"). Bold when coloured, so the change
/// reads at a glance; a terminal colour is the theme's (`Theme::said`).
fn context_style(pct: Option<u32>, stops: &[crate::theme::CtxStop], theme: &Theme) -> Style {
    let reached = |&&(at, _): &&crate::theme::CtxStop| pct.is_some_and(|p| p >= u32::from(at));
    match stops.iter().rev().find(reached).and_then(|(_, hue)| *hue) {
        Some(hue) => Style::default()
            .fg(theme.said(hue))
            .add_modifier(Modifier::BOLD),
        None => Style::default(),
    }
}

/// What the cache did, in the space a status line has — or nothing at all when
/// it did nothing.
///
/// **Writes count as something.** This asked only [`Usage::cache_hit_percent`],
/// which is `None` until tokens are *read*, on the reasoning that a permanent
/// `0%` would read as a broken feature. The reasoning was half right and the
/// result was worse: Steven's first cached turn wrote **6,147 tokens** into the
/// cache and the status line said nothing whatsoever, so a feature that was
/// working perfectly was indistinguishable from one that had never run — and he
/// reported it, correctly, as broken (dogfooding round 1).
///
/// A first request on a model can only write; the read comes next time. So the
/// write is the evidence that caching engaged, and it is exactly the evidence
/// that was being withheld. Silence is now reserved for the case that really
/// says nothing happened: neither read nor written, which is what a prompt
/// below the model's minimum cacheable length produces.
fn cache_note(u: Usage, style: TokenCounts) -> String {
    match (u.cache_hit_percent(), u.cache_creation_input_tokens) {
        // A read is the payoff and subsumes the write beside it: the percentage
        // already says how much of this prompt cost a tenth.
        (Some(pct), _) => format!("{pct}% cached"),
        (None, 0) => String::new(),
        (None, written) => format!(
            "{} stored",
            crate::status_line::token_count(u64::from(written), style)
        ),
    }
}

/// Real per-turn token usage (`↑prompt ↓output`) from the backend, or an
/// em-dash for a turn that reported none (e.g. an older persisted turn).
///
/// **`↑` is the whole prompt, not the wire's `input_tokens`.** With prompt
/// caching on, the API splits the prompt across three counters and
/// `input_tokens` holds only what fell after the cache breakpoint — usually
/// close to nothing. Rendering that as "input" would have quietly turned a
/// long conversation into a two-digit number, so the sum is what is shown and
/// the split is spelled out beside it: how much was read back from the cache,
/// and how much this turn wrote into it. Those are billed at a tenth and at
/// 1.25× of base input respectively, which is the whole reason to see them
/// apart rather than added up.
fn usage_str(u: Option<Usage>) -> String {
    let Some(u) = u else { return "—".into() };
    let head = format!("↑{} ↓{}", u.prompt_tokens(), u.output_tokens);
    match (u.cache_read_input_tokens, u.cache_creation_input_tokens) {
        (0, 0) => head,
        (read, 0) => format!("{head} ({read} cached)"),
        (0, written) => format!("{head} ({written} written to cache)"),
        (read, written) => format!("{head} ({read} cached, {written} written)"),
    }
}

/// A rough token **estimate** from a character count (~4 chars/token — the usual
/// heuristic). Marked `~` everywhere it's shown: exact per-message counts need a
/// tokenizer we deliberately don't ship (binary size); the per-turn `tokens`
/// field carries the backend's real numbers.
fn est_tokens(chars: usize) -> usize {
    chars / 4
}

/// `decision_shown` is what each session's decision pane showed this frame;
/// a session absent from it had no pane drawn. What one could not show — a
/// short terminal, or a viewer covering the layout — is announced here
/// instead, or its session just stops with nothing on screen to say why.
///
/// Where the `:` line's cursor is while it is open, for [`place`]; else
/// `None` — the row has no cursor of its own.
fn draw_status(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    decision_shown: &[(BufferId, DecisionShown)],
) -> Option<Position> {
    let theme = app.theme();
    // The `:` command line takes over the status row while it's open (vim).
    if app.cmdline.is_open() {
        // While Tab-cycling, show the candidates as a wildmenu on the row just
        // above the command line (vim's `:set wildmenu`), the current one
        // highlighted. Skipped when there's no row above to borrow.
        if let Some((candidates, selected)) = app.completion_menu()
            && area.y > 0
        {
            let menu = Rect::new(area.x, area.y - 1, area.width, 1);
            let mut spans: Vec<Span> = Vec::new();
            for (i, cand) in candidates.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("  "));
                }
                let style = if i == selected {
                    theme.ui(Ink::MenuSelected)
                } else {
                    theme.ui(Ink::TextQuiet)
                };
                spans.push(Span::styled(cand.as_str(), style));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), menu);
        }
        // **Scroll to the cursor.** The row used to draw the whole line and let
        // the terminal clip it, so an `:analyze` command longer than the width
        // was invisible past the edge with nothing saying so — you could not
        // read the command you were about to have analysed. `window` gives the
        // visible slice and the cursor's column inside it, derived from the
        // cursor each frame rather than from a stored offset that could
        // disagree with it. One cell goes to the `:` — or the `/` or `?` of
        // a search, the same line.
        let (shown, col) = app.cmdline.window(area.width.saturating_sub(1));
        let glyph = app.cmdline.kind().glyph();
        let widget = Paragraph::new(format!("{glyph}{shown}")).style(theme.ui(Ink::Text));
        frame.render_widget(widget, area);
        return cell_in(area, (0, usize::from(col) + 1));
    }

    // A pending confirmation owns the row, at the volume its action earned.
    //
    // **The row used to fill black-on-red for every one of them**, which made
    // `d` on a single session look identical to "delete ALL sessions
    // permanently" — and made the one action whose safe answer is `y`
    // (`VaultPaste`, which exists to *protect* you) look like a threat. A
    // colour that appears every time is not a warning, it is a background:
    // the user stops reading it and answers through it. So the fill is spent
    // only on [`Alarm::Grave`](crate::app::Alarm::Grave), and spent on a short
    // tag rather than the whole row — a small solid block is what peripheral
    // vision actually resolves, and it only means something because it is
    // usually absent.
    if let Some(prompt) = app.confirm_prompt() {
        // Reserved right-hand side: the answer keys and the queue depth. Cut
        // last, never cut — a decision whose `(y/N)` has been clipped off the
        // edge is a decision with no visible way out.
        let tail_cells = crate::wrap::str_cells(&prompt.tail);
        let mut spans: Vec<Span> = Vec::new();
        let mut used = 1usize; // the leading pad
        spans.push(Span::raw(" "));

        // Red is "stop" in the decision pane and in a refusal notice, and the
        // sign speaks that same vocabulary. Every unrecoverable action carries
        // it; only the unbounded one fills its background, because a solid
        // block is what the eye catches without reading and that is the part
        // that has to stay rare.
        if let Some(tag) = prompt.alarm.tag() {
            let style = if prompt.alarm.sign_is_loud() {
                theme.ui(Ink::DangerSign)
            } else {
                theme.ui(Ink::Danger).add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(tag, style));
            spans.push(Span::raw(" "));
            used += crate::wrap::str_cells(tag) + 1;
        }

        // **Hue is the verb's, not the row's.** It says what *kind* of thing is
        // about to happen, so an unrecoverable delete is red however routine it
        // is — the first cut coloured by rarity and made a permanent delete
        // yellow, which reads as "mild" rather than "common" (Steven's
        // correction). Rarity is the tag's job, above.
        let verb = theme.ui(match prompt.alarm {
            // Not an alarm: a question. Distinct from both notice colours so it
            // cannot be misread as "something went wrong".
            crate::app::Alarm::Safe => Ink::Question,
            crate::app::Alarm::Caution => Ink::Caution,
            crate::app::Alarm::Destructive | crate::app::Alarm::Grave => Ink::Danger,
        });
        let budget = (area.width as usize)
            .saturating_sub(used)
            .saturating_sub(tail_cells);
        for (text, mark) in fit_runs(&prompt.runs, budget) {
            // Two fixation points with two different jobs: *what* is happening
            // takes the danger hue, *where* it lands takes weight. Giving them
            // one colour between them would leave the eye unable to tell the
            // verb from its operand at a glance, which is the whole point of
            // marking either.
            let style = match mark {
                crate::app::Mark::Action => verb.add_modifier(Modifier::BOLD),
                // A fixed hue on purpose, and deliberately outside the
                // red/yellow/cyan vocabulary the rungs use: the operand's
                // colour must not shift with severity, or a reader would have
                // two things to decode where there is only one fact.
                //
                // Magenta because those three are spoken for and the remaining
                // options disqualify themselves — green reads as "go" next to
                // a delete, blue goes muddy on dark themes. It replaced white
                // bold, which was the mistake of marking with *weight alone*:
                // against the near-white prose it read as ordinary text
                // (Steven, drive round 5).
                crate::app::Mark::Target => theme.ui(Ink::Operand).add_modifier(Modifier::BOLD),
                // The sign's own explanation, so it shares the sign's colour:
                // red is "this is the danger" throughout the row, magenta is
                // "this is what you check". Not bold — the verb keeps that.
                crate::app::Mark::Consequence => verb,
                // Dimmer than either mark. The grammar is there to be read if
                // you stop, not to compete for the glance.
                crate::app::Mark::Plain => theme.ui(Ink::TextQuiet),
            };
            spans.push(Span::styled(text, style));
        }
        spans.push(Span::styled(
            prompt.tail,
            theme.ui(Ink::Text).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return None;
    }

    // A one-shot notice (a refused operation, a failed generation) borrows the
    // row until the next keypress — vim's message line. Feedback that only
    // reaches the log file reads as a dead key, so this outranks the ambient
    // hints below.
    if let Some(notice) = app.notice() {
        // **Red means "no", here as in the decision pane** (`Decision::Deny`)
        // — one vocabulary rather than a second one invented for this row.
        // Yellow stays "we did something and are telling you". The two used to
        // differ only by `warn!` vs `info!` in the log, so the distinction cost
        // nothing to get wrong and nobody could see it (Steven's call).
        //
        // The decision pane fills its background red; this is a foreground, so
        // the loud surface stays louder.
        let role = match app.notice_kind() {
            Some(crate::app::NoticeKind::Refusal) => Ink::Danger,
            _ => Ink::Caution,
        };
        let style = theme.ui(role).add_modifier(Modifier::BOLD);
        let (head, marker) = fit_notice(&format!(" {notice} "), area.width);
        let mut spans = vec![Span::styled(head, style)];
        if let Some(marker) = marker {
            // Dimmer than the message: this is the app talking *about* the
            // message, not part of it.
            spans.push(Span::styled(marker, theme.ui(Ink::TextFaint)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return None;
    }

    // **The row its user wrote** (`[tui.status]`, read by `status_line`) for
    // the FOCUSED pane's kind — asked of `focus`, the predicate that decides
    // who gets the keys, so the row can never describe a pane you are not
    // in. Legends used to fire on `graph_view.is_some()`, and a graph split
    // beside the prompt advertised `j/k move · q close`, dead where you stood.
    //
    // **Ordering IS policy, because this row clips.** It is one un-wrapped
    // line the width of the frame, and a narrow one gives up its rightmost
    // group first (`status_line::render`). The shipped formats put `$alerts`
    // — a decision whose pane cannot show its keys, a queued prompt, what
    // `^C` does — right after the pane's name, so it is the last group a
    // narrow row loses: `DECIDE:` exists for a terminal too small to draw the
    // decision pane, exactly when this row overflows. At 80 columns it once
    // sat past column 150, the calibration set's "action keys scrolled off
    // the bottom" rotated ninety degrees (Reviewer F). A format without
    // `$alerts` is the user's to write; the shipped ones keep it.
    //
    // **The row names a session, or says it has none** (Steven, 10-20:
    // "Status line should always be associated with a session. If for some
    // reason it isn't then show in red and say 'no agent'" — the literal
    // text). None is reached: `shannon tutor`, before a session is called
    // (11-11). The row is drawn all the same — the mode the lessons teach you
    // to watch, the pane, the position, the help — with `no agent` in red
    // FIRST, the last a narrow row loses, and the values only a session has
    // (`$model`, `$ctx`, the tokens, `$lines`) empty, their groups dropped.
    // It drew `no agent` alone, and `[NORMAL]`/`[INSERT]` went with it
    // (review of 850a72d, HIGH). The aim is read from the prompt you write
    // in, whose window `App::prompt_view` expects (`NO_PROMPT`: every
    // arrangement keeps one) — held by the workspace's removals, not said
    // here (review stop 11, A L).
    let shown = app.aimed_session();
    let kind = app.focus();
    let total = frame.area();
    // `ui.statusline` for the row, `ctx` a colour of its own
    // (`tui.ctx_colors`).
    let plain = theme.ui(Ink::Statusline);
    let format = app.status_format(kind);
    let mut row = Row {
        app,
        shown,
        kind,
        format,
        decision_shown,
        total,
    };
    let alerts = app.status_alerts();
    const NO_AGENT: &str = "no agent ";
    let said_none = if shown.is_none() {
        NO_AGENT.len() as u16
    } else {
        0
    };
    let width = area.width.saturating_sub(2 + said_none);
    let runs = crate::status_line::render(format, alerts, &mut row, plain, width);
    let mut spans = vec![Span::styled(" ", plain)];
    if shown.is_none() {
        let style = theme.ui(Ink::Danger).add_modifier(Modifier::BOLD);
        spans.push(Span::styled(NO_AGENT, style));
    }
    spans.extend(
        runs.into_iter()
            .map(|(text, style)| Span::styled(text, style)),
    );
    spans.push(Span::styled(" ", plain));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(plain), area);
    None
}

/// **What one status row is drawn from** — every value ([`status_value`])
/// is asked of the same: the focused pane's `kind`, `shown`, the session the
/// prompt is aimed at, the `format` being drawn, what each decision pane
/// drew, and the frame.
struct Row<'a> {
    app: &'a App,
    /// `None`: no session aimed at (`shannon tutor`, before one is called).
    shown: Option<&'a crate::app::SessionBuffer>,
    kind: PaneKind,
    format: &'a crate::status_line::Format,
    decision_shown: &'a [(BufferId, DecisionShown)],
    total: Rect,
}

/// **The row's values, and its alerts' facts** — a session's fact asked of
/// the session the row is about (`shown`, the aim), as each value is.
impl crate::status_line::Source for Row<'_> {
    fn value(&mut self, var: crate::status_line::Var) -> crate::status_line::Value {
        status_value(self, var)
    }

    // With no session aimed at, a session's facts are none and its inks the
    // row's own: an alert about a session has nothing to fire on.
    fn fact(&mut self, fact: crate::facts::Fact) -> u64 {
        (self.shown).map_or(0, |s| crate::app::OfSession::new(self.app, s.id).fact(fact))
    }

    fn ink(&mut self, ink: Ink) -> Style {
        (self.shown).map_or_else(Style::default, |s| {
            crate::app::OfSession::new(self.app, s.id).ink(ink)
        })
    }
}

/// **What `$var` says right now** — each value the status row can show
/// ([`Var`](crate::status_line::Var)), about the focused pane and the
/// session the prompt is aimed at (`row`). Exhaustive, so a new value says
/// what it is.
fn status_value(row: &Row, var: crate::status_line::Var) -> crate::status_line::Value {
    use crate::status_line::{Value, Var};
    let Row {
        app,
        shown,
        kind,
        decision_shown,
        total,
        ..
    } = *row;
    // Several values joined, each that has something to say — and that the
    // format does not write itself, which says it where it is written
    // (`Format::places`). `$alerts` joins its own, in `status_line`: an
    // alert is a format, not a value.
    let joined = |vars: &[Var]| -> Value {
        let parts: Vec<String> = (vars.iter())
            .filter(|&&v| !row.format.places(v))
            .map(|&v| status_value(row, v).text)
            .filter(|t| !t.is_empty())
            .collect();
        Value::from(parts.join(" · "))
    };
    // `shown`: one question — "which session is this line about?" — asked
    // once, by `draw_status`: the aim, wherever you stand (Steven, 10-16).
    // Standing in a transcript that is the one you stand in, which aims the
    // prompt; anywhere else it is where your next message goes. The focused
    // pane's `pane_session` agreed with it everywhere but a viewer lent over
    // another session, which described the session beneath it.
    let text = match var {
        // Always-on modal indicator: the focused pane's mode. Kakoune's
        // lesson — never leave the user guessing why a key did nothing.
        Var::Mode => app.focused_mode().label().to_string(),
        // The focused pane names itself — one table, in `PaneKind`, rather
        // than a second list here that has to be extended in lockstep.
        Var::Pane => kind.title().to_string(),
        Var::Readonly if kind.is_read_only() => "read-only".to_string(),
        Var::Readonly => String::new(),
        Var::Help => match kind {
            // What the pane shows, not only its kind: a review's keys are
            // not a call's (review stop 8, F 1).
            PaneKind::Tool => app.help_topic(app.workspace.focused()).to_string(),
            kind => kind.help_topic().to_string(),
        },
        Var::Position => position(app, kind),
        // A session's values, empty with none: their groups drop, and the
        // row's red `no agent` says why (`draw_status`).
        Var::Model => shown
            .map(|shown| model_value(app, shown))
            .unwrap_or_default(),
        Var::Ctx => {
            let Some(shown) = shown else {
                return Value::from(String::new());
            };
            let budget = app.context_budget(shown.id);
            let pct = shown.billing.context_percent(budget);
            return Value {
                text: context_value(shown.billing, budget, app.config.turn.budget_is_yours()),
                style: Some(context_style(pct, app.ctx_stops(), app.theme())),
                keys_from: None,
            };
        }
        Var::TokensIn | Var::TokensOut | Var::Cache => {
            let Some(shown) = shown else {
                return Value::from(String::new());
            };
            // The session's committed total PLUS whatever is streaming now.
            let mut spent = shown.billing.total;
            spent.add_turn(app.in_flight_usage(shown.id));
            let [tokens_in, tokens_out, cache] = token_values(spent, app.config.tui.status.tokens);
            match var {
                Var::TokensIn => tokens_in,
                Var::TokensOut => tokens_out,
                _ => cache,
            }
        }
        Var::Size => format!("{}x{}", total.width, total.height),
        Var::Lines => (shown)
            .map(|shown| shown.output.line_count().to_string())
            .unwrap_or_default(),
        // A WINDOW's scroll, not the buffer's — two windows on one session can
        // disagree about whether they are at the tail, and the row describes
        // the one you are standing in when it shows the aim, else the aim's.
        Var::Follow if Output::is_following_tail(&app.aim_viewport()) => "↧".to_string(),
        Var::Follow => "‖".to_string(),
        Var::Activity => {
            // The aim's own work, then the counts of everyone's.
            let own = joined(&[Var::Answering, Var::Distilling, Var::Continuing]).text;
            let counted = joined(&[Var::Sessions, Var::Children, Var::Sends]).text;
            // **Whose, in another session's decision pane** — the row is the
            // aim's, and beside B's call its `answering…` read as B's (full
            // review 10-21, F 7; Steven, 10-21: "Yes"). The aim's own work
            // only: `1 session running` under its name read as its own.
            // The row is the focused pane's: here, a decision pane.
            let aim = (kind == PaneKind::Tool)
                .then(|| app.aim_named_beside_another_call(app.workspace.focused()))
                .flatten();
            let own = match aim {
                Some(name) if !own.is_empty() => format!("{name}: {own}"),
                _ => own,
            };
            let parts: Vec<String> = [own, counted]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect();
            return Value::from(parts.join(" · "));
        }
        Var::Decide => return decide_value(app, decision_shown),
        Var::Hidden => hidden_value(app),
        Var::Unshown => unshown_value(app),
        // Prompts waiting on the slot. `submit` says "queued (N)" ONCE and
        // the notice clears on the next keypress, after which the user's
        // sentences sit in a buffer with no representation anywhere — and
        // `:qa` drops them. A one-shot claim is not a representation
        // (Reviewer F).
        Var::Queued => match app.queued_prompts() {
            0 => String::new(),
            n => format!("{n} queued"),
        },
        Var::Running => running_value(app),
        // What `^C` will DO, off the same predicate that decides it — a
        // running distillation or prefill `:continue` included, which an
        // older reading left out while the key would stop a billed request
        // (Reviewer D) — for the aim, as the row and the key both are (§9
        // phase 6b). Nothing when it does nothing (Steven, 10-14: "`^C`
        // doesn't do anything, so we should drop it"); `^L` is the
        // terminal's own redraw, known to those who would read it.
        Var::Interrupt if app.anything_to_stop() => "^C:stop".to_string(),
        Var::Interrupt => String::new(),
        // **The ordinary case, which had no segment at all.** Every other
        // kind of in-flight work was announced and the reply you are waiting
        // on was not — so "is something running", which decides what CTRL-C
        // does and whether a queued prompt will drain, went unanswered
        // (Reviewer F).
        Var::Answering if app.answering() => "answering…".to_string(),
        Var::Answering => String::new(),
        // A distillation keeps running if the graph is closed while it
        // streams — said here so the review pane it opens when written is no
        // surprise; in the tree, `<Esc>` cancels it.
        Var::Distilling => match (app.distilling(), kind) {
            (Some(label), PaneKind::Graph) => format!("distilling {label}… (Esc cancels)"),
            (Some(label), _) => format!("distilling {label}…"),
            (None, _) => String::new(),
        },
        // A prefill continuation streams off-transcript like a distillation
        // — said, or the quiet wait reads as a hang.
        Var::Continuing if app.continuing() => "continuing…".to_string(),
        Var::Continuing => String::new(),
        // **Work off the row's session, counted** (§9 phase 6c): the three
        // above say the aim's alone, so a turn in a pane off-screen would
        // otherwise be shown nowhere.
        Var::Sessions => match app.sessions_running() {
            None => String::new(),
            Some(1) => "1 session running".to_string(),
            Some(n) => format!("{n} sessions running"),
        },
        // Held is not running, and must not be folded into it: a held child
        // is waiting on the reader, so "3 children running" would tell them
        // to wait for something that is waiting for them.
        Var::Children => match (app.running_children(), app.held_children()) {
            (0, 0) => String::new(),
            (0, 1) => "1 child held".to_string(),
            (0, h) => format!("{h} children held"),
            (1, 0) => "1 child running".to_string(),
            (n, 0) => format!("{n} children running"),
            (n, h) => format!("{n} children running, {h} held"),
        },
        // `:Send` turns are NOT children — `:child` has a chapter defining
        // that word. Same (running, held) shape: a held `:Send` was once
        // reported nowhere at all (Reviewer A).
        Var::Sends => match (app.running_sends(), app.held_sends()) {
            (0, 0) => String::new(),
            (0, 1) => "1 session held".to_string(),
            (0, h) => format!("{h} sessions held"),
            (1, 0) => "1 session answering".to_string(),
            (n, 0) => format!("{n} sessions answering"),
            (n, h) => format!("{n} answering, {h} held"),
        },
    };
    Value::from(text)
}

/// `$position`: the cursor's row of how many — the tree's rows, a page's
/// lines. Empty in the prompt and the transcript, which have no rows of
/// their own to count.
fn position(app: &App, kind: PaneKind) -> String {
    let at = match kind {
        PaneKind::Graph => app
            .graph_view()
            .map(|view| (view.selected(), view.visible_len())),
        _ => app
            .text_pane(app.workspace.focused())
            .map(|text| (text.cursor(), text.lines().len())),
    };
    at.map_or(String::new(), |(row, rows)| {
        format!("{}/{}", row + 1, rows.max(1))
    })
}

/// `$model`: **which model is answering**, and the effort a turn would be
/// sent.
///
/// It was nowhere on screen, and the cost of that was not abstract: with no
/// cache note showing, there was no way to tell "this model's minimum
/// cacheable prompt is 4,096 tokens and we are under it" from "caching is
/// broken". Worse, a branch once took the model of the turn it forked from,
/// so `:model opus` followed by a branch put you back on Haiku with nothing
/// saying so (Steven, dogfooding round 1); it now takes its parent's
/// preference (`Graph::fork_anchor`), and this row says which model answers
/// either way. `answering_model`, not the config: with no API key the backend
/// IS the stub for every model, and this row said `haiku` while the stub
/// answered. The effort: nothing when none is sent, which is the model's own
/// default and also Haiku's whatever is set.
fn model_value(app: &App, shown: &crate::app::SessionBuffer) -> String {
    let effort = app
        .effort_sent(shown.id)
        .map_or(String::new(), |e| format!(" {e}"));
    format!(
        "{}{effort}",
        shannon_core::backend::models::short_name(
            app.answering_model(&shown.conversation.episode().config.model)
        )
    )
}

/// `$running`: **an allowed command, running** — the half of a tool round
/// that had no segment. It read `answering…` (the model is talking; wait)
/// through a whole `grep -r` over `target/`, which looks exactly like a hang,
/// while a prompt submitted meanwhile was told the command was "awaiting your
/// decision". It is the answer to "why is my prompt queued", and `^C:stop`
/// beside it says what to do about it. Truncated so a long pipeline cannot
/// push `^C:stop` off the row.
fn running_value(app: &App) -> String {
    match app.running_commands().as_slice() {
        [] => String::new(),
        [one] => format!("running: {}", crate::wrap::clip(one, 40)),
        [first, rest @ ..] => format!(
            "running: {} (+{} more)",
            crate::wrap::clip(first, 32),
            rest.len()
        ),
    }
}

/// **The tab line** (`tabs.md` §5, vim's): each tab's number and what its
/// focused pane shows (`App::tab_labels`), the tab you are in in
/// `ui.tabline.selected`, the others and the ground in `ui.tabline`.
/// Front-loaded: a narrow line keeps the numbers, the one thing `{N}gt`
/// needs — and a tab's mark — and cuts the names to share what is left; too
/// narrow for every number, the tab you are in and those around it
/// ([`tabline_texts`], `[[feedback_front_load_what_decides]]`).
///
/// **The hotlist, announced and never seized** (`Milestone 5.md`, Q1 (a)):
/// another tab with a decision waiting is drawn in `ui.caution`, the hue of
/// an ask; one holding a session an alert marks carries the alerts' glyph,
/// as `:ls` marks it. The hue says the kind, the glyph the news
/// (`[[feedback_alarm_must_vary]]`).
fn draw_tabline(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.theme();
    let (other, yours) = (theme.ui(Ink::TabLine), theme.ui(Ink::TabLineSelected));
    let waiting = theme.ui(Ink::Caution);
    let labels = app.tab_labels();
    let spans: Vec<Span> = (tabline_texts(&labels, area.width).into_iter())
        .map(|(cell, text)| {
            let style = match cell {
                TabCell::Tab(l) if l.current => yours,
                TabCell::Tab(l) if l.decides => waiting,
                TabCell::Cut { decides: true } => waiting,
                TabCell::Tab(_) | TabCell::Cut { decides: false } => other,
            };
            Span::styled(text, style)
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)).style(other), area);
}

/// **What a stretch of the tab line is** ([`tabline_texts`]).
pub(crate) enum TabCell<'a> {
    /// A tab's label.
    Tab(&'a crate::app::TabLabel),
    /// The `<` or `>` saying some are cut, and whether one of those waits
    /// on you: drawn in `ui.caution` then, so a decision in a tab cut from
    /// a narrow line is still announced on it (r13-F M2: the hotlist went
    /// with the label).
    Cut { decides: bool },
}

/// **The fewest cells a tab's name is cut to and still shown** — below it,
/// what was left was a lone `…` or a few characters of an id, which no verb
/// reads (r12-F L3), and the label keeps its number and mark alone. A name
/// that fits whole is shown at any width.
const TAB_NAME_FLOOR: usize = 6;

/// **The tab line in `width` cells, left to right** — each tab's label, or
/// the `<` / `>` saying some are cut ([`TabCell`]). ` N `, always,
/// the mark after; then a name and its space from the cells left, shared
/// out. **Too many for the line even so**, a window of labels around the
/// tab you are in, `<` and `>` where some are cut (vim's): the tab you are
/// in was drawn off the end (12 tabs in 30 cells). What [`draw_tabline`]
/// draws, and what a click on the line is measured against ([`tab_at`]) —
/// one layout, so where a label is drawn and where a click finds it cannot
/// part.
pub(crate) fn tabline_texts(
    labels: &[crate::app::TabLabel],
    width: u16,
) -> Vec<(TabCell<'_>, String)> {
    let width = usize::from(width);
    let bare = |l: &crate::app::TabLabel| {
        let mark = (l.mark.as_deref()).map(|m| format!("{m} "));
        format!(" {} {}", l.number, mark.unwrap_or_default())
    };
    let cells = |l: &crate::app::TabLabel| crate::wrap::str_cells(&bare(l));
    let numbers: usize = labels.iter().map(cells).sum();
    if numbers <= width {
        let share = (width - numbers) / labels.len().max(1);
        let room = share.saturating_sub(1);
        return (labels.iter())
            .map(|label| {
                let fits = crate::wrap::str_cells(&label.name) <= room || room >= TAB_NAME_FLOOR;
                let name = match fits {
                    true => crate::wrap::clip(&label.name, room),
                    false => String::new(),
                };
                let text = match name.is_empty() {
                    true => bare(label),
                    false => format!("{}{name} ", bare(label)),
                };
                (TabCell::Tab(label), text)
            })
            .collect();
    }
    // Grown from the tab you are in, a label to the right then one to the
    // left while it fits with the markers the cut ends need.
    let at = labels.iter().position(|l| l.current).unwrap_or(0);
    let (mut lo, mut hi) = (at, at + 1);
    let mut used = labels.get(at).map_or(0, cells);
    let markers = |lo: usize, hi: usize| usize::from(lo > 0) + usize::from(hi < labels.len());
    loop {
        let before = (lo, hi);
        if hi < labels.len() && used + cells(&labels[hi]) + markers(lo, hi + 1) <= width {
            used += cells(&labels[hi]);
            hi += 1;
        }
        if lo > 0 && used + cells(&labels[lo - 1]) + markers(lo - 1, hi) <= width {
            used += cells(&labels[lo - 1]);
            lo -= 1;
        }
        if (lo, hi) == before {
            break;
        }
    }
    let cut = |cut: &[crate::app::TabLabel]| TabCell::Cut {
        decides: cut.iter().any(|l| l.decides),
    };
    let hi = hi.min(labels.len());
    let mut out = Vec::new();
    if lo > 0 {
        out.push((cut(&labels[..lo]), "<".to_string()));
    }
    out.extend(labels[lo..hi].iter().map(|l| (TabCell::Tab(l), bare(l))));
    if hi < labels.len() {
        out.push((cut(&labels[hi..]), ">".to_string()));
    }
    out
}

/// **The tab whose label is under column `x`** of a tab line `width` cells
/// wide ([`tabline_texts`]) — its number, from 1; `None` on a `<` / `>` or
/// past the last label.
pub(crate) fn tab_at(labels: &[crate::app::TabLabel], width: u16, x: u16) -> Option<usize> {
    let mut right = 0;
    for (cell, text) in tabline_texts(labels, width) {
        right += crate::wrap::str_cells(&text);
        if usize::from(x) < right {
            return match cell {
                TabCell::Tab(l) => Some(l.number),
                TabCell::Cut { .. } => None,
            };
        }
    }
    None
}

/// `$decide`: **every decision the frame had no room to show the keys of.**
/// The layout yields the pane rather than squeeze the transcript to nothing
/// (the workspace's transient-pane rule, once `effective_tool_rows`), and
/// said the status line would carry it — it didn't, so on a
/// short terminal the app simply stopped accepting prompts with nothing on
/// screen to explain why.
///
/// **Each session's** (design review DR-A #3): with one slot for what was
/// drawn, a second session's pane dropped for room had no row and no pane.
/// The aim's first, the row being about it; another's named; any more
/// counted — beside the name, ahead of the command: trailing it, the count
/// went past the edge with the command's tail, on a 150-column row.
///
/// **A decision with no pane is `$hidden`'s** ([`hidden_value`]): one in no
/// window, or hidden by `:hide`. This said them too, beside the pane's name;
/// the shipped rows say them at the right.
pub(crate) fn decide_value(
    app: &App,
    drawn: &[(BufferId, DecisionShown)],
) -> crate::status_line::Value {
    let aim = app.prompt_target();
    let mut shown: Vec<_> = app.every_decision_up().collect();
    shown.sort_by_key(|(b, _)| *b != aim);
    let said: Vec<(String, Said)> = (shown.into_iter())
        // One in another tab is `$hidden`'s: its keys answer only there.
        .filter(|&(b, _)| app.decision_tab(b).is_none())
        .filter_map(|(b, on)| {
            let pane = app.decision_pane_of(b)?;
            let drawn = drawn.iter().find(|(d, _)| *d == b).map(|&(_, s)| s);
            let decide = match app.decision_name(b) {
                Some(name) => format!("DECIDE in {name}"),
                None => "DECIDE".to_string(),
            };
            let elsewhere = (app.workspace.focused() != pane).then(|| b.number());
            Some((
                decide,
                decide_one(on, app.answer_keys(pane), drawn, elsewhere)?,
            ))
        })
        .collect();
    let value = |lead: String, (what, keys): &Said| crate::status_line::Value {
        keys_from: keys.map(|at| lead.len() + at),
        text: format!("{lead}{what}"),
        style: None,
    };
    match said.as_slice() {
        [] => crate::status_line::Value::default(),
        [(decide, what)] => value(format!("{decide}: "), what),
        [(decide, what), rest @ ..] => value(format!("{decide} (+{} more): ", rest.len()), what),
    }
}

/// What one decision says on the status row, and where in it a row of keys
/// starts, if it holds one.
type Said = (String, Option<usize>);

/// `$hidden`: **every decision waiting where no pane shows it** — in a
/// session no window shows (§3.5), or hidden by `:hide` — and what brings
/// it. Nothing else on screen knows of these (Steven, 10-22: "We can show
/// hidden pending decisions in the status line … by default it should go in
/// the bottom on the right side").
///
/// One names its session, as `:decide` takes it; more are counted, since
/// `:decide` alone goes to the aim's first, then the next in no window.
///
/// **And one whose pane is in another tab** (T2): `DECIDE in tab N` and the
/// key that goes there (`tabs.md` Q1: announced, never gone to) — its keys
/// answer only there, and "hidden" would send you to `:decide` for a pane
/// that is not hidden. Mixed with others, counted, `:decide` going to each.
pub(crate) fn hidden_value(app: &App) -> String {
    let (mut hidden, mut elsewhere) = (Vec::new(), Vec::new());
    for (b, _) in app.every_decision_up() {
        match (app.decision_pane_of(b), app.decision_tab(b)) {
            (None, _) => hidden.push(b),
            (Some(_), Some(tab)) => elsewhere.push(tab),
            (Some(_), None) => {}
        }
    }
    match (hidden.as_slice(), elsewhere.as_slice()) {
        ([], []) => String::new(),
        ([one], []) => format!("decision hidden · `:decide {}`", one.number()),
        ([], [tab]) => format!("DECIDE in tab {tab} · `{tab}gt`"),
        (many, []) => format!("{} decisions hidden · `:decide`", many.len()),
        (h, e) => format!("{} decisions waiting · `:decide`", h.len() + e.len()),
    }
}

/// `$unshown`: **every session with news you have not seen**
/// (`App::unshown`, F 4, which says what that is), and what shows the
/// first: one names its session, as `:b` takes it; more are counted.
/// "News", as the notification says it (Steven, 10-23): "changed" was
/// true of your own delete or stop too, which are not news (review stop
/// 9, F8).
fn unshown_value(app: &App) -> String {
    match app.every_unshown().as_slice() {
        [] => String::new(),
        [one] => format!("news in session {0} · `:b {0}`", one.number()),
        many @ [first, ..] => format!("news in {} sessions · `:b {}`", many.len(), first.number()),
    }
}

/// [`decide_value`] for one session's decision `on` in a pane — a call or a
/// review — whose keys are `keys` and which drew `drawn`: what follows
/// `DECIDE: `, or nothing to say.
fn decide_one(
    on: crate::app::OnPane<'_>,
    keys: AnswerKeys,
    drawn: Option<DecisionShown>,
    elsewhere: Option<u64>,
) -> Option<Said> {
    let of_keys = |said: String| {
        let at = keys_start(&said);
        (said, Some(at))
    };
    Some(match drawn {
        // **What the pane could not show, and only that.** A drawn pane is
        // not the same as visible keys: squeezed to one row it shows the
        // verdict and drops its `answer:` legend, which is the "action keys
        // scrolled off the bottom" door exactly — and the keys are the one
        // thing that unwedges a program refusing input in every session. So
        // this segment used to sit beside every pane, and with the Tool
        // pane's own hint the row said `d deny · a allow` twice beside a pane
        // saying it a third time (Steven, 10-05, image 92: "cluttering things
        // again"). Now it is asked of what was DRAWN: a pane showing its keys
        // needs nothing here, and one too short for them gets them alone —
        // the command it cannot show is in its title (`ToolPrompt::question`).
        Some(DecisionShown { keys: true }) => return None,
        // Over a selection or a chord the keys are not answers: say what they
        // are, not dead keys.
        _ if keys == AnswerKeys::Selecting => of_keys(SELECTING.to_string()),
        _ if keys == AnswerKeys::Chord => of_keys(CHORD.to_string()),
        Some(DecisionShown { keys: false }) => match elsewhere {
            None => of_keys(on.page().keys_in_short()),
            // Keys drawn as keys promise they answer, and from another pane
            // `a` starts an append instead (review stop 17, F M4): said, with
            // the way there first, by number, as bare `:decide` can pick
            // another session's decision than this row's (close verifier M2).
            Some(number) => (
                format!("`:decide {number}`, then {}", on.page().keys_in_short()),
                None,
            ),
        },
        None => {
            // **What works, first.** A pane with rows answers, so where its
            // legend was dropped the keys lead (above): they are the only
            // thing that unblocks a program refusing input in EVERY session.
            // A pane with NO rows answers nothing — a decision not on screen
            // cannot be answered (`handle_tool_key`, Reviewer E) — and this
            // row offered `d deny · a allow` there, keys that then refused
            // (review stop 5, E2 #2). Room is what answers it.
            //
            // In `$alerts`, so a shipped row keeps it longest, and the command
            // trails it as context rather than leading as a headline.
            //
            // `:only` FROM ITS CONVERSATION: typed in another session's
            // window it closes this one's, and the call waits; typed in its
            // conversation it keeps the pane, which then has the room
            // (`only_command`, DR-A #8).
            let said = format!(
                "no room to answer — `:only` from its conversation, or a taller window · {}",
                on.page().subject()
            );
            (said, None)
        }
    })
}

/// What a notice too long for one row shows instead: the leading `…:messages`,
/// so the pointer to the rest is never itself the thing that got cut.
const NOTICE_MORE: &str = " …:messages";

/// Below this many cells of message left over, `…:messages` is more of the row
/// than the message is, so a bare `…` carries the "there is more" signal and
/// the verb is left to the manual.
const NOTICE_MIN_HEAD: usize = 16;

/// Fit a notice to one row: `(what to draw, an overflow marker if it was cut)`.
///
/// A notice longer than the terminal used to be handed to `Paragraph`, which
/// clips at the edge **with no signal at all** — so `stop_work`'s ~135-cell line
/// arrived at 80 columns as a sentence that just stopped, and the recovery
/// advice it ends with was gone. Silent truncation of a refusal is the same
/// class of defect as a refusal that only reaches the log: the user is told
/// something happened and cannot find out what.
///
/// Cutting is still the right shape — the status row is one line, and vim's
/// alternative (a hit-enter prompt that seizes the terminal) is a modal
/// interruption for what is usually an aside. What changes is that the cut is
/// **visible and names its remedy**.
///
/// Measured in display cells and cut on a char boundary via [`wrap::wrap_cells`]
/// — `s[..n]` on a byte count panics on multibyte input, and this string carries
/// session titles and file paths we did not write.
fn fit_notice(text: &str, width: u16) -> (String, Option<&'static str>) {
    let width = width as usize;
    if crate::wrap::str_cells(text) <= width {
        return (text.to_string(), None);
    }
    // Cells, not bytes, throughout: `…` is one cell and three bytes, so a
    // byte-length budget would over-reserve and cut the message short.
    let marker =
        (width >= crate::wrap::str_cells(NOTICE_MORE) + NOTICE_MIN_HEAD).then_some(NOTICE_MORE);
    let head_width = width.saturating_sub(marker.map_or(1, crate::wrap::str_cells));
    let chars: Vec<char> = text.chars().collect();
    let head = crate::wrap::wrap_cells(&chars, head_width)
        .swap_remove(0)
        .text;
    (head, Some(marker.unwrap_or("…")))
}

/// Fit a confirmation's styled runs into `width` cells, preserving emphasis.
///
/// The sibling of [`fit_notice`] — same underlying primitive (`wrap_cells`,
/// so the cut lands on a character boundary and a wide glyph counts as two),
/// different policy: a notice elides toward a `:messages` hint, while a
/// question has a reserved tail the caller has already subtracted and so ends
/// at a plain `…`.
///
/// Truncation is a real possibility rather than a formality: an
/// [`Alarm::Grave`](crate::app::Alarm::Grave) row spends cells on its tag, and
/// `SpawnChild` interpolates a `reason` of unbounded length.
fn fit_runs(runs: &[crate::app::Run], width: usize) -> Vec<(String, crate::app::Mark)> {
    // Nothing fits, not even the marker. Emitting one anyway would overflow the
    // reserved tail by a cell and push the answer keys off the edge — the one
    // thing the reservation exists to prevent.
    if width == 0 {
        return Vec::new();
    }
    let total: usize = runs
        .iter()
        .map(|run| crate::wrap::str_cells(&run.text))
        .sum();
    if total <= width {
        return runs
            .iter()
            .map(|run| (run.text.clone(), run.mark))
            .collect();
    }

    // **Reserve the marker's cell before filling, not after.** Deciding
    // per-run left the case where a run ends exactly on the boundary: every
    // cell was spent, and the `…` still had to go somewhere, so the row drew
    // one wider than it was given. Same shape as [`fit_notice`], which
    // subtracts its marker up front for the same reason.
    let budget = width - 1;
    let mut out = Vec::new();
    let mut used = 0usize;
    for run in runs {
        let cells = crate::wrap::str_cells(&run.text);
        if used + cells <= budget {
            out.push((run.text.clone(), run.mark));
            used += cells;
            continue;
        }
        let room = budget - used;
        if room > 0 {
            let chars: Vec<char> = run.text.chars().collect();
            // `next()`, not `swap_remove(0)`: an empty wrap result is a panic
            // waiting on a degenerate width, and this row is drawn every frame.
            if let Some(head) = crate::wrap::wrap_cells(&chars, room).into_iter().next() {
                out.push((head.text, run.mark));
            }
        }
        break;
    }
    out.push(("…".to_string(), crate::app::Mark::Plain));
    out
}

/// `usize -> u16` that saturates instead of wrapping. Used for
/// turning logical cursor positions into screen coordinates. We could
/// `as u16` everywhere — but a single silent wrap-around at the
/// cursor would mis-position it onto wildly wrong rows / columns,
/// which is exactly the kind of "almost right" bug that's hard to
/// notice.
fn saturating_u16(v: usize) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod secret_style_tests {
    use super::*;

    fn table() -> shannon_core::Aliases {
        let mut a = shannon_core::Aliases::default();
        a.insert("pw", "hunter2", "marked by you", true)
            .expect("held");
        a
    }

    /// One row standing alone — its own whole logical line.
    fn alone(a: &shannon_core::Aliases, row: &str) -> Vec<SecretSpan> {
        row_secrets(
            a,
            &[row.to_string()],
            &[(0, 0)],
            |_| Some(Cow::Borrowed(row)),
            |_| None,
        )
        .remove(0)
    }

    /// Every char marked across `rows` (row-relative spans) → the secret text
    /// they cover, joined.
    fn marked(rows: &[String], spans: &[Vec<SecretSpan>]) -> String {
        rows.iter()
            .zip(spans)
            .flat_map(|(row, spans)| {
                let chars: Vec<char> = row.chars().collect();
                spans
                    .iter()
                    .flat_map(move |s| chars[s.from..s.to].to_vec())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// **The demo bug (2026-10-02).** A pasted API key was marked — masked on
    /// the wire — and drawn as ordinary text: it was long enough to wrap, and
    /// each row was matched on its own, so neither half matched. Matched on
    /// the logical line, every row carries its part.
    #[test]
    fn a_secret_that_wraps_is_marked_on_every_row_of_the_prompt() {
        let key = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut a = shannon_core::Aliases::default();
        a.insert("anthropic", key, "marked by you", true)
            .expect("held");
        let mut prompt = Prompt::default();
        prompt.insert_pasted(&format!("use {key} please"));
        let window = prompt.visible_window(16, 10);
        assert!(window.rows.len() > 2, "premise: the key wraps");
        let spans = row_secrets(
            &a,
            &window.rows,
            &window.origins,
            |n| prompt.line_text(n).map(Cow::Owned),
            |_| None,
        );
        assert_eq!(marked(&window.rows, &spans), key);
    }

    /// The same rule in the transcript, whose word-wrap drops the space a row
    /// breaks at — the offsets, not the row text, say where each row sits.
    #[test]
    fn a_secret_that_wraps_is_marked_on_every_row_of_the_transcript() {
        let secret = "correct horse battery staple and then some more words";
        let mut a = shannon_core::Aliases::default();
        a.insert("pw", secret, "marked by you", true).expect("held");
        let mut output = Output::new();
        output.begin_turn(&format!("my passphrase is {secret}, keep it"));
        let vp = crate::viewport::Viewport::default();
        let window = output.visible_window(&vp, &crate::output::Shut::NONE, 14, 40);
        assert!(window.rows.len() > 3, "premise: it wraps");
        let spans =
            transcript_secrets(&a, &output, &window.rows, &window.origins, &window.folds).rows;
        // A space a row breaks at is not on screen; every other char is, and
        // is marked — and nothing outside the secret is.
        let without_spaces = |s: &str| s.replace(' ', "");
        assert_eq!(
            without_spaces(&marked(&window.rows, &spans)),
            without_spaces(secret)
        );
        // And the focused chat, which frames the same rows around a cursor.
        let none = crate::output::Shut::NONE;
        let focused = output.focused_window(&vp, &none, 14, 40, 0, 0, |_, _| None);
        let spans =
            transcript_secrets(&a, &output, &focused.rows, &focused.origins, &focused.folds).rows;
        assert_eq!(
            without_spaces(&marked(&focused.rows, &spans)),
            without_spaces(secret),
            "the focused chat marks it too"
        );
    }

    /// **A titled fold row's secrets are its own** — a shut command record
    /// says the command (`FoldRow::titled`), whose cells are no line's: laid
    /// on from its header line (`⚙ ran:`), a value in the command was drawn
    /// as plain text on the row that shows it.
    #[test]
    fn a_titled_fold_row_marks_the_secrets_in_its_title() {
        let a = table();
        let mut output = Output::new();
        output.begin_turn("go");
        output.push_tool_record(crate::tool_record::Record::command(
            "ran",
            "echo hunter2",
            None,
        ));
        let vp = crate::viewport::Viewport::default();
        let folds = crate::output::Folds::default();
        let shut = folds.shut(crate::output::FoldRule::SHIPPED);
        let window = output.visible_window(&vp, &shut, 60, 20);
        let at = (window.folds.iter())
            .position(|fold| fold.is_some_and(|fold| fold.titled))
            .unwrap_or_else(|| panic!("a titled row: {:#?}", window.rows));
        assert!(
            window.rows[at].contains("echo hunter2"),
            "premise: {:?}",
            window.rows[at]
        );
        let spans =
            transcript_secrets(&a, &output, &window.rows, &window.origins, &window.folds).rows;
        assert_eq!(marked(&window.rows[at..=at], &spans[at..=at]), "hunter2");

        // Cut by the `…` through the value, the part drawn is still marked
        // (the close verifier, LOW 4: read on the cut row alone, it was not).
        let window = output.visible_window(&vp, &shut, 30, 20);
        let at = (window.folds.iter())
            .position(|fold| fold.is_some_and(|fold| fold.titled && fold.cut))
            .unwrap_or_else(|| panic!("a titled row, cut: {:#?}", window.rows));
        let spans =
            transcript_secrets(&a, &output, &window.rows, &window.origins, &window.folds).rows;
        let drawn = marked(&window.rows[at..=at], &spans[at..=at]);
        assert!(
            !drawn.is_empty() && "hunter2".starts_with(&drawn),
            "{drawn:?} on {:?}",
            window.rows[at]
        );
    }

    /// A row with no secret in it is untouched — the styling must not become a
    /// reason every line goes through the slow path.
    ///
    /// **Over a table that holds one** (`table()`, `hunter2`): this asked an
    /// empty table, which could never find anything (review stop 12, T7 14).
    /// The row's spans none, and its line no run in the secret's style — with
    /// a premise that the same table styles a row that does hold it.
    #[test]
    fn an_ordinary_row_is_unstyled() {
        let a = table();
        let theme = Theme::terminal();
        let styled = |row: &str| {
            let line = bubble_line(row, &[], BlockRole::User, &alone(&a, row), 80, &theme);
            (line.spans.iter()).any(|s| s.style == secret_style(&theme))
        };
        assert!(styled("my password is hunter2 ok"), "premise: it finds one");
        let row = "just an ordinary line";
        assert!(alone(&a, row).is_empty(), "no secret span");
        assert!(!styled(row), "and no run in the secret's style");
    }

    /// **The reported bug.** A marked value in the transcript was rendering in
    /// the ordinary role colour — DIM for the user echo, markdown styling for an
    /// assistant reply — because neither chat renderer consulted the table.
    #[test]
    fn a_secret_is_styled_in_every_role() {
        let a = table();
        for role in [BlockRole::User, BlockRole::Assistant, BlockRole::Tool] {
            let row = "my password is hunter2 ok";
            let theme = Theme::terminal();
            let line = bubble_line(row, &[], role, &alone(&a, row), 80, &theme);
            let styled: Vec<&Span> = line
                .spans
                .iter()
                .filter(|s| s.style == secret_style(&theme))
                .collect();
            assert_eq!(styled.len(), 1, "one styled run for {role:?}");
            assert_eq!(styled[0].content.as_ref(), "hunter2");
        }
    }

    /// The secret style **replaces** the base rather than blending with it. A
    /// withheld value inside a markdown code span was rendering as *code* —
    /// which reads as "a literal to copy", the opposite of what we mean.
    #[test]
    fn the_secret_style_outranks_markdown() {
        let a = table();
        // Every cell claims to be code; the secret run must still be ours.
        // (It said so over PLAIN cells, so nothing was outranked.)
        let row = "my password is hunter2 ok";
        let styles = vec![crate::markdown::CODE_STYLE; row.chars().count()];
        let theme = Theme::terminal();
        let line = bubble_line(
            row,
            &styles,
            BlockRole::Assistant,
            &alone(&a, row),
            80,
            &theme,
        );
        assert!(
            line.spans
                .iter()
                .any(|s| s.style == theme.style(Context::Code, crate::markdown::CODE_STYLE)),
            "the rest of the row is code: {line:?}"
        );
        let secret_spans: Vec<&Span> = line
            .spans
            .iter()
            .filter(|s| s.style == secret_style(&theme))
            .collect();
        assert_eq!(secret_spans.len(), 1);
        assert_eq!(secret_spans[0].content.as_ref(), "hunter2");
    }

    /// **The transcript paints as code** — a reply, a record and the focused
    /// chat, each read on a role whose two maps differ (review of f50fda5,
    /// Reviewer C: all three painting as the decision pane passed the
    /// suite, since the roles it read paint alike in both).
    #[test]
    fn the_transcript_paints_as_code() {
        let theme = Theme::terminal();
        let string = CellStyle::ink(Ink::String);
        let code = theme.style(Context::Code, string);
        assert_ne!(code, theme.style(Context::Decision, string), "premise");
        let row = "say \"hi\"";
        let styles = vec![string; row.chars().count()];
        for role in [BlockRole::Assistant, BlockRole::Tool] {
            let line = bubble_line(row, &styles, role, &[], 80, &theme);
            assert!(
                line.spans.iter().all(|s| s.style == code),
                "{role:?}: {line:?}"
            );
        }
        let line = focused_chat_line(&[], row, &styles, BlockRole::Assistant, None, 80, &theme);
        assert!(line.spans.iter().all(|s| s.style == code), "{line:?}");
    }
}

#[cfg(test)]
mod splash_tests {
    use super::splash;

    /// The two lines vim itself always shows are the two a stuck newcomer
    /// needs: how to read the manual, and how to get out.
    #[test]
    fn the_essentials_survive_a_short_pane() {
        let s = splash(80, 8);
        assert!(s.contains(":help"), "how to learn: {s}");
        // The LITERAL command, not a substring both versions satisfy. This said
        // `contains(":q")` — which `":qa"` also satisfies — so reverting the
        // CRITICAL fix (launch is two panes, so `:q` closes the newcomer's
        // PROMPT instead of exiting) left the whole suite green. A test that
        // passes for the bug and the fix alike measures nothing (Reviewer C).
        assert!(
            s.contains(":qa<Enter>"),
            "names the unconditional exit, not `:q` which closes a pane: {s}"
        );
        assert!(
            !s.contains(":q<Enter>"),
            "and never advertises `:q` as the way out: {s}"
        );
    }

    /// A tall pane also says what the program is FOR — the newcomer's real
    /// question is "why is this not a web page".
    #[test]
    fn a_tall_pane_says_what_it_is_for() {
        let s = splash(80, 30);
        // The WHOLE recipe. `contains(":vs")` was satisfied by the bare `:vs`
        // line too — the one that splits the PROMPT from the launch state and
        // teaches the exact bug the manual warns about — so reverting that
        // CRITICAL also left the suite green (Reviewer C).
        assert!(
            s.contains("<C-w>k :vs :new"),
            "the recipe must start by leaving the prompt: {s}"
        );
        assert!(s.contains(":model"));
        assert!(
            s.contains("run   shannon --resume"),
            "how to come back, from the shell: {s}"
        );
        assert!(
            s.contains("run   shannon tutor"),
            "where to learn, from the shell: {s}"
        );
    }

    /// The FALLBACK tier names the real exit too.
    ///
    /// Third instance of one shape in this function: the essentials and pitch
    /// tiers were pinned to literal strings and the fallback branch was not, so
    /// reverting IT to `:q` left the whole suite green. `it_fits_every_pane…`
    /// sweeps into this branch and only checks geometry (Reviewer C).
    #[test]
    fn the_narrow_fallback_names_the_real_exit() {
        // Below `widest(essentials)` and below its line count, so this is the
        // one-line tier.
        for (w, h) in [(20u16, 3u16), (40, 6), (60, 4)] {
            let s = splash(w, h);
            assert!(
                s.contains(":qa"),
                "the fallback must name the unconditional exit at {w}x{h}: {s:?}"
            );
            assert!(
                !s.contains(":q "),
                "and never `:q`, which closes a pane, at {w}x{h}: {s:?}"
            );
        }
    }

    /// It must never be wider or taller than the pane it is drawn in — a
    /// splash that overflows is worse than none.
    #[test]
    fn it_fits_every_pane_it_is_drawn_in() {
        for w in [10u16, 30, 44, 60, 80, 200] {
            for h in [1u16, 3, 8, 20, 60] {
                let s = splash(w, h);
                assert!(
                    s.lines().count() <= h.max(1) as usize,
                    "too tall at {w}x{h}: {} lines",
                    s.lines().count()
                );
                for line in s.lines() {
                    assert!(
                        line.chars().count() <= w as usize,
                        "too wide at {w}x{h}: {line:?}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod title_fit_tests {
    use super::{NOTICE_MORE, PaneRect, elide_title, fit_notice, fit_runs};
    use crate::wm::PaneId;
    use ratatui::layout::Rect;

    /// A confirmation's question is elidable; its answer keys are not.
    ///
    /// The caller subtracts the tail before calling, so `fit_runs` staying
    /// inside its budget at **every** width is what keeps `(y/N)` on screen.
    /// Swept rather than spot-checked because the interesting widths are the
    /// ones that land mid-run and mid-glyph, and picking them by hand is how a
    /// boundary gets missed.
    #[test]
    fn a_question_is_cut_to_its_budget_at_every_width() {
        use crate::app::{Mark, Run};
        let runs = [
            Run {
                text: "delete".into(),
                mark: Mark::Action,
            },
            Run {
                text: " session ".into(),
                mark: Mark::Plain,
            },
            Run {
                text: "7e1f00d1".into(),
                mark: Mark::Target,
            },
            // Wide glyphs: a byte-length budget would over-count and a naive
            // slice would land inside a character.
            Run {
                text: " 永久に削除?".into(),
                mark: Mark::Plain,
            },
        ];
        let full: usize = runs
            .iter()
            .map(|run| crate::wrap::str_cells(&run.text))
            .sum();
        for width in 0..=full + 2 {
            let out = fit_runs(&runs, width);
            let drawn: usize = out
                .iter()
                .map(|(text, _)| crate::wrap::str_cells(text))
                .sum();
            assert!(
                drawn <= width,
                "width {width} drew {drawn} cells, stealing the reserved tail: {out:?}"
            );
        }

        // Premise, so the sweep above is not measuring an always-empty result:
        // at full width nothing is dropped and the emphasis survives intact.
        let whole = fit_runs(&runs, full);
        assert_eq!(whole.len(), runs.len(), "{whole:?}");
        assert_eq!(whole[0], ("delete".to_string(), Mark::Action));
        assert_eq!(whole[2], ("7e1f00d1".to_string(), Mark::Target));
    }

    fn pane(width: u16, number: usize) -> PaneRect {
        PaneRect {
            id: PaneId(0),
            kind: crate::workspace::PaneKind::Output,
            rect: Rect::new(0, 0, width, 10),
            focused: false,
            number,
        }
    }

    /// The failure that mattered: a title long enough to reach the number was
    /// silently clipped, and since the id is the TAIL of the title the border
    /// showed a well-formed id for a different session.
    #[test]
    fn a_title_never_reaches_the_pane_number() {
        for width in 0u16..44 {
            for number in [1usize, 9, 10, 42] {
                let p = pane(width, number);
                for title in [
                    " 7d7f16e4 — branch ← 4b5a6978 ",
                    // `:model` puts arbitrary user text in the title, so a
                    // wide-glyph name is reachable, not hypothetical.
                    " 7d7f16e4 · 日本語のモデル ",
                    " 7d7f16e4 · 🙂🙂🙂🙂 ",
                ] {
                    let fitted = elide_title(title, &p);
                    let digits = number.to_string().chars().count();
                    let limit = (width as usize).saturating_sub(digits + 4);
                    // DISPLAY cells, which is what the terminal spends.
                    let used: usize = fitted
                        .chars()
                        .filter_map(unicode_width::UnicodeWidthChar::width)
                        .sum();
                    assert!(
                        used <= limit,
                        "title {fitted:?} takes {used} cells, over {limit} at width {width}, pane {number}"
                    );
                }
            }
        }
    }

    /// And a shortened title LOOKS shortened, so a clipped id can never be
    /// mistaken for a whole one.
    #[test]
    fn a_shortened_title_says_so() {
        let fitted = elide_title(" 7d7f16e4 — branch ← 4b5a6978 ", &pane(16, 1));
        assert!(fitted.contains('…'), "elision is visible: {fitted:?}");
    }

    /// A title that already fits is left exactly alone.
    #[test]
    fn a_short_title_is_untouched() {
        let title = " 7d7f16e4 ";
        assert_eq!(elide_title(title, &pane(60, 1)), title);
    }

    /// A notice that fits is untouched — no marker on a message with nothing
    /// missing, or the marker stops meaning anything.
    #[test]
    fn a_notice_that_fits_carries_no_marker() {
        let (head, marker) = fit_notice(" saved ", 40);
        assert_eq!(head, " saved ");
        assert_eq!(marker, None);
        // Exactly-fits is the boundary that decides which side `<=` is on.
        let (head, marker) = fit_notice(" saved ", 7);
        assert_eq!((head.as_str(), marker), (" saved ", None));
    }

    /// The defect this exists for: `stop_work`'s joined line at 80 columns used
    /// to just stop, losing the recovery advice with no signal at all. It must
    /// now be cut **visibly**, and the cut must name where the rest is.
    #[test]
    fn a_notice_too_long_is_cut_visibly_and_names_the_remedy() {
        let long = format!(" {} ", "recovery advice ".repeat(12));
        let (head, marker) = fit_notice(&long, 80);
        assert_eq!(marker, Some(NOTICE_MORE));
        // The whole row is used, and not one cell over it.
        let drawn = crate::wrap::str_cells(&head) + crate::wrap::str_cells(NOTICE_MORE);
        assert!(drawn <= 80, "overflows the row: {drawn}");
        assert!(drawn > 70, "wastes the row it was given: {drawn}");
        assert!(head.starts_with(" recovery"), "{}", head);
    }

    /// On a row too narrow to spend eleven cells naming a command, a bare `…`
    /// still says "there is more" — the signal survives even when the verb
    /// cannot. Silence is the thing that must never happen.
    #[test]
    fn a_narrow_row_keeps_the_signal_if_not_the_verb() {
        let (head, marker) = fit_notice(" a rather long refusal indeed ", 20);
        assert_eq!(marker, Some("…"));
        assert_eq!(crate::wrap::str_cells(&head), 19);
    }

    /// Notices carry session titles and file paths we did not write, so the cut
    /// lands on a char boundary and measures in cells: a byte slice would panic
    /// and a char count would overflow the row with wide text.
    #[test]
    fn cutting_a_notice_is_multibyte_safe() {
        for width in 0u16..40 {
            let (head, marker) = fit_notice(" 世界世界 — naïve ☃ ", width);
            let drawn = crate::wrap::str_cells(&head) + marker.map_or(0, crate::wrap::str_cells);
            assert!(
                drawn <= width as usize || width == 0,
                "width {width}: drew {drawn} cells"
            );
        }
    }
}

#[cfg(test)]
mod cursor_door_tests {
    use super::*;

    /// **A cursor stays in its content**: a cell past it is the content's
    /// last, never the border; no content, no cursor.
    #[test]
    fn a_cursor_is_kept_inside_its_content() {
        let inner = Rect::new(1, 1, 4, 2);
        assert_eq!(cell_in(inner, (0, 0)), Some(Position::new(1, 1)));
        assert_eq!(cell_in(inner, (1, 3)), Some(Position::new(4, 2)));
        assert_eq!(cell_in(inner, (9, 9)), Some(Position::new(4, 2)), "past it");
        assert_eq!(cell_in(Rect::new(1, 1, 0, 2), (0, 0)), None, "no room");
    }

    /// **`place` draws the cell under the cursor plain** — a wide character
    /// plain on screen, as the terminal is sent it (the cell it covers never
    /// is, though a row's style reversed that too) — and no other; then puts
    /// the terminal's cursor there.
    #[test]
    fn the_door_draws_the_cell_under_the_cursor_plain() {
        let reversed = Style::default().add_modifier(Modifier::REVERSED);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(6, 1)).unwrap();
        terminal
            .draw(|frame| {
                frame.buffer_mut().set_string(0, 0, "漢xy", reversed);
                frame
                    .buffer_mut()
                    .set_style(Rect::new(0, 0, 6, 1), reversed);
                place(frame, Some(Position::new(0, 0)));
            })
            .unwrap();
        let backend = terminal.backend();
        let buf = backend.buffer();
        let on = |x: u16| buf[(x, 0)].modifier.contains(Modifier::REVERSED);
        assert!(!on(0) && !on(1), "`漢` plain, as sent");
        assert!(on(2) && on(3), "`xy` still selected");
        assert!(backend.cursor_visible());
        assert_eq!(backend.cursor_position(), Position::new(0, 0));
    }
}

#[cfg(test)]
mod token_readout_tests {
    use super::*;
    use shannon_core::TurnCost;

    fn cached(read: u32, written: u32, fresh: u32, out: u32) -> Usage {
        Usage {
            input_tokens: fresh,
            output_tokens: out,
            cache_creation_input_tokens: written,
            cache_read_input_tokens: read,
        }
    }

    /// The row's values with every digit — for the tests below, whose subject
    /// is WHICH number is shown, not how it is written.
    fn exact(u: Usage) -> [String; 3] {
        token_values(u, TokenCounts::Exact)
    }

    /// **Short by default** (Steven, 10-15: "make these numbers human
    /// readable and do that by default") — his own row's numbers, `↑10415096
    /// ↓39996`, as the shipped config draws them; and the cache's write note
    /// in the same hand.
    #[test]
    fn the_row_writes_token_counts_short_by_default() {
        let session = cached(9_894_342, 0, 520_754, 39_996);
        assert_eq!(session.prompt_tokens(), 10_415_096, "premise");
        assert_eq!(
            token_values(session, TokenCounts::default()),
            ["10.4M", "40k", "95% cached"].map(String::from)
        );
        assert_eq!(
            cache_note(cached(0, 6147, 2, 2312), TokenCounts::default()),
            "6.1k stored"
        );
    }

    /// **The reported number is the prompt, not the wire's `input_tokens`.**
    ///
    /// With a cache breakpoint on the request, `input_tokens` counts only what
    /// came after it. A session of 1,246 prompt tokens answered almost entirely
    /// from cache reports `input_tokens: 12`, so the old rendering put `↑12` on
    /// screen — a plausible, wrong number that nothing would have caught.
    #[test]
    fn the_arrow_counts_the_whole_prompt() {
        let u = cached(1100, 134, 12, 40);
        assert_eq!(exact(u)[0], "1246", "status: {:?}", exact(u));
        assert!(
            usage_str(Some(u)).starts_with("↑1246 ↓40"),
            "inspector: {}",
            usage_str(Some(u))
        );
    }

    /// **A turn answered entirely from cache still has a token segment.**
    ///
    /// The emptiness test used to ask `input_tokens`, the same field caching
    /// empties — so a fully-cached session showed no usage at all, which reads
    /// as "no turn has run here" rather than "every turn was free".
    #[test]
    fn a_fully_cached_session_still_reports() {
        let [tokens_in, _, cache] = exact(cached(2000, 0, 0, 0));
        assert_eq!(tokens_in, "2000");
        assert_eq!(cache, "100% cached");
    }

    /// A session that has run nothing shows nothing — every value empty, so
    /// the `tok` group hides rather than reading a noisy `0/0` on a fresh
    /// pane.
    #[test]
    fn a_fresh_session_shows_no_segment() {
        for style in [TokenCounts::Short, TokenCounts::Exact] {
            assert_eq!(
                token_values(Usage::default(), style),
                <[String; 3]>::default()
            );
        }
    }

    /// Caching that never engaged is silent rather than reported as `0%`: below
    /// a model's minimum cacheable length nothing is cached and no error is
    /// returned, and a permanent `0%` would read as a broken feature instead of
    /// a prompt that is simply short.
    #[test]
    fn an_uncached_turn_says_nothing_about_the_cache() {
        let u = cached(0, 0, 500, 40);
        assert_eq!(exact(u), ["500", "40", ""].map(String::from));
        assert_eq!(usage_str(Some(u)), "↑500 ↓40");
    }

    /// **A cache WRITE is visible.** The reported bug: a turn wrote 6,147
    /// tokens into the cache and the status line said nothing at all, so a
    /// feature that was working was indistinguishable from one that had never
    /// run. A first request on a model can only write — the read comes next
    /// time — so the write is precisely the evidence that caching engaged.
    #[test]
    fn a_write_with_no_read_yet_still_shows() {
        let first = cached(0, 6147, 2, 2312);
        let [tokens_in, _, cache] = exact(first);
        assert_eq!(cache, "6147 stored");
        assert_eq!(tokens_in, "6149", "…and the prompt is still the sum");
        // Next turn reads: the percentage takes over, since it says more.
        let second = cached(6147, 300, 2, 400);
        assert!(exact(second)[2].ends_with("% cached"));
        // Neither: silence, which is what a prompt under the model's minimum
        // cacheable length honestly produces.
        assert_eq!(cache_note(cached(0, 0, 500, 40), TokenCounts::Exact), "");
    }

    /// **`ctx` is the last turn, `tok ↑` is every turn.** Confusing them is
    /// what left "what percent of the context is filled" unanswerable from a
    /// status line that was already showing a token count.
    #[test]
    fn context_is_the_last_turn_not_the_running_total() {
        let mut b = crate::app::Billing::default();
        b.record(TurnCost::single_round(cached(0, 0, 1000, 500)));
        b.record(TurnCost::single_round(cached(0, 0, 2000, 500)));
        assert_eq!(b.total.input_tokens, 3000, "the bill accumulates");
        assert_eq!(b.context, 2500, "the window holds only the last exchange");
        // 2500 against Haiku's 200k window = 1%.
        assert_eq!(context_value(b, 200_000, false), "1%");
        // …and against an Opus 1M window, the same exchange is well under 1%.
        assert_eq!(context_value(b, 1_000_000, false), "0%");
        assert_eq!(
            context_value(crate::app::Billing::default(), 200_000, false),
            "",
            "nothing to report before the first answer"
        );

        // **A budget the USER set is marked.** "83% full" means a different
        // thing against a ceiling you chose than against the model's hard
        // limit, and the gauge must not make the two look alike.
        assert_eq!(context_value(b, 10_000, true), "25%*");
    }

    /// **A multi-round turn does not report the SUM of its prompts as the
    /// context.**
    ///
    /// Each round's prompt already contains every earlier round, so summing
    /// them double-counts the shared prefix. Both call sites passed
    /// `Generation::spent()` — the sum — so a two-round turn read 2.24× too
    /// full and the error grew with every round (Reviewer A, reproduced). The
    /// doc on `record` stated the correct rule the whole time; the argument
    /// broke it.
    #[test]
    fn the_context_is_the_last_round_not_the_sum_of_rounds() {
        // Round 1 carried 1000 prompt / 500 out; round 2 carried 1200 / 10 —
        // and 1200 ALREADY INCLUDES round 1, which is the whole point.
        let round_one = cached(0, 0, 1000, 500);
        let last_round = cached(0, 0, 1200, 10);
        let mut whole_turn = round_one;
        whole_turn.add_turn(last_round);

        let mut b = crate::app::Billing::default();
        b.record(TurnCost {
            whole: whole_turn,
            final_round: last_round,
        });

        assert_eq!(
            b.total.input_tokens, 2200,
            "the BILL is every round: that is what was paid"
        );
        assert_eq!(
            b.context, 1210,
            "the WINDOW is the last round alone — summing counts the shared \
             prefix twice"
        );
        assert_ne!(
            b.context,
            whole_turn.prompt_tokens() + whole_turn.output_tokens,
            "…which is exactly the number the old code reported"
        );
    }

    /// A context reading can never exceed the window, even if a model's is
    /// bigger than the one we measure against.
    #[test]
    fn the_context_percentage_saturates() {
        let mut b = crate::app::Billing::default();
        b.record(TurnCost::single_round(cached(0, 0, u32::MAX, u32::MAX)));
        assert_eq!(b.context_percent(200_000), Some(100));
        // A zero budget is not a divide by zero.
        assert_eq!(b.context_percent(0), None);
    }

    /// The inspector splits what the status line summarises: read and written
    /// are billed an order of magnitude apart (0.1× against 1.25× of base
    /// input), so a turn that wrote its whole prefix and a turn that read it
    /// back must not render the same.
    #[test]
    fn the_inspector_separates_read_from_written() {
        assert_eq!(
            usage_str(Some(cached(1100, 134, 12, 40))),
            "↑1246 ↓40 (1100 cached, 134 written)"
        );
        assert_eq!(
            usage_str(Some(cached(0, 900, 10, 40))),
            "↑910 ↓40 (900 written to cache)"
        );
        assert_eq!(
            usage_str(Some(cached(900, 0, 10, 40))),
            "↑910 ↓40 (900 cached)"
        );
        assert_eq!(usage_str(None), "—");
    }
}
