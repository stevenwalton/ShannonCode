//! Tests for [`App`](super::App) — the event loop, the pane commands, the turn
//! gate, and the session/graph operations reached through them.
//!
//! Split out of `app.rs` at the end-of-M3 sweep: the tests had grown to 5,960
//! lines inside a 10,486-line file, so *both* halves were hard to navigate and
//! `app.rs` looked far larger than the ~4,500 lines of behaviour it actually
//! holds. `#[cfg(test)] mod tests;` keeps `super::*` reaching the private items
//! exactly as an inline block does — the same shape `wm/` and `workspace/`
//! already use. Nothing here changed in the move; only the file boundary did.
//!
//! **This file is the harness now**: the doors that build an `App`
//! (`test_app` and its kin, each forbidding real execution), the seams that
//! stand in for the loop, the fixtures, and the `mod` list. The tests are in
//! `tests/`, a module a topic, each reaching all of this by `use super::*` —
//! split out at review stop 12's Tier 4 (10-27), when this file had grown to
//! 42,442 lines and 994 tests. A new test goes beside its topic; a helper
//! two topics share comes here.

use super::*;
use crate::clipboard::MemoryClipboard;
use crate::decision_page::DecisionPage;
use crate::keymap::{Direction, EditAction};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use shannon_core::JsonlStore;
use shannon_core::backend::CannedStub;
use shannon_core::{BackendError, StartMode, TurnCost};
use std::time::Duration;
use tempfile::TempDir;

mod aim;
mod billing;
mod buffers;
mod checkout_clone;
mod claude_code;
mod claude_in_graph;
mod clipboard;
mod cmdline;
mod compact;
mod config;
mod continuation;
mod decision_pane;
mod decision_queue;
mod delete;
mod edit;
mod effort;
mod file_tools;
mod folds;
mod fork;
mod gate_rules;
mod graph_keys;
mod graph_view;
mod inspector;
mod jobs;
mod knowledge;
mod layout;
mod logging;
mod marks;
mod model_honesty;
mod naming;
mod notices;
mod page_window;
mod pages;
mod panes;
mod parallel;
mod persistence;
mod position_marks;
mod project_config;
mod prompt_vim;
mod prompt_window;
mod prompts;
mod review;
mod search;
mod secrets;
mod sessions;
mod setup;
mod squash;
mod status_row;
mod stop;
mod streaming;
mod tabs;
mod teardown;
mod themes;
mod thinking;
mod tool_loop;
mod transcript_window;
mod trust;
mod tutor;
mod viewers;
mod vim_fidelity;
mod visual_rows;
mod window_history;
mod windows;

/// **Draw one frame the way the loop does** — settle, then draw.
///
/// `view::draw` takes `&App`, so anything a pane needs derived must already be
/// derived: `App::before_frame` resolves held `:help <tag>` jumps, re-clamps
/// after a resize, and rebuilds the inspector from the graph pane beside it. A
/// test that calls `view::draw` on its own is asserting against a state the
/// program never renders — which is how the inspector's whole page could be
/// missing from a frame while every render test passed.
fn draw_frame<B: ratatui::backend::Backend>(app: &mut App, terminal: &mut ratatui::Terminal<B>) {
    let size = terminal.size().expect("test backend has a size");
    app.before_frame(size.width, size.height);
    terminal
        .draw(|f| crate::view::draw(f, app))
        .expect("frame draw");
}

/// A test app that can never run a real command.
///
/// Deliberately structural rather than careful: relying on "that path does
/// not exist" or "a rule denies it" would leave `cargo test` one edit away
/// from deleting the working tree (this suite contains `find . -delete`).
fn test_app() -> App {
    // Tight cadence so any test that lets the backend actually
    // run completes quickly. Default config — the tests here don't
    // depend on any tunable.
    no_real_execution(App::new(
        Arc::new(CannedStub::with_cadence(Duration::from_millis(1), 64)),
        Arc::new(Config::default()),
    ))
}

/// `test_app` with a custom config — the same no-real-execution guarantee.
fn test_app_with(cfg: Config) -> App {
    no_real_execution(App::new(test_backend(), Arc::new(cfg)))
}

/// **The door for a test that needs its own store** — `App::with_store` plus
/// the same no-real-execution guarantee.
///
/// It did not exist, and the comment above `test_app_with` claimed *"every test
/// app must come from one of these two, so the safety property is structural"*.
/// That was false in **34 places**: every test needing a `JsonlStore` or a
/// custom backend called `App::with_store` directly and got the **real**
/// executor. A doc asserting completeness is this project's top-risk artifact,
/// and here it was guarding a suite that contains `find . -delete`
/// ([[feedback_doc_comments_are_claims]]).
///
/// Nothing had bitten because most of those tests never gate a command — but
/// "which tests could ever run one" is an enumeration standing in for a
/// property nobody computed ([[feedback_enumeration_means_unmodeled]]). One of
/// them did finally run `mkdir` for real, under a mutation, and left a
/// directory in the repo root that `git status` cannot show because git does
/// not track empty directories.
///
/// **An extraction is done when the old copies are gone, not when the
/// abstraction exists.** `App::with_store` no longer appears in this file.
///
/// **And every store here loads its structure without bodies**
/// ([`LazyBodies`](shannon_core::store::LazyBodies)). Most store-backed tests
/// run on JSONL to check what reached disk, and JSONL loads everything — so
/// until this wrapper, the composing paths' tests never met the unloaded body
/// a lazy backend hands over, and a path that skipped `App::graph_reading`
/// passed them all.
fn test_app_with_store(
    backend: Arc<dyn Backend>,
    config: Arc<Config>,
    store: Box<dyn shannon_core::Store>,
) -> App {
    let store = Box::new(shannon_core::store::LazyBodies::new(store));
    no_real_execution(App::with_store(backend, config, store))
}

/// A test app on `backend`, for the tests that assert on what was *sent*.
fn test_app_backed(backend: Arc<dyn Backend>, config: Arc<Config>) -> App {
    no_real_execution(App::new(backend, config))
}

/// **The one statement of "a test may not run a real command" — nor touch
/// the real keychain.**
///
/// Every door above ends here, so the rule is written once instead of once per
/// constructor — which is how it came to be written in two of four. A test
/// that marks a secret keeps it in memory, never in the user's secure store.
fn no_real_execution(app: App) -> App {
    sandboxed(app, Arc::new(shannon_core::MemoryVault::default()))
}

/// The rule itself: a recording executor, the given in-memory vault — and a
/// recording Claude Code, since a `claude-code/…` session would otherwise
/// start the real `claude`, which runs tools of its own.
///
/// **And no `claude` at all**, for the login check: before this line,
/// `:model claude-code/…` in a test ran the real `claude auth status`, which
/// read the developer's own login and tried to reach Anthropic and a
/// telemetry host — blocked only by the sandbox the suite happened to run in
/// (10-04). A path that cannot exist answers "not installed", every time.
/// And no network for a key setup saves: the API backend it builds is a
/// recording stub.
fn sandboxed(mut app: App, vault: Arc<shannon_core::MemoryVault>) -> App {
    // The store in memory: no test reaches the user's state directory.
    app.vault = vault;
    isolated(app)
}

/// [`sandboxed`] but for the store — for an app built over its own
/// ([`test_app_with_vault`]).
fn isolated(mut app: App) -> App {
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.claude_code = Arc::new(shannon_core::backend::RecordingProvider::new());
    app.claude_program = NO_CLAUDE.into();
    app.make_api = |_key, _cache| Ok(Arc::new(shannon_core::backend::RecordingStub::new()));
    // Claude Code's conversations are the user's own: a test names a
    // directory of its own fixtures, or none.
    app.claude_projects = None;
    // And the system clipboard is the user's own: a yank writes OSC 52 to
    // the real stdout, which a terminal obeys, so a test that yanked on the
    // default sink would set the developer's clipboard (10-05: a mutant's
    // yank put one in the harness's output, ahead of its `FAILED` line). A
    // test that reads the clipboard installs its own `MemoryClipboard`.
    app.osc = Box::new(MemoryClipboard::default());
    // And the user's instructions, memories and skills are their own: no
    // roots, so nothing is told — not this repository's `AGENTS.md` either.
    // A test of memory and skills builds roots in a temporary directory
    // (`tests::knowledge`).
    app.knowledge_roots = None;
    app
}

/// A `claude` that cannot exist — see [`sandboxed`].
const NO_CLAUDE: &str = "/nonexistent/shannon-tests-never-run-claude";

/// [`sandboxed`]'s recording Claude Code swapped for `provider`, so a test
/// can read what the gate answered it — and the directory the tests run in
/// trusted, in memory, as a user who runs Claude Code here would have
/// (`crate::trust`; a test of trust itself builds its app without).
fn with_provider(mut app: App, provider: &shannon_core::backend::RecordingProvider) -> App {
    app.claude_code = Arc::new(provider.clone());
    app.trusted
        .trust(
            &std::env::current_dir().unwrap(),
            crate::trust::Lasting::ThisRun,
        )
        .expect("trusted in memory");
    app
}

/// A test app on `store` keeping secrets in `vault` — shared across the
/// "runs" of a test, so a secret marked in one comes back in the next.
/// Handed to the constructor, because a resumed session's secrets are
/// recalled while the app is being built. Lazy like every other store here
/// ([`test_app_with_store`]): a resume recalls secrets from the words, so it
/// must meet them unloaded first.
fn test_app_with_vault(
    config: Config,
    store: Box<dyn shannon_core::Store>,
    vault: Arc<shannon_core::MemoryVault>,
) -> App {
    test_app_opening(config, store, vault, crate::app::Opens::Session)
}

/// [`test_app_with_vault`] launched on what `opens` says — `shannon tutor`'s
/// lessons (`Opens::Tutor`), whose `b1` is held for them.
fn test_app_opening(
    config: Config,
    store: Box<dyn shannon_core::Store>,
    vault: Arc<shannon_core::MemoryVault>,
    opens: crate::app::Opens,
) -> App {
    let store = Box::new(shannon_core::store::LazyBodies::new(store));
    isolated(App::with_vault(
        test_backend(),
        Arc::new(config),
        store,
        vault,
        opens,
    ))
}

fn test_backend() -> Arc<dyn Backend> {
    Arc::new(CannedStub::with_cadence(Duration::from_millis(1), 64))
}

/// **The title of the prompt you write in** — `App::prompt_title` of the
/// prompt this tab types into (`Workspace::prompt_pane`); each prompt window
/// says its own (10-26).
fn prompt_title(app: &App) -> String {
    app.prompt_title(app.workspace.prompt_pane().expect("premise: a prompt"))
}

/// Drive a model tool call the way the stream does — the request recorded on
/// the turn, then `Complete`, so the generation is parked on the decision
/// exactly as it is in production. Tests that used `gate_tool_call` directly
/// were exercising a path only `:tool` takes.
///
/// The `Complete` matters: without it the turn sits mid-stream forever, a
/// state the backend never produces, and every assertion about "is the slot
/// wedged" is being made against a shape that cannot occur.
fn model_call(app: &mut App, id: &str, command: &str) {
    if !app.conversation().turn_in_progress() {
        // Open the turn on the model only — `start_turn` would also send a
        // real request.
        app.conversation_mut().begin_turn("go");
    }
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        id, command,
    )));
    app.feed_stream(StreamMsg::Complete);
}

/// **`S` with its summary reviewed in `$EDITOR`** (`[merge] distill_review =
/// true`) — what these tests were written against, when every `S` was
/// reviewed; unreviewed is the default since 10-21, as `:compact`'s is.
fn distill_reviewed(app: &mut App) {
    std::sync::Arc::make_mut(&mut app.config)
        .merge
        .distill_review = true;
    app.llm_squash(false);
}

/// **`:graph` open on the aim's session, the whole of it marked** — what
/// `S` then distils, in place. `S` resolves its plan first and asks whether
/// the session it lives in is busy (`squash_home`, full review 10-21, D 1),
/// so a test of its busy refusal picks something to distil.
fn mark_the_aims_session(app: &mut App) {
    let episode = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
}

/// **Go to the decision, as the user does** (`<C-w>` to it): a decision no
/// longer takes focus when it arrives (review stop 5, E2 CRITICAL — typing
/// ahead answered it), so a test that answers one goes there first.
fn to_decision(app: &mut App) {
    assert!(app.shown_call().is_some(), "premise: a decision is up");
    app.set_focus(PaneKind::Tool);
    app.read_the_call_for_test();
}

/// **Aim the prompt at `session`** — `:open` it, as the user does — and
/// check it took: a stop or a send that a test means for `session` would
/// otherwise reach the session that happened to be in front.
fn aim_at(app: &mut App, session: EpisodeId) {
    app.open_by_hash(&session.to_hex()[..8]);
    assert_eq!(
        app.aimed_buffer().conversation.episode().id,
        session,
        "premise: aimed at {}",
        session.to_hex()
    );
}

/// Press one plain key at the tool gate — there, as the user is when they
/// answer ([`to_decision`]), having read the call (`tui.approval_delay_ms`
/// past) — with a roomy viewport (so the scroll clamp is never what a
/// decision test is measuring).
fn tool_key(app: &mut App, c: char) -> ToolKeyEffect {
    if app.shown_call().is_some() && app.focus() != PaneKind::Tool {
        to_decision(app);
    }
    app.read_the_call_for_test();
    let effect = app.handle_tool_key(
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        (80, 100),
    );
    // The event's end, as the loop's after every key: unsettled, a test
    // could assert a state no frame shows (review stop 12, T1 4).
    app.settle();
    effect
}

/// [`to_decision`], to **`buffer`'s** decision pane, focused by pane id — so
/// a test with two sessions' calls presses into the one it means, never the
/// one made first (design review DR-A #12).
fn to_decision_of(app: &mut App, buffer: BufferId) {
    assert!(
        app.shown_call_in(buffer).is_some(),
        "premise: its decision is up"
    );
    let pane = app.decision_pane_of(buffer).expect("premise: its pane");
    app.focus_pane(pane);
    app.read_the_call_for_test();
}

/// [`tool_key`], in **`buffer`'s** decision pane ([`to_decision_of`]).
fn tool_key_in(app: &mut App, buffer: BufferId, c: char) -> ToolKeyEffect {
    to_decision_of(app, buffer);
    let effect = app.handle_tool_key(
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        (80, 100),
    );
    app.settle(); // the event's end, as `tool_key`'s
    effect
}

/// **`$EDITOR`, scripted** (`editor::Editor`): each opening answered by the
/// next of its answers, and what it was opened on kept, to assert.
struct Scripted {
    answers: std::collections::VecDeque<anyhow::Result<Option<String>>>,
    opened: Vec<(String, crate::editor::Filetype)>,
}

impl Scripted {
    fn answering(answers: impl IntoIterator<Item = anyhow::Result<Option<String>>>) -> Scripted {
        Scripted {
            answers: answers.into_iter().collect(),
            opened: Vec::new(),
        }
    }
}

impl crate::editor::Editor for Scripted {
    fn edit_saved(
        &mut self,
        initial: &str,
        kind: crate::editor::Filetype,
    ) -> anyhow::Result<Option<String>> {
        self.opened.push((initial.to_string(), kind));
        (self.answers.pop_front()).expect("the editor opened more often than scripted")
    }
}

/// **A key in `buffer`'s review pane**, as the user presses it there — gone
/// to (`{n}<C-w>w`), its keys past their wait — with a roomy viewport.
fn review_key_in(app: &mut App, buffer: BufferId, code: KeyCode) -> ToolKeyEffect {
    assert!(
        matches!(app.decisions.on_pane(buffer), Some(OnPane::Review(_))),
        "premise: its review is up"
    );
    let pane = app.decision_pane_of(buffer).expect("premise: its pane");
    app.focus_pane(pane);
    app.read_the_call_for_test();
    let effect = app.handle_tool_key(KeyEvent::new(code, KeyModifiers::NONE), (80, 100));
    app.settle(); // the event's end, as `tool_key`'s
    effect
}

/// Run `command` as a **manual `:tool` invocation** — the user's own action,
/// as distinct from a tool call the model made.
///
/// The three lines this replaces (bump the seq, mint `manual-N`, record it in
/// `manual_tool_ids`, gate it) were written out at every site that needed one,
/// which is how the distinction between "the user started this" and "the model
/// asked for this" came to be re-derived per test.
///
/// **Through `:tool`'s own door** (`App::typed_tool_call`): this copied its
/// steps, arrival then gate, and so missed the one A1.4 added — the session
/// shown first — passing where `:tool` would not.
///
/// Deliberately does NOT settle: the command stays in `running_tools`, which is
/// the state every caller here is actually about.
fn manual_tool(app: &mut App, command: &str) -> String {
    app.typed_tool_call(command)
}

/// **The shown call's command edited to `edited`**, as `e` and `$EDITOR`'s
/// return do it (`apply_tool_edit`): the edit resolved for the gate, judged
/// under the call's own rules, the pane refit. Tests called `re_gate`
/// themselves under the comment "what `edit_tool_command` does once
/// `$EDITOR` returns", which skipped all three (review stop 12, T1 6).
fn edit_shown(app: &mut App, edited: &str) {
    let (seed, _) = app.edit_seed_for_test().expect("`e` opens the shown call");
    app.apply_edit_for_test(&seed, edited);
}

/// Await the background command started by an approval and feed its outcome
/// back in — the event loop's exec arm, in test form.
async fn settle(app: &mut App) {
    if let Ok(Some(o)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), app.exec_rx.recv()).await
    {
        app.handle_exec_outcome(o);
        // And the event's end, as the loop's exec arm does: the outcome
        // alone was half of it (review stop 12, T1 4).
        app.settle();
    }
}

/// Every `tool_result` the app is holding or has already returned.
///
/// Two places, because a result is *pending* only until the round completes —
/// at which point `resume_after_tools` moves it into the message that answers
/// the model. Looking in one place made a test's meaning depend on whether the
/// round happened to have finished, which is exactly the kind of accidental
/// premise that makes an assertion stop measuring anything.
fn tool_results(app: &App) -> Vec<(String, String, bool)> {
    let pending = app.tool_results.values();
    let returned = app
        .conversation()
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter());
    pending
        .chain(returned)
        .filter_map(|b| match b {
            shannon_core::Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id.clone(), content.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

/// The text of some `tool_result`, plus whether it is an error.
fn tool_result(app: &App) -> Option<(String, bool)> {
    tool_results(app)
        .into_iter()
        .next()
        .map(|(_, content, is_error)| (content, is_error))
}

/// The result recorded for a specific call.
fn tool_result_for(app: &App, id: &str) -> Option<(String, bool)> {
    tool_results(app)
        .into_iter()
        .find(|(call, ..)| call == id)
        .map(|(_, content, is_error)| (content, is_error))
}

/// Two sessions, side by side, the second in front — the shape in which a
/// session's call can come next after another's.
fn two_sessions_side_by_side(dir: &std::path::Path) -> (App, BufferId, BufferId) {
    let mut app = app_with_n_turns(dir, 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.set_focus(PaneKind::Output);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.split_command("output", SplitDir::Vertical);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    assert_ne!(a_buffer, b_buffer, "premise: two sessions");
    (app, a_buffer, b_buffer)
}

/// The fg colour of a pane rect's top-left border cell — the cell whose
/// brightness is the whole focus signal on screen.
fn corner_color(buf: &ratatui::buffer::Buffer, rect: Rect) -> ratatui::style::Color {
    buf[(rect.x, rect.y)].fg
}

/// **The danger fill means the same thing wherever the cursor is.**
///
/// `REVERSED` swaps foreground and background, so parking the cursor on the
/// decision pane's white-on-red verdict row repainted it red-on-white — two
/// findings of the same severity rendering in different colours depending on
/// incidental cursor position, in a design whose rule is "colour is
/// communication, not decoration" (Reviewer F, round 3).
///
/// A painted cursor of one cell kept the rule by being one cell (Steven: "just
/// use the cursor like normal, same thing vim does"). **The terminal's cursor
/// keeps it by painting nothing** (S21-Q23): a page's rows are its text's
/// alone, the danger row intact, and the cursor is measured, not drawn — on
/// an empty line too (one you cannot see is one that moved somewhere you
/// cannot follow), and by its cell on a wide character, not its char.
#[test]
fn a_page_paints_no_cursor_and_measures_where_it_is() {
    use crate::keymap::{CursorMotion, EditAction};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};

    let danger = Style::default().fg(Color::White).bg(Color::Red);
    let view = crate::text_view::TextView::new(vec![
        Line::from(Span::styled("[deny] rm · destructive", danger)),
        Line::from("plain explanatory text"),
        Line::from(""),
        Line::from("漢字 wide first characters"),
    ]);
    let (width, height) = (80, 10);

    let drawn = crate::view::page_rows_for_test(&view, width, height);
    let reversed = drawn.iter().any(|line| {
        line.style.add_modifier.contains(Modifier::REVERSED)
            || (line.spans.iter()).any(|s| s.style.add_modifier.contains(Modifier::REVERSED))
    });
    assert!(!reversed, "no cell painted as a cursor: {drawn:?}");
    assert_eq!(drawn[0].spans.len(), 1, "the danger row is one run");
    assert_eq!(drawn[0].spans[0].style.bg, Some(Color::Red));
    assert_eq!(view.cursor_cell(width, height as usize), Some((0, 0)));

    let mut empty = view.clone();
    empty.goto_line(2, 0);
    assert_eq!(
        empty.cursor_cell(width, height as usize),
        Some((2, 0)),
        "an empty line"
    );

    let mut wide = view;
    wide.goto_line(3, 0);
    wide.apply(EditAction::Move(CursorMotion::Right), 1, 0);
    assert_eq!(wide.position(), (3, 1), "premise: on `字`");
    assert_eq!(
        wide.cursor_cell(width, height as usize),
        Some((3, 2)),
        "its cell, past `漢`'s two"
    );
}

/// **A wrapped page draws its rows as it counts them** (the one ruler,
/// `PageLine::wrap`): the cursor measured on its line's FIRST row, a
/// Visual range reversed on every row of every line in it, and a window
/// inside a tall line drawn from the row it is scrolled to — not the
/// line's first, which a re-wrap from the line's top would show.
#[test]
fn a_wrapped_page_draws_the_rows_it_counts() {
    use crate::text_view::TextView;
    use ratatui::style::Modifier;
    let words = (0..30)
        .map(|i| format!("w{i:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut view = TextView::new(vec![
        ratatui::text::Line::from("top"),
        ratatui::text::Line::from(words.clone()),
        ratatui::text::Line::from("end"),
    ]);
    let (width, height) = (12u16, 40u16);
    view.clamp(height as usize, width, 0);
    view.goto_line(1, 0);
    let rows = view.rows_at(width);
    assert!(rows > 5, "premise: the middle line wraps ({rows} rows)");
    assert_eq!(
        view.cursor_cell(width, height as usize),
        Some((1, 0)),
        "the cursor on its line's first row"
    );

    // Visual (`V`) over the wrapped line: every char of every row of it
    // reversed, and nothing else.
    view.apply(crate::keymap::EditAction::SetAnchorLine, 1, 0);
    let drawn = crate::view::page_rows_for_test(&view, width, height);
    let wrapped = view.lines()[1].rows(width as usize);
    let reversed = |row: &ratatui::text::Line| {
        row.spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::REVERSED))
    };
    for (i, row) in drawn.iter().enumerate().take(rows) {
        let selected = (1..1 + wrapped).contains(&i);
        assert_eq!(reversed(row), selected, "row {i}");
        if selected {
            assert!(
                row.spans
                    .iter()
                    .all(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
                "row {i} whole"
            );
        }
    }

    // A window three rows tall, paged INTO the tall line: it starts at the
    // row the page is scrolled to.
    view.clear_selection();
    view.clamp(3, width, 0);
    view.goto_line(1, 0);
    view.page(3, 0);
    let scroll = view.scroll() as usize;
    assert!(scroll > 1, "premise: scrolled inside line 1 ({scroll})");
    let drawn = view.visible_rows(width, 3);
    let cut = view.lines()[1].wrap(width as usize);
    assert_eq!(drawn[0].line, 1);
    assert!(!drawn[0].first(), "not the line's first row");
    assert_eq!(drawn[0].row.text(), cut[scroll - 1].text());
}

/// **A pane of `kind`, opened as the program opens it, and focused** — a
/// page by its command, the decision pane by a call, the theme preview by
/// `:themes`, the graph by `:graph` — **with room to move in both
/// directions**: the pages whose content is the app's to give are given
/// pages of it (200 messages, a 60-line answer, 60 config files, 12 tabs).
/// One arm per kind and per page, no `_`, so a new kind has no opener until
/// it is given one, and every test that walks [`PaneKind::all`] through
/// this reaches it. The match was written seven times, the roomy copy behind
/// an `unreachable!`, and the "every" tests named three or four kinds of
/// twelve (review stop 12, T1 9 and T3 14).
///
/// **Ended as the loop ends an opening**: focused on purpose (`set_focus`,
/// the door a `<C-w>` takes), the event settled, then the frame's sync and
/// clamp. Unsettled, the first key's settle did the opening's — it saw the
/// focus change and dropped a count half-typed, so `5j` moved one line in
/// every page and the decision pane fell back to the prompt (Tier 3, probed).
fn open_pane_of(app: &mut App, kind: PaneKind) {
    match kind {
        PaneKind::Output => {
            app.output_mut().begin_turn("hello");
            app.output_mut().end_turn();
        }
        PaneKind::Prompt => {}
        PaneKind::Graph => open_graph(app),
        // Opened BY the graph, of its selection: the answer's body.
        PaneKind::Inspector => {
            app.conversation_mut().begin_turn("a question");
            app.conversation_mut()
                .push_chunk(&(1..=60).map(|i| format!("line {i}\n")).collect::<String>());
            app.conversation_mut().commit_turn();
            open_graph(app);
            for code in ['j', 'l', 'j', 'j'] {
                app.graph_key_for_test(KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE));
            }
        }
        // A decision pane is opened by a tool call, never by hand.
        PaneKind::Tool => model_call(app, "tu-1", "rm -rf /tmp/shannon-test-absent-pane"),
        PaneKind::Page(PageKind::Help) => app.open_help_command(None),
        PaneKind::Page(PageKind::Tutor) => app.open_tutor(),
        PaneKind::Page(PageKind::Analyze) => {
            app.open_analyze_command("ls -la | grep x | wc -l > /tmp/out.txt")
        }
        PaneKind::Page(PageKind::Messages) => {
            for i in 0..200 {
                app.notify(format!("message {i}"));
            }
            app.open_messages_command();
        }
        PaneKind::Page(PageKind::Buffers) => {
            for n in 0..4 {
                app.conversation_mut().begin_turn(format!("session {n}"));
                app.conversation_mut().push_chunk("a reply");
                app.conversation_mut().commit_turn();
                app.new_session_command();
            }
            app.dispatch_command_for_test("ls");
        }
        PaneKind::Page(PageKind::Aliases) => {
            for n in 0..12 {
                app.mark_secret(&format!("pretend secret number {n}"), true)
                    .expect("marked");
            }
            app.dispatch_command_for_test("alias");
        }
        // The shipped rules alone are a page with room to move.
        PaneKind::Page(PageKind::Rules) => {
            app.dispatch_command_for_test("rules");
        }
        // Empty, so the stack changes no setting; the listing holds paths,
        // not texts, so the files may go with the dir.
        PaneKind::Page(PageKind::Scripts) => {
            let dir = TempDir::new().unwrap();
            for n in 0..60 {
                let file = dir.path().join(format!("{n}.toml"));
                std::fs::write(&file, "").unwrap();
                app.source_file(shannon_core::config::Origin::Sourced, &file);
            }
            app.dispatch_command_for_test("scriptnames");
        }
        PaneKind::Page(PageKind::Tabs) => {
            for _ in 0..12 {
                app.dispatch_command_for_test("tabnew");
            }
            app.dispatch_command_for_test("tabnext 1");
            app.dispatch_command_for_test("tabs");
        }
        // A mark on each of 26 turns' echoes: a heading, a group and 26 rows.
        PaneKind::Page(PageKind::Marks) => {
            for i in 0..30 {
                app.output_mut().begin_turn(&format!("ask {i}"));
                app.output_mut().append_chunk("  ok");
                app.output_mut().end_turn();
            }
            for (i, name) in ('a'..='z').enumerate() {
                let anchor = app.output().anchor_at(i * 6 + 1, 0).expect("a turn's echo");
                app.buffer_mut().marks.insert(name, anchor);
            }
            app.dispatch_command_for_test("marks");
        }
        // The shipped themes alone are a list with room to move; the preview
        // is built by the frame, from the list beside it.
        PaneKind::Page(PageKind::Themes) | PaneKind::ThemePreview => {
            app.dispatch_command_for_test("themes");
        }
    }
    app.set_focus(kind);
    app.settle();
    app.sync_chat();
    app.clamp_text_panes(80, 24);
}

/// The kinds a writer key is refused in — the predicate's answer
/// (`PaneKind::is_read_only`), not a list: every kind but the prompt. The
/// decision pane among them, in its legend's voice while it decides.
fn read_only_kinds() -> Vec<PaneKind> {
    (PaneKind::all().into_iter())
        .filter(|k| k.is_read_only())
        .collect()
}

/// A focused read-only pane of `kind`, with enough content to move in.
async fn read_only_pane(kind: PaneKind) -> App {
    let mut app = test_app();
    open_pane_of(&mut app, kind);
    // **Clear the slot the way a keypress does**, not with a test-only reset:
    // opening a viewer notifies, and a leftover notice would make every
    // assertion below pass on somebody else's message. `begin_moment` is the
    // production rule ("a notice survives exactly one input cycle"), so this
    // fixture cannot diverge from it.
    let cleared = app.begin_moment(&crossterm::event::Event::Key(KeyEvent::new(
        KeyCode::F(12),
        KeyModifiers::NONE,
    )));
    assert!(
        cleared,
        "premise — F12 must reach the moment, or nothing cleared"
    );
    assert!(
        app.notice().is_none(),
        "{kind:?}: fixture — the notice slot must start empty: {:?}",
        app.notice()
    );
    app
}

fn center_of(r: Rect) -> Position {
    Position {
        x: r.x + r.width / 2,
        y: r.y + r.height / 2,
    }
}

/// **`a`'s call is kept, answered by nothing, its pane in no tab, and
/// said** — what a forced close leaves (`reconcile_pane_state`).
///
/// **With `a` still shown, in some tab** — a premise: in no window, hidden
/// and merely unplaced look alike, so a test of it has to keep the session
/// in view for "no pane" to mean hidden. Deleting the reconcile's `hide`
/// passed a test of a review that asserted "no pane" with its session shown
/// nowhere (review stop 12, T5 F5).
#[track_caller]
fn waits_hidden(app: &App, a: BufferId) {
    assert!(
        app.panes_on(a, Over::EveryTab).next().is_some(),
        "premise: the session shown, in some tab"
    );
    // A call or a review, unanswered: what its pane would show.
    assert!(app.decisions.on_pane(a).is_some(), "the request kept");
    assert!(
        app.decision_pane_of(a).is_none(),
        "hidden: no pane, any tab"
    );
    let said = format!("the request waits, hidden: `:decide {}`", a.number());
    assert!(
        (app.notices.iter()).any(|(_, text)| text.contains(&said)),
        "said: {:?}",
        app.notices
    );
    assert!(
        !(app.notices.iter()).any(|(_, text)| text.contains("DENIED")),
        "nothing denied: {:?}",
        app.notices
    );
}

/// **What a session can be asked to decide** — a model's call, or a
/// summary's review: the two requests a decision pane holds. Each row of
/// `decision-panes-and-alerts.md` §3.6 is one rule for both, so a row's
/// test walks both. The review's column was empty where its fixture — a
/// review under a `:graph` lent over the only transcript — gave the
/// transcript back on a close and never reached the chat rule (review stop
/// 12, T5 F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Call,
    Review,
}

impl Request {
    const BOTH: [Self; 2] = [Self::Call, Self::Review];

    /// This request asked of the session in front, its pane under the
    /// transcript showing it — a call the model made, or `:compact!`'s
    /// review of two turns, written. The session's buffer.
    fn ask(self, app: &mut App) -> BufferId {
        let chat = app.current;
        match self {
            Self::Call => model_call(app, "tu_1", "rm -rf /tmp/shannon-test-absent"),
            Self::Review => {
                persist_n_turns(app, 2);
                app.dispatch_command_for_test("compact!");
                app.feed_summary(StreamMsg::Chunk("the gist".into()));
                app.feed_summary(StreamMsg::Complete);
            }
        }
        let pane = (app.decision_pane_of(chat)).unwrap_or_else(|| panic!("{self:?}: its pane"));
        let host = (app.window_of(pane).and_then(Window::decides)).map(|(_, host)| host);
        assert!(
            host.is_some_and(|host| app.pane_shows(host, chat)),
            "{self:?}: premise: under the transcript showing it"
        );
        chat
    }

    /// What a refusal calls it, after `bN's`.
    fn named(self) -> &'static str {
        match self {
            Self::Call => "request",
            Self::Review => "summary review",
        }
    }
}

/// The `<Tab>` completion list and the resolver must agree. A name that
/// completes but does not resolve is worse than one that does neither.
#[test]
fn every_completable_pane_name_resolves() {
    for name in crate::command::PANE_NAMES {
        assert!(
            PaneKind::from_name(name).is_some(),
            "{name:?} completes but does not resolve"
        );
    }
    for kind in PaneKind::openable_kinds() {
        assert!(
            crate::command::PANE_NAMES.contains(&kind.title()),
            "{kind:?} is openable but never completes"
        );
    }
}

/// The pinned legend's text.
fn legend(app: &App) -> String {
    let prompt = app.shown_call().expect("a decision is open");
    prompt
        .legend()
        .map(crate::paint::PageLine::text)
        .unwrap_or_default()
}

/// A config that **resumes** the last session on launch — the opt-in mode the
/// persistence/resume tests exercise (the shipped default is `Fresh`).
fn config_resume_last() -> Arc<Config> {
    let mut c = Config::default();
    c.tui.resume = ResumeMode::Last;
    Arc::new(c)
}

/// The prompt's content rows in a standard test frame — the observable
/// the old `Workspace::prompt_rows()` reader stood in for, now measured
/// where it actually matters (the computed layout) instead of read out of a
/// field that only one arrangement could have.
fn prompt_content_rows(app: &App) -> u16 {
    pane_rect(
        &app.workspace.frame_layout(Rect::new(0, 0, 80, 50)),
        PaneKind::Prompt,
    )
    .height
    .saturating_sub(crate::layout::BORDER_SHAPE.0)
}

/// The prompt's content columns in a standard test frame.
fn prompt_content_cols(app: &App) -> u16 {
    pane_rect(
        &app.workspace.frame_layout(Rect::new(0, 0, 80, 50)),
        PaneKind::Prompt,
    )
    .width
    .saturating_sub(crate::layout::BORDER_SHAPE.1)
}

/// Does the pane showing `a` come before the one showing `b` on screen
/// (above it, or to its left)?
///
/// The **observable** form of the old `prompt_first: bool` field, which
/// could only ever describe two panes and named one of them. Order is a
/// property of where things land, so that is what the tests read.
fn is_before(app: &App, a: PaneKind, b: PaneKind) -> bool {
    let layout = app.workspace.frame_layout(Rect::new(0, 0, 80, 50));
    let (ra, rb) = (pane_rect(&layout, a), pane_rect(&layout, b));
    (ra.y, ra.x) < (rb.y, rb.x)
}

/// Are the two panes side by side rather than stacked?
fn side_by_side(app: &App, a: PaneKind, b: PaneKind) -> bool {
    let layout = app.workspace.frame_layout(Rect::new(0, 0, 80, 50));
    pane_rect(&layout, a).x != pane_rect(&layout, b).x
}

/// Open `:graph` the way the command does — build the view AND open the
/// pane. Setting `graph_view` alone no longer shows anything, which is the
/// point: the content and the viewport are separate now.
fn open_graph(app: &mut App) {
    open_graph_pane(app);
    let gv = app.build_graph_view();
    app.set_graph_view_for_test(gv);
}

/// **One way to drive a `:graph` verb** — its key or its command — for the
/// tests over a verb family, whose members are spelled both ways.
type Press = fn(&mut App);

/// Open the graph's panes over a view the test built itself.
///
/// Focuses the transcript first, because a viewer now opens **in the focused
/// pane** with no exceptions — opening it from the prompt would replace the
/// prompt, which is correct behaviour and not what these tests are about.
fn open_graph_pane(app: &mut App) {
    app.set_focus(PaneKind::Output);
    app.open_kind(PaneKind::Graph);
    // The `:graph` event ends, as every event does in the loop: focus moved
    // to a graph where an Output was, and `settle_focus` says so now — not
    // at whichever later event happens to settle first.
    app.settle();
}

/// What the pane under a screen position shows, or `None` off the panes.
fn kind_at(app: &App, frame: Rect, pos: Position) -> Option<PaneKind> {
    kind_at_ws(&app.workspace, frame, pos)
}

fn kind_at_ws<W: crate::workspace::Pane>(
    ws: &Workspace<W>,
    frame: Rect,
    pos: Position,
) -> Option<PaneKind> {
    ws.pane_at(frame, pos).and_then(|id| ws.kind_of(id))
}

/// A pane's rect in a computed layout, found by **what it shows**. Panics
/// if it isn't on screen — the tests that use this are asserting geometry
/// and want the absence to be loud.
fn pane_rect(layout: &crate::layout::FrameLayout, kind: PaneKind) -> Rect {
    layout
        .first_of(kind)
        .unwrap_or_else(|| panic!("no {kind:?} pane on screen"))
        .rect
}

/// The same, where absence is the thing under test.
fn maybe_pane_rect(layout: &crate::layout::FrameLayout, kind: PaneKind) -> Option<Rect> {
    layout.first_of(kind).map(|p| p.rect)
}

fn rendered_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = *buf.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// What the screen shows at `width` × 30, as text — one draw, the way the
/// event loop draws.
fn screen(app: &mut App, width: u16) -> String {
    text_of(app, width, 30)
}

/// What a `w` × `h` screen shows, as text, a line a row ([`frame_of`]).
fn text_of(app: &mut App, w: u16, h: u16) -> String {
    rendered_text(&frame_of(app, w, h))
}

/// **One `w` × `h` frame, drawn the way the loop draws** ([`draw_frame`]),
/// and its cells — for a test that draws once; one that draws again, or
/// compares two frames of one terminal, keeps its own.
fn frame_of(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    draw_frame(app, &mut terminal);
    terminal.backend().buffer().clone()
}

/// **[`frame_of`], and where the terminal's cursor is** — `None` when the
/// frame hid it. The cursor is the terminal's, placed by `view::place`, so
/// no cell of the buffer says where it is: ask the backend.
fn cursor_of(app: &mut App, w: u16, h: u16) -> (ratatui::buffer::Buffer, Option<Position>) {
    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    draw_frame(app, &mut terminal);
    let backend = terminal.backend();
    let at = backend.cursor_visible().then(|| backend.cursor_position());
    (backend.buffer().clone(), at)
}

/// **A `w` × `h` frame drawn for what drawing does** — the app's draw-time
/// state (a call read, a layout measured), what is on it not asked.
fn draw_at(app: &mut App, w: u16, h: u16) {
    frame_of(app, w, h);
}

/// **Every cell where `needle` is drawn**, top to bottom, left to right —
/// the cell its first symbol is in. Matched cell by cell, so a column is a
/// cell: the copies this replaced joined a row as text, found the needle's
/// BYTE offset and counted its CHARS, which agree with cells only while
/// every cell holds one one-char symbol (review stop 12, T1 14, T4 6).
fn cells_of<'a>(
    buf: &'a ratatui::buffer::Buffer,
    needle: &'a str,
) -> impl Iterator<Item = Position> + 'a {
    let area = *buf.area();
    (area.top()..area.bottom()).flat_map(move |y| {
        (area.left()..area.right()).filter_map(move |x| {
            let mut rest = needle;
            let mut at = x;
            while !rest.is_empty() && at < area.right() {
                rest = rest.strip_prefix(buf[(at, y)].symbol())?;
                at += 1;
            }
            (rest.is_empty() && !needle.is_empty()).then_some(Position::new(x, y))
        })
    })
}

/// The first of [`cells_of`].
fn cell_of(buf: &ratatui::buffer::Buffer, needle: &str) -> Option<Position> {
    cells_of(buf, needle).next()
}

#[test]
fn cells_of_matches_cell_by_cell() {
    let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 12, 2));
    buf.set_string(0, 0, "ab ab", ratatui::style::Style::default());
    // A symbol of two chars in one cell: a char offset is one cell off.
    buf.set_string(0, 1, "e\u{301}x ab", ratatui::style::Style::default());
    let found: Vec<_> = cells_of(&buf, "ab").collect();
    assert_eq!(
        found,
        [
            Position::new(0, 0),
            Position::new(3, 0),
            Position::new(3, 1)
        ]
    );
    assert_eq!(cell_of(&buf, "x ab"), Some(Position::new(1, 1)));
    assert_eq!(
        cell_of(&buf, "e\u{301}x"),
        Some(Position::new(0, 1)),
        "the cell, whole"
    );
    assert_eq!(cell_of(&buf, "abc"), None, "no partial match at the edge");
    assert_eq!(cell_of(&buf, ""), None, "nothing is not drawn");
    // Ending in the last column — the copy that never tried it missed this.
    buf.set_string(9, 0, "xyz", ratatui::style::Style::default());
    assert_eq!(cell_of(&buf, "xyz"), Some(Position::new(9, 0)));
}

/// The status row as drawn, with `[tui.status] <kind>` set to `format` — a
/// row showing the values a test asks about, which the shipped one for that
/// pane may not.
fn status_row(app: &mut App, kind: &str, format: &str) -> String {
    app.dispatch_command_for_test(&format!("set tui.status.{kind}={format}"));
    app.new_moment(); // the next key clears what `:set` said
    let shown = screen(app, 100);
    shown.lines().last().expect("a status row").to_string()
}

/// Override the graph style on a test app (config is an `Arc`).
fn set_graph_style(app: &mut App, style: shannon_core::GraphStyle) {
    let mut cfg = (*app.config).clone();
    cfg.tui.graph_style = style;
    app.config = std::sync::Arc::new(cfg);
}

/// What the newest request carried, as message texts — the request itself,
/// as the backend received it (`RecordingStub`).
fn last_sent(backend: &shannon_core::backend::RecordingStub) -> Vec<String> {
    let requests = backend.requests.lock().unwrap();
    let last = requests.last().expect("a request went out");
    last.messages.iter().map(|m| m.content.clone()).collect()
}

/// An app over a JsonlStore at `dir` with two persisted turns (q1/a1, q2/a2).
fn app_with_two_turns(dir: &std::path::Path, cfg: Config) -> App {
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir).unwrap()),
    );
    for (q, a) in [("q1", "a1"), ("q2", "a2")] {
        app.conversation_mut().begin_turn(q);
        app.conversation_mut().push_chunk(a);
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    app
}

fn branch_msgs(app: &App) -> Vec<String> {
    app.conversation()
        .messages()
        .iter()
        .map(|m| m.content.clone())
        .collect()
}

/// An app over a JsonlStore at `dir` with `n` persisted turns
/// ("Hello 0"/"reply 0" … "Hello n-1"/"reply n-1").
fn app_with_n_turns(dir: &std::path::Path, n: usize) -> App {
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir).unwrap()),
    );
    persist_n_turns(&mut app, n);
    app
}

/// [`app_with_n_turns`]'s history, on an app built otherwise — another
/// backend, another config.
fn persist_n_turns(app: &mut App, n: usize) {
    for i in 0..n {
        app.conversation_mut().begin_turn(format!("Hello {i}"));
        app.conversation_mut().push_chunk(&format!("reply {i}"));
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
}

/// **A second launch over `dir`'s history**, starting fresh — so every stored
/// session is open NOWHERE, and its words reach the app only through the store
/// (which hands over outlines first: `test_app_with_store`).
///
/// The fixture the lazy-body tests need and the rest of this file never built:
/// a session open in a buffer has its bodies overlaid from memory, so a test
/// that composes, edits or deletes the session it is standing in cannot tell a
/// path that loads the words from one that forgot to. Six mutations that
/// skipped the load passed the suite for exactly that reason.
fn relaunched(dir: &std::path::Path) -> App {
    relaunched_counting(dir).0
}

/// [`relaunched`], and how many times anything has asked the store for
/// bodies since — for the guards whose whole subject is a cost.
fn relaunched_counting(dir: &std::path::Path) -> (App, Arc<std::sync::atomic::AtomicUsize>) {
    let (store, body_loads) =
        shannon_core::store::LazyBodies::counted(Box::new(JsonlStore::new(dir).unwrap()));
    let app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    assert!(
        app.buffers
            .sessions()
            .all(|b| b.conversation.episode().segments().is_empty()),
        "premise: a fresh launch holds no stored session open"
    );
    (app, body_loads)
}

/// Select `episode`'s turn `index` in a `:graph` pane over `app`.
fn select_stored_turn(app: &mut App, episode: EpisodeId, index: usize) {
    let mut gv = app.build_graph_view();
    gv.reveal_and_select_episode(episode);
    open_graph_pane(app);
    app.set_graph_view_for_test(gv);
    for _ in 0..=index {
        app.graph_key_for_test(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    }
    assert!(
        matches!(
            app.graph_view().and_then(GraphView::selected_node),
            Some(Node::Turn { index: i, .. }) if i == index
        ),
        "premise: the cursor is on turn {index}"
    );
}

/// Build the edited-transcript text for a `commit_history_edit` call from the
/// (role, content) messages the user "typed" — the same `to_editable` the seed
/// uses, so a test drives `e` exactly as `$EDITOR` would.
fn edited_transcript(msgs: &[(Role, &str)]) -> String {
    let messages: Vec<Message> = msgs
        .iter()
        .map(|(r, c)| match r {
            Role::User => Message::user(*c),
            Role::Assistant => Message::assistant(*c),
        })
        .collect();
    to_editable(&messages)
}

/// Continue the foreground session with one turn, persisted.
fn continue_with(app: &mut App, prompt: &str) {
    app.conversation_mut().begin_turn(prompt);
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
}

/// What `episode` reads, by first message, from a fresh load of `dir` —
/// what a restart would show.
fn on_disk(dir: &std::path::Path, episode: &EpisodeId) -> Vec<String> {
    let disk = JsonlStore::new(dir).unwrap().load_graph().unwrap();
    disk.records(episode)
        .unwrap_or_default()
        .iter()
        .map(|t| t.messages[0].content.clone())
        .collect()
}

#[test]
fn a_child_prefix_is_recognized_exactly() {
    // `/` isn't special in Insert-mode vim, so the prefix is ordinary
    // typing — recognition must be exact-word, never substring.
    assert_eq!(child_invocation("/child say hi"), Some("say hi"));
    assert_eq!(child_invocation("/child"), Some(""), "bare → $EDITOR path");
    assert_eq!(child_invocation("  /child -m haiku"), Some("-m haiku"));
    assert_eq!(
        child_invocation("/child\na multi-line\nbriefing"),
        Some("a multi-line\nbriefing")
    );
    assert_eq!(
        child_invocation("/childish stories please"),
        None,
        "a longer word is a prompt, not a command"
    );
    assert_eq!(
        child_invocation("tell me about /child"),
        None,
        "only a PREFIX invokes"
    );
}

/// `Config::default()` with the `[agents]` guardrails set — the knobs the
/// posture tests turn.
fn agents_config(max_depth: usize, max_fanout: usize, on_limit: OnLimit) -> Config {
    let mut c = Config::default();
    c.agents.max_depth = max_depth;
    c.agents.max_fanout = max_fanout;
    c.agents.on_limit = on_limit;
    c
}

#[test]
fn tail_chars_is_char_boundary_safe() {
    assert_eq!(tail_chars("hello", 2), "lo");
    assert_eq!(tail_chars("hello", 99), "hello", "n past the start = all");
    assert_eq!(tail_chars("hello", 0), "");
    // Multibyte: the cut must land on a boundary, never mid-codepoint.
    assert_eq!(tail_chars("héllo wörld", 4), "örld");
    assert_eq!(tail_chars("😀😀😀", 2), "😀😀");
}

/// How many rows the graph pane has marked; `None` without one.
fn marked(app: &App) -> Option<usize> {
    app.graph_view().map(|v| v.marked_nodes().len())
}

/// Build a second, persisted session and return its id (the previous session
/// stays on disk; the new one is active).
fn add_persisted_session(app: &mut App, prompt: &str) -> EpisodeId {
    app.reset_foreground();
    app.conversation_mut().begin_turn(prompt);
    app.conversation_mut().push_chunk("reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    app.conversation().episode().id
}

fn type_into(app: &mut App, s: &str) {
    for c in s.chars() {
        app.prompt_mut().apply(EditAction::Insert(c));
    }
}

/// Type and submit a command via `command_input` (records it in history).
fn submit_command(app: &mut App, cmd: &str) {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    app.open_command_line();
    for c in cmd.chars() {
        app.command_input(k(KeyCode::Char(c)));
    }
    app.command_input(k(KeyCode::Enter));
}

/// Open the `:` line and type `line` (no submit) — the setup for completion.
fn type_command(app: &mut App, line: &str) {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    app.open_command_line();
    for c in line.chars() {
        app.command_input(k(KeyCode::Char(c)));
    }
}

#[test]
fn pane_at_maps_positions_to_panes() {
    let area = Rect::new(0, 0, 80, 24);
    let ws = test_app().workspace;
    let layout = ws.frame_layout(area);
    let center = |r: Rect| Position {
        x: r.x + r.width / 2,
        y: r.y + r.height / 2,
    };
    // The output pane is the chat surface; the prompt pane is the prompt.
    assert_eq!(
        kind_at_ws(&ws, area, center(pane_rect(&layout, PaneKind::Output))),
        Some(PaneKind::Output)
    );
    assert_eq!(
        kind_at_ws(&ws, area, center(pane_rect(&layout, PaneKind::Prompt))),
        Some(PaneKind::Prompt)
    );
    // A point outside the frame belongs to no pane.
    assert_eq!(kind_at_ws(&ws, area, Position { x: 200, y: 200 }), None);
}

/// **The first key of `spec` in vim's notation, and the rest** — `<Esc>`,
/// `<CR>`, `<BS>`, `<Tab>`, `<Up>`, `<Down>`, `<Left>`, `<Right>` and
/// `<C-x>` name a key; any other char is itself, a capital with SHIFT, as a
/// terminal reports it.
fn next_key(spec: &str) -> Option<(KeyEvent, &str)> {
    const NAMED: [(&str, KeyCode); 8] = [
        ("<Esc>", KeyCode::Esc),
        ("<CR>", KeyCode::Enter),
        ("<BS>", KeyCode::Backspace),
        ("<Tab>", KeyCode::Tab),
        ("<Up>", KeyCode::Up),
        ("<Down>", KeyCode::Down),
        ("<Left>", KeyCode::Left),
        ("<Right>", KeyCode::Right),
    ];
    if let Some((name, code)) = NAMED.iter().find(|(name, _)| spec.starts_with(name)) {
        return Some((
            KeyEvent::new(*code, KeyModifiers::NONE),
            &spec[name.len()..],
        ));
    }
    if let Some(rest) = spec.strip_prefix("<C-") {
        let mut chars = rest.char_indices();
        if let (Some((_, c)), Some((end, '>'))) = (chars.next(), chars.next()) {
            return Some((
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                &rest[end + 1..],
            ));
        }
    }
    let c = spec.chars().next()?;
    let mods = match c.is_ascii_uppercase() {
        true => KeyModifiers::SHIFT,
        false => KeyModifiers::NONE,
    };
    Some((KeyEvent::new(KeyCode::Char(c), mods), &spec[c.len_utf8()..]))
}

/// **One char as a terminal sends it** — a capital with SHIFT ([`next_key`]).
fn key(c: char) -> KeyEvent {
    let (key, _) = next_key(c.encode_utf8(&mut [0; 4])).expect("a char is a key");
    key
}

/// **`keys` typed into an 80×24 terminal** — see [`type_keys_in`].
fn type_keys(app: &mut App, keys: &str) {
    type_keys_in(app, Rect::new(0, 0, 80, 24), keys);
}

/// **`keys` typed into a terminal of `frame`** — each at the focused pane's
/// own content size there, asked per key, since a key can move the focus.
/// See [`type_each`].
fn type_keys_in(app: &mut App, frame: Rect, keys: &str) {
    type_each(app, keys, |app| {
        app.pane_content_size(app.workspace.focused(), frame)
    });
}

/// **`keys` typed into a focused pane of `size`** — for a test whose
/// arithmetic is a pane of that size, its readers asking the same. See
/// [`type_each`].
fn type_keys_sized(app: &mut App, size: (u16, u16), keys: &str) {
    type_each(app, keys, |_| size);
}

/// **`keys` typed as a user types them**: each through the route a keypress
/// takes, ended as the loop ends an event (`key_for_test`), at the pane size
/// `size` answers. [`next_key`] reads the notation.
///
/// The one typer: there were thirteen, at five sizes, three without a
/// capital's SHIFT, one replaying `handle_outcome` by hand, one that
/// settled (review stop 12, Tier 3).
fn type_each(app: &mut App, keys: &str, size: impl Fn(&App) -> (u16, u16)) {
    let mut rest = keys;
    while let Some((key, more)) = next_key(rest) {
        let size = size(app);
        app.key_for_test(key, size);
        rest = more;
    }
}

#[test]
fn next_key_reads_vims_notation() {
    let read = |spec: &str| {
        let mut rest = spec;
        let mut out = Vec::new();
        while let Some((key, more)) = next_key(rest) {
            out.push((key.code, key.modifiers));
            rest = more;
        }
        out
    };
    let (none, shift, ctrl) = (
        KeyModifiers::NONE,
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
    );
    assert_eq!(
        read("<Esc>gG<C-w>l<CR>"),
        vec![
            (KeyCode::Esc, none),
            (KeyCode::Char('g'), none),
            (KeyCode::Char('G'), shift),
            (KeyCode::Char('w'), ctrl),
            (KeyCode::Char('l'), none),
            (KeyCode::Enter, none),
        ]
    );
    // A `<` that names nothing is typed; so is a wide char.
    assert_eq!(
        read("<x<C-é"),
        vec![
            (KeyCode::Char('<'), none),
            (KeyCode::Char('x'), none),
            (KeyCode::Char('<'), none),
            (KeyCode::Char('C'), shift),
            (KeyCode::Char('-'), none),
            (KeyCode::Char('é'), none),
        ]
    );
}

/// The prompt holding `lines`, in Normal mode, the cursor at `(row, col)` —
/// reached by keys, as a user would.
fn normal_at(lines: &[&str], row: usize, col: usize) -> App {
    let mut app = test_app();
    app.prompt_mut().replace_with(&lines.join("\n"));
    type_keys(&mut app, "<Esc>gg0");
    for _ in 0..row {
        type_keys(&mut app, "j");
    }
    for _ in 0..col {
        type_keys(&mut app, "l");
    }
    assert_eq!(app.prompt().cursor(), (row, col), "premise: placed");
    app
}

/// **A terminal squeeze is temporary; the prompt comes back.**
///
/// Steven, dogfooding 2026-08-24: shrinking a ghostty split took the prompt
/// down to one line, and expanding it again left it there. `clamp` wrote the
/// squeezed value back over the size he had asked for, so growing the terminal
/// had nothing to restore — a ratchet, and the only way out was `<C-w>=`.
///
/// Both directions are asserted because only the pair is the bug: clamping
/// down was always correct, and a fix that stopped clamping would hide the
/// conversation on a small frame instead.
#[test]
fn a_terminal_squeeze_does_not_outlive_the_small_terminal() {
    let mut app = test_app();
    let wanted = prompt_content_rows(&app);
    assert!(
        wanted > 2,
        "premise: the prompt asks for more than the floor"
    );

    app.workspace.clamp(100, 8);
    let squeezed = prompt_content_rows(&app);
    assert!(
        squeezed < wanted,
        "premise: 8 rows really is too few for {wanted}, or the grow proves \
         nothing — got {squeezed}"
    );

    app.workspace.clamp(100, 40);
    assert_eq!(
        prompt_content_rows(&app),
        wanted,
        "the prompt must return to the size it asked for once there is room"
    );
}

/// **...and what comes back is what you last asked for, not the config.**
///
/// The other half of the same fix: `clamp` re-derives the live size from a
/// remembered wish, so a `<C-w>+` has to *become* that wish or the next
/// terminal resize would quietly undo it. Without this the fix above would
/// trade a ratchet for an eraser.
#[test]
fn a_deliberate_resize_becomes_the_size_a_squeeze_restores() {
    let mut app = test_app();
    let base = prompt_content_rows(&app);
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(3), 100, 40),
        "not a quit"
    );
    assert_eq!(
        prompt_content_rows(&app),
        base + 3,
        "premise: the resize took"
    );

    app.workspace.clamp(100, 8);
    assert!(prompt_content_rows(&app) < base, "premise: squeezed");
    app.workspace.clamp(100, 40);

    assert_eq!(
        prompt_content_rows(&app),
        base + 3,
        "a squeeze restores what you ASKED for, not the configured default"
    );
}

#[test]
fn shrinking_the_frame_reclamps_the_prompt() {
    let mut app = test_app();
    // Grow the prompt to fill a tall (50-row) frame...
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(9999), 80, 50),
        "not a quit"
    );
    assert!(prompt_content_rows(&app) > app.config.tui.prompt.height);
    // ...then the terminal shrinks. The `Event::Resize` arm re-fits the
    // prompt via exactly this call, so the output keeps its sliver.
    app.workspace.clamp(80, 12);
    assert!(prompt_content_rows(&app) <= 12u16.saturating_sub(6));
}

#[test]
fn scroll_delta_maps_magnitudes_and_clamps() {
    // One table for the whole mapping (consolidated in the M3 test
    // review from four one-liners): page = height−1 (one line of
    // overlap), half-page = height/2, line = 1 — and every magnitude
    // clamps up to 1 on panes too small to have a meaningful page.
    for (mag, h, want) in [
        (ScrollMagnitude::Page, 10, 9),
        (ScrollMagnitude::HalfPage, 10, 5),
        (ScrollMagnitude::Line, 10, 1),
        (ScrollMagnitude::Page, 1, 1),
        (ScrollMagnitude::Page, 0, 1),
        (ScrollMagnitude::HalfPage, 1, 1),
    ] {
        assert_eq!(scroll_delta(mag, h), want, "{mag:?} at height {h}");
    }
}

/// An app copying as `mode` says, its clipboard, and the handle of `hunter2`
/// — marked, and typed in the prompt as `pw=hunter2!` with `pw=hunt`, half of
/// it, selected.
fn copying(mode: shannon_core::CopySecrets) -> (App, MemoryClipboard, String) {
    let mut cfg = Config::default();
    cfg.tui.copy_secrets = mode;
    cfg.tui.copy_on_select = true;
    let mut app = test_app_with(cfg);
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    let handle = app.mark_secret("hunter2", true).expect("marked");
    type_into(&mut app, "pw=hunter2!");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0");
    type_keys(&mut app, "v");
    for _ in 0..6 {
        type_keys(&mut app, "l");
    }
    (app, clip, handle)
}

/// An app holding one pretend secret, and the handle minted for it.
fn app_with_secret(name: &str, value: &str) -> (App, String) {
    app_holding(name, value, true)
}

/// An app holding one entry of the given sensitivity.
fn app_holding(name: &str, value: &str, sensitive: bool) -> (App, String) {
    let mut app = test_app();
    let mut aliases = shannon_core::Aliases::default();
    let handle = aliases
        .insert(name, value, "test", sensitive)
        .expect("mint");
    let aliases = Arc::new(aliases);
    app.aliases = aliases.clone();
    app.rules = shannon_core::permission::RuleSet::defaults().with_aliases(aliases);
    (app, handle)
}

/// The bash tool's description as the newest request carried it — what the
/// model is told, not what `ToolDef` would build.
fn sent_tool_description(backend: &shannon_core::backend::RecordingStub) -> String {
    let requests = backend.requests.lock().unwrap();
    let last = requests.last().expect("a request went out");
    last.tools
        .first()
        .expect("the bash tool")
        .description
        .clone()
}

/// An app on a recording backend, `aliases` its vault.
fn recorded_holding(
    aliases: shannon_core::Aliases,
) -> (App, Arc<shannon_core::backend::RecordingStub>) {
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    let aliases = Arc::new(aliases);
    app.aliases = aliases.clone();
    app.rules = shannon_core::permission::RuleSet::defaults().with_aliases(aliases);
    (app, backend)
}

/// **Everything written to the log while `f` runs.**
///
/// Exists because a scrub on the way to the log has no other witness: assert on
/// `App::scrub` and you have tested `scrub`, not the call site that was
/// supposed to use it — the log sites are the half that rots
/// (`[[feedback_wiring_is_untested]]`). Every guard here is mutation-checked
/// against this, so removing a `scrub(…)` from a `warn!` fails a test rather
/// than passing quietly.
///
/// **ONE global subscriber for the whole binary, routing to a per-thread
/// buffer.** Not `with_default`, and the reason is a bug this very helper had.
///
/// The first version installed a *scoped* subscriber per test. It failed about
/// once in forty — green on the run it was committed on, which is worse than
/// red. The captured failure showed the shape exactly: `post` emits an `info!`
/// and a `warn!` microseconds apart, both reached the screen, and only the
/// `INFO` line was captured. A level filter cannot do that — `WARN` is *less*
/// verbose than `INFO`, so anything admitting one admits the other. It is
/// per-**callsite**: `tracing` caches an `Interest` per callsite, and that
/// cache is computed only from **globally registered** dispatchers. A
/// `with_default` subscriber is not one, so a callsite first hit by another
/// test on another thread can cache `Interest::never()` and then be skipped
/// before any dispatcher is consulted.
///
/// So the fix is not to serialize (tried; it still failed) but to use the
/// configuration tracing's caching is designed for: a global subscriber,
/// installed once, never dropped. Routing is per-thread instead, so concurrent
/// capture tests cannot see each other's lines and no serialization is needed.
///
/// `TRACE` so a `debug!` under test is captured whatever `RUST_LOG` says in the
/// environment the suite happens to run in. Events from threads that are not
/// capturing are formatted and dropped — a few thousand over the suite, which
/// does not move the wall clock.
///
/// It exists at all because a scrub on the way to the log has no other witness:
/// assert on `App::scrub` and you have tested `scrub`, not the call site that
/// was supposed to use it — the log sites are the half that rots
/// (`[[feedback_wiring_is_untested]]`). Every guard here is mutation-checked
/// through it.
fn captured_logs(f: impl FnOnce()) -> String {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, OnceLock};

    thread_local! {
        /// Where this thread's lines go while it is capturing.
        static CAPTURE: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Copy)]
    struct ThreadSink;

    impl std::io::Write for ThreadSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            CAPTURE.with(|slot| {
                if let Some(sink) = slot.borrow().as_ref() {
                    sink.lock().expect("log sink").extend_from_slice(buf);
                }
            });
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ThreadSink {
        type Writer = ThreadSink;
        fn make_writer(&'a self) -> Self::Writer {
            *self
        }
    }

    /// Clears the thread's slot even if `f` panics, so one failing capture
    /// test cannot leave a later one writing into a buffer nobody reads.
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            CAPTURE.with(|slot| *slot.borrow_mut() = None);
        }
    }

    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadSink)
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        // Ignore the error: another harness setting one first is fine, and a
        // test binary must not abort over it.
        let _ = tracing::subscriber::set_global_default(subscriber);
    });

    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&buf)));
    let _clear = Clear;
    f();
    drop(_clear);
    let bytes = buf.lock().expect("log sink").clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What the log holds after a command finishes under `level` — driven
/// through `handle_exec_outcome`, the caller: these called `audit` with
/// arguments of their own, so the caller passing no output passed them
/// (review stop 12, T2 4; the rule at
/// `an_audit_completion_record_can_be_matched_to_its_command`).
fn audit_of_an_outcome(level: shannon_core::AuditLevel) -> String {
    let mut cfg = Config::default();
    cfg.exec.audit = level;
    let mut app = test_app_with(cfg);
    let outcome = shannon_core::ExecOutcome {
        tool_use_id: "toolu_01".to_string(),
        command: "cargo test --quiet".to_string(),
        output: "3 passed".to_string(),
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: Vec::new(),
        attempts: 1,
        kept: None,
        left: shannon_core::LeftBehind::Nothing,
        file: None,
    };
    captured_logs(|| app.handle_exec_outcome(outcome))
}

/// Every character drawn in the secret style, in screen order — what a user
/// sees marked. Read off the rendered cells, not the spans the view meant to
/// paint: the mapping from "is a secret" to a colour is its own place to be
/// wrong.
fn secret_cells(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            let style = buf[(x, y)].style();
            if style.fg == Some(crate::palette::SECRET)
                && style.add_modifier.contains(ratatui::style::Modifier::BOLD)
            {
                out.push_str(buf[(x, y)].symbol());
            }
        }
    }
    out
}

/// A key long enough to wrap in a 40-column pane — the shape of the demo bug
/// (a pasted API key, auto-marked, drawn as plain text past its first row).
const WRAPPING_SECRET: &str = "sk-test-notreal-0123456789abcdefghijklmnopqrstuvwxyz0123456789";

/// A transcript whose secret is in a LATER block — the third, after a whole
/// exchange — and on that block's SECOND line, so a row's origin must name
/// both the block and the line within it — then a reply that names its
/// handle. What is marked, in screen order: the value in the user's line
/// (shown literally), then the reply's `value (handle)` — value and handle.
/// Returns that expected marking beside the app.
fn transcript_with_a_late_wrapped_secret() -> (App, String) {
    let mut app = test_app();
    let handle = app.mark_secret(WRAPPING_SECRET, true).expect("marked");
    app.submit("first, an ordinary question");
    app.feed_stream(StreamMsg::Chunk("an ordinary answer".into()));
    app.feed_stream(StreamMsg::Complete);
    app.submit(&format!("then the key\n{WRAPPING_SECRET}"));
    app.feed_stream(StreamMsg::Chunk(format!("kept as {handle}")));
    app.feed_stream(StreamMsg::Complete);
    (app, format!("{WRAPPING_SECRET}{WRAPPING_SECRET}{handle}"))
}

/// **`:scriptnames`, as its pane shows it** — dispatched, the pane open (a
/// window, not a line: Steven, 10-13), a frame drawn (the step that keeps
/// a listing live), and the PANE's own lines read: the listing function's
/// text passed with the pane showing anything (review 2026-10-13, C).
fn scriptnames(app: &mut App) -> String {
    app.dispatch_command_for_test("scriptnames");
    scripts_pane(app)
}

/// What the open `:scriptnames` pane shows, after a frame.
fn scripts_pane(app: &mut App) -> String {
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Scripts))
        .expect("`:scriptnames` opens its pane");
    draw_at(app, 100, 30);
    app.pane_listing(pane)
        .expect("with its page")
        .lines()
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// **The capture state around `$EDITOR` is what the terminal is doing, not
/// the live `tui.mouse`** (Reviewers A and C): a `:set` of it moves the
/// setting and leaves the recorded capture alone, both ways — and no source
/// file reads the live setting where the capture state belongs (the eight
/// suspend/resume sites need a terminal, so the lexical check is their guard).
#[test]
fn mouse_capture_is_what_launch_set_not_the_live_setting() {
    let mut app = test_app();
    assert!(app.mouse_captured, "premise: launched capturing");
    app.dispatch_command_for_test("set notui.mouse");
    assert!(!app.config.tui.mouse, "the setting moved");
    assert!(app.mouse_captured, "the terminal did not");

    let mut off = Config::default();
    off.tui.mouse = false;
    let mut app = test_app_with(off);
    app.dispatch_command_for_test("set tui.mouse");
    assert!(!app.mouse_captured, "nothing turned capture on");

    for (name, src) in [
        ("app.rs", include_str!("../app.rs")),
        ("app/session_ops.rs", include_str!("session_ops.rs")),
        ("app/config_file.rs", include_str!("config_file.rs")),
    ] {
        assert!(
            src.contains("self.mouse_captured") || name != "app.rs",
            "premise: {name} is the source that passes it"
        );
        assert!(
            !src.contains(concat!("self.config", ".tui.mouse")),
            "{name} passes the live setting where the capture state belongs"
        );
    }
}

/// An app with a session and a clone of it, the clone in front, and `:graph`
/// open with `marks` marked — what a batch delete starts from.
fn original_and_clone(dir: &std::path::Path) -> (App, EpisodeId, EpisodeId) {
    let mut app = app_with_n_turns(dir, 2);
    let original = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(original);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.dispatch_command_for_test("clone");
    let clone = app.conversation().episode().id;
    assert_ne!(clone, original, "premise: cloned");
    (app, original, clone)
}

/// Mark `nodes` in a fresh graph view, press `d`, and answer `answer`.
fn delete_marked(app: &mut App, nodes: &[Node], answer: char) {
    let mut gv = GraphView::new(app.current_graph());
    assert_eq!(gv.mark_nodes(nodes), nodes.len(), "premise: all marked");
    app.set_graph_view_for_test(gv);
    app.notices.clear();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(app.confirm_prompt().is_some(), "premise: asked");
    type_keys(app, &answer.to_string());
}

/// Two persisted sessions and two transcript panes, focus on the second pane.
/// Returns `(A, B, pane showing A's neighbour, focused pane)`.
fn app_with_two_sessions(
    dir: &std::path::Path,
) -> (
    App,
    EpisodeId,
    EpisodeId,
    crate::wm::PaneId,
    crate::wm::PaneId,
) {
    let mut app = app_with_n_turns(dir, 2); // session A, persisted
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0"); // now the active session
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("output", SplitDir::Vertical);
    let second = app.workspace.focused();
    assert_ne!(first, second, "two transcript panes");
    (app, a, b, first, second)
}

/// Does `pane`'s window keep a tree in its history — `<CR>`'s alternate?
fn keeps_a_tree(app: &App, pane: crate::wm::PaneId) -> bool {
    app.window_of(pane)
        .is_some_and(|v| (v.history.iter()).any(|away| matches!(away, Away::View(Viewed::Tree(_)))))
}

/// What `pane`'s window history holds, oldest first, said plainly.
fn history_of(app: &App, pane: crate::wm::PaneId) -> Vec<String> {
    (app.window_of(pane).into_iter())
        .flat_map(|v| v.history.iter())
        .map(|away| match away {
            Away::View(view) => format!("view {}", view.kind().title()),
            Away::Session(buffer, _) => format!("session b{}", buffer.number()),
        })
        .collect()
}

/// The buffers `pane`'s window holds in its history, oldest first —
/// [`history_of`]'s entries by WHICH buffer, `:b N`'s question, where a
/// view's says only its kind (V5.3 verify, F6).
fn held_buffers(app: &App, pane: crate::wm::PaneId) -> Vec<BufferId> {
    (app.window_of(pane).into_iter())
        .flat_map(|v| v.history.iter())
        .map(Away::buffer)
        .collect()
}

/// `pane`'s window, scrolled back with its cursor on line 2 of what it
/// shows and line 1 at its top — a place to come back to — and that place:
/// its cursor and the text at its top.
fn stand_back_in(
    app: &mut App,
    pane: crate::wm::PaneId,
) -> ((usize, usize), Option<(usize, usize)>) {
    app.sync_chat_in(pane);
    let lines = app.pane_chat(pane).line_count();
    assert!(lines > 3, "premise: lines to stand on ({lines})");
    app.pane_chat_mut(pane).unwrap().place_cursor(2, 0);
    app.window_of_mut(pane)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport
        .freeze_at(1, 0);
    let vp = app.pane_viewport(pane);
    assert!(!vp.is_following_tail(), "premise: scrolled back");
    (app.pane_chat(pane).cursor(), vp.top())
}

/// `<C-^>` as a terminal sends it — the byte 0x1E, CTRL with `6` — or as one
/// reporting keys in full sends it, CTRL and SHIFT with `^`.
fn ctrl_hat(full: bool) -> KeyEvent {
    match full {
        false => KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL),
        true => KeyEvent::new(
            KeyCode::Char('^'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
    }
}

/// `<C-^>` pressed in the focused pane as production takes it: no viewer
/// claims a CTRL chord (`viewer_claims_key`), so the keymap translates it,
/// and its window command is applied — the one arm the key seam leaves to
/// `window_command_for_test`.
fn press_ctrl_hat(app: &mut App, full: bool) {
    let key = ctrl_hat(full);
    assert!(
        matches!(app.route_key(&key), KeyRoute::Keymap),
        "premise: the keymap's key"
    );
    match app.focused_keymap_mut().translate(key) {
        KeyOutcome::Window(WindowCmd::Alternate, _) => {
            app.window_command_for_test(WindowCmd::Alternate);
        }
        other => panic!("<C-^> is the alternate, not {other:?}"),
    }
}

/// Two stored sessions, A then B, each in its own buffer, on a store that
/// refuses to delete whatever the returned list names: `(app, refused, a,
/// a_buffer, b, b_buffer)`.
#[allow(clippy::type_complexity)]
fn two_buffers_on_a_store_refusing_deletes() -> (
    App,
    Arc<std::sync::Mutex<Vec<EpisodeId>>>,
    EpisodeId,
    BufferId,
    EpisodeId,
    BufferId,
) {
    let (store, refused) = shannon_core::store::FailingStore::refusing_deletes();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "A 0");
    let (a, a_buffer) = (app.conversation().episode().id, app.current);
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Vertical);
    app.new_session_command();
    continue_with(&mut app, "B 0");
    let (b, b_buffer) = (app.conversation().episode().id, app.current);
    assert_ne!(a_buffer, b_buffer, "premise: two buffers");
    assert_eq!(
        app.store.list_episodes().map(|ids| ids.len()).ok(),
        Some(2),
        "premise: two stored"
    );
    (app, refused, a, a_buffer, b, b_buffer)
}

/// An app with a `prefill` continuation in flight — the shared premise of the
/// three tests above, so a change to how one is started lands in one place.
///
/// Hands the `TempDir` back rather than leaking it: the store borrows the path
/// for the app's life, and a leaked temp directory is a cost this suite would
/// be pushing onto the machine it runs on (`[[feedback_no_externalized_costs]]`).
async fn app_mid_prefill_continuation() -> (App, TempDir) {
    use shannon_core::{ContinueStrategy, StopReason};
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // A turn truncated by `max_tokens` — the only thing `:continue` resumes.
    app.output_mut().begin_turn("q");
    app.conversation_mut().begin_turn("q");
    app.feed_stream(StreamMsg::Chunk("cut".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.persist_committed_turn();
    app.continue_last_turn();
    (app, dir)
}

/// A delete of the first turn of `episode`, as `d` on its row builds it.
fn first_turn_delete(app: &App, episode: EpisodeId) -> ConfirmAction {
    let own = app
        .buffers
        .sessions()
        .find(|b| b.conversation.episode().id == episode)
        .map(|b| b.conversation.episode().own_turns().to_vec())
        .unwrap_or_default();
    ConfirmAction::Delete {
        episodes: Vec::new(),
        turns: vec![TurnTarget {
            episode,
            index: 0,
            turn: own[0],
            of_total: own.len(),
        }],
        leaves_gap: false,
        claude_left: 0,
        from: None,
    }
}

/// A test app whose commands never end, and the executor, to ask what was
/// cancelled.
fn app_on_a_hanging_executor() -> (App, Arc<shannon_core::HangingExecutor>) {
    let mut app = test_app();
    let exec = Arc::new(shannon_core::HangingExecutor::default());
    app.executor = exec.clone();
    (app, exec)
}

/// Yield until `exec` has cancelled `n` commands.
async fn cancelled_after_yielding(exec: &shannon_core::HangingExecutor, n: usize) -> bool {
    after_yielding(|| exec.cancelled() >= n).await
}

/// Yield until `done` holds — an aborted task's future is dropped when the
/// runtime next polls it, so a cancel is seen a poll or two after the
/// `abort()`. Bounded, with no clock: the commands' cancels
/// ([`cancelled_after_yielding`]) and the requests' (`HangingBackend`).
async fn after_yielding(done: impl Fn() -> bool) -> bool {
    for _ in 0..64 {
        if done() {
            return true;
        }
        tokio::task::yield_now().await;
    }
    done()
}

/// A kind of work, as the teardown matrix walks it: `Work`'s variants with a
/// turn's two stages and a job's two kinds apart — the rows of `resolve`'s
/// match, and the lines its arms do not branch on but a user can tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkKind {
    Streaming,
    ToolRound,
    Child,
    Send,
    Summary,
    Continuation,
}

impl WorkKind {
    /// Which kind `work` is — exhaustive over the product's types, so a new
    /// kind of work has no row until it has one here.
    fn of(work: &Work) -> Self {
        match work {
            Work::Turn {
                stage: Stage::Streaming,
            } => Self::Streaming,
            Work::Turn {
                stage: Stage::ToolRound,
            } => Self::ToolRound,
            Work::Job(job) => match job.kind {
                JobKind::Child => Self::Child,
                JobKind::Send => Self::Send,
            },
            Work::Summary(_) => Self::Summary,
            Work::Continuation(_) => Self::Continuation,
        }
    }

    /// Does what it streams go into the transcript as a turn? Written out,
    /// not read off `Work::sink`: this is the matrix's expectation.
    fn makes_a_turn(self) -> bool {
        match self {
            Self::Streaming | Self::ToolRound | Self::Child | Self::Send => true,
            Self::Summary | Self::Continuation => false,
        }
    }

    /// What a delete says went with the session — written out, not asked of
    /// `Lost`, whose words these check.
    fn went_with_it(self) -> &'static str {
        match self {
            Self::Streaming | Self::ToolRound | Self::Child | Self::Send => "a turn in flight",
            Self::Summary => "a distillation",
            Self::Continuation => "a continuation",
        }
    }
}

/// One cell's work, standing: the app, the work's generation, where it runs,
/// and the backend that reports whether its request was cancelled.
struct Standing {
    app: App,
    _dir: TempDir,
    backend: Arc<shannon_core::backend::HangingBackend>,
    id: GenerationId,
    buffer: BufferId,
    episode: EpisodeId,
    /// The session the app started in, with two turns — what lands over the
    /// work's buffer when it is superseded, since a child's own has none.
    first: EpisodeId,
}

/// `kind` of work running, started through the door a user's starts it —
/// a prompt, `:Send`'s, `:child`'s, `:compact`, `:continue` — on a request
/// that never ends by itself, with "half" streamed.
///
/// **The `:Send` goes into a session never written** (`:new`): the one
/// session a delete's seal would put back on disk (`Outcome::seals`) — a
/// child's is written at its spawn, and the others stand on two turns.
fn standing(kind: WorkKind) -> Standing {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(shannon_core::backend::HangingBackend::new());
    let mut cfg = Config::default();
    cfg.continue_.strategy = shannon_core::ContinueStrategy::Prefill;
    let mut app = test_app_with_store(
        backend.clone(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    persist_n_turns(&mut app, 2);
    let first = app.conversation().episode().id;
    match kind {
        WorkKind::Streaming | WorkKind::ToolRound => app.submit("the work"),
        WorkKind::Send => {
            app.new_session_command();
            let fresh = app.conversation().episode().id;
            assert!(
                app.store.load_episode(&fresh).is_err(),
                "premise: unwritten"
            );
            app.start_job_on(app.current, "the work").expect("free");
        }
        WorkKind::Child => {
            app.spawn_child_job("the work", None).expect("spawned");
        }
        WorkKind::Summary => {
            app.dispatch_command_for_test("compact");
        }
        WorkKind::Continuation => {
            // A turn cut off by `max_tokens`, the one thing `:continue`
            // resumes — on a stream of the test's, not the backend's.
            app.output_mut().begin_turn("q");
            app.conversation_mut().begin_turn("q");
            app.feed_stream(StreamMsg::Chunk("cut".into()));
            app.feed_stream(StreamMsg::Stopped(shannon_core::StopReason::MaxTokens));
            app.feed_stream(StreamMsg::Complete);
            app.persist_committed_turn();
            app.continue_last_turn();
        }
    }
    assert_eq!(app.generations.len(), 1, "{kind:?}: premise: one work");
    let (id, buffer) = (app.generations[0].id, app.generations[0].buffer);
    app.feed_turn(id, StreamMsg::Chunk("half".into()));
    if kind == WorkKind::ToolRound {
        model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-matrix");
    }
    let work = &(app.generations.iter().find(|g| g.id == id))
        .expect("premise: still running")
        .work;
    assert_eq!(WorkKind::of(work), kind, "premise: the kind stood");
    assert!(!backend.cancelled(), "{kind:?}: premise: its request runs");
    let episode = (app.session_of(buffer).expect("its session"))
        .conversation
        .episode()
        .id;
    Standing {
        app,
        _dir: dir,
        backend,
        id,
        buffer,
        episode,
        first,
    }
}

/// What a session's records on disk say, turn by turn — its words and how
/// each ended; `None` when the store has no such session.
type OnDiskTurns = Option<Vec<(Vec<String>, Option<shannon_core::StopReason>)>>;

fn records_on_disk(app: &App, episode: &EpisodeId) -> OnDiskTurns {
    let (_, turns) = load_full_session(app.store.as_ref(), episode).ok()?;
    Some(
        turns
            .iter()
            .map(|t| {
                let words = t.messages.iter().map(|m| m.content.clone()).collect();
                (words, t.provenance.stop_reason.clone())
            })
            .collect(),
    )
}

/// The gate above, for one kind.
///
/// Split out so the gate can LOOP. Hardcoding `Help` is what let `<C-f>` be a
/// dead key in `:messages`: `scroll_pane` reached a second, narrower "does this
/// pane hold a text page" predicate, and a per-kind test passed about itself
/// while the kinds disagreed (Reviewer A) — the same mistake the P2 commit
/// message warned about, made in the test written to prevent it.
async fn every_advertised_key_works_in(kind: PaneKind) {
    // Each key, named as the old legend named it, and what performs it.
    let advertised: &[(&str, &[(KeyCode, KeyModifiers)])] = &[
        ("j/k move", &[(KeyCode::Char('j'), KeyModifiers::NONE)]),
        ("j/k move", &[(KeyCode::Char('k'), KeyModifiers::NONE)]),
        (
            "C-f/C-b page",
            &[(KeyCode::Char('f'), KeyModifiers::CONTROL)],
        ),
        (
            "C-f/C-b page",
            &[(KeyCode::Char('b'), KeyModifiers::CONTROL)],
        ),
        (
            "gg/G top/bottom",
            &[
                (KeyCode::Char('g'), KeyModifiers::NONE),
                (KeyCode::Char('g'), KeyModifiers::NONE),
            ],
        ),
        (
            "gg/G top/bottom",
            &[(KeyCode::Char('G'), KeyModifiers::NONE)],
        ),
        // Driven through the resolver below, not as a key — but still DRIVEN,
        // because a legend promising a close that does not close is exactly
        // the defect this gate exists for.
        (":q close", &[]),
    ];

    for (token, keys) in advertised {
        let mut app = test_app();
        open_pane_of(&mut app, kind);
        let pane = app.workspace.focused();
        // **Start in the middle, deterministically.** Driving `G` then ten `k`
        // reads well and depends on the document being longer than ten lines —
        // `:analyze`'s report is not, so the cursor pinned at 0 and `k` had
        // nowhere to go, failing for a reason that looks exactly like the dead
        // key this gate hunts ([[feedback_assert_the_subject]]).
        let len = app.text_pane_len_for_test(pane).expect("content");
        assert!(
            len >= 4,
            "{kind:?}: premise — the page needs room to move in both directions ({len} lines)"
        );
        if let Some(text) = app.text_pane_mut_for_test(pane) {
            text.goto_line(len / 2, 0);
        }
        // **Settle through the same door the key drive uses.** The seam clamps
        // at its own geometry before every key, and `clamp` follows the cursor
        // — so a `before` captured outside that path differs from `after` by
        // the clamp alone, and every assertion below passes whatever the key
        // did. An unbound key is the cheapest way to run the settle without
        // moving anything. (Found by mutation: skipping `Messages` in
        // `scroll_pane` left this gate green.)
        let nothing = KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE);
        app.handle_text_pane_key_for_test(kind, nothing, (80, 20));
        let before = app.text_pane_pos_for_test(pane);
        app.handle_text_pane_key_for_test(kind, nothing, (80, 20));
        assert_eq!(
            app.text_pane_pos_for_test(pane),
            before,
            "{kind:?}: premise — an unbound key must not move anything, or this \
             gate cannot tell a working key from a settling clamp"
        );

        // The last window's `:q` leaves (`App::last_window`): the tutor's.
        let mut left = false;
        if *token == ":q close" {
            let last = app.last_window();
            left = app.dispatch_command_for_test("q");
            assert_eq!(
                left, last,
                "{kind:?}: `:q` leaves from the last window only"
            );
        } else {
            for (code, mods) in *keys {
                app.handle_text_pane_key_for_test(kind, KeyEvent::new(*code, *mods), (80, 20));
            }
        }

        let after = app.text_pane_pos_for_test(pane);
        // Either the position changed, or the pane is gone (`:q`), or the app
        // left. A key that does none is the dead key this exists to catch.
        assert!(
            left || after.is_none() || after != before,
            "{kind:?}: the legend advertises {token:?} and nothing happened"
        );
    }
}

/// Everything a `:graph` key could visibly change, in one comparable value.
///
/// Broad on purpose: the keys it covers act on five different things (the
/// selection, the marks, which session you are in, a pending confirmation, the
/// notice line), and a gate that watched only one of them would call the other
/// four dead.
#[derive(Debug, PartialEq)]
struct GraphProbe {
    /// Selected row, rows on show, marks, a Visual range — `None` once the
    /// viewer is not the focused pane.
    view: Option<(usize, usize, usize, bool)>,
    session: shannon_core::EpisodeId,
    confirms: usize,
    notice: Option<String>,
    distilling: bool,
    /// What the `:` line holds, if it is open — where `r` puts `:rename `.
    command_line: Option<String>,
}

fn graph_probe(app: &App) -> GraphProbe {
    GraphProbe {
        view: app.graph_view().map(|v| {
            (
                v.selected(),
                v.visible_len(),
                v.marked_nodes().len(),
                v.in_visual(),
            )
        }),
        session: app.conversation().episode().id,
        confirms: app.pending_confirm.len(),
        notice: app.notice(),
        distilling: app.distilling().is_some(),
        command_line: app.cmdline.line().map(str::to_string),
    }
}

/// A decision pane capped at `max` rows over a report that does not fit, drawn
/// once at 80×24, with the report's lines as plain text.
async fn capped_decision(max: u16) -> (App, TestTerminal, Vec<String>) {
    use ratatui::{Terminal, backend::TestBackend};
    let mut cfg = Config::default();
    cfg.tui.approval_max_height = max;
    let mut app = test_app_with(cfg);
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("go");
    model_call(&mut app, "tu-1", "rm -rf /tmp/a | grep x | wc -l > /tmp/b");
    to_decision(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let lines = app
        .shown_call()
        .expect("a decision")
        .text()
        .lines()
        .iter()
        .map(|l| l.text())
        .collect();
    (app, terminal, lines)
}

type TestTerminal = ratatui::Terminal<ratatui::backend::TestBackend>;

/// One key at the focused decision pane, at its real size, then a frame.
fn decision_key(app: &mut App, terminal: &mut TestTerminal, key: KeyEvent) -> String {
    let size = app.pane_content_size(app.workspace.focused(), Rect::new(0, 0, 80, 24));
    app.key_for_test(key, size);
    draw_frame(app, terminal);
    rendered_text(terminal.backend().buffer())
}

/// A backend that records every request it is handed, so a test can assert on
/// **what actually went out** rather than on a predicate that says what should.
///
/// Reviewer C's top finding: the regression test for "a resumed `:Send` stays
/// attended" read `resumable()` directly and never went through
/// `resume_after_tools`. Reverting that call site to the old binding — the
/// exact bug the fix exists for — left all 959 tests green. The fix added the
/// door, the test checked the lock, and nothing checked the door swings from
/// it. This is how you check the door.
#[derive(Default)]
struct RecordingBackend {
    sent: std::sync::Arc<std::sync::Mutex<Vec<shannon_core::SendRequest>>>,
}

impl shannon_core::Backend for RecordingBackend {
    fn send_message(
        &self,
        req: shannon_core::SendRequest,
        tx: tokio::sync::mpsc::UnboundedSender<shannon_core::StreamMsg>,
    ) -> tokio::task::JoinHandle<()> {
        self.sent.lock().expect("not poisoned").push(req);
        tokio::spawn(async move {
            let _ = tx.send(shannon_core::StreamMsg::Complete);
        })
    }

    /// **Not `STUB`.** A session on the stub model is routed to the app's own
    /// internal `CannedStub` rather than to the installed backend, so a
    /// recording backend that calls itself the stub records nothing and every
    /// assertion about "what was sent" is made against an empty list.
    fn model(&self) -> &str {
        "claude-haiku-4-5-20251001"
    }
}

/// **Every path that sends a request, on a session on `model`** — at a run
/// default of `max` and a ceiling of 8,000 — and what each request carried:
/// a foreground turn (an instruct `:continue` is one: it submits), `S`, a
/// prefill `:continue`, a `:child`, a `:Send`.
async fn every_request_path(
    model: &str,
) -> Vec<(Option<String>, Option<shannon_core::Effort>, Option<u32>)> {
    use shannon_core::{ContinueStrategy, Effort, StopReason};
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let seen = backend.requests.clone();
    // `RecordingStub` answers nothing, so each path is driven to the point of
    // ISSUING its request and no further; what the request carries is the
    // subject.
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    cfg.turn.effort = Some(Effort::Max);
    cfg.turn.max_output_tokens = 8_000;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        backend.clone(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.model_command(model);
    assert_eq!(
        app.conversation().episode().config.model,
        model,
        "premise: the session runs on a model the backend was NOT built with"
    );
    assert_ne!(app.backend.model(), model, "…and they really differ");

    // 1. a foreground turn.
    app.submit("hello");
    app.feed_stream(StreamMsg::Complete);

    // 2. `S`, a distillation of this session — which needs a branch to aim at.
    for n in 0..3 {
        app.conversation_mut().begin_turn(format!("q{n}"));
        app.conversation_mut().push_chunk(&format!("a{n}"));
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    // The branch is a NEW episode, and `:model` set the model on the old one —
    // so assert the aim rather than assuming it followed.
    assert_eq!(
        app.conversation().episode().config.model,
        model,
        "premise: a branch inherits the model it was branched from"
    );
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    distill_reviewed(&mut app);
    assert!(app.distilling().is_some(), "premise: `S` really started");
    app.ctrl_c_stop_for_test(); // free the single slot for the next path

    // 3. a prefill `:continue`, which splices into a turn this model produced
    // — started the way a turn starts, since a continuation now asks for what
    // its turn was sealed under.
    app.start_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.continue_last_turn();
    assert!(app.continuing(), "premise: the prefill really started");
    app.ctrl_c_stop_for_test();

    // 4. a `:child`, whose model defaults to the parent's.
    app.spawn_child_job("go", None).expect("spawns");

    // 5. a `:Send` to this session — a job, through its own door.
    app.start_job_on(app.prompt_target(), "sent")
        .expect("premise: the send goes");

    let knobs: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| (r.model.clone(), r.effort, r.max_tokens))
        .collect();
    assert!(
        knobs.len() >= 6,
        "premise: every path above really issued a request — got {}: {knobs:?}",
        knobs.len()
    );
    knobs
}

/// **Every test `App` is built by a door that forbids real execution — and
/// this checks it, rather than claiming it.**
///
/// The claim existed: `test_app_with`'s comment read *"every test app must come
/// from one of these two, so the safety property is structural rather than
/// something each test has to remember"*. It was false in **34 places** — every
/// test needing a `JsonlStore` or its own backend called `App::with_store` or
/// `App::new` directly and got the **real** executor, in a suite whose fixtures
/// include `find . -delete`.
///
/// Nothing had bitten, because most of those tests never gate a command. But
/// "which tests could ever run one" is an enumeration standing in for a
/// property nobody computed ([[feedback_enumeration_means_unmodeled]]), and one
/// finally did: a mutation run executed `mkdir` for real and left a directory
/// in the repo root — which `git status` cannot show, because git does not
/// track empty directories.
///
/// A prose claim about completeness is this project's most expensive artifact
/// ([[feedback_doc_comments_are_claims]]). The difference between that comment
/// and this test is that **a test starts from the type rather than from the
/// list**: it scans for every construction and demands each one be wrapped,
/// so a thirty-fifth cannot be added quietly.
///
/// **Every file of the suite, and every door.** It scanned `tests.rs` alone —
/// not the modules beside it, 21,880 lines — and watched two of `App`'s three
/// constructors: a bare constructor planted in `tests/tabs.rs` passed it
/// (review stop 12, T3 5). Then the modules were read from the `mod` lines
/// here — which a `pub(crate) mod` hid from it (stop 13, B 1). Now every
/// Rust file of the crate is read by walking it, the real launch in
/// `main.rs` the one bare constructor allowed, and the doors are `App`'s
/// constructors as its own source declares them.
#[test]
fn every_test_app_is_built_by_a_door_that_forbids_real_execution() {
    use std::collections::BTreeSet;
    // Each constructor and the guard that must wrap it, the names assembled
    // from parts so this test's own source does not match the pattern it
    // hunts for — a guard that trips on itself measures nothing.
    let doors = [
        ("new", ["no_real", "_execution("].concat()),
        ("with_store", ["no_real", "_execution("].concat()),
        ("with_vault", ["iso", "lated("].concat()),
    ];
    let constructors: BTreeSet<&str> = (include_str!("../app.rs").split("\n    pub fn "))
        .skip(1)
        .filter_map(|rest| {
            let (name, rest) = rest.split_once('(')?;
            rest[..rest.find('{')?].contains("-> Self").then_some(name)
        })
        .collect();
    let watched: BTreeSet<&str> = doors.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        constructors, watched,
        "every way to build an `App` is watched"
    );

    // **Every Rust file of the crate, by walking it** — not the `mod` lines
    // here: read from `mod x;`, a module respelled `pub(crate) mod x;` went
    // unread with a bare constructor in it, and a test anywhere else in the
    // crate (63 other files hold `#[cfg(test)]` code, and `tests/`,
    // `benches/`, `examples/`) was never read (review stop 13, B 1,
    // mutated). The one bare constructor allowed is the real launch.
    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    for dir in ["src", "tests", "benches", "examples"] {
        rust_files(&root.join(dir), &mut paths);
    }
    let files: Vec<(String, String)> = (paths.iter())
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"));
            (name, text)
        })
        .collect();
    // The subject: this file, every module of the split, and the launch.
    let read = |name: &str| files.iter().any(|(file, _)| file == name);
    assert!(
        read("src/app/tests.rs") && read("src/app/tests/teardown.rs") && read("src/main.rs"),
        "premise, the files read: {}",
        files.len()
    );
    let launch = [
        "App",
        "::",
        "with_vault",
        "(backend, config, store, vault, opens)",
    ]
    .concat();

    let mut strays = Vec::new();
    for (file, text) in &files {
        for (name, guard) in &doors {
            let door = ["App", "::", name, "("].concat();
            for (at, _) in text.match_indices(door.as_str()) {
                let real_launch = file == "src/main.rs" && text[at..].starts_with(&launch);
                if !text[..at].ends_with(guard.as_str()) && !real_launch {
                    let line = text[..at].matches('\n').count() + 1;
                    strays.push(format!("{door} at {file}:{line}"));
                }
            }
        }
    }
    assert!(
        strays.is_empty(),
        "a test app was built without the no-real-execution guarantee — route \
         it through `test_app`, `test_app_with`, `test_app_with_store`, \
         `test_app_backed` or `test_app_with_vault`:\n  {}",
        strays.join("\n  ")
    );
}

/// Open `:help` in a focused pane with `scrolloff` set, and a key-presser that
/// goes through the production dispatch (`handle_text_pane_key_for_test` runs
/// the real refusal door, `apply_edit_outcome` and `scroll_pane`). 80×24: the
/// manual is held to 80 columns, so one source line is one row and a line
/// index can be read straight off the scroll.
fn help_at(scrolloff: u16) -> (App, crate::wm::PaneId) {
    let mut cfg = Config::default();
    cfg.tui.scrolloff = scrolloff;
    let mut app = test_app_with(cfg);
    // Opened as the `:help` event ends: unsettled, the first key's settle
    // saw the focus move and dropped the count it began (`20zt`).
    open_pane_of(&mut app, PaneKind::Page(PageKind::Help));
    let pane = app.workspace.focused();
    (app, pane)
}

fn keys_in_help(app: &mut App, keys: &str) {
    for c in keys.chars() {
        let code = if c == '\r' {
            KeyCode::Enter
        } else {
            KeyCode::Char(c)
        };
        app.handle_text_pane_key_for_test(
            PaneKind::Page(PageKind::Help),
            KeyEvent::new(code, KeyModifiers::NONE),
            (80, 24),
        );
    }
}

/// A focused `:graph` of one session and `turns` turns — `turns + 1` rows —
/// with `scrolloff` set, and the height its window draws at in an 80×24 frame.
fn graph_of(turns: usize, scrolloff: u16) -> (App, usize) {
    let mut cfg = Config::default();
    cfg.tui.scrolloff = scrolloff;
    let mut app = test_app_with(cfg);
    for i in 0..turns {
        app.conversation_mut().begin_turn(format!("question {i}"));
        app.conversation_mut().push_chunk("an answer");
        app.conversation_mut().commit_turn();
    }
    open_graph(&mut app);
    let pane = app.workspace.focused();
    let h = app.pane_content_size(pane, Rect::new(0, 0, 80, 24)).1 as usize;
    (app, h)
}

fn graph_sel(app: &App) -> usize {
    app.graph_view().expect("a graph").selected()
}

/// The graph's window top at the height it is drawn at — the same height the
/// keys used, so reading it is a frame, not a resize.
fn graph_top(app: &App, h: usize) -> usize {
    app.graph_view().expect("a graph").window_top(h)
}

/// A focused decision pane over a report longer than its body, with
/// `scrolloff` 0, and the report's line count.
async fn decision_of() -> (App, usize) {
    let mut cfg = Config::default();
    cfg.tui.scrolloff = 0;
    let mut app = test_app_with(cfg);
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("go");
    model_call(&mut app, "tu-1", "rm -rf /tmp/a | grep x | wc -l > /tmp/b");
    to_decision(&mut app);
    let len = app.shown_call().expect("a decision").text().lines().len();
    (app, len)
}

/// Keys at the decision pane, sized so its body is four rows: six rows are the
/// pinned legend, the fold marker and four of report.
fn decision_keys(app: &mut App, keys: &str) {
    type_keys_sized(app, (80, 6), keys);
}

fn decision_cursor(app: &mut App) -> usize {
    let pane = app.workspace.focused();
    app.text_pane_pos_for_test(pane).expect("a report").0
}

/// The decision the app's own gate would make for `line`.
fn verdict(app: &App, line: &str) -> shannon_core::permission::Decision {
    app.rules.decide_line(line).decision
}

/// The `:ls` row for buffer `n`.
fn ls_row(app: &App, n: u64) -> String {
    app.buffers_listing(false)
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
        .unwrap_or_else(|| panic!("no b{n} in\n{}", app.buffers_listing(false)))
        .to_string()
}

/// Two session ids sharing their first [`SHORT_ID_LEN`] hex characters, a
/// letter among them — FOUND, not hard-coded: `EpisodeId::from_seed` is
/// deterministic, so this is the same pair every run, after some tens of
/// thousands of seeds (thirty-two bits' birthday bound).
///
/// [`SHORT_ID_LEN`]: shannon_core::SHORT_ID_LEN
fn colliding_sessions() -> (shannon_core::EpisodeId, shannon_core::EpisodeId) {
    let mut seen = std::collections::HashMap::new();
    for i in 0u32.. {
        let id = shannon_core::EpisodeId::from_seed(format!("collide {i}").as_bytes());
        // As long as the shortest prefix a verb takes (`id_prefix`): about
        // 2^16 seeds to a collision at 8, a few milliseconds of hashing.
        let key = id.to_hex()[..shannon_core::SHORT_ID_LEN].to_string();
        if !key.chars().any(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        if let Some(other) = seen.insert(key, id) {
            return (other, id);
        }
    }
    unreachable!("the search is unbounded")
}

/// b1 and b2 hold the two colliding sessions (b1 with a committed turn, so
/// `:new` really leaves it).
fn app_with_colliding_sessions() -> (App, shannon_core::EpisodeId, shannon_core::EpisodeId) {
    let mut app = test_app();
    let (a, b) = colliding_sessions();
    let root = |id| {
        shannon_core::Episode::root(
            id,
            shannon_core::TurnConfig {
                model: "stub".to_string(),
                max_tokens: None,
                effort: None,
            },
        )
    };
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    app.adopt_session(
        app.current,
        root(a),
        Vec::new(),
        OnDisk::Absent,
        recall,
        Landing::Asked,
    );
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    app.adopt_session(
        app.current,
        root(b),
        Vec::new(),
        OnDisk::Absent,
        recall,
        Landing::Asked,
    );
    (app, a, b)
}

/// Runs nothing; reports the leftovers it is given, and whether Shannon
/// ended them.
#[derive(Debug, Default)]
struct LeftoversExecutor {
    left: std::sync::Mutex<Vec<shannon_core::Leftover>>,
    ended: std::sync::atomic::AtomicBool,
}

impl shannon_core::Executor for LeftoversExecutor {
    fn spawn(
        &self,
        tool_use_id: String,
        command: String,
        cfg: shannon_core::ExecConfig,
        tx: tokio::sync::mpsc::UnboundedSender<shannon_core::ExecOutcome>,
    ) -> tokio::task::JoinHandle<()> {
        shannon_core::RecordingExecutor::default().spawn(tool_use_id, command, cfg, tx)
    }

    fn file(
        &self,
        tool_use_id: String,
        job: shannon_core::files::Job,
        cfg: shannon_core::ExecConfig,
        tx: tokio::sync::mpsc::UnboundedSender<shannon_core::ExecOutcome>,
    ) -> tokio::task::JoinHandle<()> {
        shannon_core::RecordingExecutor::default().file(tool_use_id, job, cfg, tx)
    }

    fn leftovers(&self) -> Vec<shannon_core::Leftover> {
        self.left.lock().unwrap().clone()
    }

    fn end_leftovers(&self) {
        self.ended.store(true, std::sync::atomic::Ordering::SeqCst);
        self.left.lock().unwrap().clear();
    }
}

/// `app`, its executor reporting one leftover: `python3 -m http.server &`
/// (call `toolu_srv`) left python3 (pid 4242) running, has printed
/// `printed` bytes since, the last line `last_line`.
fn with_a_leftover(
    mut app: App,
    printed: u64,
    last_line: Option<&str>,
) -> (App, Arc<LeftoversExecutor>) {
    let exec = Arc::new(LeftoversExecutor::default());
    exec.left.lock().unwrap().push(shannon_core::Leftover {
        tool_use_id: "toolu_srv".into(),
        command: "python3 -m http.server &".into(),
        since: std::time::SystemTime::now(),
        processes: vec![shannon_core::exec::LeftProcess {
            pid: 4242,
            name: "python3".into(),
        }],
        printed,
        last_line: last_line.map(str::to_string),
    });
    app.executor = exec.clone();
    (app, exec)
}

fn app_with_a_leftover(printed: u64) -> (App, Arc<LeftoversExecutor>) {
    with_a_leftover(test_app(), printed, None)
}

/// Steven's layout: `:ls` and `:help` side by side, no transcript anywhere,
/// standing in `:ls`. Returns `(ls, help)`.
fn viewers_only(app: &mut App) -> (crate::wm::PaneId, crate::wm::PaneId) {
    app.set_focus(PaneKind::Output);
    app.split_command("ls", SplitDir::Horizontal);
    let ls = app.workspace.focused();
    app.set_focus(PaneKind::Output);
    app.close_pane_command(Asked::Plain);
    app.workspace.focus_pane(ls);
    app.split_command("help", SplitDir::Vertical);
    let help = app.workspace.focused();
    app.workspace.focus_pane(ls);
    assert!(
        !app.workspace.has(PaneKind::Output),
        "premise: no transcript pane anywhere"
    );
    assert_eq!(
        app.workspace.kind_of(help),
        Some(PaneKind::Page(PageKind::Help)),
        "premise"
    );
    (ls, help)
}

/// A hidden b1 (a committed turn, so `:new` really leaves it) and b2 in
/// front.
fn two_sessions_b1_hidden(app: &mut App) {
    app.conversation_mut().begin_turn("b1's first turn");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    assert!(
        ls_row(app, 1).contains(" h "),
        "premise: {}",
        ls_row(app, 1)
    );
    assert_eq!(app.current, BufferId(1), "premise: b2 in front");
}

/// A committed, persisted turn whose prompt holds `text` — as `start_turn`
/// would have sent it, handles and all — sealed as production seals it, with
/// the held handles it names recorded.
fn commit_a_turn_naming(app: &mut App, text: &str) -> EpisodeId {
    let held = Arc::clone(&app.aliases);
    app.conversation_mut()
        .begin_turn(format!("the password is {text} ok"));
    app.conversation_mut().push_chunk("noted");
    app.conversation_mut().commit_turn_holding(&held);
    app.persist_committed_turn();
    app.conversation().episode().id
}

/// Every handle the turns placed in `episode` record holding, in order — what
/// a resume of it may ask the vault about. A seal site that forgot to record
/// leaves `Some(vec![])` (none held), so it contributes nothing here.
fn recorded_handles(app: &App, episode: EpisodeId) -> Vec<String> {
    app.current_graph()
        .placed(&episode)
        .iter()
        .flat_map(|p| p.provenance.handles.clone().unwrap_or_default())
        .collect()
}

fn nosecret_says_which_returns(mut app: App) {
    let mut table = (*app.aliases).clone();
    let lent = table
        .insert("github", "ghp_notreal", "env GITHUB_TOKEN", true)
        .unwrap();
    let marked = table
        .insert("secret", "ghp_notreal", shannon_core::MARKED_SOURCE, true)
        .unwrap();
    app.aliases = Arc::new(table);

    app.dispatch_command_for_test("nosecret ghp_notreal");
    let said = app.notice().unwrap_or_default();
    assert!(app.aliases.secrets().next().is_none(), "both off the table");
    assert!(
        app.aliases.secret_ranges("x ghp_notreal").is_empty(),
        "no longer masked, as the notice says"
    );
    assert!(said.contains(&lent) && said.contains(&marked), "{said}");
    assert!(
        said.contains("lent by env GITHUB_TOKEN") && !said.contains("will not bring it back"),
        "the lent one returns, and the notice does not promise otherwise: {said}"
    );
    assert!(
        !said.contains("ghp_notreal"),
        "the value is never echoed: {said}"
    );
}

/// A test app whose Claude Code is `provider`, so a test reads what the gate
/// answered it.
fn provider_app() -> (App, shannon_core::backend::RecordingProvider) {
    let provider = shannon_core::backend::RecordingProvider::new();
    (with_provider(test_app(), &provider), provider)
}

/// Claude Code asking, through its hook, whether a call may run — fed the way
/// the stream does, on a turn in flight.
fn provider_asks(app: &mut App, id: &str, tool: &str, input: serde_json::Value) {
    if !app.conversation().turn_in_progress() {
        app.conversation_mut().begin_turn("go");
    }
    let ask = asked_by_hook(app, id, tool, input);
    app.feed_stream(ask);
}

/// **Claude Code running a call Shannon allowed**, in the order its relay
/// keeps (`claude_code.rs`' `relay`): the hook's question, the allow — `a`
/// when the gate asks, by itself when it allows — then the record of what
/// ran. A record for a call never asked is the other ending, a turn stopped
/// in an error (`an_ungated_claude_code_call_stops_the_turn_and_says_why`):
/// four tests fed that record and then a `Complete`, a shape the relay never
/// sends (review stop 12, T4 3).
fn claude_code_ran(app: &mut App, id: &str, tool: &str, input: serde_json::Value, output: &str) {
    provider_asks(app, id, tool, input.clone());
    if (app.shown_call()).is_some_and(|c| c.tool_use_id() == id) {
        tool_key(app, 'a');
    }
    assert!(
        app.shown_call().is_none_or(|c| c.tool_use_id() != id),
        "premise: {id} allowed"
    );
    app.feed_stream(StreamMsg::ToolRecord {
        call: shannon_core::Block::ToolUse {
            id: id.into(),
            name: tool.into(),
            input,
        },
        result: shannon_core::Block::ToolResult {
            tool_use_id: id.into(),
            content: output.into(),
            is_error: false,
        },
    });
}

/// Claude Code's turn ending as its relay ends one: the `result` line's
/// stop reason, then the stream's one `Complete`.
fn claude_code_turn_ends(app: &mut App) {
    app.feed_stream(StreamMsg::Stopped(shannon_core::StopReason::EndTurn));
    app.feed_stream(StreamMsg::Complete);
}

/// Claude Code's hook asking about one call, from where `app` started — as
/// it does in production, where both are the directory Shannon runs in.
fn asked_by_hook(app: &App, id: &str, tool: &str, input: serde_json::Value) -> StreamMsg {
    StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: id.into(),
        name: tool.into(),
        input,
        cwd: here(app),
        ask: question(id),
        answer_by: hook_answer_by(),
    })
}

/// **When a hook asking now stops waiting** for its answer: its whole
/// deadline from now, as Claude Code's backend says it
/// (`ToolQuestion::answer_by`) — and [`HOOK_READ_MARGIN`] more, the real
/// clock's moment between asking and the app reading it, so a count on the
/// app's frozen clock is exact.
fn hook_answer_by() -> Option<std::time::Instant> {
    let deadline = shannon_core::backend::claude_code::HOOK_DEADLINE + HOOK_READ_MARGIN;
    std::time::Instant::now().checked_add(deadline)
}

/// **The real clock's moment between a test's question and its reading** —
/// microseconds, but a count cut to the minute reads an hour less them as
/// 58 minutes. Shorter than any count a test asserts (a second), and than
/// `DEADLINE_SLACK`.
const HOOK_READ_MARGIN: Duration = Duration::from_millis(100);

/// **The question `asked_by_hook` asks about `id`** — the token its answer
/// and its withdrawal go by (`shannon_core::AskId`). One per id, its serial
/// made from the id so no two ids share one, as minted serials never do
/// (review stop 7, Reviewer E 8); a test that means a second question for
/// the same call names its own serial.
fn question(id: &str) -> shannon_core::AskId {
    let serial = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    shannon_core::AskId::for_test(id, serial)
}

/// Where `app` started (`App::start`), and so where Claude Code's hook says
/// a call runs. One field for both: a test that moves it moves the rules'
/// start and the hook's together (review of 716f4f9, C-M5).
fn here(app: &App) -> Option<String> {
    app.start.as_ref().map(|dir| dir.display().to_string())
}

/// The answer that lets a call run as asked.
fn allowed(id: &str) -> (String, shannon_core::hook::Reply) {
    (
        id.to_string(),
        shannon_core::hook::Reply::Allow {
            updated_input: None,
        },
    )
}

/// An ordinary file, in a temp directory — so a test's verdict does not depend
/// on where the repo sits (a checkout under any `/.claude/` matches the
/// shipped `~/.claude` credential pattern: review 21, Reviewer A) — with the
/// gate started there, so reading it is a read inside where Shannon started
/// (`outside_places`, 2026-10-05), not one outside it.
fn plain_file(app: &mut App, dir: &TempDir) -> String {
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello\n").unwrap();
    // Started there, through the door production uses: the rules and the
    // hook's directory move together.
    app.start = Some(dir.path().to_path_buf());
    app.rebuild_rules();
    path.display().to_string()
}

/// Two of the model's calls in one round, both asking: the first on the pane,
/// the second queued behind it.
fn two_calls_asking(app: &mut App) {
    app.conversation_mut().begin_turn("go");
    for id in ["tu_1", "tu_2"] {
        let call = shannon_core::Block::bash_call(id, "rm -rf /tmp/shannon-test-absent");
        app.feed_stream(StreamMsg::ToolUse(call));
    }
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        app.shown_call()
            .map(|p| p.tool_use_id().to_string())
            .as_deref(),
        Some("tu_1"),
        "premise: the first on the pane"
    );
    assert_eq!(
        app.waiting_calls().len(),
        1,
        "premise: the second behind it"
    );
}

/// The call on the decision pane, by id.
fn on_the_pane(app: &App) -> Option<String> {
    app.shown_call().map(|p| p.tool_use_id().to_string())
}

/// A child of `app`'s session, on Claude Code — the generation its questions
/// arrive on — with a committed turn to hang off.
fn claude_code_child(app: &mut App) -> GenerationId {
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look", None).expect("spawns");
    app.generation_ids_for_test()[0]
}

/// A test app whose Claude Code is `provider`, on a `claude-code/…` model,
/// and which trusts NO directory — a first prompt somewhere new.
fn untrusted_claude_code_app() -> (App, shannon_core::backend::RecordingProvider) {
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = test_app();
    app.claude_code = Arc::new(provider.clone());
    let buffer = app.prompt_target();
    assert!(app.switch_model(buffer, "claude-code/sonnet", "claude-code/sonnet"));
    (app, provider)
}

fn requests_sent(provider: &shannon_core::backend::RecordingProvider) -> usize {
    provider.requests.lock().unwrap().len()
}

/// Is the trust screen up?
fn trust_screen_up(app: &App) -> bool {
    app.setup()
        .and_then(crate::setup::Setup::trust_asked)
        .is_some()
}

/// A key on a setup screen, in the event loop's order (`handle_event`): the
/// moment it begins, then the screen, then the event's end (`App::settle`,
/// as every seam ends one; review stop 12, Tier 3). Without the first, no
/// test could see a launch note wiped by the key that answered the trust
/// screen.
fn press_setup(app: &mut App, code: KeyCode) {
    let key = Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
    if app.begin_moment(&key) {
        app.setup_event_for_test(&key);
    }
    app.settle();
}

/// A test app whose config file lives in `dir` (it need not exist yet).
fn app_configured_in(dir: &std::path::Path) -> App {
    let mut app = test_app();
    app.user_config = Ok(dir.join("config.toml"));
    app
}

/// A stand-in `claude` that says it is logged in (`claude auth status`).
fn logged_in_claude(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("claude");
    std::fs::write(
        &path,
        "#!/bin/sh\necho '{\"loggedIn\": true, \"authMethod\": \"claude.ai\"}'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

/// One entry of a Claude Code transcript, as a line: its kind, its uuid (and
/// timestamp, so the file's order is the conversation's), its parent.
fn claude_entry(
    kind: &str,
    uuid: &str,
    parent: Option<&str>,
    message: serde_json::Value,
) -> String {
    let entry = serde_json::json!({"type": kind, "uuid": uuid, "parentUuid": parent,
        "isSidechain": false, "timestamp": uuid, "cwd": "/work/proj", "message": message});
    format!("{entry}\n")
}

/// A Claude Code `projects` directory of the test's own, holding one
/// conversation for the directory the tests run in: a prompt, a tool round
/// whose output holds a credential, and a reply.
fn claude_projects_with_one(secret: &str) -> (TempDir, std::path::PathBuf) {
    use serde_json::json;
    let entry = claude_entry;
    claude_projects_holding(&[
        entry(
            "user",
            "1",
            None,
            json!({"role": "user", "content": "what is in .env?"}),
        ),
        entry(
            "assistant",
            "2",
            Some("1"),
            json!({"id": "M1", "model": "claude-opus-5-5", "role": "assistant", "content": [
                {"type": "text", "text": "Let me look."},
                {"type": "tool_use", "id": "T1", "name": "Bash", "input": {"command": "cat .env"}}
            ]}),
        ),
        entry(
            "user",
            "3",
            Some("2"),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "T1", "content": secret}
            ]}),
        ),
        entry(
            "assistant",
            "4",
            Some("3"),
            json!({"id": "M2", "model": "claude-opus-5-5", "role": "assistant", "content": [
                {"type": "text", "text": "It holds one key."}
            ]}),
        ),
    ])
}

/// A Claude Code `projects` directory of the test's own, holding `lines` as
/// conversation `abc12345…` for the directory the tests run in.
fn claude_projects_holding(lines: &[String]) -> (TempDir, std::path::PathBuf) {
    let projects = TempDir::new().unwrap();
    let here: String = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = projects.path().join(here);
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("abc12345-0000-0000-0000-000000000000.jsonl");
    std::fs::write(&file, lines.concat()).unwrap();
    (projects, file)
}

/// **Claude Code's directory at `projects`, and Shannon let read it** for
/// this run — what a test of what is read there takes as given. The asking
/// itself is tested on its own (`claude_in_graph`, "asks before reading").
fn reading_claude(app: &mut App, projects: &std::path::Path) {
    app.set_claude_projects(Some(projects.to_path_buf()));
    app.claude_sessions.allow(projects, false).unwrap();
}

/// The fixture conversation imported, then made an import from before the
/// record of where it parted: its record `inherited` instead, in the buffer
/// and on disk. Returns its id.
fn an_older_import(app: &mut App, inherited: Option<shannon_core::Inherited>) -> EpisodeId {
    app.import_command("abc12345");
    let id = app.conversation().episode().id;
    app.conversation_mut().set_inherited(inherited.clone());
    let mut stored = app.store.load_episode(&id).unwrap().episode;
    stored.inherited = inherited;
    app.store.put_episode(&stored).unwrap();
    id
}

/// The fixture conversation's twin, written after it — the same turns under
/// another id, newer: a fork, or the copy `claude --resume` writes.
fn a_newer_copy(file: &std::path::Path) -> String {
    std::thread::sleep(std::time::Duration::from_millis(20));
    let id = "def45678-0000-0000-0000-000000000000";
    // Written, not `fs::copy`'d: a copy keeps the source's time on macOS,
    // and the twin would be no newer — the tie this exists for, unmade.
    let copy = file.with_file_name(format!("{id}.jsonl"));
    std::fs::write(&copy, std::fs::read(file).unwrap()).unwrap();
    let time = |p: &std::path::Path| std::fs::metadata(p).unwrap().modified().unwrap();
    assert!(time(&copy) > time(file), "premise: newer");
    id.to_string()
}

/// The fixture conversation's one turn, as Shannon runs it through Claude
/// Code: the transcript's prompt and reply, and what it cost recorded — which
/// an import's never is.
fn ran_through_claude_code(app: &mut App, claude: &std::path::Path) {
    app.claude_program = logged_in_claude(claude);
    app.model_command("claude-code/sonnet");
    app.submit("what is in .env?");
    app.feed_stream(StreamMsg::Chunk("It holds one key.".into()));
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 10,
        output_tokens: 5,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);
}
