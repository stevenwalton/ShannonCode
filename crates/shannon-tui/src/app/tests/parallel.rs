//! **One turn per session** (`buffers.md` §9): what a second turn running
//! beside the first needs of the code. Written ahead of the flip (§9 phase
//! 6), while one turn ran app-wide, so these tests open turns with
//! `fake_stream_on` and drive each by its id (`feed_turn`) — the harness
//! review of `buffers.md`, 10-15, Reviewer C 13 asked for first: "a harness
//! that cannot produce the state under test" measures nothing.

use super::*;

/// Two sessions, each in a buffer of its own, each with a turn begun on the
/// model and a generation streaming it: `(app, (a, ga), (b, gb))`, B started
/// second.
fn two_turns() -> (App, (BufferId, GenerationId), (BufferId, GenerationId)) {
    let mut app = test_app();
    let a = app.current;
    // A turn of history, so `:new` opens a buffer rather than reusing this.
    app.conversation_mut().begin_turn("A 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let b = app.current;
    assert_ne!(a, b, "premise: two buffers");
    let ga = app.fake_stream_on(a);
    app.session_of_mut(a)
        .unwrap()
        .conversation
        .begin_turn("A 1");
    let gb = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 1");
    (app, (a, ga), (b, gb))
}

fn running(app: &App, id: GenerationId) -> bool {
    app.generations.iter().any(|g| g.id == id)
}

fn stage(app: &App, id: GenerationId) -> Option<Stage> {
    app.generations
        .iter()
        .find(|g| g.id == id)
        .and_then(|g| match g.work {
            Work::Turn { stage } => Some(stage),
            _ => None,
        })
}

/// **The second turn's reply and ending are its own** — sealed into its
/// session, its generation resolved, the first turn running on untouched.
/// A reply alone would prove nothing: chunks already route by id; the
/// ending is what `handle_turn_msg` dropped for any turn but the first.
#[tokio::test]
async fn the_second_turns_ending_is_its_own() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(gb, StreamMsg::Chunk("B's reply".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    assert!(!running(&app, gb), "B's turn resolved");
    let b_last = app
        .session_of(b)
        .unwrap()
        .conversation
        .messages()
        .last()
        .cloned();
    assert_eq!(b_last.map(|m| m.content), Some("B's reply".to_string()));
    assert!(running(&app, ga), "A's turn runs on");
    assert!(
        app.session_of(a).unwrap().conversation.turn_in_progress(),
        "and A's session is still mid-turn"
    );
}

/// **…and so is its error** — the second turn's failure ends the second
/// turn only.
#[tokio::test]
async fn the_second_turns_error_ends_it_alone() {
    let (mut app, (_, ga), (_, gb)) = two_turns();
    app.feed_turn(gb, StreamMsg::Error(BackendError::RateLimited));
    assert!(!running(&app, gb), "B's turn ended");
    assert!(running(&app, ga), "A's runs on");
}

/// **The second turn's tool round is its own stage** — a call it asks to
/// run puts IT in `ToolRound`, waiting on the decision; the first turn goes
/// on streaming. `set_stage` moved the first turn's stage, so the second,
/// never in its round, could never resume after its decision.
#[tokio::test]
async fn the_second_turns_tool_round_is_its_own_stage() {
    let (mut app, (_, ga), (b, gb)) = two_turns();
    app.feed_turn(
        gb,
        StreamMsg::ToolUse(shannon_core::Block::bash_call(
            "tu_b",
            "rm -rf /tmp/shannon-test-absent",
        )),
    );
    app.feed_turn(gb, StreamMsg::Complete);
    assert!(
        app.shown_call_in(b).is_some(),
        "premise: B's call waits on you"
    );
    assert_eq!(stage(&app, gb), Some(Stage::ToolRound), "B is in its round");
    assert_eq!(stage(&app, ga), Some(Stage::Streaming), "A streams on");
}

// --- what is said names its session ----------------------------------------
//
// One turn, on A, with the prompt aimed at B.

/// A turn streaming on A, the prompt aimed at a second session B:
/// `(app, a, b, ga, A's name as `:ls` gives it)`.
fn a_turn_off_the_aim() -> (App, BufferId, BufferId, GenerationId, String) {
    let mut app = test_app();
    let a = app.current;
    app.conversation_mut().begin_turn("A 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let b = app.current;
    assert_eq!(app.prompt_target(), b, "premise: aimed at B");
    let ga = app.fake_stream_on(a);
    app.session_of_mut(a)
        .unwrap()
        .conversation
        .begin_turn("A 1");
    let name = app.session_label(app.session_of(a).unwrap().conversation.episode().id);
    (app, a, b, ga, name)
}

/// **Another session's turn holds nothing here** — the flip (`buffers.md`
/// §9): A answering refuses nothing on B, and on A says so, unnamed. (This
/// was a test that the refusal for B named A's turn, while a turn held every
/// session's slot.)
#[tokio::test]
async fn another_sessions_turn_holds_nothing_here() {
    let (app, a, b, _, _) = a_turn_off_the_aim();
    assert_eq!(app.busy_reason(b), None, "B is free");
    assert_eq!(
        app.busy_reason(a).as_deref(),
        Some("a turn is in flight"),
        "A is held by its own turn"
    );
}

/// **`:qa`'s refusal names a turn off the aim** — two turns would otherwise
/// be the same line twice.
#[tokio::test]
async fn quitting_names_a_turn_off_the_aim() {
    let (mut app, _, b, ga, name) = a_turn_off_the_aim();
    let ends = app.quitting_ends();
    assert!(
        ends.contains(&format!("a turn is in flight in {name}")),
        "{ends:?}"
    );
    // A deciding turn off the aim is named too, and the aim's own is not:
    // `said_in` names only what is not the aim (both halves were unpinned,
    // full review 10-21, A M09 and C 8).
    asks_you(&mut app, ga, "tu_a");
    let _gb = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 1");
    let ends = app.quitting_ends();
    assert!(
        ends.contains(&format!("a turn is waiting on your decision in {name}")),
        "{ends:?}"
    );
    assert!(
        ends.contains(&"a turn is in flight".to_string()),
        "the aim's own, unnamed: {ends:?}"
    );
}

/// **Claude Code compacting a turn off the aim says whose** — it went
/// unnamed for the first turn and "the turn: " for any other.
#[tokio::test]
async fn a_compaction_off_the_aim_names_its_session() {
    let (mut app, _, _, ga, name) = a_turn_off_the_aim();
    app.notices.clear();
    app.feed_turn(
        ga,
        StreamMsg::ProviderCompacted {
            auto: true,
            before: 1000,
        },
    );
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("{name}: Claude Code compacted")),
        "{said}"
    );
}

/// **A call Claude Code's gate refused unasked is said, and whose** — its
/// output never named it (B F2, property 4), so it never reached the pane;
/// unsaid, a background task's work would fail with nothing on screen. The
/// switch that runs that work where it is shown is named.
#[tokio::test]
async fn a_call_refused_unnamed_is_said_with_its_switch() {
    let (mut app, _, _, ga, name) = a_turn_off_the_aim();
    app.notices.clear();
    let unnamed = StreamMsg::ToolUnnamed {
        id: "toolu_bg".into(),
        name: "Bash".into(),
    };
    app.feed_turn(ga, unnamed);
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("{name}: Claude Code's Bash call refused")),
        "{said}"
    );
    assert!(said.contains("background_tasks = false"), "{said}");
    assert!(app.shown_call().is_none(), "nothing put to the pane");
}

// --- sessions run apart (the flip) -----------------------------------------

/// The call `turn` asks for that waits on you.
fn asks_you(app: &mut App, turn: GenerationId, id: &str) {
    app.feed_turn(
        turn,
        StreamMsg::ToolUse(shannon_core::Block::bash_call(
            id,
            "rm -rf /tmp/shannon-test-absent",
        )),
    );
    app.feed_turn(turn, StreamMsg::Complete);
}

/// **A decision holds its own session's round only** (review of
/// `buffers.md`, 10-15, Reviewer C 3) — A's call, one the rules allow, runs
/// and comes back while B's decision waits on the pane, and A goes on. Any
/// decision on screen held every turn's resume.
#[tokio::test]
async fn a_round_resumes_while_another_sessions_decision_waits() {
    let (mut app, (_, ga), (b, gb)) = two_turns();
    asks_you(&mut app, gb, "tu_b");
    assert!(
        app.shown_call_in(b).is_some(),
        "premise: B's call waits on you"
    );
    app.feed_turn(
        ga,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu_a", "echo hi")),
    );
    app.feed_turn(ga, StreamMsg::Complete);
    assert_eq!(
        stage(&app, ga),
        Some(Stage::ToolRound),
        "premise: A runs it"
    );
    settle(&mut app).await;
    assert_eq!(
        stage(&app, ga),
        Some(Stage::Streaming),
        "A's result went back and A goes on"
    );
    assert!(app.shown_call_in(b).is_some(), "B's decision still waits");
}

/// **…but a decision of its own session holds it** — a `:tool` typed in A,
/// waiting on you, keeps A's turn from going on with its own answered call
/// until you answer it, as a decision on screen always has: it is A's.
#[tokio::test]
async fn a_round_waits_for_a_decision_of_its_own_session() {
    let (mut app, (a, ga), _) = two_turns();
    app.aim_prompt_at(a);
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent");
    assert!(
        app.shown_call_in(a).is_some(),
        "premise: A's typed `:tool` asks"
    );
    app.feed_turn(
        ga,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu_a", "echo hi")),
    );
    app.feed_turn(ga, StreamMsg::Complete);
    settle(&mut app).await;
    assert_eq!(
        stage(&app, ga),
        Some(Stage::ToolRound),
        "held while A's own decision waits"
    );
    tool_key(&mut app, 'd');
    assert_eq!(
        stage(&app, ga),
        Some(Stage::Streaming),
        "answered: A goes on"
    );
}

/// **A decision of B's holds B** — a `:tool` typed in B while A's call is
/// up: B's prompt is refused, and B's round does not resume past it. B's
/// call is in B's own pane (`decision-panes-and-alerts.md` §3), never queued
/// behind A's. A queued call of B's holds B too, the queue's half of
/// `decision_holds` (full review 10-21, C 4, mutated and probed).
#[tokio::test]
async fn a_queued_decision_holds_its_own_session() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    asks_you(&mut app, ga, "tu_a");
    app.aim_prompt_at(b);
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call_in(a).is_some(), "premise: A's, in A's pane");
    assert!(app.shown_call_in(b).is_some(), "premise: B's, in B's own");
    assert!(app.waiting_calls().is_empty(), "neither behind the other");
    assert_eq!(
        app.busy_reason(b).as_deref(),
        Some("a command is awaiting your decision"),
        "B is held by its own decision"
    );

    app.feed_turn(
        gb,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu_b", "echo hi")),
    );
    app.feed_turn(gb, StreamMsg::Complete);
    settle(&mut app).await;
    assert_eq!(
        stage(&app, gb),
        Some(Stage::ToolRound),
        "B's round waits for B's decision"
    );
}

/// **A waiting call whose record is gone holds nothing** (review stop 8,
/// D 1's rest: "a waiting entry with no record must not hold forever") —
/// no answer can reach a call with no record, so it would hold its session
/// for good. Its sibling above pins that one whose record stands does.
#[test]
fn a_waiting_call_with_no_record_holds_nothing() {
    let mut app = test_app();
    let b = app.current;
    app.decisions.wait(
        crate::app::decisions::Waiting {
            buffer: b,
            id: "tu_forgotten".into(),
            command: "true".into(),
        },
        false,
    );
    assert!(
        app.decisions.calls_of(b).any(|id| id == "tu_forgotten"),
        "premise: it waits in b's entry"
    );
    assert_eq!(app.busy_reason(b), None, "it holds nothing");
}

/// **A prompt for B starts while A's decision waits** (Reviewer C 4; call
/// 2) — the decision is A's, and B has nothing to do with it.
#[tokio::test]
async fn a_prompt_for_another_session_starts_while_a_decision_waits() {
    let (mut app, a, b, ga, _) = a_turn_off_the_aim();
    asks_you(&mut app, ga, "tu_a");
    assert!(
        app.shown_call_in(a).is_some(),
        "premise: A's call waits on you"
    );
    assert!(app.busy_reason(a).is_some(), "premise: A is held by it");
    app.submit_to(b, "for B");
    assert!(app.pending.is_empty(), "not queued behind A's decision");
    assert!(app.turns().any(|g| g.buffer == b), "B answers");
}

/// **Each session's queue drains when its own turn ends** (Reviewer C 8) —
/// B's next prompt starts as B's turn ends, while A's waits for A.
#[tokio::test]
async fn each_sessions_queue_drains_when_its_own_turn_ends() {
    let (mut app, (a, _), (b, gb)) = two_turns();
    app.submit_to(a, "A next");
    app.submit_to(b, "B next");
    assert_eq!(app.pending.len(), 2, "premise: one queued for each");
    app.feed_turn(gb, StreamMsg::Complete);
    let queued: Vec<(BufferId, &str)> = app
        .pending
        .iter()
        .map(|(buffer, text)| (*buffer, text.as_str()))
        .collect();
    assert_eq!(queued, [(a, "A next")], "A's still waits for A");
    assert!(
        app.turns().any(|g| g.buffer == b && g.id != gb),
        "B's next turn runs"
    );
}

// --- CTRL-C stops the work where the prompt goes (phase 6b) ---------------

/// The shipped status row as drawn — the last line of a frame, its notice
/// cleared. (Named apart from `tests.rs`'s `status_row`, which sets a
/// format first and which this shadowed through the glob import, full
/// review 10-21, C.)
fn shipped_row(app: &mut App) -> String {
    app.notices.clear();
    let shown = screen(app, 160);
    shown.lines().last().unwrap_or_default().to_string()
}

/// **`CTRL-C` stops the aimed session's turn, and only it** (Steven, 10-20,
/// call 3) — B's reply streams on; and the row's `^C:stop`, drawn, follows
/// the aim: gone in A once A's turn is stopped, there in B while B's runs.
#[tokio::test]
async fn ctrl_c_stops_the_aimed_sessions_turn_only() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Chunk("A's half".into()));
    app.feed_turn(gb, StreamMsg::Chunk("B's half".into()));
    app.aim_prompt_at(a);
    assert!(
        shipped_row(&mut app).contains("^C:stop"),
        "premise: A's runs"
    );

    app.notices.clear();
    app.interrupt();
    assert!(!running(&app, ga), "A's turn stopped");
    assert!(running(&app, gb), "B's runs on: a key in A is A's");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("stopped"), "{said}");

    let row = shipped_row(&mut app);
    assert!(!row.contains("^C:stop"), "nothing here to stop:\n{row}");
    app.aim_prompt_at(b);
    let row = shipped_row(&mut app);
    assert!(row.contains("^C:stop"), "B's turn is B's to stop:\n{row}");
}

/// **`unshown` clears on the first frame a pane draws the session, and not
/// before** (§5.4, §8) — A's reply streams where no window shows it, is no
/// news while it streams, and is unshown once it ends; `:b` brings it into
/// the pane, and it stays so until that pane is drawn; then B, out of the
/// pane now but not changed, is not. Each said on the row as `$unshown`,
/// and counted as a fact.
#[tokio::test]
async fn unshown_clears_on_the_first_frame_that_draws_it() {
    use crate::facts::{AppFact, SessionFact};
    let (mut app, (a, ga), (b, _)) = two_turns();
    app.set_focus(PaneKind::Output);
    let unshown = |app: &App| [a, b].map(|buffer| app.session_fact(SessionFact::Unshown, buffer));
    // A frame, its row saying `$unshown` between `<` and `>`.
    let row = |app: &mut App| status_row(app, "output", "<$unshown>");
    assert_eq!(
        unshown(&app),
        [0, 0],
        "before any frame: nothing seen, nothing news"
    );
    assert!(row(&mut app).contains("<>"), "both seen at the first frame");
    assert_eq!(unshown(&app), [0, 0], "what they held is not news");

    app.feed_turn(ga, StreamMsg::Chunk("A's reply".into()));
    assert!(!in_a_window(&app, a), "premise: no window shows A");
    assert_eq!(
        unshown(&app),
        [0, 0],
        "changed, but still answering: no news yet"
    );
    app.feed_turn(ga, StreamMsg::Complete);
    assert!(!running(&app, ga), "premise: A's turn ended");
    assert_eq!(unshown(&app), [1, 0], "ended where no window shows it");
    assert_eq!(app.app_fact(AppFact::UnshownCount), 1);
    let n = a.number();
    let drawn = row(&mut app);
    assert!(
        drawn.contains(&format!("<news in session {n} · `:b {n}`>")),
        "{drawn}"
    );

    app.dispatch_command_for_test(&format!("b {n}"));
    assert!(in_a_window(&app, a), "premise: shown");
    assert_eq!(unshown(&app), [1, 0], "in the pane, not yet drawn");
    assert!(row(&mut app).contains("<>"), "drawn: seen");
    assert_eq!(unshown(&app), [0, 0], "and B, out of view, has not changed");
}

/// **A pane that draws nothing shows nothing** — `unshown` asks the frame,
/// not the window tree: a transcript in a pane the layout leaves no room is
/// not seen, nor one with a viewer lent over it (DR-B #14).
#[tokio::test]
async fn a_pane_with_no_room_or_a_viewer_over_it_shows_nothing() {
    use crate::facts::SessionFact;
    let (mut app, (a, ga), _) = two_turns();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    assert!(in_a_window(&app, a), "premise: shown");
    app.before_frame(160, 40);
    app.feed_turn(ga, StreamMsg::Chunk("more".into()));
    app.feed_turn(ga, StreamMsg::Complete);
    // A frame with no row for the transcript.
    app.before_frame(160, 1);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        1,
        "no room: unseen"
    );
    app.before_frame(160, 40);
    assert_eq!(app.session_fact(SessionFact::Unshown, a), 0, "room: seen");

    app.dispatch_command_for_test("help");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "premise: a viewer over it"
    );
    let again = app.fake_stream_on(a);
    app.session_of_mut(a)
        .unwrap()
        .conversation
        .begin_turn("A 2");
    app.feed_turn(again, StreamMsg::Chunk("and more".into()));
    app.feed_turn(again, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        1,
        "under a viewer: unseen"
    );
}

/// **Two unshown are counted, and the first named** — `:b` takes one, so
/// the row says which: A off screen, B under a viewer, both replying.
#[tokio::test]
async fn two_unshown_are_counted_and_the_first_named() {
    let (mut app, (a, ga), (_, gb)) = two_turns();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("help");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "premise: a viewer over B"
    );
    app.before_frame(160, 40);
    for turn in [ga, gb] {
        app.feed_turn(turn, StreamMsg::Chunk("a reply".into()));
        app.feed_turn(turn, StreamMsg::Complete);
    }
    let row = status_row(&mut app, "page", "<$unshown>");
    let n = a.number();
    assert!(
        row.contains(&format!("<news in 2 sessions · `:b {n}`>")),
        "{row}"
    );
}

/// **A `:Send` reply in a session no window shows is unshown once it ends**
/// — the case a mark at the landing would miss (DR-B #14): no composition
/// lands, the reply streams into the transcript. And not while it answers:
/// the `:Send` wrote your echo there the moment you sent it, and a bell on
/// your own keystroke would be noise.
#[tokio::test]
async fn a_send_reply_off_screen_is_unshown() {
    use crate::facts::SessionFact;
    let (mut app, (a, ga), (b, _)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    // A seen as its turn left it: drawn once, then out of view again.
    app.set_focus(PaneKind::Output);
    for shown in [a, b] {
        app.dispatch_command_for_test(&format!("b {}", shown.number()));
        app.before_frame(160, 40);
    }
    assert!(!in_a_window(&app, a), "premise: no window shows A");
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        0,
        "premise: seen"
    );
    app.start_job_on(a, "a background question").expect("free");
    let episode = app.session_of(a).unwrap().conversation.episode().id;
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        0,
        "your own echo, while it answers: no news"
    );
    app.feed_job(episode, StreamMsg::Chunk("an answer".into()));
    app.feed_job(episode, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(app.running_sends(), 0, "premise: the reply ended");
    assert_eq!(app.session_fact(SessionFact::Unshown, a), 1);
    assert_eq!(app.session_fact(SessionFact::Unshown, b), 0);
}

/// **The shipped alerts say news you have not seen, and mark it** (Steven,
/// 10-23: "let's ship the `unshown` alert … both") — with no config of
/// yours: A's turn ends where no window shows it, the shipped row says so
/// and `:ls` puts the bell beside A alone; once a pane draws A, both are
/// gone.
#[tokio::test]
async fn the_shipped_alerts_say_news_and_mark_it() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(gb, StreamMsg::Complete);
    let n = a.number();
    let news = format!("news in session {n} · `:b {n}`");
    let row = shipped_row(&mut app);
    assert!(!row.contains("news in"), "premise: nothing new yet: {row}");
    app.feed_turn(ga, StreamMsg::Chunk("A's reply".into()));
    app.feed_turn(ga, StreamMsg::Complete);
    let row = shipped_row(&mut app);
    assert!(row.contains(&news), "{row}");
    let marked = |app: &App, buffer: BufferId| {
        let hex = app
            .session_of(buffer)
            .unwrap()
            .conversation
            .episode()
            .id
            .to_hex();
        let listing = app.buffers_listing(false);
        let row = (listing.lines())
            .find(|l| l.contains(&hex[..8]))
            .unwrap()
            .to_string();
        row.contains('🔔')
    };
    assert!(
        marked(&app, a) && !marked(&app, b),
        "the bell beside A alone"
    );

    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test(&format!("b {n}"));
    let row = shipped_row(&mut app);
    assert!(!row.contains("news in"), "seen: {row}");
    assert!(!marked(&app, a), "and unmarked");
}

// --- what the alerts send (B2.3) -------------------------------------------

/// `app` with `config` adopted, as a `:reload` of a file holding it would.
fn adopting(app: &mut App, config: &str) {
    app.adopt_config(shannon_core::Config::parse(config).unwrap());
}

/// **News that arrives where you are not looking rings once** — the shipped
/// `unshown_session`, with no config of yours: A's turn ends off screen and
/// the next write after a frame is the bell and A's notification, in
/// Shannon's words; a frame while it still holds sends nothing more; seen,
/// and news again, it rings again.
#[tokio::test]
async fn news_arriving_off_screen_rings_once() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "nothing yet");
    app.feed_turn(ga, StreamMsg::Complete);
    app.before_frame(160, 40);
    let label = app.session_label(app.session_of(a).unwrap().conversation.episode().id);
    let expected = format!("\x07\x1b]9;Shannon: news in {label}\x07");
    assert_eq!(app.take_rings(), expected.into_bytes());
    app.before_frame(160, 40);
    assert!(
        app.take_rings().is_empty(),
        "still news: once, not every frame"
    );

    // Seen, then news again: it rings again.
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "seen: nothing");
    app.dispatch_command_for_test(&format!("b {}", b.number()));
    let again = app.fake_stream_on(a);
    app.session_of_mut(a)
        .unwrap()
        .conversation
        .begin_turn("A 2");
    app.before_frame(160, 40);
    app.feed_turn(again, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert!(!app.take_rings().is_empty(), "news again: it rings again");
}

/// **Nothing rings for what already holds** — at the first frame, nor for a
/// rule you just wrote or rewrote (a new identity, `RuleKey`: taken without
/// a word) — while a config change that keeps the rules keeps what they
/// held, so an edge in the same frame still rings: one elsewhere, and one
/// to the alerts themselves (the glyph), which re-seeded every edge while
/// they were kept by place (review stop 9, D5).
#[tokio::test]
async fn nothing_rings_for_what_already_holds() {
    let mut app = test_app();
    let rule = "[tui.alert.q]\nwhen = \"queued\"\nbell = true\n";
    adopting(&mut app, rule);
    app.pending.push_back((app.current, "later".into()));
    app.before_frame(160, 40);
    assert!(
        app.take_rings().is_empty(),
        "the first frame: seeded, silent"
    );
    adopting(
        &mut app,
        &format!("{rule}[tui.alert.q2]\nwhen = \"queued\"\nbell = true\n"),
    );
    app.before_frame(160, 40);
    // Silent at first sight: a new rule is a new key, with no edge yet.
    assert!(app.take_rings().is_empty(), "a rule just written: silent");
    // A rule changed in place — its name the same, what it asks not — is a
    // new key too, and rings nothing for what it now finds holding.
    let strict = "[tui.alert.q]\nwhen = \"queued >= 5\"\nbell = true\n";
    adopting(
        &mut app,
        &format!("{strict}[tui.alert.q2]\nwhen = \"queued >= 5\"\n"),
    );
    app.before_frame(160, 40);
    adopting(
        &mut app,
        &format!("{rule}[tui.alert.q2]\nwhen = \"queued\"\nbell = true\n"),
    );
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "a rule rewritten: silent");

    app.pending.clear();
    app.before_frame(160, 40);
    adopting(
        &mut app,
        &format!("{rule}[tui.alert.q2]\nwhen = \"queued\"\nbell = true\n[tui]\nscrolloff = 2\n"),
    );
    app.pending.push_back((app.current, "later".into()));
    app.before_frame(160, 40);
    assert_eq!(
        app.take_rings(),
        b"\x07".to_vec(),
        "an unrelated change kept the edge"
    );

    app.pending.clear();
    app.before_frame(160, 40);
    adopting(
        &mut app,
        &format!(
            "{rule}[tui.alert.q2]\nwhen = \"queued\"\nbell = true\n[tui.alerts]\nglyph = \"⚑\"\n"
        ),
    );
    app.pending.push_back((app.current, "later".into()));
    app.before_frame(160, 40);
    assert_eq!(
        app.take_rings(),
        b"\x07".to_vec(),
        "the glyph changed, the rules not: the edge kept"
    );
}

/// **`[tui.alerts] bell` and `notify` switch every alert's off**, each
/// alone; and `notify_via = "tmux"` wraps the notification for tmux.
#[tokio::test]
async fn the_switches_turn_the_bell_and_notifications_off() {
    let rings = |switches: &str| {
        let mut app = test_app();
        adopting(
            &mut app,
            &format!("[tui.alert.q]\nwhen = \"queued\"\nbell = true\nnotify = \"q!\"\n{switches}"),
        );
        app.before_frame(160, 40);
        app.pending.push_back((app.current, "later".into()));
        app.before_frame(160, 40);
        app.take_rings()
    };
    assert_eq!(rings(""), b"\x07\x1b]9;q!\x07".to_vec());
    assert_eq!(
        rings("[tui.alerts]\nbell = false\n"),
        b"\x1b]9;q!\x07".to_vec()
    );
    assert_eq!(rings("[tui.alerts]\nnotify = false\n"), b"\x07".to_vec());
    assert_eq!(
        rings("[tui.alerts]\nnotify_via = \"tmux\"\n"),
        b"\x07\x1bPtmux;\x1b\x1b]9;q!\x07\x1b\\".to_vec()
    );
    // Words that come to nothing send no notification; the bell still rings.
    assert_eq!(
        rings("[tui.alert.q2]\nwhen = \"queued\"\nnotify = \"\"\n"),
        b"\x07\x1b]9;q!\x07".to_vec(),
        "q2's words are nothing: it sends none"
    );
}

/// **A notification's words pass the door on their way out** — words a
/// config can write (a project's, too), holding an `ESC ] 52` clipboard
/// write and a `BEL`: out of the terminal they are text, and the sequence
/// they travel in ends where Shannon ends it.
#[tokio::test]
async fn a_notifications_words_pass_the_door() {
    let mut app = test_app();
    adopting(
        &mut app,
        "[tui.alert.q]\nwhen = \"queued\"\nnotify = \"\\u001b]52;c;aGk=\\u0007 hi\"\n",
    );
    app.before_frame(160, 40);
    app.pending.push_back((app.current, "later".into()));
    app.before_frame(160, 40);
    // Led, too: what is left begins with no letter (review stop 9, E3).
    assert_eq!(
        app.take_rings(),
        b"\x1b]9;Shannon: ]52;c;aGk= hi\x07".to_vec()
    );
}

/// **A notification holds no held value** (review stop 17, E1 H1): the door
/// is tested with a closure; this is the scrub it is handed. An alert's words
/// are the user's, and a desktop notification leaves the terminal.
#[tokio::test]
async fn a_notification_masks_a_held_value() {
    let mut app = test_app();
    app.mark_secret("plainword7", true).expect("marked");
    adopting(
        &mut app,
        "[tui.alert.q]\nwhen = \"queued\"\nnotify = \"deploy plainword7 now\"\n",
    );
    app.before_frame(160, 40);
    app.pending.push_back((app.current, "later".into()));
    app.before_frame(160, 40);
    let rang = String::from_utf8(app.take_rings()).expect("text");
    assert!(rang.contains("deploy"), "premise: it rang: {rang:?}");
    assert!(!rang.contains("plainword7"), "{rang:?}");
}

/// **A session is seeded silently the first frame it is seen** (Steven,
/// 10-23: ringing on `:open` "would be a bit noisy") — opened already
/// holding, it rings nothing; holding again later, it rings.
#[tokio::test]
async fn a_session_opened_holding_rings_nothing() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    adopting(
        &mut app,
        "[tui.alert.full]\nwhen = \"ctx_percent >= 50\"\nbell = true\n",
    );
    app.before_frame(160, 40);
    let fill = |app: &mut App, buffer: BufferId, pct: u64| {
        let budget = u64::from(app.context_budget(buffer));
        app.session_of_mut(buffer).unwrap().billing.context = (budget * pct / 100) as u32;
    };
    // A third session, opened already at 90% (B's turn ended, so `:new`
    // opens a buffer rather than reusing B's).
    app.new_session_command();
    let c = app.current;
    assert!(c != a && c != b, "premise: a new session");
    fill(&mut app, c, 90);
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "opened holding: silent");
    fill(&mut app, c, 10);
    app.before_frame(160, 40);
    fill(&mut app, c, 90);
    app.before_frame(160, 40);
    assert_eq!(
        app.take_rings(),
        b"\x07".to_vec(),
        "holding again: it rings"
    );
}

/// **A reply ending in the session you are watching rings nothing** — the
/// frame sees it before it asks what rang (`settle_seen`, then
/// `settle_rings`): asked the other way round, the reply was unshown for
/// that one frame, and the shipped `unshown_session` rang and notified for
/// every reply you watched arrive (review stop 9, C 2: the order was
/// unpinned).
#[tokio::test]
async fn a_reply_ending_on_screen_rings_nothing() {
    use crate::facts::SessionFact;
    let (mut app, _, (b, gb)) = two_turns();
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "premise: nothing yet");
    assert!(in_a_window(&app, b), "premise: B is on screen");
    app.feed_turn(gb, StreamMsg::Chunk("B's reply".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        0,
        "seen as it ended"
    );
    assert!(app.take_rings().is_empty(), "so nothing rang");
}

/// **News behind a whole screen is not seen** — setup, the trust question
/// and `:models` draw no panes, so a reply ending under one is news, and
/// rings; seen once the screen closes and a frame draws the pane (review
/// stop 9, A F2: `settle_seen` counted the panes the layout held while
/// `view::draw` drew none).
#[tokio::test]
async fn news_behind_a_whole_screen_is_not_seen() {
    use crate::app::FrameDraws;
    use crate::facts::SessionFact;
    let (mut app, _, (b, gb)) = two_turns();
    app.before_frame(160, 40);
    assert!(in_a_window(&app, b), "premise: B is on screen");
    app.open_setup();
    assert!(
        matches!(app.frame_draws(), FrameDraws::Whole(_)),
        "premise: setup is the whole frame"
    );
    app.feed_turn(gb, StreamMsg::Chunk("B's reply".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        1,
        "behind setup: unseen"
    );
    assert!(!app.take_rings().is_empty(), "and it rang");
    press_setup(&mut app, KeyCode::Esc);
    assert!(app.setup().is_none(), "premise: closed");
    app.before_frame(160, 40);
    assert_eq!(app.session_fact(SessionFact::Unshown, b), 0, "drawn: seen");
}

/// **News under a float is not seen; above a panel it is** — `:memories`
/// covers most of the frame, the transcript's tail among it, and takes
/// every key, so a reply ending under it is news (Steven, 10-23: "it isn't
/// seen, so therefore it is hidden"); `:effort`'s panel leaves the session
/// readable above it (review stop 9, D3 and F's Q4).
#[tokio::test]
async fn news_under_a_float_is_not_seen_and_above_a_panel_is() {
    use crate::app::FrameDraws;
    use crate::facts::SessionFact;
    let (mut app, _, (b, gb)) = two_turns();
    app.before_frame(160, 40);
    assert!(in_a_window(&app, b), "premise: B is on screen");
    // The screen `:memories` opens, without the places a launch finds.
    let (memories, skills) = Default::default();
    let screen =
        crate::shelf::Screen::new(crate::shelf::Shelf::Memories, "m".into(), memories, skills);
    app.setup = Some(crate::setup::Setup::shelf(screen));
    assert!(
        matches!(app.frame_draws(), FrameDraws::Float(_)),
        "premise: `:memories` floats"
    );
    app.feed_turn(gb, StreamMsg::Chunk("B's reply".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        1,
        "under the float: unseen"
    );
    assert!(!app.take_rings().is_empty(), "and it rang");
    press_setup(&mut app, KeyCode::Char('q'));
    assert!(app.setup().is_none(), "premise: closed");
    app.before_frame(160, 40);
    assert_eq!(app.session_fact(SessionFact::Unshown, b), 0, "drawn: seen");

    let gb = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 2");
    app.dispatch_command_for_test("effort");
    assert!(
        matches!(app.frame_draws(), FrameDraws::Panel(_)),
        "premise: `:effort` is a panel"
    );
    app.feed_turn(gb, StreamMsg::Chunk("B's second".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        0,
        "above the panel: seen"
    );
    assert!(app.take_rings().is_empty(), "so nothing rang");
}

// --- what is news: yours, or arrived (review stop 9, D2) -------------------

/// **What you delete is not news** (Steven, 10-23: "your own actions are
/// not news. You explicitly did it") — a turn deleted from a session no
/// pane draws, then the session itself: neither is unshown, neither rings.
/// Each rang "news in" the session left in its place (review stop 9, A F1).
/// Nor does what holds of the session left there: an alert on a fact a
/// fresh session holds (an emptied context) rang for it, its edges kept by
/// the buffer. And the buffer keeps nothing of the session before it: that
/// one's write failure went with it (D2).
#[tokio::test]
async fn what_you_delete_is_not_news() {
    use crate::facts::SessionFact;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    adopting(
        &mut app,
        "[tui.alert.light]\nwhen = \"ctx_percent < 10\"\nbell = true\n",
    );
    let a = app.current;
    let episode = app.conversation().episode().id;
    app.new_session_command();
    assert!(!in_a_window(&app, a), "premise: no pane draws A");
    // A's context half full, so `light` holds for A only once emptied.
    let half = |app: &mut App| {
        let budget = app.context_budget(a);
        app.session_of_mut(a).unwrap().billing.context = budget / 2;
    };
    half(&mut app);
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "premise: nothing yet");
    app.session_of_mut(a).unwrap().write_failed = Some("the disk said no".into());

    app.remove_turns_from(episode, &[0]).expect("deletes");
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        0,
        "a turn you deleted"
    );
    assert!(
        app.take_rings().is_empty(),
        "a turn you deleted rings nothing"
    );
    assert_eq!(
        app.session_of(a).unwrap().write_failed,
        None,
        "the failure was the session before's"
    );

    half(&mut app);
    app.before_frame(160, 40);
    let _ = app.take_rings();
    app.delete_episodes(&[episode]);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        0,
        "a session you deleted"
    );
    assert!(
        app.take_rings().is_empty(),
        "a session you deleted rings nothing"
    );
}

/// **What you stop is not news** (Steven, 10-23, Q3) — a `:Send` to a
/// session no pane draws, stopped where its `CTRL-C` is (the aim): before a
/// word of its reply, nothing is marked and nothing rings (review stop 9,
/// D1: it rang "news in" it, your echo the only change); after part of
/// one, it is marked — you have not read it — and still rings nothing.
#[tokio::test]
async fn a_stop_of_yours_rings_nothing_and_marks_only_a_partial_reply() {
    use crate::facts::SessionFact;
    let (mut app, (a, ga), (_, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert!(!in_a_window(&app, a), "premise: no pane draws A");
    let episode = app.session_of(a).unwrap().conversation.episode().id;
    app.aim_prompt_at(a);
    for partial in [false, true] {
        app.start_job_on(a, "a background question").expect("free");
        if partial {
            app.feed_job(episode, StreamMsg::Chunk("half an answer".into()));
        }
        app.before_frame(160, 40);
        app.interrupt();
        assert_eq!(app.running_sends(), 0, "premise: stopped");
        app.before_frame(160, 40);
        let marked = if partial { 1 } else { 0 };
        assert_eq!(
            app.session_fact(SessionFact::Unshown, a),
            marked,
            "partial = {partial}"
        );
        assert!(
            app.take_rings().is_empty(),
            "partial = {partial}: you stopped it"
        );
    }
}

/// **A `:Send` stopped before a word of its reply leaves no echo** — the
/// conversation drops a turn nothing arrived in, and the transcript drops
/// it too, as the foreground's seal does: your question stayed on screen,
/// one the model was never asked (review stop 9, D's N1). Stopped after
/// part of a reply, both stay.
#[tokio::test]
async fn a_send_stopped_before_a_word_leaves_no_echo() {
    let (mut app, (a, ga), (_, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    let episode = app.session_of(a).unwrap().conversation.episode().id;
    let shown = |app: &App| app.session_of(a).unwrap().output.lines().join("\n");
    let messages = |app: &App| app.session_of(a).unwrap().conversation.messages().len();
    app.aim_prompt_at(a);
    let before = messages(&app);
    app.start_job_on(a, "never answered").expect("free");
    assert!(shown(&app).contains("never answered"), "premise: echoed");
    app.interrupt();
    assert_eq!(
        messages(&app),
        before,
        "premise: the conversation dropped it"
    );
    assert!(!shown(&app).contains("never answered"), "{}", shown(&app));

    app.start_job_on(a, "half answered").expect("free");
    app.feed_job(episode, StreamMsg::Chunk("a half".into()));
    app.interrupt();
    let text = shown(&app);
    assert!(
        text.contains("half answered") && text.contains("a half"),
        "kept, both: {text}"
    );
}

/// **Your stop rings no alert of the app's either, and a reply does**
/// (review stop 9, G 1) — `unshown`'s count with a notification: a `:Send`
/// stopped after part of its reply moves the count and rings nothing; a
/// reply that ends there rings it. The app's edges were not compared across
/// your change, and no test reached them: kept by nothing, no alert on an
/// app fact rang at all, and the suite stayed green.
#[tokio::test]
async fn your_stop_rings_no_app_alert_and_a_reply_does() {
    use crate::facts::AppFact;
    let (mut app, (a, ga), (b, gb)) = two_turns();
    adopting(
        &mut app,
        "[tui.alert.unshown]\nnotify = \"N rang\"\n[tui.alerts]\ndisabled = [\"unshown_session\"]\n",
    );
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    let _ = app.take_rings();
    assert!(!in_a_window(&app, a), "premise: no pane draws A");
    let episode = app.session_of(a).unwrap().conversation.episode().id;
    app.aim_prompt_at(a);
    app.start_job_on(a, "a background question").expect("free");
    app.feed_job(episode, StreamMsg::Chunk("half an answer".into()));
    app.before_frame(160, 40);
    app.interrupt();
    app.before_frame(160, 40);
    assert_eq!(app.app_fact(AppFact::UnshownCount), 1, "premise: counted");
    let rang = String::from_utf8_lossy(&app.take_rings()).to_string();
    assert!(!rang.contains("N rang"), "your stop: {rang:?}");

    // Seen, then a reply that ends where no pane draws it: news, rung.
    app.set_focus(PaneKind::Output);
    for shown in [a, b] {
        app.dispatch_command_for_test(&format!("b {}", shown.number()));
        app.before_frame(160, 40);
    }
    let _ = app.take_rings();
    app.start_job_on(a, "again").expect("free");
    app.feed_job(episode, StreamMsg::Chunk("a whole answer".into()));
    app.feed_job(episode, StreamMsg::Complete);
    app.before_frame(160, 40);
    let rang = String::from_utf8_lossy(&app.take_rings()).to_string();
    assert!(rang.contains("N rang"), "a reply: {rang:?}");
}

/// **A stop while a command runs is yours too** (review stop 9, G 4) —
/// the session under `:help`, a command of its reply running: `<C-c>`
/// cancels it and says so in the transcript (`⚙ cancelled`), and that line
/// is your stop's: not marked, nothing rung. It was written outside the door
/// that says so.
#[tokio::test]
async fn a_stop_while_a_command_runs_is_not_news() {
    use crate::facts::SessionFact;
    let mut app = test_app();
    let b = app.current;
    app.submit("look around");
    app.feed_stream(StreamMsg::Chunk("Let me look.".into()));
    model_call(&mut app, "tu_ls", "ls -la");
    assert!(!app.running_tools.is_empty(), "premise: the command runs");
    app.set_focus(PaneKind::Output);
    app.before_frame(160, 40);
    app.dispatch_command_for_test("help");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "premise: a viewer over it"
    );
    app.before_frame(160, 40);
    let _ = app.take_rings();
    app.interrupt();
    app.before_frame(160, 40);
    let shown = app.session_of(b).unwrap().output.lines().join("\n");
    assert!(shown.contains("cancelled"), "premise: said: {shown}");
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        0,
        "your stop's line"
    );
    assert!(app.take_rings().is_empty(), "nothing rang");
}

/// **A `:Send` that fails says so in its session** (review stop 9, G 3) —
/// as a turn's failure does: `[error: …]` in the transcript, so the news it
/// rings is there to read when you go. It rang "news in" a transcript that
/// said nothing new, the failure on the row alone.
#[tokio::test]
async fn a_send_that_fails_says_so_in_its_session() {
    use crate::facts::SessionFact;
    let (mut app, (a, ga), (_, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert!(!in_a_window(&app, a), "premise: no pane draws A");
    let episode = app.session_of(a).unwrap().conversation.episode().id;
    let numbered = app.session_of(a).unwrap().output.last_turn();
    app.start_job_on(a, "a background question").expect("free");
    app.before_frame(160, 40);
    app.feed_job(episode, StreamMsg::Error(BackendError::RateLimited));
    app.before_frame(160, 40);
    assert_eq!(app.running_sends(), 0, "premise: it ended");
    let shown = app.session_of(a).unwrap().output.lines().join("\n");
    assert!(shown.contains("[error:"), "said where it happened: {shown}");
    assert!(
        shown.contains("[not in the conversation: the request failed]"),
        "and that the conversation has none of it, as a turn's: {shown}"
    );
    // As no turn's: the job's turn was dropped, and numbered, it put every
    // later mark in A a turn late (review stop 14, A 2).
    assert_eq!(
        app.session_of(a).unwrap().output.last_turn(),
        numbered,
        "numbered as before the job"
    );
    assert_eq!(
        app.session_fact(SessionFact::Unshown, a),
        1,
        "news, with something to read"
    );
}

/// **What lands over a buffer keeps its alerts' edges** (review stop 9,
/// G 5) — a `queued` alert with a notification rings once as a prompt
/// queues; a session landing over that buffer (as an unreviewed summary or
/// a continuation does) leaves it rung: it held throughout. Its edges were
/// a new buffer's — nothing held before — and it rang again.
#[tokio::test]
async fn what_lands_over_a_buffer_keeps_its_alerts_edges() {
    let (mut app, (_, ga), (b, gb)) = two_turns();
    adopting(
        &mut app,
        "[tui.alert.q]\nwhen = \"queued\"\nnotify = \"Q rang\"\n",
    );
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    let _ = app.take_rings();
    app.pending.push_back((b, "later".into()));
    app.before_frame(160, 40);
    let rang = String::from_utf8_lossy(&app.take_rings()).to_string();
    assert!(rang.contains("Q rang"), "premise: queued rang: {rang:?}");
    app.supersede_for_test(b);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(crate::facts::SessionFact::Queued, b),
        1,
        "premise: it holds still"
    );
    let rang = String::from_utf8_lossy(&app.take_rings()).to_string();
    assert!(!rang.contains("Q rang"), "held throughout: {rang:?}");
}

/// **A buffer given a new session takes its windows back to the tail**
/// (review stop 9, G 2) — a pane scrolled back on B, a session landed over
/// B through the one door (`App::hold`): the pane follows the end again.
/// Kept, a stale row with no tail to follow, a streaming reply did not
/// appear; nothing pinned the reset.
#[tokio::test]
async fn a_new_session_in_a_buffer_takes_its_windows_to_the_tail() {
    let (mut app, (_, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    assert_eq!(app.pane_session(pane).id, b, "premise: the pane shows B");
    (app.window_of_mut(pane)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport)
        .freeze_at(0, 0);
    let following = |app: &App| app.window_of(pane).unwrap().viewport().is_following_tail();
    assert!(!following(&app), "premise: scrolled back");
    app.supersede_for_test(b);
    assert!(following(&app), "back to the tail");
}

/// **Under a viewer, your stop is quiet and your reply is news** — the
/// prompt's own session with `:help` lent over its pane: a turn you stop
/// before a word rings nothing (review stop 9, D1); one that ends is
/// unshown, and rings (Steven, 10-23, Q5: "that's a useful signal" — F2
/// had proposed that the aim is never unshown) — even ending inside the
/// frame that echoed it: your echo's edges are taken at once, not at that
/// frame, which swallowed the reply's rise.
#[tokio::test]
async fn under_a_viewer_your_stop_is_quiet_and_your_reply_is_news() {
    use crate::facts::SessionFact;
    let (mut app, (_, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.feed_turn(gb, StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("help");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "premise: a viewer over B"
    );
    assert_eq!(app.prompt_target(), b, "premise: aimed at B");
    app.before_frame(160, 40);
    let _ = app.take_rings();

    app.start_turn_on(b, "a question", Vec::new());
    app.before_frame(160, 40);
    app.interrupt();
    assert!(app.turns().all(|g| g.buffer != b), "premise: stopped");
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        0,
        "stopped before a word"
    );
    assert!(app.take_rings().is_empty(), "your stop rings nothing");

    app.start_turn_on(b, "again", Vec::new());
    let turn = (app.turns().find(|g| g.buffer == b)).expect("B's turn").id;
    app.feed_turn(turn, StreamMsg::Chunk("an answer".into()));
    app.feed_turn(turn, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        1,
        "your reply, under a viewer: news"
    );
    assert!(!app.take_rings().is_empty(), "and it rings");
}

/// **Not looking is not seen** (Steven, 10-24: "the window not being in
/// focus is not considered seen" — a turn can last over an hour, and you
/// work in another window meanwhile) — a reply ending in the pane you sent
/// it from, while the terminal is not in front, is news and rings; back in
/// front, the next frame has seen it. Drawn alone, it was seen, and rang
/// nothing. A key is someone looking, so a missed report heals on it.
#[tokio::test]
async fn a_reply_ending_while_you_look_away_is_news() {
    use crate::facts::SessionFact;
    use crossterm::event::{Event, KeyEvent, KeyModifiers};
    let (mut app, (_, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.before_frame(160, 40);
    let _ = app.take_rings();
    assert!(in_a_window(&app, b), "premise: B is drawn");

    app.looked(&Event::FocusLost);
    app.feed_turn(gb, StreamMsg::Chunk("B's reply".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        1,
        "drawn, but you were not looking"
    );
    assert!(!app.take_rings().is_empty(), "so it rings");
    app.looked(&Event::FocusGained);
    app.before_frame(160, 40);
    assert_eq!(app.session_fact(SessionFact::Unshown, b), 0, "back: seen");

    app.looked(&Event::FocusLost);
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    app.looked(&Event::Key(key));
    let again = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 2");
    app.feed_turn(again, StreamMsg::Chunk("more".into()));
    app.feed_turn(again, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(
        app.session_fact(SessionFact::Unshown, b),
        0,
        "a key: you were looking"
    );
    assert!(app.take_rings().is_empty(), "so nothing rings");
}

/// **What arrives is news, wherever it lands** (Steven, 10-23) — a
/// session nobody just asked for, landed in a buffer of its own in no pane
/// (`Apart`), then over a buffer no pane draws (`Over`): unshown both
/// times, and ringing. `Apart` was seeded silently, its buffer new — the
/// same arrival news or not by where it landed (review stop 9, A F6).
#[tokio::test]
async fn what_arrives_is_news_wherever_it_lands() {
    use crate::facts::SessionFact;
    let dir = TempDir::new().unwrap();
    let stored = app_with_n_turns(dir.path(), 2).conversation().episode().id;
    // A launch where it is open nowhere.
    let mut app = relaunched(dir.path());
    let home = app.current;
    app.before_frame(160, 40);
    let arriving = |app: &App| {
        let (episode, turns) = load_full_session(app.store.as_ref(), &stored).expect("stored");
        let on_disk = OnDisk::Own(episode.own_len());
        (episode, turns, on_disk)
    };

    let (episode, turns, on_disk) = arriving(&app);
    let landed =
        (app.land(Lands::Apart, Landing::Arrived, episode, turns, on_disk)).expect("lands");
    assert!(!in_a_window(&app, landed), "premise: in no pane");
    app.before_frame(160, 40);
    assert_eq!(app.session_fact(SessionFact::Unshown, landed), 1, "apart");
    assert!(!app.take_rings().is_empty(), "apart: it rings");

    // Seen, and in no pane again.
    app.set_focus(PaneKind::Output);
    for shown in [landed, home] {
        app.dispatch_command_for_test(&format!("b {}", shown.number()));
        app.before_frame(160, 40);
    }
    let _ = app.take_rings();
    assert_eq!(
        app.session_fact(SessionFact::Unshown, landed),
        0,
        "premise: seen"
    );
    assert!(!in_a_window(&app, landed), "premise: in no pane");
    let (episode, turns, on_disk) = arriving(&app);
    (app.land(
        Lands::Over(landed),
        Landing::Arrived,
        episode,
        turns,
        on_disk,
    ))
    .expect("lands");
    app.before_frame(160, 40);
    assert_eq!(app.session_fact(SessionFact::Unshown, landed), 1, "over");
    assert!(!app.take_rings().is_empty(), "over: it rings");
}

/// **`:ls` marks nothing the same frame draws** — a reply ending in a
/// session the pane beside `:ls` shows: the first frame after puts no bell
/// beside it (review stop 9, A F4: `:ls` was built before the frame's sight
/// was settled, and wore the bell for that frame).
#[tokio::test]
async fn ls_marks_nothing_the_same_frame_draws() {
    let (mut app, _, (b, gb)) = two_turns();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("ls");
    let listing = (app.workspace)
        .first_of(PaneKind::Page(PageKind::Buffers))
        .expect("`:ls` opens a pane");
    assert!(in_a_window(&app, b), "premise: B is drawn beside it");
    app.before_frame(160, 40);
    app.feed_turn(gb, StreamMsg::Chunk("B's reply".into()));
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    let page = (app.pane_listing(listing).expect("content").lines().iter())
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n");
    let hex = app
        .session_of(b)
        .unwrap()
        .conversation
        .episode()
        .id
        .to_hex();
    let row = (page.lines())
        .find(|l| l.contains(&hex[..8]))
        .unwrap_or_else(|| panic!("B's row in\n{page}"));
    assert!(!row.contains('🔔'), "drawn this frame, no news: {row}");
}

/// **`:set …notify=false` turns a notification off** — the shipped
/// `notify` holds words, and `:set` types a value by what the key holds, so
/// `false` arrived as the word and was every notification after (review
/// stop 9, A F3): news off screen now rings the bell alone.
#[tokio::test]
async fn set_notify_false_sends_no_notification() {
    let (mut app, (_, ga), (_, gb)) = two_turns();
    app.dispatch_command_for_test("set tui.alert.unshown_session.notify=false");
    app.feed_turn(gb, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert!(app.take_rings().is_empty(), "premise: nothing yet");
    app.feed_turn(ga, StreamMsg::Complete);
    app.before_frame(160, 40);
    assert_eq!(app.take_rings(), b"\x07".to_vec(), "the bell, no words");
}

/// **Each alert keeps its own edge** — what held last frame is the alert's
/// and the session's, not the session's alone: beside a second alert on
/// another fact, a held queue rings once, not again on every frame the
/// other does not hold (review stop 9, C 3: an edge kept without its alert
/// passed every test, whose two alerts asked one fact).
#[tokio::test]
async fn two_alerts_keep_their_own_edges() {
    let mut app = test_app();
    adopting(
        &mut app,
        "[tui.alert.a_q]\nwhen = \"queued\"\nbell = true\n\
         [tui.alert.b_ctx]\nwhen = \"ctx_percent >= 50\"\nbell = true\n",
    );
    app.before_frame(160, 40);
    app.pending.push_back((app.current, "later".into()));
    app.before_frame(160, 40);
    assert_eq!(app.take_rings(), b"\x07".to_vec(), "the queue rose: once");
    for _ in 0..2 {
        app.before_frame(160, 40);
        assert!(app.take_rings().is_empty(), "held, b_ctx not: no more");
    }
}

/// **The row asks the facts of the session the prompt is aimed at, not the
/// one in front** — an alert on `ctx_percent`, A nearly full and B nearly
/// empty, B in front: aimed at A it holds, aimed at B it does not (review
/// stop 9, C 1: the test of the aim made one session both, so a row asking
/// the one in front passed it).
#[tokio::test]
async fn the_row_asks_the_aim_not_the_session_in_front() {
    let (mut app, (a, _), (b, _)) = two_turns();
    adopting(
        &mut app,
        "[tui.status]\nprompt = '<$alerts>'\n\
         [tui.alert.hot]\nwhen = \"ctx_percent >= 80\"\nsay = \"HOT\"\n",
    );
    let fill = |app: &mut App, buffer: BufferId, pct: u64| {
        let budget = u64::from(app.context_budget(buffer));
        app.session_of_mut(buffer).unwrap().billing.context = (budget * pct / 100) as u32;
    };
    fill(&mut app, a, 90);
    fill(&mut app, b, 10);
    app.set_focus(PaneKind::Prompt);
    app.aim_prompt_at(a);
    assert!(
        app.prompt_target() == a && app.buffer().id == b,
        "premise: aimed away from the session in front"
    );
    let row = shipped_row(&mut app);
    assert!(row.contains("HOT"), "A's facts: {row}");
    app.aim_prompt_at(b);
    let row = shipped_row(&mut app);
    assert!(!row.contains("HOT"), "B's facts: {row}");
}

/// **An alert's border marks the windows of each session it holds for**
/// (B2.1) — and only theirs; drawn on the cells, in every combination with
/// focus and the aim: the colour is focus's, then the aim's, then the
/// alert's, and the heavy line is the alert's alone, so a focused or aimed
/// window still shows it (DR-B #13).
#[tokio::test]
async fn an_alerts_border_marks_its_sessions_windows() {
    use ratatui::style::Color;
    let (mut app, (a, ga), (b, _)) = two_turns();
    // Text in A's transcript, or focused it draws as unfocused (the splash).
    app.feed_turn(ga, StreamMsg::Chunk("A's reply".into()));
    assert!(
        !app.session_of(a).unwrap().output.is_empty(),
        "premise: text"
    );
    app.set_focus(PaneKind::Output);
    let pb = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    let pa = app.workspace.focused();
    assert!(
        app.pane_session(pa).id == a && app.pane_session(pb).id == b,
        "premise"
    );
    let config = shannon_core::Config::parse(
        "[tui.alert.hot]\nwhen = \"ctx_percent >= 80\"\nborder = \"ui.danger\"\n",
    )
    .unwrap();
    app.adopt_config(config);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.toml"),
        "\"ui.border\" = \"#010101\"\n\"ui.border.focused\" = \"#020202\"\n\
         \"ui.border.aimed\" = \"#030303\"\n\"ui.danger\" = \"#040404\"\n",
    )
    .unwrap();
    app.set_theme(
        crate::theme::Theme::load("t", Some(dir.path()))
            .expect("loads")
            .0,
    );
    let grey = |n| Some(Color::Rgb(n, n, n));
    let (plain, focused, aimed, danger) = (grey(1), grey(2), grey(3), grey(4));
    let fill = |app: &mut App, buffer: BufferId, pct: u64| {
        let budget = u64::from(app.context_budget(buffer));
        app.session_of_mut(buffer).unwrap().billing.context = (budget * pct / 100) as u32;
    };
    // Each pane's top-left corner: its line, and its colour.
    let corners = |app: &mut App| {
        app.notices.clear();
        let frame = frame_of(app, 120, 30);
        let layout = (app.workspace).frame_layout(ratatui::layout::Rect::new(0, 0, 120, 30));
        let buf = frame.clone();
        [pa, pb].map(|p| {
            let rect = layout.pane(p).expect("drawn").rect;
            let cell = &buf[(rect.x, rect.y)];
            (cell.symbol().to_string(), cell.style().fg)
        })
    };
    let heavy = |fg| ("┏".to_string(), fg);
    let light = |fg| ("┌".to_string(), fg);

    fill(&mut app, a, 90);
    fill(&mut app, b, 10);
    app.set_focus(PaneKind::Prompt);
    app.aim_prompt_at(b);
    assert_eq!(
        corners(&mut app),
        [heavy(danger), light(aimed)],
        "A holds; B aimed"
    );
    app.aim_prompt_at(a);
    assert_eq!(
        corners(&mut app),
        [heavy(aimed), light(plain)],
        "the aim's colour wins"
    );
    app.focus_pane(pa);
    assert_eq!(
        corners(&mut app),
        [heavy(focused), light(plain)],
        "focus's wins"
    );
    fill(&mut app, a, 50);
    app.set_focus(PaneKind::Prompt);
    app.aim_prompt_at(b);
    assert_eq!(
        corners(&mut app),
        [light(plain), light(aimed)],
        "it holds no more"
    );
}

/// **`:ls` marks each session an alert with `mark` holds for** (B2.2) — the
/// glyph in a column of its own, the columns after it where they were for
/// every row, counted in cells (a 🔔 is two; `:ls`' widths count chars).
#[tokio::test]
async fn ls_marks_each_session_an_alert_holds_for() {
    let (mut app, (a, _), (b, _)) = two_turns();
    let config =
        shannon_core::Config::parse("[tui.alert.hot]\nwhen = \"ctx_percent >= 80\"\nmark = true\n")
            .unwrap();
    app.adopt_config(config);
    for (buffer, pct) in [(a, 90), (b, 10)] {
        let budget = u64::from(app.context_budget(buffer));
        app.session_of_mut(buffer).unwrap().billing.context = (budget * pct / 100) as u32;
    }
    let listing = app.buffers_listing(false);
    let row_of = |buffer: BufferId| {
        let hex = app
            .session_of(buffer)
            .unwrap()
            .conversation
            .episode()
            .id
            .to_hex();
        let row = (listing.lines())
            .find(|l| l.contains(&hex[..8]))
            .unwrap_or_else(|| panic!("listed:\n{listing}"))
            .to_string();
        let at = row.find(&hex[..8]).unwrap();
        (row.clone(), crate::wrap::str_cells(&row[..at]))
    };
    let ((row_a, hex_a), (row_b, hex_b)) = (row_of(a), row_of(b));
    assert!(row_a.contains('🔔'), "A holds: {row_a}");
    assert!(!row_b.contains('🔔'), "B does not: {row_b}");
    assert_eq!(hex_a, hex_b, "aligned by cells:\n{row_a}\n{row_b}");
}

/// Three transcript panes side by side, each on a session of its own:
/// `(app, [panes], [buffers])`, the third in front.
fn three_sessions() -> (App, [crate::wm::PaneId; 3], [BufferId; 3]) {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let mut panes = Vec::new();
    for i in 0..3 {
        if i > 0 {
            app.split_command("", SplitDir::Vertical);
            app.new_session_command();
        }
        // A turn of history, so the next `:new` mints rather than reuses.
        app.conversation_mut().begin_turn("0");
        app.conversation_mut().push_chunk("r");
        app.conversation_mut().commit_turn();
        panes.push(app.workspace.focused());
    }
    let panes: [crate::wm::PaneId; 3] = panes.try_into().unwrap();
    let buffers = panes.map(|p| app.pane_session(p).id);
    assert!(
        buffers[0] != buffers[1] && buffers[1] != buffers[2] && buffers[0] != buffers[2],
        "premise: three sessions"
    );
    (app, panes, buffers)
}

/// **With panes marked, one `CTRL-C` stops what one `<Enter>` started** —
/// pressed, not through a seam: each marked session's `:Send`, with the aim
/// on a third session that runs nothing. The row, drawn, promises it there
/// and drops the promise once they are stopped. (The key's marks half and
/// the row's could change with every test green, full review 10-21, C 1.)
#[tokio::test]
async fn ctrl_c_with_marks_stops_every_marked_sessions_work() {
    let (mut app, [pa, pb, _], [_, _, c]) = three_sessions();
    app.workspace.toggle_mark(pa);
    app.workspace.toggle_mark(pb);
    app.aim_prompt_at(c);
    app.submit("same question");
    assert_eq!(app.job_episodes().len(), 2, "premise: two sends");
    assert!(
        !app.generations.iter().any(|g| g.buffer == c),
        "premise: the aim runs nothing"
    );
    let row = shipped_row(&mut app);
    assert!(row.contains("^C:stop"), "the marked sessions' work:\n{row}");

    app.interrupt();
    assert!(app.job_episodes().is_empty(), "both marked sends stopped");
    let row = shipped_row(&mut app);
    assert!(!row.contains("^C:stop"), "nothing left to stop:\n{row}");
}

/// **…and the aim's own turn with them, never a session neither aimed nor
/// marked** — the marks are added to the aim, not put in its place: with
/// only them, a pane marked elsewhere left the aim's own reply running and
/// called it "running elsewhere" (full review 10-21, C 2 and B).
#[tokio::test]
async fn ctrl_c_with_marks_still_stops_the_aims_own_turn() {
    let (mut app, [pa, _, _], [_, b, c]) = three_sessions();
    let gb = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 1");
    let gc = app.fake_stream_on(c);
    app.session_of_mut(c)
        .unwrap()
        .conversation
        .begin_turn("C 1");
    app.feed_turn(gc, StreamMsg::Chunk("C's half".into()));
    app.workspace.toggle_mark(pa);
    app.aim_prompt_at(c);

    app.notices.clear();
    app.interrupt();
    assert!(!running(&app, gc), "the aim's own turn stopped");
    assert!(running(&app, gb), "B, neither aimed nor marked, runs on");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("stopped"), "{said}");
}

/// **`CTRL-C` in the decision pane stops no session's work** (Steven, 10-21:
/// "In the decision pane I don't think it should stop the sessions work. Do
/// that from the prompt pane"). Aimed at A, inside B's decision, the press
/// stopped A's reply and left B's call standing (full review 10-21, A 13,
/// probed). Now both run on, the press says where the stop is, and the row
/// there promises none.
///
/// **And where B's stop is, said of B** — "from the prompt" sent you to a
/// press that stopped A's reply, B's call still up (F 1, probed): the press
/// names B and its pane, and following it stops B. Aimed at the asker, the
/// prompt is right, and said so, exactly.
#[tokio::test]
async fn ctrl_c_in_the_decision_pane_stops_no_sessions_work() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Chunk("A's half".into()));
    asks_you(&mut app, gb, "tu_b");
    app.aim_prompt_at(a);
    app.set_focus(PaneKind::Prompt);
    let row = shipped_row(&mut app);
    assert!(row.contains("^C:stop"), "premise: A's runs:\n{row}");
    app.set_focus(PaneKind::Tool);
    assert_eq!(
        on_the_pane(&app).as_deref(),
        Some("tu_b"),
        "premise: B's call"
    );

    app.interrupt();
    assert!(running(&app, ga), "A's reply runs on");
    assert!(running(&app, gb), "B's turn waits on its call");
    assert_eq!(
        on_the_pane(&app).as_deref(),
        Some("tu_b"),
        "B's call stands"
    );
    let said = app.notice().unwrap_or_default().to_string();
    let b_name = name_of(&app, b);
    assert_eq!(
        said,
        format!(
            "{b_name}: CTRL-C in its pane stops its work, not the prompt's — here, `d` denies the call"
        ),
        "says whose, and where its stop is"
    );
    let row = shipped_row(&mut app);
    assert!(!row.contains("^C:stop"), "promises no stop here:\n{row}");

    // Do what it says: B's pane, then CTRL-C — B's work stops, A's runs on.
    app.aim_prompt_at(b);
    app.set_focus(PaneKind::Prompt);
    app.interrupt();
    assert!(!running(&app, gb), "B's turn stops");
    assert!(running(&app, ga), "and A's is A's");

    // Aimed at the asker, the prompt's press reaches it: said as such.
    let (mut app, _, (b, gb)) = two_turns();
    asks_you(&mut app, gb, "tu_b");
    app.aim_prompt_at(b);
    app.set_focus(PaneKind::Tool);
    app.interrupt();
    assert_eq!(
        app.notice().as_deref(),
        Some("CTRL-C stops work from the prompt — here, `d` denies the call")
    );
    assert!(running(&app, gb), "nothing stopped from the pane");
}

/// **In another session's decision pane, the row names whose activity it
/// shows** — the aim's: "[NORMAL] decision · answering…" read as the
/// asker's, beside B's call (full review 10-21, F 7, probed; Steven: "Yes").
/// The aim's own call needs no name, and gets none; nor do the counts of
/// everyone's work ("1 session running"), which are not the aim's.
#[tokio::test]
async fn the_decision_row_names_the_aim_beside_another_sessions_call() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Chunk("A's half".into()));
    // B asks through Claude Code's hook, its stream live: aimed at, B has
    // work of its own on the row beside its own call.
    app.feed_turn(gb, StreamMsg::Chunk("B's half".into()));
    let input = serde_json::json!({"command": "rm -r /tmp/shannon-test-absent"});
    let asked = asked_by_hook(&app, "tu_b", "Bash", input);
    app.dispatch_stream_msg(gb, asked);
    assert!(app.shown_call_in(b).is_some(), "premise: B's call is up");
    app.aim_prompt_at(a);
    app.set_focus(PaneKind::Tool);
    let row = shipped_row(&mut app);
    let a_name = name_of(&app, a);
    assert!(row.contains(&format!("{a_name}: answering…")), "{row}");

    // The prompt pane's row: the aim's own, unnamed.
    app.set_focus(PaneKind::Prompt);
    let row = shipped_row(&mut app);
    assert!(row.contains("answering…"), "premise: {row}");
    assert!(!row.contains(&format!("{a_name}: ")), "{row}");

    // The aim idle: no name before nothing.
    app.feed_turn(ga, StreamMsg::Complete);
    app.set_focus(PaneKind::Tool);
    let row = shipped_row(&mut app);
    assert!(!row.contains(&format!("{a_name}: ")), "{row}");

    // The asker aimed at: its own pane, unnamed.
    app.aim_prompt_at(b);
    app.set_focus(PaneKind::Tool);
    let row = shipped_row(&mut app);
    assert!(row.contains("answering…"), "premise: B's own work: {row}");
    assert!(!row.contains(&format!("{}: ", name_of(&app, b))), "{row}");
}

/// **`$running` is the aim's commands, as `^C:stop` beside it is** — B's
/// allowed command, running, is not shown in A's row (where CTRL-C would not
/// stop it), and is in B's (full review 10-21, B 2, probed).
#[tokio::test]
async fn the_rows_running_command_is_the_aims() {
    let (mut app, (a, _), (b, gb)) = two_turns();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.feed_turn(
        gb,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu_b", "echo hi")),
    );
    app.feed_turn(gb, StreamMsg::Complete);
    assert!(app.running_tools.contains_key("tu_b"), "premise: B's runs");

    app.aim_prompt_at(a);
    let row = shipped_row(&mut app);
    assert!(!row.contains("echo hi"), "not A's command:\n{row}");
    app.aim_prompt_at(b);
    let row = shipped_row(&mut app);
    assert!(row.contains("running: echo hi"), "B's:\n{row}");
}

/// **A session's facts are its own** (B1.2) — each read of the session it
/// names, as the row's value beside it is: B's command running, A's prompt
/// queued, A's reply streaming while B's turn is in its tool round; and the
/// app's count of sessions at work counts both.
#[tokio::test]
async fn a_sessions_facts_are_its_own() {
    use crate::facts::{AppFact, SessionFact};
    let (mut app, (a, _), (b, gb)) = two_turns();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.feed_turn(
        gb,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu_b", "echo hi")),
    );
    app.feed_turn(gb, StreamMsg::Complete);
    assert!(app.running_tools.contains_key("tu_b"), "premise: B's runs");
    app.pending.push_back((a, "later".into()));
    let of = |fact| [a, b].map(|buffer| app.session_fact(fact, buffer));
    assert_eq!(of(SessionFact::Running), [0, 1]);
    assert_eq!(of(SessionFact::Queued), [1, 0]);
    assert_eq!(
        of(SessionFact::Answering),
        [1, 0],
        "B's is in its tool round"
    );
    assert_eq!(of(SessionFact::Decision), [0, 0], "B's call was allowed");
    assert_eq!(app.app_fact(AppFact::Sessions), 2);
    assert_eq!(app.app_fact(AppFact::Children), 0);
}

/// **A session's decisions are counted where they wait** — A's call and
/// B's, none for C; and a call whose record is gone counts for nothing, as
/// it holds nothing (`decision_holds`).
#[tokio::test]
async fn a_sessions_decisions_are_counted_where_they_wait() {
    use crate::facts::SessionFact;
    let (mut app, _, [a, b, c], _) = two_decisions();
    let of = |app: &App, fact| [a, b, c].map(|buffer| app.session_fact(fact, buffer));
    assert_eq!(of(&app, SessionFact::Decision), [1, 1, 0]);
    assert_eq!(of(&app, SessionFact::Review), [0, 0, 0]);
    app.tool_calls.retain(|call| call.buffer != b);
    assert_eq!(
        of(&app, SessionFact::Decision),
        [1, 0, 0],
        "B's record gone"
    );
}

/// **A refusal for another session's busy slot says where its CTRL-C is
/// pressed** — a `:model` switch for B, refused while B answers, with the
/// prompt aimed at A: "stop it (CTRL-C)" would name a key that, pressed
/// here, stops A's work and not B's.
#[tokio::test]
async fn a_busy_refusal_for_another_session_says_where_ctrl_c_is_pressed() {
    let (mut app, (a, ga), (b, _)) = two_turns();
    app.feed_turn(ga, StreamMsg::Complete);
    app.aim_prompt_at(a);
    app.notices.clear();
    assert!(
        !app.switch_model(b, "claude-sonnet-4-5", "sonnet"),
        "refused"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("stop it (CTRL-C in its session)"), "{said}");

    app.aim_prompt_at(b);
    app.notices.clear();
    assert!(
        !app.switch_model(b, "claude-sonnet-4-5", "sonnet"),
        "refused"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("stop it (CTRL-C) or wait"),
        "aimed at B: {said}"
    );

    // Aimed at A with B's pane marked, CTRL-C reaches B too: pressed here.
    app.aim_prompt_at(a);
    let b_pane = app
        .session_panes(Over::ThisTab)
        .find(|&p| app.pane_session(p).id == b)
        .expect("a pane shows B");
    app.workspace.toggle_mark(b_pane);
    app.notices.clear();
    assert!(
        !app.switch_model(b, "claude-sonnet-4-5", "sonnet"),
        "refused"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("stop it (CTRL-C) or wait"),
        "B marked: {said}"
    );
}

/// **In a decision pane, the refusal sends you to the prompt to stop** —
/// `CTRL-C` there stops nothing (Steven, 10-21: "Do that from the prompt
/// pane"), so "stop it (CTRL-C)" would name a key that, pressed where you
/// stand, does nothing. Standing in B's decision with the prompt aimed at A
/// while A answers, `:model` and `:compact` for A say "CTRL-C at the
/// prompt" (`StopKey::AtPrompt`). Untested until review stop 12 asked
/// whether any settled state reached it; this one does, through the command
/// seam (10-27, probed). From the prompt, the same refusal says "CTRL-C".
#[tokio::test]
async fn a_busy_refusal_in_a_decision_pane_says_ctrl_c_is_pressed_at_the_prompt() {
    let (mut app, (a, _), (b, gb)) = two_turns();
    app.feed_turn(
        gb,
        StreamMsg::ToolUse(shannon_core::Block::bash_call(
            "tu_b",
            "rm /tmp/shannon-test-absent-1",
        )),
    );
    app.feed_turn(gb, StreamMsg::Complete);
    app.aim_prompt_at(a);
    app.settle();
    let pane = app.decision_pane_of(b).expect("premise: B asks");
    app.focus_pane(pane);
    app.settle();
    assert_eq!(app.focus(), PaneKind::Tool, "premise: in B's decision");
    assert_eq!(app.prompt_target(), a, "premise: aimed at A");
    for cmd in ["model sonnet", "compact"] {
        app.notices.clear();
        app.dispatch_command_for_test(cmd);
        let said = app.notice().unwrap_or_default().to_string();
        assert!(
            said.contains("a turn is in flight — stop it (CTRL-C at the prompt) or wait"),
            ":{cmd} in B's decision: {said}"
        );
        assert_eq!(
            app.focus(),
            PaneKind::Tool,
            ":{cmd} keeps you where you are"
        );
    }

    app.set_focus(PaneKind::Prompt);
    app.settle();
    app.notices.clear();
    app.dispatch_command_for_test("model sonnet");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("stop it (CTRL-C) or wait"),
        "from the prompt, CTRL-C is pressed here: {said}"
    );
}

/// **A `:Send` has a targeted stop now** — `CTRL-C` in its session — and
/// `:child --stop` on one says so, naming the session. It said "CTRL-C stops
/// everything running" while that was the only stop there was.
#[tokio::test]
async fn child_stop_on_a_send_names_the_session_its_ctrl_c_is_pressed_in() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let second = app.workspace.focused();
    app.workspace.toggle_mark(first);
    app.workspace.toggle_mark(second);
    app.submit("same question");
    let send = app.job_episodes()[0];

    app.notices.clear();
    let _ = app.child_dispatch(&format!("--stop {}", &send.to_hex()[..8]));
    let said = app.notice().unwrap_or_default().to_string();
    let name = app.session_label(send);
    assert!(
        said.contains(&format!("CTRL-C in {name} stops it")),
        "{said}"
    );
    assert_eq!(app.job_episodes().len(), 2, "refused: nothing stopped");
}

/// **A stop of everything says each turn it ended** (review stop 7, E 5) —
/// `:qa!`'s, every session's: the account held one turn, overwritten per
/// turn, so with two stopped together only the last was said. Off the aim,
/// named.
#[tokio::test]
async fn a_stop_of_everything_says_each_turn_it_ended() {
    let (mut app, (a, ga), (_, gb)) = two_turns();
    app.feed_turn(ga, StreamMsg::Chunk("A's half".into()));
    app.feed_turn(gb, StreamMsg::Chunk("B's half".into()));
    let a_name = app.session_label(app.session_of(a).unwrap().conversation.episode().id);
    app.notices.clear();
    assert!(app.stop_work(Stop::Everything));
    assert!(app.generations.is_empty(), "both stopped");
    let said = app.notice().unwrap_or_default().to_string();
    assert_eq!(said.matches("stopped —").count(), 2, "{said}");
    assert!(said.contains(&format!("{a_name}: stopped")), "{said}");
}

// --- the row: the aim's work, and a count of the rest (phase 6c) ----------

/// **The row says the aim's work and counts the rest** — drawn: `answering…`
/// is the aimed session's reply, and `N sessions running` is said whenever
/// one of them is not the aim, so B's turn is never invisible from A.
#[tokio::test]
async fn the_row_says_the_aims_work_and_counts_the_rest() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    // B streams throughout; A's turn ends partway.
    app.feed_turn(ga, StreamMsg::Chunk("A's half".into()));
    app.feed_turn(gb, StreamMsg::Chunk("B's half".into()));
    app.aim_prompt_at(a);
    let row = shipped_row(&mut app);
    assert!(
        row.contains("answering…") && row.contains("2 sessions running"),
        "{row}"
    );

    app.feed_turn(ga, StreamMsg::Complete);
    let row = shipped_row(&mut app);
    assert!(!row.contains("answering…"), "A's reply is done:\n{row}");
    assert!(
        row.contains("1 session running"),
        "B's, off the aim, still shown:\n{row}"
    );

    app.aim_prompt_at(b);
    let row = shipped_row(&mut app);
    assert!(row.contains("answering…"), "B's reply, aimed:\n{row}");
    assert!(
        !row.contains("session running") && !row.contains("sessions running"),
        "nothing off the aim to count:\n{row}"
    );
}

/// **`Esc` in `:graph` cancels the aimed session's distillation, and only
/// it** — the one the row's "distilling … (Esc cancels)" names. It took the
/// first of any session's, so with the aim moved off the session distilling,
/// the key ended work the row did not show.
#[tokio::test]
async fn esc_cancels_the_aimed_sessions_distillation_only() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.current;
    app.dispatch_command_for_test("compact");
    assert!(app.distilling().is_some(), "premise: A distils");
    let summary_on = |app: &App, buffer| {
        app.generations
            .iter()
            .any(|g| g.buffer == buffer && matches!(g.work, Work::Summary(_)))
    };

    app.new_session_command();
    assert_ne!(app.prompt_target(), a, "premise: aimed off A");
    assert!(
        app.distilling().is_none(),
        "the row's distilling is the aim's"
    );
    assert_eq!(app.sessions_running(), Some(1), "A's is counted instead");
    // The KEY, in the tree: calling `cancel_summary` left the claim on `Esc`
    // free to widen with every test green (full review 10-21, C 7).
    app.dispatch_command_for_test("graph");
    assert_eq!(
        app.focus(),
        PaneKind::Graph,
        "premise: standing in the tree"
    );
    let esc = || KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    app.graph_key_for_test(esc());
    assert!(summary_on(&app, a), "Esc off A leaves A's distillation");

    app.aim_prompt_at(a);
    assert!(app.distilling().is_some(), "aimed at A, the row says it");
    app.graph_key_for_test(esc());
    assert!(!summary_on(&app, a), "and Esc cancels it");

    // A child is a session's work too, and has its own count: not twice.
    app.spawn_child_job("go", None).expect("spawned");
    assert_eq!(app.running_children(), 1, "premise: a child runs, off A");
    assert_eq!(app.sessions_running(), None, "counted as a child alone");
}

/// **A row with no session says so, in red** (Steven, 10-20: "Status line
/// should always be associated with a session. If for some reason it isn't
/// then show in red and say 'no agent'"). Planted here, on an app with a
/// session, by aiming at a buffer that is none; `shannon tutor` reaches it
/// before a session is called (`tutor::the_row_says_the_mode_and_no_agent_
/// before_a_session`). The row read its session through an `expect` and took
/// the program down mid-draw.
#[tokio::test]
async fn a_row_with_no_session_says_no_agent() {
    let mut app = test_app();
    let none = BufferId::numbered(9_999).expect("a buffer number");
    app.aim_prompt_at(none);
    app.current = none;
    assert!(app.aimed_session().is_none(), "premise: no session");

    // The WHOLE frame, as the loop draws it: the row alone drew `no agent`
    // while the frame panicked first, so what this promised was never seen
    // (full review 10-21, B 5 and C 5, probed).
    let backend = ratatui::backend::TestBackend::new(40, 6);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    draw_frame(&mut app, &mut terminal);
    let buf = terminal.backend().buffer();
    let last = rendered_text(buf)
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    let at = last.find("no agent").expect("says no agent") as u16;
    let danger = app.theme().ui(crate::style::Ink::Danger).fg;
    assert_eq!(buf[(at, 5)].style().fg, danger, "in red, the danger role");
}

// --- §9's own list: what had to hold before it landed ----------------------

/// The result `buffer`'s session was given for `id`, once its round returned
/// it into the conversation — `None` if that session holds none.
fn result_in(app: &App, buffer: BufferId, id: &str) -> Option<String> {
    app.session_of(buffer)?
        .conversation
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|b| match b {
            shannon_core::Block::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == id => Some(content.clone()),
            _ => None,
        })
}

/// **Two sessions' decisions each answer their own call** (§9, "Tests that
/// must exist before it lands") — both up at once, each in its own pane: A's
/// denied, B's allowed. Each result is returned into the session that asked,
/// never the other's, and the one command that ran is B's.
///
/// **With the aim at B, `d` in A's pane denies A's** — the pane's session,
/// never the aim's (design review DR-A #12: a pane resolving the aim's call
/// would deny B's here).
#[tokio::test]
async fn two_sessions_decisions_each_answer_their_own_call() {
    let (mut app, (a, ga), (b, gb)) = two_turns();
    // A beside B: a session no window shows has no pane, its call waiting
    // (`decision-panes-and-alerts.md` §3.5), and `:new` took A's.
    app.set_focus(PaneKind::Output);
    let pb = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    app.focus_pane(pb); // standing in B's window: the aim at B
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    asks_you(&mut app, ga, "tu_a");
    asks_you(&mut app, gb, "tu_b");
    let in_pane = |app: &App, s| app.shown_call_in(s).map(|p| p.tool_use_id().to_string());
    assert_eq!(in_pane(&app, a).as_deref(), Some("tu_a"), "premise: A's");
    assert_eq!(
        in_pane(&app, b).as_deref(),
        Some("tu_b"),
        "premise: B's, at once"
    );
    assert_eq!(app.prompt_target(), b, "premise: aimed at B");

    tool_key_in(&mut app, a, 'd');
    assert_eq!(in_pane(&app, a), None, "A's answered");
    assert_eq!(in_pane(&app, b).as_deref(), Some("tu_b"), "B's untouched");
    tool_key_in(&mut app, b, 'a');
    settle(&mut app).await;

    let a_said = result_in(&app, a, "tu_a").expect("A's answered");
    assert!(a_said.contains("denied"), "{a_said}");
    let b_said = result_in(&app, b, "tu_b").expect("B's answered");
    assert!(b_said.contains("recorded, not run"), "B's ran: {b_said}");
    assert_eq!(result_in(&app, a, "tu_b"), None, "B's result is not A's");
    assert_eq!(result_in(&app, b, "tu_a"), None, "A's result is not B's");
    assert_eq!(exec.ran().len(), 1, "one command ran: {:?}", exec.ran());
}

/// **A `:compact` runs beside another session's turn, and not over its own**
/// (§9's list) — a distillation is its session's work: A distils while B
/// answers, and B, answering, refuses one.
#[tokio::test]
async fn a_compact_runs_beside_another_sessions_turn_but_not_its_own() {
    let mut app = test_app();
    let a = app.current;
    app.conversation_mut().begin_turn("A 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let b = app.current;
    app.conversation_mut().begin_turn("B 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    let gb = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 1");
    let summary_on = |app: &App, buffer| {
        app.generations
            .iter()
            .any(|g| g.buffer == buffer && matches!(g.work, Work::Summary(_)))
    };

    app.aim_prompt_at(a);
    app.dispatch_command_for_test("compact");
    assert!(summary_on(&app, a), "A distils: {:?}", app.notice());
    assert!(running(&app, gb), "while B answers");

    app.aim_prompt_at(b);
    app.notices.clear();
    app.dispatch_command_for_test("compact");
    assert!(!summary_on(&app, b), "B, answering, does not");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("in flight"), "and says why: {said}");
}

/// **Deleting a session starts no command, and leaves none to answer**
/// (review stop 8, E1 1) — `leaving`'s rule, for a delete. The turn's call
/// up, a `:tool` typed there waiting behind it, a rule taken that allows the
/// typed one: ending the turn put the typed call to the gate, which ran it.
/// And a `:tool` alone stayed up in the fresh session, `a` running it there.
#[tokio::test]
async fn deleting_a_session_starts_nothing_and_leaves_nothing_to_answer() {
    let mut app = test_app();
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    let chat = app.current;
    let episode = app.conversation().episode().id;
    model_call(&mut app, "tu_1", "rm /tmp/shannon-test-x");
    manual_tool(&mut app, "mkdir -p shannon-test-y");
    assert_eq!(
        app.waiting_calls().len(),
        1,
        "premise: the typed call waits"
    );
    let taken = app.add_session_rule("allow", "mkdir:*");
    assert!(taken.is_ok(), "premise: the rule is taken: {taken:?}");
    app.notices.clear();
    app.delete_episodes(&[episode]);
    assert!(
        exec.ran().is_empty(),
        "started by a delete: {:?}",
        exec.ran()
    );
    assert!(app.running_tools.is_empty(), "nothing running");
    assert!(app.shown_call_in(chat).is_none(), "nothing shown");
    assert!(app.waiting_calls().is_empty(), "nothing waiting");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("a turn in flight") && said.contains("a command awaiting your decision"),
        "{said}"
    );

    // A `:tool` alone: it goes with its session, said, not left up.
    let mut app = test_app();
    let chat = app.current;
    let episode = app.conversation().episode().id;
    manual_tool(&mut app, "rm /tmp/shannon-test-x");
    assert!(app.shown_call_in(chat).is_some(), "premise: it asks");
    app.notices.clear();
    app.delete_episodes(&[episode]);
    app.settle();
    assert!(
        app.shown_call_in(chat).is_none(),
        "not left up in the fresh session"
    );
    assert!(!app.workspace.has(PaneKind::Tool), "its pane gone");
    assert!(
        app.tool_calls.is_empty(),
        "and its record: `:qa` listed it still"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("(with a command awaiting your decision)"),
        "{said}"
    );
}

/// **`:qa!` starts no command** — not even a queued call a rule now allows.
/// Stopping a session withdrew its shown decision, and the queue's door put
/// its next call to the gate, which ran it on the way out; the teardown then
/// killed it part-way (full review 10-21, A 5, probed). B's two calls: the
/// first shown, the second queued behind it, and allowed by a rule taken
/// while it waits.
#[tokio::test]
async fn quitting_starts_no_queued_command_a_rule_now_allows() {
    let (mut app, (_, ga), (_, gb)) = two_turns();
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    asks_you(&mut app, ga, "tu_a");
    for (id, command) in [
        ("tu_b", "rm -rf /tmp/shannon-test-absent"),
        ("tu_b2", "touch notes.txt"),
    ] {
        app.feed_turn(
            gb,
            StreamMsg::ToolUse(shannon_core::Block::bash_call(id, command)),
        );
    }
    app.feed_turn(gb, StreamMsg::Complete);
    assert_eq!(
        app.waiting_calls().len(),
        1,
        "premise: B's second waits behind B's first"
    );
    let taken = app.add_session_rule("allow", "touch notes.txt");
    assert!(taken.is_ok(), "premise: the rule is taken: {taken:?}");
    assert_eq!(app.waiting_calls().len(), 1, "premise: still waiting");

    let logs = captured_logs(|| assert!(app.leaving(true)));
    assert!(
        exec.ran().is_empty(),
        "started on the way out: {:?}",
        exec.ran()
    );
    // Each call it dropped is audited as cancelled, once: the queue is
    // cleared before the stop, so no teardown finds them to audit (full
    // review 10-21, D 2 — deleting this audit passed every test).
    for id in ["tu_a", "tu_b", "tu_b2"] {
        let records = logs
            .lines()
            .filter(|l| {
                l.contains(r#"decision="cancelled""#)
                    && l.contains(&format!(r#"tool_use_id="{id}""#))
            })
            .count();
        assert_eq!(records, 1, "{id}:\n{logs}");
    }
}

/// **Each session's call opens beside ITS transcript** — position and title
/// both say whose (Steven: "said two ways"), both up at once. One pane once
/// served both, and B's call filled the band under A's conversation (full
/// review 10-21, A 3, probed). Answering A's leaves B's pane as it was:
/// the same pane, where it was (design review DR-A #4, #5).
#[tokio::test]
async fn each_sessions_call_opens_beside_its_own_transcript() {
    let (mut app, [pa, pb, _], [a, b, _]) = three_sessions();
    let ga = app.fake_stream_on(a);
    app.session_of_mut(a)
        .unwrap()
        .conversation
        .begin_turn("A 1");
    let gb = app.fake_stream_on(b);
    app.session_of_mut(b)
        .unwrap()
        .conversation
        .begin_turn("B 1");
    asks_you(&mut app, ga, "tu_a");
    asks_you(&mut app, gb, "tu_b");
    let x_of = |app: &App, pane| {
        app.workspace
            .rects(ratatui::layout::Rect::new(0, 0, 150, 40))
            .iter()
            .find(|r| r.id == pane)
            .map(|r| r.rect.x)
    };
    let (da, db) = (
        app.decision_pane_of(a).expect("A's"),
        app.decision_pane_of(b).expect("B's"),
    );
    assert_ne!(x_of(&app, pa), x_of(&app, pb), "premise: side by side");
    assert_eq!(x_of(&app, da), x_of(&app, pa), "A's under A");
    assert_eq!(x_of(&app, db), x_of(&app, pb), "B's under B, at once");

    tool_key_in(&mut app, a, 'd');
    assert_eq!(app.decision_pane_of(a), None, "A's answered, its pane gone");
    assert_eq!(app.decision_pane_of(b), Some(db), "B's the same pane");
    assert_eq!(x_of(&app, db), x_of(&app, pb), "where it was");
}

/// **A held job outranks a decision in its session** — a `:tool` typed into
/// a held child's session waits on you, but answering it never unholds the
/// child: the refusal names `:child --stop`, not "answer it first". Pinned
/// by nothing (full review 10-21, A M18: dropping `!held` passed).
#[tokio::test]
async fn a_held_job_outranks_a_decision_in_its_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    let job = app.generation_ids_for_test()[0];
    app.gate_job_tool_call_for_test(job, "rm -rf /tmp/shannon-test-absent");
    assert_eq!(app.held_children(), 1, "premise: held");
    aim_at(&mut app, child);
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-2");
    assert!(app.shown_call().is_some(), "premise: a decision waits");

    let (_, how) = app
        .busy_reason_and_recovery(app.prompt_target())
        .expect("held");
    assert!(how.contains(":child --stop"), "{how}");
    assert!(!how.contains("answer it first"), "{how}");
}

/// **`children` and `sends` count each job running or held** (B1.2), as
/// `$children` and `$sends` say both: a held job waits on you, and a fact
/// leaving it out would fall to 0 exactly when you are wanted.
#[tokio::test]
async fn the_job_facts_count_running_and_held() {
    use crate::facts::AppFact;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.spawn_child_job("go", None).expect("spawned");
    assert_eq!(app.app_fact(AppFact::Children), 1, "running");
    assert_eq!(
        app.app_fact(AppFact::Sessions),
        0,
        "a job is not a session's own work"
    );
    let job = app.generation_ids_for_test()[0];
    app.gate_job_tool_call_for_test(job, "rm -rf /tmp/shannon-test-absent");
    assert_eq!(
        [app.running_children(), app.held_children()],
        [0, 1],
        "premise: held, not running"
    );
    assert_eq!(app.app_fact(AppFact::Children), 1, "held");
    assert_eq!(app.app_fact(AppFact::Sends), 0);

    let mut app = app_with_n_turns(dir.path(), 1);
    app.start_job_on(app.current, "a background reply")
        .expect("free");
    assert_eq!(app.running_sends(), 1, "premise: a :Send runs");
    assert_eq!(app.app_fact(AppFact::Sends), 1, "running");
    assert_eq!(app.app_fact(AppFact::Children), 0, "a :Send is no child");
    // Held the only way an attended job can be: a duplicate `tool_use` id.
    let job = app.generation_ids_for_test()[0];
    app.gate_job_tool_call_for_test(job, "mkdir build");
    app.gate_job_tool_call_for_test(job, "mkdir build");
    assert_eq!(
        [app.running_sends(), app.held_sends()],
        [0, 1],
        "premise: held, not running"
    );
    assert_eq!(app.app_fact(AppFact::Sends), 1, "held");
}

// --- a call's record is its session (full review 10-21, A 1) ---------------

/// **An unknown tool asked for in a session not in front is refused INTO
/// that session's round** — which then goes on. Untracked, its refusal was
/// read against `current`'s round and dropped as "a call this turn never
/// made", so the session waited on it forever (probed).
#[tokio::test]
async fn an_unknown_tool_off_current_is_refused_into_its_own_round() {
    let (mut app, (a, ga), (b, _)) = two_turns();
    assert_eq!(app.current, b, "premise: A is not in front");
    // Both arms that refuse at arrival: an unknown tool, and a `bash` call
    // whose `command` cannot be read.
    let unknown = shannon_core::Block::ToolUse {
        id: "tu_x".into(),
        name: "no_such_tool".into(),
        input: serde_json::Value::Null,
    };
    let no_command = shannon_core::Block::ToolUse {
        id: "tu_y".into(),
        name: shannon_core::BASH_TOOL.into(),
        input: serde_json::json!({}),
    };
    app.feed_turn(ga, StreamMsg::ToolUse(unknown));
    app.feed_turn(ga, StreamMsg::ToolUse(no_command));
    app.feed_turn(ga, StreamMsg::Complete);
    let said = result_in(&app, a, "tu_x").expect("A's round has its refusal");
    assert!(said.contains("unknown tool"), "{said}");
    let said = result_in(&app, a, "tu_y").expect("and the other's");
    assert!(said.contains("no readable `command`"), "{said}");
    assert_ne!(stage(&app, ga), Some(Stage::ToolRound), "and A goes on");
    // And said of A, which is not the aim (full review 10-21, D 3).
    let a_name = name_of(&app, a);
    for said in [
        "the model asked for an unknown tool",
        "the model sent a bash call",
    ] {
        assert!(
            app.notices
                .iter()
                .any(|(_, n)| n.starts_with(&format!("{a_name}: {said}"))),
            "{said}: {:?}",
            app.notices
        );
    }
}

/// **"secret spent" says whose command spent it** — with two sessions
/// running commands it named no one (full review 10-21, D 3).
#[tokio::test]
async fn a_secret_spent_off_the_aim_names_its_session() {
    let (mut app, (a, ga), (b, _)) = two_turns();
    assert_eq!(app.prompt_target(), b, "premise: aimed at B");
    app.track_tool_call("tu_a", Some(ga));
    app.notices.clear();
    app.handle_exec_outcome(shannon_core::ExecOutcome {
        tool_use_id: "tu_a".to_string(),
        command: "echo hi".to_string(),
        output: "hi".to_string(),
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: vec!["{{secret:demo}}".to_string()],
        attempts: 1,
        kept: None,
        left: shannon_core::LeftBehind::Nothing,
        file: None,
    });
    let a_name = name_of(&app, a);
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.starts_with(&format!("{a_name}: secret spent:"))),
        "{:?}",
        app.notices
    );
}

/// **CTRL-C's account names a marked session's cancelled distillation**,
/// as it names the turns beside it — "distillation cancelled" said nothing
/// of whose (full review 10-21, D 3, probed).
#[tokio::test]
async fn ctrl_c_names_a_marked_sessions_cancelled_distillation() {
    let (mut app, [pa, _, _], [a, _, c]) = three_sessions();
    app.workspace.toggle_mark(pa);
    app.aim_prompt_at(c);
    let episode = app.session_of(a).unwrap().conversation.episode();
    let (turn, id, config) = (episode.own_turns()[0], episode.id, episode.config.clone());
    let target = SquashTarget::Range {
        episode: id,
        start: 0,
        end: 1,
        fork_turn: turn,
        config,
    };
    app.fake_summary_on(a, target, vec![turn]);
    app.notices.clear();
    app.interrupt();
    assert!(
        !app.generations.iter().any(|g| g.buffer == a),
        "premise: stopped"
    );
    let a_name = name_of(&app, a);
    assert_eq!(
        app.notice().as_deref(),
        Some(format!("{a_name}: {SUMMARY_CANCELLED}").as_str()),
        "{:?}",
        app.notices
    );
}

/// **A command finishing after its turn was stopped prints nothing into
/// another session** — its outcome already queued when CTRL-C tore the turn
/// down, it wrote "⚙ output of" into `current`'s transcript (probed).
#[tokio::test]
async fn a_stopped_turns_late_output_stays_out_of_the_session_in_front() {
    let (mut app, (a, ga), (b, _)) = two_turns();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.feed_turn(
        ga,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu_a", "echo late")),
    );
    app.feed_turn(ga, StreamMsg::Complete);
    assert!(app.running_tools.contains_key("tu_a"), "premise: it runs");
    app.aim_prompt_at(a);
    app.interrupt();
    assert!(!running(&app, ga), "premise: A's turn is stopped");

    settle(&mut app).await;
    let in_b = app.session_of(b).unwrap().output.lines().join("\n");
    assert!(!in_b.contains("echo late"), "nothing of A's in B: {in_b}");
}

/// **`:b N` on the session a pane already shows leaves the window where it
/// is** — current or not. In A's pane with B current, it rebound the pane
/// and forgot its cursor, while the same command in B's pane did nothing;
/// `current` is drawn nowhere (full review 10-21, A 7, probed). It still
/// makes A the active session.
#[tokio::test]
async fn b_on_the_session_a_pane_shows_leaves_its_window_where_it_is() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, first, second) = app_with_two_sessions(dir.path());
    app.focus_pane(first);
    app.open_by_hash(&a.to_hex()[..8]);
    app.focus_pane(second);
    app.open_by_hash(&b.to_hex()[..8]);
    app.focus_pane(first);
    let a_buffer = app.pane_session(first).id;
    assert_ne!(app.current, a_buffer, "premise: A is not current");
    // A frame first: the window's cursor runs over its mirror of the
    // transcript, which a frame brings up to date (`sync_chat`).
    app.before_frame(80, 24);
    for c in ['G', 'k', 'k'] {
        app.key_for_test(
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            (80, 24),
        );
    }
    let before = app.pane_chat(first).cursor();
    assert!(before.0 > 0, "premise: the cursor moved off the top");

    app.dispatch_command_for_test(&format!("b {}", a_buffer.number()));
    assert_eq!(app.pane_chat(first).cursor(), before, "left where you were");
    assert_eq!(app.current, a_buffer, "and A is the active session");
}

/// **A marked pane covered by a viewer is not marked** — its mark is not
/// drawn there, so it must not route `<Enter>` (or what CTRL-C reaches) to
/// its session: the guard was pinned by nothing (full review 10-21, A M28).
#[tokio::test]
async fn a_marked_pane_under_a_viewer_routes_nothing() {
    let (mut app, [pa, pb, _], [a, b, _]) = three_sessions();
    app.workspace.toggle_mark(pa);
    app.workspace.focus_pane(pa);
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(pa),
        Some(PaneKind::Page(PageKind::Help)),
        "premise: lent"
    );
    assert!(
        app.workspace.marked_panes().is_empty(),
        "no mark drawn, none counted"
    );

    app.workspace.focus_pane(pb);
    app.focus_moved();
    assert_eq!(app.prompt_target(), b, "premise: aimed at B");
    app.submit("for B");
    assert!(
        app.job_episodes().is_empty(),
        "no multi-send to the covered A"
    );
    assert!(
        !app.generations.iter().any(|g| g.buffer == a),
        "nothing started in A"
    );
}

/// **A composition made in a `:graph` lent over the aim's only transcript
/// lands in that pane** — shown when the tree closes, the aim still on it.
/// Without `land`'s fallback to the pane a viewer borrowed, the lent pane
/// kept the old session, closing the tree showed it, and the aim was moved
/// off the new one (full review 10-21, A M35: the fallback was pinned by
/// nothing).
#[tokio::test]
async fn a_composition_in_a_lent_graph_lands_in_the_pane_beneath() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let original = app.current;
    app.dispatch_command_for_test("graph");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "premise: lent"
    );

    app.dispatch_command_for_test("clone");
    let landed = app.prompt_target();
    assert_ne!(landed, original, "premise: the aim follows the clone");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "the tree stays"
    );

    app.workspace.focus_pane(pane);
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "premise: closed"
    );
    assert_eq!(
        app.pane_session(pane).id,
        landed,
        "the pane beneath shows the clone"
    );
    assert_eq!(app.prompt_target(), landed, "and the aim stays on it");
}

/// **…and under two views, it lands BENEATH both** — `:help`, then `:graph`
/// in the same pane, over the aim's only transcript; `:clone` from the
/// tree. `:q` gives back the manual first, then the clone where the
/// window's session lay (V5.3 design review, finding 6: put after the
/// views, the next `:q` skipped the manual for the clone).
#[tokio::test]
async fn a_composition_under_two_views_lands_beneath_both() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("help");
    app.dispatch_command_for_test("graph");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "premise: two views over it"
    );

    app.dispatch_command_for_test("clone");
    let landed = app.prompt_target();
    app.workspace.focus_pane(pane);
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "the view beneath the tree comes back first"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "then the session"
    );
    assert_eq!(
        app.pane_session(pane).id,
        landed,
        "the clone, where the session lay"
    );
}

/// The name a notice gives `buffer`'s session when it is not the aim's.
fn name_of(app: &App, buffer: BufferId) -> String {
    app.session_label(app.session_of(buffer).unwrap().conversation.episode().id)
}

/// **A notice about work off the aim names its session** (full review
/// 10-21, A 10, probed) — the queued prompt that starts in B while you are
/// in A, B's pending command dropped by its turn's end, a distillation of A
/// failing while you are in B. Each read as if it were about the session in
/// front.
#[tokio::test]
async fn notices_about_work_off_the_aim_name_its_session() {
    let (mut app, (a, _), (b, gb)) = two_turns();
    app.submit_to(b, "B next");
    assert_eq!(app.pending.len(), 1, "premise: queued behind B's turn");
    app.aim_prompt_at(a);
    app.notices.clear();
    app.feed_turn(gb, StreamMsg::Complete);
    let b_name = name_of(&app, b);
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.starts_with(&format!("{b_name}: the queued prompt is sent"))),
        "{:?}",
        app.notices
    );

    // B's turn, now the queued prompt's, asks; it fails with the call up.
    let gb = app
        .turns()
        .find(|g| g.buffer == b)
        .expect("B's next turn")
        .id;
    asks_you(&mut app, gb, "tu_b");
    assert!(
        app.shown_call_in(b).is_some(),
        "premise: B's decision is up"
    );
    app.notices.clear();
    app.feed_turn(gb, StreamMsg::Error(BackendError::RateLimited));
    assert!(
        app.notices.iter().any(|(_, n)| n.starts_with(&format!(
            "{b_name}: the turn ended — the pending command was not run"
        ))),
        "{:?}",
        app.notices
    );
}

/// **…and a distillation of A that fails while you are in B** says so of A.
#[tokio::test]
async fn a_failed_distillation_off_the_aim_names_its_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.current;
    app.dispatch_command_for_test("compact");
    let summary = app
        .generations
        .iter()
        .find(|g| matches!(g.work, Work::Summary(_)))
        .map(|g| g.id)
        .expect("premise: A distils");
    app.new_session_command();
    assert_ne!(app.prompt_target(), a, "premise: aimed off A");
    app.notices.clear();
    app.dispatch_stream_msg(summary, StreamMsg::Error(BackendError::RateLimited));
    let a_name = name_of(&app, a);
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.starts_with(&format!("{a_name}: summary generation failed"))),
        "{:?}",
        app.notices
    );
}

/// The notice a test's events left that contains `needle`, or a panic
/// listing what was said.
fn said_with(app: &App, needle: &str) -> String {
    app.notices
        .iter()
        .map(|(_, n)| n.clone())
        .find(|n| n.contains(needle))
        .unwrap_or_else(|| panic!("nothing said `{needle}`: {:?}", app.notices))
}

/// **…and one that lands says whose, and whether you are in it** — compacted
/// while you are in A, "now in session"; while you are in B, of A, and not
/// "now in": you are not (full review 10-21, A 10; both halves survived a
/// mutation of `compose_summary_into`).
#[tokio::test]
async fn a_landed_distillation_says_whose_and_whether_you_are_in_it() {
    // (aimed away from A, A still shown in a pane of its own)
    for (away, shown) in [(false, true), (true, false), (true, true)] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let a = app.current;
        // What the squashed session was, by its own tag (F 6: after the
        // landing, its buffer names the result).
        let from = app.session_tag(app.conversation().episode().id);
        app.dispatch_command_for_test("compact");
        assert!(app.distilling().is_some(), "premise: A distils");
        if away {
            if shown {
                app.set_focus(PaneKind::Output);
                app.split_command("", SplitDir::Vertical);
            }
            app.new_session_command();
            assert_ne!(app.prompt_target(), a, "premise: aimed off A");
        }
        app.notices.clear();
        app.feed_summary(StreamMsg::Chunk("the gist".into()));
        app.feed_summary(StreamMsg::Complete);
        assert!(app.decisions.review(a).is_none(), "unreviewed");
        let said = said_with(&app, "squashed");
        let a_name = name_of(&app, a);
        let case = format!("away = {away}, shown = {shown}: {said}");
        let squashed = format!("squashed 2 turns of {from} → ");
        if away {
            assert!(said.contains(&format!("{a_name}: {squashed}")), "{case}");
            assert!(!said.contains("now in"), "you are not in it: {case}");
            // In no pane, it says how to show it, FIRST (F 4); shown, it
            // does not.
            assert_eq!(said.contains("in no pane"), !shown, "{case}");
            assert_eq!(said.starts_with("`:b"), !shown, "{case}");
        } else {
            assert!(said.starts_with(&squashed), "the aim's, unnamed: {case}");
            assert!(said.contains("now in session"), "{case}");
        }
    }
}

/// **A prefill `:continue` says whose, and whether you are in it** — landed
/// and failed alike, as a distillation does (full review 10-21, A 10; both
/// survived a mutation).
#[tokio::test]
async fn a_continuation_says_whose_and_whether_you_are_in_it() {
    use shannon_core::{ContinueStrategy, StopReason};
    for (away, fails) in [(false, false), (true, false), (true, true)] {
        let mut cfg = Config::default();
        cfg.continue_.strategy = ContinueStrategy::Prefill;
        let dir = TempDir::new().unwrap();
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(cfg),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        let a = app.current;
        app.output_mut().begin_turn("write a poem");
        app.conversation_mut().begin_turn("write a poem");
        app.feed_stream(StreamMsg::Chunk("Roses are".into()));
        app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
        app.feed_stream(StreamMsg::Complete);
        let from = app.session_tag(app.conversation().episode().id);
        app.continue_last_turn();
        assert!(app.continuing(), "premise: A continues");
        if away {
            app.new_session_command();
            assert_ne!(app.prompt_target(), a, "premise: aimed off A");
        }
        app.notices.clear();
        if fails {
            app.feed_continuation(StreamMsg::Error(BackendError::RateLimited));
        } else {
            app.feed_continuation(StreamMsg::Chunk(" red".into()));
            app.feed_continuation(StreamMsg::Stopped(StopReason::EndTurn));
            app.feed_continuation(StreamMsg::Complete);
        }
        let said = said_with(
            &app,
            if fails {
                "continue failed"
            } else {
                "continued"
            },
        );
        let a_name = name_of(&app, a);
        let case = format!("away = {away}, fails = {fails}: {said}");
        if away && fails {
            assert!(said.starts_with(&format!("{a_name}: ")), "{case}");
        } else if away {
            // `:new` took A's only pane: in no pane, so the remedy first, as
            // every landing says it (full review 10-21, D 5).
            let shows = format!("`:b{}` shows it — {a_name}: continued {from}", a.number());
            assert!(said.starts_with(&shows), "{case}");
            assert!(!said.contains("now in"), "{case}");
        } else {
            // Of what, by its own tag (F 6).
            assert!(
                said.starts_with(&format!("continued {from} → now in session")),
                "{case}"
            );
        }
    }
}

/// **Claude Code's notices about a session off the aim name it** — a call
/// refused (it would run in another directory) and a question withdrawn,
/// both arriving on A's turn while you are aimed at B. Unnamed, either read
/// as the session in front's (full review 10-21, D 3; both survived a
/// mutation of the door until this).
#[tokio::test]
async fn claude_codes_notices_off_the_aim_name_their_session() {
    let (mut app, _provider) = provider_app();
    // A turn of history, so `:new` opens a buffer rather than reusing this.
    app.conversation_mut().begin_turn("A 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    let a = app.current;
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": "rm -r /tmp/shannon-test-absent-1"}),
    );
    assert!(
        app.shown_call_in(a).is_some(),
        "premise: A's question is up"
    );
    let ga = (app.generations.iter())
        .find(|g| g.buffer == a)
        .map(|g| g.id)
        .expect("premise: A's turn");
    app.new_session_command();
    assert_ne!(app.prompt_target(), a, "premise: aimed off A");
    let a_name = name_of(&app, a);

    app.notices.clear();
    let elsewhere = StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_2".into(),
        name: "Bash".into(),
        input: serde_json::json!({"command": "echo hi"}),
        cwd: Some("/shannon-test-elsewhere".into()),
        ask: question("tu_2"),
        answer_by: hook_answer_by(),
    });
    app.dispatch_stream_msg(ga, elsewhere);
    let said = said_with(&app, "call refused");
    assert!(
        said.starts_with(&format!("{a_name}: Claude Code's Bash call refused")),
        "{said}"
    );

    app.notices.clear();
    app.dispatch_stream_msg(ga, StreamMsg::ToolWithdrawn(question("tu_1")));
    let said = said_with(&app, "stopped waiting");
    assert!(
        said.starts_with(&format!("{a_name}: Claude Code stopped waiting for")),
        "{said}"
    );
}

/// **A reviewed summary whose `$EDITOR` failed says whose, and keeps the
/// draft** — in the distillation family's words, named off the aim: it said
/// "squash cancelled", unnamed, of A's draft while you were in B (full
/// review 10-21, D 3 and D 8). And the draft waits: paid work goes only by
/// `d` (design review DR-B #9); it was "cancelled — nothing composed", and
/// gone.
#[tokio::test]
async fn a_failed_summary_review_off_the_aim_names_its_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.current;
    app.dispatch_command_for_test("compact!");
    assert!(app.distilling().is_some(), "premise: A distils, reviewed");
    app.new_session_command();
    assert_ne!(app.prompt_target(), a, "premise: aimed off A");
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(app.decisions.review(a).is_some(), "premise: up for review");
    app.notices.clear();
    let mut editor = Scripted::answering([Err(anyhow::anyhow!("no editor here"))]);
    app.edit_review(a, &mut editor);
    let said = said_with(&app, "the editor failed");
    let a_name = name_of(&app, a);
    assert!(
        said.starts_with(&format!(
            "{a_name}: summary review: the editor failed (no editor here) — the draft waits"
        )),
        "{said}"
    );
    assert!(app.decisions.review(a).is_some(), "and the draft waits");
}

/// **A queued prompt whose session is gone is dropped, and said** — never
/// sent to another conversation, never announced as sent. No path closes a
/// buffer today (this guard waits for `:bd`), so the entry is planted; the
/// guard was pinned by nothing, and without it the queue said "the queued
/// prompt is sent" for a start that went nowhere (full review 10-21, C 9).
#[tokio::test]
async fn a_queued_prompt_for_a_session_gone_is_dropped_and_said() {
    let mut app = test_app();
    let gone = BufferId::numbered(9_999).expect("a buffer number");
    app.pending.push_back((gone, "for nobody".into()));
    app.notices.clear();
    app.settle();
    assert!(app.pending.is_empty(), "dropped");
    assert!(app.generations.is_empty(), "nothing started");
    let said = app
        .notices
        .iter()
        .map(|(_, n)| n.as_str())
        .collect::<Vec<_>>();
    assert!(
        said.iter()
            .any(|n| n.contains("a queued prompt was dropped")),
        "{said:?}"
    );
    assert!(!said.iter().any(|n| n.contains("is sent")), "{said:?}");
}

// --- a squash lives in the session it squashes (full review 10-21, D 1) ----

/// A with three turns, B (the aim) with one, `:graph` selecting A:
/// `(app, a_buf, b_buf, a, b)`.
fn picked_off_the_aim(dir: &std::path::Path) -> (App, BufferId, BufferId, EpisodeId, EpisodeId) {
    let mut app = app_with_n_turns(dir, 3);
    let a_buf = app.current;
    let a = app.conversation().episode().id;
    app.new_session_command();
    let b_buf = app.current;
    app.conversation_mut().begin_turn("B 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let b = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    assert_eq!(app.prompt_target(), b_buf, "premise: aimed at B");
    (app, a_buf, b_buf, a, b)
}

fn summary_on(app: &App, buffer: BufferId) -> bool {
    app.generations
        .iter()
        .any(|g| g.buffer == buffer && matches!(g.work, Work::Summary(_)))
}

/// **`S` on A's turns, aimed at B, distils on A and lands over A** — B's
/// buffer became A's squash, under A's name, B in no buffer at all; and it
/// ran on B's slot, billed to B, beyond CTRL-C at A (D 1, probed).
#[tokio::test]
async fn s_of_another_sessions_turns_distils_and_lands_there() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buf, b_buf, a, b) = picked_off_the_aim(dir.path());
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.llm_squash(false);
    assert!(summary_on(&app, a_buf), "on A's slot: {:?}", app.notices);
    assert!(!summary_on(&app, b_buf), "not B's");
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(app.decisions.every_review().next().is_none(), "unreviewed");
    let holds = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_ne!(holds(&app, a_buf), a, "A's buffer holds A's squash");
    assert_eq!(holds(&app, b_buf), b, "B's holds B");
}

/// **A squash that replaces a session lands over THAT session, whatever
/// buffer it is handed** — `adopt_composed` derives it from what it carries.
/// The callers hand it the right one now (`squash_home`); this pins the
/// door itself, which took the caller's word for it (D 1, probed through
/// `:squash`'s half after `$EDITOR`, which composed over the aim).
#[tokio::test]
async fn a_replacing_squash_lands_over_what_it_carries() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buf, b_buf, a, b) = picked_off_the_aim(dir.path());
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let plan = app.squash_plan().expect("the marks resolve");
    let text = to_editable(&[Message::user("q"), Message::assistant("a")]);
    let composed =
        app.commit_summary_into(b_buf, plan.graph, plan.target, &plan.sources, &text, None);
    assert!(composed.is_some(), "premise: it composed");
    let holds = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_ne!(holds(&app, a_buf), a, "over A, whose turns it squashed");
    assert_eq!(
        holds(&app, b_buf),
        b,
        "not over B, the buffer it was handed"
    );
}

/// **…and says so at the press, with where its stop is** — it said
/// nothing, the row "1 session running", and the graph's `Esc`, the
/// legend's "Esc cancels", fell through silent (full review 10-21, F 3,
/// probed). `Esc` cancels only the aim's, as CTRL-C stops only the aim's
/// reach: it says so, and A goes on.
#[tokio::test]
async fn s_of_another_sessions_turns_says_so_and_where_its_stop_is() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buf, _, _, _) = picked_off_the_aim(dir.path());
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.notices.clear();
    app.llm_squash(false);
    let a_name = name_of(&app, a_buf);
    let said = said_with(&app, "distilling");
    assert!(said.starts_with(&format!("{a_name}: distilling")), "{said}");
    assert!(said.contains("CTRL-C in its pane stops it"), "{said}");

    app.notices.clear();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let said = said_with(&app, "Esc");
    assert!(
        said.starts_with(&format!(
            "{a_name}: distilling — `Esc` cancels only the aimed session's"
        )),
        "{said}"
    );
    assert!(summary_on(&app, a_buf), "A distils on");
}

/// **…and is refused while A answers, whoever is aimed** — it asked the
/// aim, so `S` on A's turns ran beside A's own reply (D 1, probed).
#[tokio::test]
async fn s_of_a_busy_sessions_turns_is_refused_off_the_aim() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buf, _, _, _) = picked_off_the_aim(dir.path());
    let ga = app.fake_stream_on(a_buf);
    app.session_of_mut(a_buf)
        .unwrap()
        .conversation
        .begin_turn("A 3");
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.notices.clear();
    app.llm_squash(false);
    assert!(app.distilling().is_none(), "refused");
    assert!(running(&app, ga), "A's reply runs on");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("in flight"), "says why: {said}");
    // Of A, named — you are in B, which is idle (F 5, probed).
    let a_name = name_of(&app, a_buf);
    assert!(said.starts_with(&format!("{a_name}: S: ")), "{said}");
}

/// **`:squash` of a session whose reply streams is refused before
/// `$EDITOR`** — it checked nothing and composed over the aim, so a squash
/// of the aim's own early turns ended its reply: silently if nothing had
/// arrived, else "the turn you left was interrupted" (D 1, probed). Through
/// `squash_by_hand`, the half before the editor.
#[tokio::test]
async fn a_hand_squash_of_a_session_answering_is_refused() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let a = app.conversation().episode().id;
    let a_buf = app.current;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char(' '),
        KeyCode::Char('j'),
        KeyCode::Char(' '),
    ] {
        app.graph_key_for_test(k(code));
    }
    assert!(app.squash_by_hand().is_some(), "premise: the marks resolve");
    let ga = app.fake_stream_on(a_buf);
    app.session_of_mut(a_buf)
        .unwrap()
        .conversation
        .begin_turn("A 3");
    app.notices.clear();
    assert!(app.squash_by_hand().is_none(), "refused before `$EDITOR`");
    assert!(running(&app, ga), "the reply runs on");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("in flight"), "says why: {said}");
}

/// **`:compact` of a session picked in `:graph`, aimed elsewhere, is that
/// session's** — refused while it answers, distilled on its slot, landed
/// over it, the aim untouched. Right since it was built, and pinned by
/// nothing: asking or distilling on the aim passed every test (D 2's D1,
/// D2).
#[tokio::test]
async fn compacting_a_session_picked_in_the_graph_is_that_sessions() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buf, b_buf, a, b) = picked_off_the_aim(dir.path());
    let ga = app.fake_stream_on(a_buf);
    app.session_of_mut(a_buf)
        .unwrap()
        .conversation
        .begin_turn("A 3");
    app.dispatch_command_for_test("compact");
    assert!(app.distilling().is_none(), "refused while A answers");
    assert!(running(&app, ga), "A's reply runs on");
    app.feed_turn(ga, StreamMsg::Complete);

    app.dispatch_command_for_test("compact");
    assert!(summary_on(&app, a_buf), "on A's slot: {:?}", app.notices);
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(app.decisions.every_review().next().is_none(), "unreviewed");
    let holds = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_ne!(holds(&app, a_buf), a, "A's buffer holds A's compaction");
    assert_eq!(holds(&app, b_buf), b, "B is untouched");
}

/// **The audit files a call under the session that asked** — B's call,
/// answered from the pane while aimed at A, was audited under A: on a key
/// the span is the focused session's, and `audit` named none (full review
/// 10-21, E 2, probed). Aimed at B, under B too (the control).
#[tokio::test]
async fn an_answer_from_the_pane_is_audited_under_the_calls_session() {
    for aimed_at_asker in [false, true] {
        let dir = TempDir::new().unwrap();
        let (mut app, a_buffer, b_buffer) = two_sessions_side_by_side(dir.path());
        model_call(&mut app, "tu_b1", "rm /tmp/shannon-test-absent-1");
        app.aim_prompt_at(if aimed_at_asker { b_buffer } else { a_buffer });
        app.settle();
        assert_eq!(
            app.tool_buffer("tu_b1"),
            Some(b_buffer),
            "premise: B's call"
        );
        let hex = |app: &App, b| {
            app.session_of(b)
                .unwrap()
                .conversation
                .episode()
                .id
                .short_hex()
        };
        let (a_hex, b_hex) = (hex(&app, a_buffer), hex(&app, b_buffer));
        assert_ne!(a_hex, b_hex, "premise: two sessions");
        to_decision(&mut app);
        let logs = captured_logs(|| {
            // What `handle_event` enters before any key.
            let _session = app.span_for(app.focused_buffer().id).entered();
            tool_key(&mut app, 'd');
        });
        let line = (logs.lines())
            .find(|l| l.contains("shannon::audit"))
            .unwrap_or_else(|| panic!("no audit line: {logs}"));
        // The event's own `episode` — the field the log's formatter prefers
        // to the span's (`logging.rs`: explicit over ambient). This harness
        // prints the span too, which is A's when aimed at A.
        let (_, fields) = line
            .split_once("shannon::audit:")
            .expect("the audit target");
        assert!(
            fields.contains(&format!("episode=\"{b_hex}"))
                && !fields.contains(&format!("episode=\"{a_hex}")),
            "aimed at the asker: {aimed_at_asker}: {line}"
        );
    }
}

/// **A failure's reason reaches the log scrubbed, whatever failed** — a
/// distillation, a prefill continuation, a child's turn. Each notice was
/// scrubbed, and the same text was logged raw beside it ("summary
/// generation failed", "distillation ended", "continuation failed",
/// "continuation ended", "child session turn failed"), so a marked secret
/// in `claude`'s reason sat in `shannon.log` in clear (full review 10-21,
/// E 3, probed).
#[tokio::test]
async fn a_failures_reason_logs_no_secret() {
    use shannon_core::{ContinueStrategy, StopReason};
    let secret = "hunter2beornot2be";
    let failed = || {
        StreamMsg::Error(BackendError::ClaudeCode(format!(
            "`claude` exited (exit status: 1) before finishing the turn — could not reach {secret}"
        )))
    };
    for kind in ["distillation", "continuation", "child"] {
        let mut cfg = Config::default();
        cfg.continue_.strategy = ContinueStrategy::Prefill;
        let dir = TempDir::new().unwrap();
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(cfg),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        app.mark_secret(secret, true).expect("marked");
        app.conversation_mut().begin_turn("first");
        app.conversation_mut().push_chunk("r");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
        let (logs, premise) = match kind {
            "distillation" => {
                let turn = app.conversation().episode().own_turns()[0];
                let target = SquashTarget::Gather {
                    episode: app.conversation().episode().id,
                    turn,
                    index: 0,
                    config: app.conversation().episode().config.clone(),
                };
                app.fake_summary_on(app.current, target, vec![turn]);
                let logs = captured_logs(|| {
                    app.feed_summary(failed());
                });
                (logs, "summary generation failed")
            }
            "continuation" => {
                app.output_mut().begin_turn("write a poem");
                app.conversation_mut().begin_turn("write a poem");
                app.feed_stream(StreamMsg::Chunk("Roses are".into()));
                app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
                app.feed_stream(StreamMsg::Complete);
                app.continue_last_turn();
                assert!(app.continuing(), "premise: a continuation drafts");
                let logs = captured_logs(|| app.feed_continuation(failed()));
                (logs, "continuation failed")
            }
            _ => {
                let child = app.spawn_child_job("look", None).expect("spawns");
                let logs = captured_logs(|| app.feed_job(child, failed()));
                (logs, "child session turn failed")
            }
        };
        assert!(logs.contains(premise), "{kind}: premise: logged:\n{logs}");
        assert!(!logs.contains(secret), "{kind}:\n{logs}");
    }
}

/// **What `claude` said as it died is scrubbed before it is cut** — a
/// `password=` pair whose name falls just before the 600-character cut:
/// core cut first, so the scrub saw a bare value no rule matched, and it
/// reached the notice and the log (full review 10-21, E 3, probed).
#[tokio::test]
async fn what_claude_said_as_it_died_is_scrubbed_before_it_is_cut() {
    let mut app = test_app();
    let value = "hunter2seekrit";
    // The pair's name, then 600 characters from its value to the end.
    let said = format!("password={value} {}", "x".repeat(585));
    let cut_first = app.scrub(tail_chars(&said, CLAUDE_SAID_CHARS));
    assert!(
        cut_first.contains(value),
        "premise: cut first, the value goes unmatched"
    );
    assert!(
        !app.scrub(&said).contains(value),
        "premise: whole, it is matched"
    );

    // Something streamed, so the turn seals and its notice quotes the reason.
    app.conversation_mut().begin_turn("go");
    app.feed_stream(StreamMsg::Chunk("half".into()));
    app.notices.clear();
    let logs = captured_logs(|| {
        app.feed_stream(StreamMsg::Error(shannon_core::BackendError::ClaudeExited {
            status: "exit status: 1".into(),
            said: said.clone(),
        }));
    });
    let notices: Vec<&str> = app.notices.iter().map(|(_, n)| n.as_str()).collect();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("before finishing the turn")),
        "premise: it said so: {notices:?}"
    );
    assert!(!notices.iter().any(|n| n.contains(value)), "{notices:?}");
    assert!(!logs.contains(value), "{logs}");
}

/// **"NOT SAVED TO DISK" says whose** — a turn ending on A while you are in
/// B, the write refused: the data-loss notice named no session, and its
/// reason said "this session" of one you were not in (full review 10-21,
/// E 5).
#[tokio::test]
async fn not_saved_off_the_aim_names_its_session() {
    use shannon_core::store::FailingStore;
    let (store, failing) = FailingStore::recoverable();
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "Hello 0");
    let a = app.current;
    app.new_session_command();
    assert_ne!(app.prompt_target(), a, "premise: aimed off A");
    let ga = app.fake_stream_on(a);
    app.session_of_mut(a)
        .unwrap()
        .conversation
        .begin_turn("A 1");
    app.feed_turn(ga, StreamMsg::Chunk("reply".into()));
    failing.store(true, std::sync::atomic::Ordering::SeqCst);
    app.notices.clear();
    app.feed_turn(ga, StreamMsg::Complete);
    let a_name = name_of(&app, a);
    let said = said_with(&app, "NOT SAVED TO DISK");
    assert!(
        said.starts_with(&format!("{a_name}: NOT SAVED TO DISK")),
        "{said}"
    );
}

/// **A record takes only its own turn's edit** — B's edit note waiting,
/// and a record of that id arriving on A's turn: by bare id, A's
/// conversation would get B's "the user did not run your call" note.
/// Unreachable while Claude Code's ids are the API's, so the entry is
/// planted (full review 10-21, E 6).
#[tokio::test]
async fn a_record_takes_only_its_own_turns_edit() {
    let (mut app, (_, ga), (_, gb)) = two_turns();
    let edit = serde_json::json!({"command": "echo edited"});
    app.provider_edits.insert("tu_x".into(), (Some(gb), edit));
    let record = StreamMsg::ToolRecord {
        call: shannon_core::Block::ToolUse {
            id: "tu_x".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo asked"}),
        },
        result: shannon_core::Block::ToolResult {
            tool_use_id: "tu_x".into(),
            content: "asked".into(),
            is_error: false,
        },
    };
    app.dispatch_stream_msg(ga, record);
    assert!(
        app.provider_edits.contains_key("tu_x"),
        "B's edit, not taken by A's record"
    );
}

/// **A fresh session with work waiting is not blank** — a typed `:tool`
/// waiting on you, a prompt queued behind it. `is_blank` asked only for a
/// generation, which a typed `:tool` has none of, so `:clone` reused the
/// buffer: the call and the prompt went with it into the clone, the prompt
/// a billed turn on another session's history (full review 10-21, E 4,
/// probed). One turn of history and the clone landed beside it instead.
#[tokio::test]
async fn a_fresh_session_with_work_waiting_is_not_reused() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.conversation().episode().id;
    app.new_session_command();
    let (fresh_buf, fresh) = (app.current, app.conversation().episode().id);
    assert_ne!(fresh, a, "premise: a fresh session");
    let call = manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    assert!(app.shown_call_in(fresh_buf).is_some(), "premise: it asks");
    app.submit("hello");
    assert_eq!(app.pending.len(), 1, "premise: queued behind the decision");

    app.clone_episode(a);
    let holds = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_eq!(holds(&app, fresh_buf), fresh, "its buffer still holds it");
    assert_eq!(
        app.tool_call(&call).map(|c| (c.buffer, c.session)),
        Some((fresh_buf, fresh)),
        "the call is still the fresh session's"
    );
    assert_eq!(app.pending[0].0, fresh_buf, "and the prompt waits there");
    assert_ne!(
        app.aimed_buffer().id,
        fresh_buf,
        "the clone has a buffer of its own"
    );

    // **A typed `:tool` running**, which no busy question counts — it has
    // no generation and nothing waits on you — kept the session too.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    let a = app.conversation().episode().id;
    app.new_session_command();
    let (fresh_buf, fresh) = (app.current, app.conversation().episode().id);
    let call = manual_tool(&mut app, "echo hi");
    assert!(app.running_tools.contains_key(&call), "premise: it runs");
    assert!(
        app.busy_reason(fresh_buf).is_none(),
        "premise: nothing busy counts it"
    );
    app.clone_episode(a);
    let holds = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_eq!(holds(&app, fresh_buf), fresh, "a running `:tool` keeps it");
}

// --- decisions per session (`decision-panes-and-alerts.md` §3) -------------

/// Sessions A and B side by side (and C, aimed at, beside them), each with a
/// turn whose call waits on you — two different commands, each in its own
/// pane. Returns the windows, the sessions and their turns.
fn two_decisions() -> (
    App,
    [crate::wm::PaneId; 3],
    [BufferId; 3],
    [GenerationId; 2],
) {
    let (mut app, panes, [a, b, c]) = three_sessions();
    let mut turns = Vec::new();
    for (buffer, id) in [(a, "tu_a"), (b, "tu_b")] {
        let turn = app.fake_stream_on(buffer);
        app.session_of_mut(buffer)
            .unwrap()
            .conversation
            .begin_turn("1");
        let command = format!("rm -rf /tmp/shannon-test-absent-{id}");
        app.feed_turn(
            turn,
            StreamMsg::ToolUse(shannon_core::Block::bash_call(id, &command)),
        );
        app.feed_turn(turn, StreamMsg::Complete);
        turns.push(turn);
    }
    app.settle();
    assert!(app.shown_call_in(a).is_some() && app.shown_call_in(b).is_some());
    (app, panes, [a, b, c], turns.try_into().unwrap())
}

/// **A policy change re-judges every session's call** (design review DR-A
/// #1) — a rule denying B's command, taken while you stand in A's pane: B's
/// is denied, A's untouched. One open call was revisited, so the others'
/// panes kept offering `a` for a command the policy now denied, and
/// `run_tool_call` re-judges nothing.
#[tokio::test]
async fn a_policy_change_rejudges_every_sessions_call() {
    let (mut app, _, [a, b, _], _) = two_decisions();
    to_decision_of(&mut app, a);
    let taken = app.add_session_rule("deny", "rm -rf /tmp/shannon-test-absent-tu_b");
    assert!(taken.is_ok(), "premise: the rule is taken: {taken:?}");
    assert!(app.shown_call_in(b).is_none(), "B's re-judged");
    let said = result_in(&app, b, "tu_b").expect("B's answered");
    assert!(said.contains("denied"), "{said}");
    assert!(app.shown_call_in(a).is_some(), "A's untouched");
}

/// **Each decision pane is fitted at its own width** (DR-A #2) — side by
/// side at two widths, each measured where it is drawn: one width for all
/// was the first pane's, so the second's report was sized for another.
#[tokio::test]
async fn each_decision_pane_is_measured_at_its_own_width() {
    let (mut app, [pa, _, _], [a, b, _], _) = two_decisions();
    // A's column narrowed: two widths.
    app.focus_pane(pa);
    app.workspace.resize(SplitDir::Vertical, -12, 150, 30);
    app.settle();
    screen(&mut app, 150);
    let (da, db) = (
        app.decision_pane_of(a).unwrap(),
        app.decision_pane_of(b).unwrap(),
    );
    let rects = app
        .workspace
        .rects(ratatui::layout::Rect::new(0, 0, 150, 30));
    let content_width = |pane| {
        (rects.iter())
            .find(|r| r.id == pane)
            .map(|r| r.content_size().0)
            .expect("drawn")
    };
    let drawn = |app: &App, pane| {
        (app.window_of(pane).and_then(Window::decision)).map_or(0, |d| d.drawn_width)
    };
    assert_ne!(content_width(da), content_width(db), "premise: two widths");
    assert_eq!(drawn(&app, da), content_width(da), "A's at A's width");
    assert_eq!(drawn(&app, db), content_width(db), "B's at B's width");
}

/// **A pane with no room is said, each session's** (DR-A #3) — a frame too
/// short for either decision pane: the row names one and counts the other.
/// With one slot for what was drawn, a second session's call dropped for
/// room had no pane and no row.
#[tokio::test]
async fn every_decision_without_room_is_on_the_row() {
    let (mut app, _, [a, b, _], _) = two_decisions();
    let short = ratatui::layout::Rect::new(0, 0, 150, 9);
    let fitted = app.workspace.frame_layout(short);
    assert!(
        fitted.first_of(PaneKind::Tool).is_none(),
        "premise: neither fits"
    );
    let row = text_of(&mut app, 150, 9);
    let row = row.lines().last().unwrap_or_default().to_string();
    // The other counted beside the name, ahead of the command, whose tail the
    // row clips: trailing it, the count went with it at 150 columns.
    let named = [a, b].map(|s| name_of(&app, s));
    assert!(
        (named.iter()).any(|n| row.contains(&format!("DECIDE in {n} (+1 more): no room"))),
        "one named, the other counted: {row}"
    );
}

/// **One session's withdrawal leaves another's pane alone** (DR-A #4) —
/// B's turn stopped while you stand in A's pane: A's pane is the same pane,
/// its call still shown, and you are still in it. Closing "the" decision
/// pane closed every one.
#[tokio::test]
async fn one_sessions_withdrawal_leaves_anothers_pane_alone() {
    let (mut app, _, [a, b, _], [_, gb]) = two_decisions();
    to_decision_of(&mut app, a);
    let da = app.decision_pane_of(a).unwrap();
    assert!(app.withdraw_decisions(gb), "premise: B's withdrawn");
    app.settle();
    assert!(app.shown_call_in(b).is_none(), "B's gone");
    assert_eq!(app.decision_pane_of(a), Some(da), "A's the same pane");
    assert!(app.shown_call_in(a).is_some(), "A's still shown");
    assert_eq!(app.workspace.focused(), da, "and you are still in it");
}

/// **Closing the pane you stand in leaves you at the prompt** (DR-A #6) —
/// A's turn stopped while you stand in A's pane, B's beside it: the prompt,
/// never wherever focus would fall.
#[tokio::test]
async fn a_closing_decision_pane_hands_focus_to_the_prompt() {
    let (mut app, _, [a, b, _], [ga, _]) = two_decisions();
    to_decision_of(&mut app, a);
    assert!(app.withdraw_decisions(ga), "premise: A's withdrawn");
    app.settle();
    assert_eq!(app.focus(), PaneKind::Prompt, "at the prompt");
    assert!(app.decision_pane_of(b).is_some(), "B's still there");
}

/// **A decision pane stays where it is** (DR-A #5) — A in two windows, its
/// pane under the first: moving focus to A's other window, and into the
/// pane, moves nothing, and a size you give it holds through the events
/// after. Re-placed each event, it moved under the cursor.
#[tokio::test]
async fn a_decision_pane_stays_where_it_is() {
    let (mut app, [pa, _, pc], [a, _, _], _) = two_decisions();
    app.focus_pane(pc);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    assert_eq!(app.pane_session(pc).id, a, "premise: A in two windows");
    let da = app.decision_pane_of(a).expect("A's pane");
    let frame = ratatui::layout::Rect::new(0, 0, 150, 30);
    let rect_of = |app: &App, pane| {
        (app.workspace.rects(frame).iter())
            .find(|r| r.id == pane)
            .map(|r| r.rect)
    };
    let before = rect_of(&app, da);
    for pane in [pc, pa, da] {
        app.focus_pane(pane);
        app.settle();
        assert_eq!(app.decision_pane_of(a), Some(da), "the same pane");
        assert_eq!(rect_of(&app, da), before, "where it was");
    }
    app.workspace.resize(SplitDir::Horizontal, 3, 150, 30);
    let sized = rect_of(&app, da);
    assert_ne!(sized, before, "premise: resized");
    for _ in 0..10 {
        app.settle();
    }
    assert_eq!(rect_of(&app, da), sized, "the size holds");
}

/// **A pane that comes back re-arms** (DR-A #7) — A in two windows, the
/// one its pane sits under turned to another session: the pane moves under
/// A's other window, and its keys wait again, so a keystroke on its way
/// answers nothing that reappeared under it.
#[tokio::test]
async fn a_decision_pane_that_comes_back_re_arms() {
    let (mut app, [pa, _, pc], [a, b, _], _) = two_decisions();
    app.focus_pane(pc);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    app.read_the_call_for_test();
    let before = app.decision_pane_of(a).expect("A's pane");
    assert_eq!(app.answer_keys(before), AnswerKeys::Live, "premise: read");
    app.let_time_pass(std::time::Duration::ZERO);
    app.focus_pane(pa);
    app.dispatch_command_for_test(&format!("b {}", b.number()));
    let after = app.decision_pane_of(a).expect("A's pane, still");
    assert_ne!(after, before, "it moved under A's other window");
    assert_eq!(
        app.answer_keys(after),
        AnswerKeys::Waiting,
        "and waits again"
    );
}

/// **A held pane waits for its own session's next call** — A and B each
/// with a call shown and one behind it, B's answered from B's pane: B's
/// pane holds for B's next, though A's queue comes first in every session's
/// order. Asked of every session's queue, the wait read A's next as B's
/// and closed B's pane (A1.3, a mutant that survived every other test).
#[tokio::test]
async fn a_held_pane_waits_for_its_own_sessions_next_call() {
    let (mut app, (_, ga), (b, gb)) = two_turns();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    for (turn, ids) in [(ga, ["tu_a", "tu_a2"]), (gb, ["tu_b", "tu_b2"])] {
        for id in ids {
            let command = format!("rm -rf /tmp/shannon-test-absent-{id}");
            let call = shannon_core::Block::bash_call(id, &command);
            app.feed_turn(turn, StreamMsg::ToolUse(call));
        }
        app.feed_turn(turn, StreamMsg::Complete);
    }
    app.settle();
    let behind: Vec<&str> = app.waiting_calls().into_iter().map(|(id, _)| id).collect();
    assert_eq!(behind, ["tu_a2", "tu_b2"], "premise: A's queue first");
    let db = app.decision_pane_of(b).expect("premise: B's pane");

    tool_key_in(&mut app, b, 'a');
    assert_eq!(app.decision_pane_of(b), Some(db), "B's pane holds");
    let (title, _) = app.wait_text_for_test().expect("for B's next call");
    assert!(title.contains("· 1 queued"), "{title:?}");
}

// --- a session in no window (`decision-panes-and-alerts.md` §3.5, A1.4) ---

/// Whether any window shows `buffer`'s session — its own, or one a viewer
/// borrowed.
fn in_a_window(app: &App, buffer: BufferId) -> bool {
    (app.session_panes(Over::ThisTab)).any(|p| app.pane_session(p).id == buffer)
}

/// The window `buffer`'s decision pane sits under.
fn host_of(app: &App, buffer: BufferId) -> crate::wm::PaneId {
    let pane = app.decision_pane_of(buffer).expect("its pane");
    (app.window_of(pane).and_then(Window::decides))
        .map(|(_, host)| host)
        .expect("under a window")
}

/// **A decision in no window waits, said, and `:decide` brings it** (§3.5;
/// design review DR-A #9) — A's call while `:new` has B in A's only window:
/// no pane opens beside B's conversation, the row says whose call waits and
/// how to reach it, and A's turn waits on an answer that can come. It opened
/// beside the first conversation on screen, B's, said whose by its title
/// alone. `:decide` shows A and stands you in its pane, its keys waiting.
#[tokio::test]
async fn a_decision_in_no_window_waits_and_decide_brings_it() {
    let (mut app, (a, ga), _) = two_turns();
    assert!(!in_a_window(&app, a), "premise: no window shows A");
    asks_you(&mut app, ga, "tu_a");
    app.settle();
    assert!(app.shown_call_in(a).is_some(), "A's call is up");
    assert!(
        !app.workspace.has(PaneKind::Tool),
        "with no pane beside B's"
    );
    assert_eq!(
        stage(&app, ga),
        Some(Stage::ToolRound),
        "A's turn waits on it"
    );
    app.set_focus(PaneKind::Prompt);
    let row = status_row(&mut app, "prompt", "$hidden");
    let waits = format!("decision hidden · `:decide {}`", a.number());
    assert!(row.contains(&waits), "said: {row}");
    let row = status_row(&mut app, "prompt", "$decide");
    assert!(
        !row.contains("DECIDE"),
        "and not by `$decide`, which is for a pane's keys: {row}"
    );

    // Long after it was shown: the keys wait again because `:decide` armed
    // them, not because the call is new.
    app.let_time_pass(std::time::Duration::from_secs(5));
    app.dispatch_command_for_test("decide");
    let pane = app.decision_pane_of(a).expect("`:decide` opens A's pane");
    assert_eq!(app.pane_session(host_of(&app, a)).id, a, "under A, shown");
    assert_eq!(app.workspace.focused(), pane, "and stands you in it");
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Waiting,
        "its keys waiting"
    );
    tool_key_in(&mut app, a, 'd'); // a deny runs nothing: no outcome to wait for
    let said = result_in(&app, a, "tu_a").expect("answered");
    assert!(said.contains("denied"), "{said}");
}

/// **`:decide` picks, and says what it cannot go to** — with no N the aim's
/// first, then one in no window over one on screen; a session with no
/// decision, and a number no session has, refused by name.
#[tokio::test]
async fn decide_picks_the_aims_then_one_in_no_window() {
    let (mut app, (a, _), (_, gb)) = two_turns();
    app.dispatch_command_for_test("decide");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("no decision is waiting on you"), "{said}");

    asks_you(&mut app, gb, "tu_b");
    app.settle();
    app.dispatch_command_for_test(&format!("decide {}", a.number()));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains(&format!("no decision is waiting in b{}", a.number())),
        "{said}"
    );
    app.dispatch_command_for_test("decide 99");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("there is no buffer b99"), "{said}");

    // B's window turned to C: B's call in no window, A's on screen and first
    // by number, the aim at C, which has none.
    let (mut app, [_, pb, _], [a, b, c], _) = two_decisions();
    app.focus_pane(pb);
    app.dispatch_command_for_test(&format!("b {}", c.number()));
    assert_eq!(app.decision_pane_of(b), None, "premise: B's in no window");
    assert!(app.decision_pane_of(a).is_some(), "premise: A's on screen");
    let in_its_pane = |app: &App, s| app.decision_pane_of(s) == Some(app.workspace.focused());
    // The aim's first while B's still waits in no window — after the other
    // case B's has a pane, and the order between the two keys is untested.
    for (aim, goes_to, why) in [
        (a, a, "the aim's first, though B's waits in no window"),
        (
            c,
            b,
            "with none at the aim, the one in no window, not the first",
        ),
    ] {
        app.aim_prompt_at(aim);
        app.set_focus(PaneKind::Prompt);
        app.dispatch_command_for_test("decide");
        assert!(in_its_pane(&app, goes_to), "{why}");
    }
}

/// **A call in no window wakes nothing** — the event loop wakes when a
/// pane's keys come alive (`arming_ends_at`), and a call with no pane has
/// none. It is re-armed at every event's end (`settle_decisions`, which
/// arms what it fitted, opened or not), so the wake asking of every shown
/// call, not of a pane's, would come every `tui.approval_delay_ms` for as
/// long as the call waited.
#[tokio::test]
async fn a_call_in_no_window_wakes_nothing() {
    let (mut app, (a, ga), _) = two_turns();
    asks_you(&mut app, ga, "tu_a");
    app.settle();
    assert_eq!(app.decision_pane_of(a), None, "premise: no window, no pane");
    app.let_time_pass(std::time::Duration::from_secs(5));
    app.settle();
    let now = app.now();
    assert!(
        app.arming_ends_at().is_none_or(|at| at <= now),
        "no wake ahead for it: {:?}",
        app.arming_ends_at()
            .map(|at| at.saturating_duration_since(now))
    );
}

/// **A pane moved by placement leaves nothing to deny by** — A in two
/// windows, the one its pane sat under turned to C: the pane moves under
/// A's other window, and a close after it, of a pane deciding nothing,
/// leaves A's call open. The pane closed for the move kept its view, which
/// the next close read as a decision pane you closed, its session shown;
/// since V5.3 (4) it is the window the workspace gives back, which the move
/// claims (`close_decision_pane_by_id`). Left unclaimed, the call is hidden
/// — waiting, its pane gone — since 10-26, not denied: so "open, never
/// denied" held under a move that stopped claiming, and the pane itself is
/// asked after now (review stop 12, T5 F1: probed RED without the claim).
#[tokio::test]
async fn a_pane_moved_by_placement_leaves_nothing_to_deny_by() {
    let (mut app, [pa, _, pc], [a, _, c], _) = two_decisions();
    app.focus_pane(pc);
    app.dispatch_command_for_test(&format!("b {}", a.number()));
    let before = app.decision_pane_of(a).expect("A's pane");
    app.focus_pane(pa);
    app.dispatch_command_for_test(&format!("b {}", c.number()));
    let after = app.decision_pane_of(a).expect("A's pane, moved");
    assert_ne!(after, before, "premise: moved under A's other window");

    app.split_command("", SplitDir::Vertical);
    app.notices.clear();
    app.close_pane_command(Asked::Plain);
    app.settle();
    assert!(app.shown_call_in(a).is_some(), "A's still open");
    assert_eq!(result_in(&app, a, "tu_a"), None, "never denied");
    assert_eq!(
        app.decision_pane_of(a),
        Some(after),
        "and still in its pane: the close of another took nothing of it"
    );
    let said = app.notice().map(|n| n.to_string()).unwrap_or_default();
    assert!(!said.contains("waits, hidden"), "nor hid it: {said}");
}

/// **`:tool` shows its session first** (§3.5, DR-A #9) — typed with the aim
/// in no window: a window turns to it, and its call asks under it. Filed
/// under the aim and no window, it would wait unseen for an answer to a
/// command you had just typed.
#[tokio::test]
async fn a_typed_tool_shows_its_session_first() {
    let (mut app, (a, _), _) = two_turns();
    app.aim_prompt_at(a);
    assert!(!in_a_window(&app, a), "premise: no window shows A");
    app.dispatch_command_for_test("tool rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call_in(a).is_some(), "premise: it asks");
    assert_eq!(app.pane_session(host_of(&app, a)).id, a, "under A, shown");
}

/// **`:setup` opens over a decision, its legend saying one waits behind it
/// only when one does** (Steven, 10-26: "I'd open it up so these don't
/// block"; refused over one on screen from DR-A #11 until then). A call in
/// no window is under nothing setup covers, and the row points to it; one
/// in a pane is, and the legend takes the place of setup's keys.
#[tokio::test]
async fn setup_opens_over_a_decision_its_legend_saying_when_one_waits_behind() {
    let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let (mut app, (a, ga), (b, gb)) = two_turns();
    asks_you(&mut app, ga, "tu_a");
    app.settle();
    assert_eq!(app.decision_pane_of(a), None, "premise: A's in no window");
    app.dispatch_command_for_test("setup");
    assert!(app.setup().is_some(), "opened over one in no window");
    let shown = screen(&mut app, 100);
    assert!(
        !shown.contains("a decision is waiting"),
        "none behind: {shown}"
    );
    assert!(app.setup_event_for_test(&esc), "premise: Esc leaves");

    asks_you(&mut app, gb, "tu_b");
    app.settle();
    assert!(app.decision_pane_of(b).is_some(), "premise: B's on screen");
    app.dispatch_command_for_test("setup");
    assert!(app.setup().is_some(), "opened over one on screen");
    let shown = screen(&mut app, 100);
    assert!(shown.contains("a decision is waiting"), "said: {shown}");
}

// --- a request holds its own window, never the one beside -------------------

/// **Steven's geometry** (10-26, Q3): A's chat with its call's pane under it,
/// B's chat beside it, the prompt below — standing in B's window, A's call
/// up and unanswered. Returns the windows `[A's, B's]` and the sessions.
fn a_asks_beside_b() -> (App, [crate::wm::PaneId; 2], [BufferId; 2]) {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let (pa, a) = (app.workspace.focused(), app.current);
    // A turn of history in each, so `:new` mints rather than reuses.
    let history = |app: &mut App| {
        app.conversation_mut().begin_turn("0");
        app.conversation_mut().push_chunk("r");
        app.conversation_mut().commit_turn();
    };
    history(&mut app);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    history(&mut app);
    let (pb, b) = (app.workspace.focused(), app.current);
    assert_ne!(a, b, "premise: two sessions");
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("1");
    asks_you(&mut app, turn, "tu_a");
    app.settle();
    assert_eq!(host_of(&app, a), pa, "premise: under A's window");
    assert_eq!(app.workspace.focused(), pb, "premise: standing in B's");
    (app, [pa, pb], [a, b])
}

/// A's call, still up and unanswered, in its pane under `pa`, and the
/// cursor not in it.
fn a_still_asks_under(app: &App, a: BufferId, pa: crate::wm::PaneId, what: &str) {
    let up = app.shown_call_in(a).map(|p| p.tool_use_id().to_string());
    assert_eq!(up.as_deref(), Some("tu_a"), "{what}: A's call up");
    assert_eq!(result_in(app, a, "tu_a"), None, "{what}: not answered");
    assert_eq!(host_of(app, a), pa, "{what}: under A's window");
    assert_ne!(
        Some(app.workspace.focused()),
        app.decision_pane_of(a),
        "{what}: the cursor not moved into it"
    );
}

/// **A request holds its own window, never the one beside** (Steven, 10-26,
/// Q3: "a decision in Pane 1 doesn't prevent changing the view in Pane 2"),
/// in his geometry — from B's window, each way to change what a window
/// shows, to open one, to go to a tab or move a window to one, and to close
/// B's: done, nothing refused over A's call, which waits in its pane under
/// A's window, unanswered, the cursor never moved into it. Only a close that
/// would take A's window is a close over it — `:only` from B, refused.
#[tokio::test]
async fn a_request_holds_its_own_window_never_the_one_beside() {
    type Act = fn(&mut App, [BufferId; 2]);
    type Done = fn(&App, [crate::wm::PaneId; 2], [BufferId; 2]) -> bool;
    let cases: [(&str, Act, Done); 14] = [
        (
            ":b A",
            |app, [a, _]| _ = app.dispatch_command_for_test(&format!("b {}", a.number())),
            |app, [_, pb], [a, _]| app.pane_session(pb).id == a,
        ),
        (
            ":help",
            |app, _| _ = app.dispatch_command_for_test("help"),
            |app, [_, pb], _| app.workspace.kind_of(pb) != Some(PaneKind::Output),
        ),
        (
            ":help, then CTRL-^",
            |app, _| {
                app.dispatch_command_for_test("help");
                app.window_command_for_test(WindowCmd::Alternate);
            },
            |app, [_, pb], [_, b]| {
                app.workspace.kind_of(pb) == Some(PaneKind::Output) && app.pane_session(pb).id == b
            },
        ),
        (
            ":ls",
            |app, _| _ = app.dispatch_command_for_test("ls"),
            |app, [_, pb], _| app.workspace.kind_of(pb) != Some(PaneKind::Output),
        ),
        (
            ":graph",
            |app, _| _ = app.dispatch_command_for_test("graph"),
            |app, [_, pb], _| app.workspace.kind_of(pb) != Some(PaneKind::Output),
        ),
        (
            ":new",
            |app, _| _ = app.dispatch_command_for_test("new"),
            |app, [_, pb], [a, b]| ![a, b].contains(&app.pane_session(pb).id),
        ),
        (
            ":vsplit",
            |app, _| _ = app.dispatch_command_for_test("vsplit"),
            |app, _, _| app.workspace.panes_of(PaneKind::Output).count() == 3,
        ),
        (
            ":tabnew",
            |app, _| _ = app.dispatch_command_for_test("tabnew"),
            |app, _, _| (app.tabs.count(), app.tabs.current()) == (2, 1),
        ),
        (
            ":tabnew, then back",
            |app, _| {
                app.dispatch_command_for_test("tabnew");
                app.dispatch_command_for_test("tabnext 1");
            },
            |app, [_, pb], _| app.tabs.current() == 0 && app.workspace.focused() == pb,
        ),
        (
            ":tab help",
            |app, _| _ = app.dispatch_command_for_test("tab help"),
            |app, _, _| (app.tabs.count(), app.tabs.current()) == (2, 1),
        ),
        (
            "<C-w>T",
            |app, _| _ = app.window_command_for_test(WindowCmd::MoveToTab),
            |app, _, [_, b]| {
                (app.tabs.count(), app.tabs.current()) == (2, 1)
                    && app.pane_session(app.workspace.focused()).id == b
            },
        ),
        (
            ":close",
            |app, _| _ = app.dispatch_command_for_test("close"),
            |app, [_, pb], _| app.workspace.window(pb).is_none(),
        ),
        (
            ":q",
            |app, _| _ = app.dispatch_command_for_test("q"),
            |app, [_, pb], _| app.workspace.window(pb).is_none(),
        ),
        (
            ":hide",
            |app, _| _ = app.dispatch_command_for_test("hide"),
            |app, [_, pb], _| app.workspace.window(pb).is_none(),
        ),
    ];
    for (what, act, done) in cases {
        let (mut app, panes, sessions) = a_asks_beside_b();
        app.notices.clear();
        act(&mut app, sessions);
        app.settle();
        let said: Vec<&str> = app.notices.iter().map(|(_, s)| s.as_str()).collect();
        assert!(done(&app, panes, sessions), "{what}: done — {said:?}");
        assert!(
            !said
                .iter()
                .any(|s| s.contains("not closed") || s.contains("waits in")),
            "{what}: nothing refused over A's: {said:?}"
        );
        a_still_asks_under(&app, sessions[0], panes[0], what);
    }

    // The one close over it: `:only` from B would take A's window.
    let (mut app, [pa, _], [a, _]) = a_asks_beside_b();
    let before = app.workspace.pane_ids();
    app.dispatch_command_for_test("only");
    assert_eq!(app.workspace.pane_ids(), before, "`:only`: refused");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains(&format!("b{}'s request waits in its pane", a.number())),
        "`:only`: naming whose: {said}"
    );
    a_still_asks_under(&app, a, pa, "`:only`");

    // …and `<C-w>o`, the key, is the same `:only`: refused alike, never
    // forced. No test pressed it, and making it force passed (review stop
    // 12, T5 F2).
    let (mut app, [pa, _], [a, _]) = a_asks_beside_b();
    let before = app.workspace.pane_ids();
    let key = |code, mods| KeyEvent::new(code, mods);
    app.key_for_test(key(KeyCode::Char('w'), KeyModifiers::CONTROL), (120, 40));
    app.key_for_test(key(KeyCode::Char('o'), KeyModifiers::NONE), (120, 40));
    assert_eq!(app.workspace.pane_ids(), before, "`<C-w>o`: refused");
    a_still_asks_under(&app, a, pa, "`<C-w>o`");
}

/// **`:b N` in the chat a request sits under is not refused** (Steven, 10-26,
/// Q3, the lean kept: vim's `:b` refuses to leave a modified buffer, but
/// nothing here is lost) — A's call waits, in no pane while no window shows
/// A, unanswered; CTRL-^ brings A back to the window, and its pane with it.
/// B's window beside is untouched throughout.
#[tokio::test]
async fn b_in_a_requests_chat_takes_its_pane_and_ctrl_caret_brings_it_back() {
    let (mut app, [pa, pb], [a, b]) = a_asks_beside_b();
    let c = {
        app.focus_pane(pa);
        app.dispatch_command_for_test("new");
        app.pane_session(pa).id
    };
    assert!(
        ![a, b].contains(&c),
        "premise: a third session, C, in A's window"
    );
    app.window_command_for_test(WindowCmd::Alternate);
    app.settle();
    assert_eq!(app.pane_session(pa).id, a, "premise: A back");
    assert_eq!(host_of(&app, a), pa, "premise: its pane back under it");

    app.notices.clear();
    app.dispatch_command_for_test(&format!("b {}", c.number()));
    let said = app.notice().unwrap_or_default().to_string();
    assert_eq!(app.pane_session(pa).id, c, "`:b` not refused: {said}");
    assert_eq!(app.decision_pane_of(a), None, "A's pane gone with A");
    assert!(app.shown_call_in(a).is_some(), "A's call kept");
    assert_eq!(result_in(&app, a, "tu_a"), None, "not answered");
    assert_eq!(app.pane_session(pb).id, b, "B's window untouched");

    app.window_command_for_test(WindowCmd::Alternate);
    app.settle();
    assert_eq!(app.pane_session(pa).id, a, "CTRL-^: A back");
    assert_eq!(host_of(&app, a), pa, "and its pane with it");
    assert_eq!(result_in(&app, a, "tu_a"), None, "still unanswered");
    assert_eq!(app.pane_session(pb).id, b, "B's window untouched");
}

/// A deferred generation bills **the session it was started from**.
///
/// `PendingSummary` and `PendingContinuation` each carry a `buffer` whose doc
/// says it exists because the foreground may have moved by the time they
/// resolve. The *compose* used it; all five *billing* sites read
/// `buffer_mut()` — whatever session was in front. Three reviewers found it
/// independently, and gating `buffer_mut` as test-only turned the class into
/// five compile errors.
#[tokio::test]
async fn a_distillation_bills_the_session_it_was_started_from() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a_buffer = app.current;
    let spent = shannon_core::Usage {
        input_tokens: 100,
        output_tokens: 50,
        ..Default::default()
    };

    // A distillation in flight, started from A. Constructed directly: what is
    // under test is where the tokens land when it resolves, not how `S` picks
    // its sources.
    let turn = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: app.conversation().episode().id,
        turn,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    app.fake_summary_on(a_buffer, target, vec![turn]);

    // Walk away to a fresh session while it runs.
    app.new_session_command();
    let b_buffer = app.current;
    assert_ne!(a_buffer, b_buffer, "we really moved");

    app.feed_summary(StreamMsg::Usage(spent));
    app.feed_summary(StreamMsg::Error(BackendError::RateLimited));

    assert_eq!(
        app.session_of(a_buffer).expect("A is open").billing.total,
        Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        },
        "A asked for it, A pays"
    );
    assert_eq!(
        app.session_of(b_buffer).expect("B is open").billing.total,
        Usage::default(),
        "B was merely where you happened to be standing"
    );
}

/// Closing the pane you were typing to must not leave the prompt aimed at a
/// session **nothing on screen shows**.
///
/// Both drawn channels go dark together in that state: no pane can wear the
/// accent (none shows the target) and `prompt_title` falls back to the bare
/// `" prompt "` once one transcript remains. The user sees the unambiguous
/// single-session picture while the next message goes somewhere else. Found by
/// two reviewers; the fix reconciles the aim through the same door every other
/// pane-bound resource does.
#[tokio::test]
async fn closing_the_pane_you_were_typing_to_re_aims_the_prompt_and_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.set_focus(PaneKind::Output);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.split_command("output", SplitDir::Vertical);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    assert_eq!(app.prompt_target(), b_buffer, "aimed at the pane we are in");

    app.close_pane_command(Asked::Plain); // close it out from under the aim

    assert_ne!(
        app.prompt_target(),
        b_buffer,
        "the aim did not survive its pane"
    );
    assert_eq!(
        app.prompt_target(),
        a_buffer,
        "it moved to what IS on screen"
    );
    let notice = app
        .notice()
        .expect("a re-aim nobody asked for must be announced");
    assert!(notice.contains("aiming at"), "and says where: {notice}");
}

/// Answering a decision **after** a session switch resolves the turn.
///
/// `finish_tool_call` asked `current` about a round owned by the generation's
/// buffer, so the answer read as "a call this turn never made", was dropped
/// with a `warn!`, and the generation sat in `AwaitingDecision` forever — every
/// later prompt queuing in silence. The fix landed with no test; a reviewer
/// mutation-verified that `turn_buffer()` could be replaced by `self.current`
/// with the whole suite still green.
#[tokio::test]
async fn answering_a_decision_after_a_session_switch_does_not_wedge_the_slot() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.open_by_hash(&a.to_hex()[..8]);

    model_call(&mut app, "tu-1", "rm -rf /tmp/shannon-test-absent-wedge");
    assert!(app.awaiting_decision(), "A's turn is parked on a decision");

    // Walk away, then answer it.
    app.open_by_hash(&b.to_hex()[..8]);
    tool_key(&mut app, 'd'); // deny

    assert!(
        !app.awaiting_decision(),
        "the round resolved — the answer was recognised as this turn's"
    );
    assert!(
        app.shown_call().is_none(),
        "and the decision is off the screen"
    );
}

// ---------------------------------------------------------------------------
// Ownership: the bugs a full review round found in the ownership work itself.
// Every one of these passed the suite before its fix — the guards were real
// and nothing held them ([[feedback_mutation_check_guards]]).
// ---------------------------------------------------------------------------

/// A queued prompt must not start a turn on a buffer a **child job** is
/// streaming into.
///
/// `drain_next` re-checked "is the slot free" via `turn_in_flight()`, which
/// asks about the *prompt's target*. Since P4, a queued prompt carries its own
/// buffer, which may be a different one — so the check could not see the job it
/// was about to collide with. `begin_turn` on a conversation that already has
/// one asserts in debug and **silently destroys the child's draft in release**,
/// with two backend requests writing one conversation.
///
/// Found independently by two reviewers; the doc on `drain_next` named this
/// exact hazard and then performed the check that could not see it.
#[tokio::test]
async fn a_queued_prompt_does_not_double_book_a_buffer_with_a_running_job() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    let child_buffer = app
        .buffers
        .sessions()
        .find(|b| b.conversation.episode().id == child)
        .expect("the child has a buffer")
        .id;

    // A foreground turn on the main session, and a prompt queued FOR THE CHILD.
    app.submit("first");
    assert!(app.streaming(), "the foreground is busy");
    app.pending
        .push_back((child_buffer, "for the child".into()));

    // The foreground finishes. Draining must not start a turn on the child.
    app.feed_stream(StreamMsg::Complete);

    assert_eq!(
        app.pending.len(),
        1,
        "still queued — the child's slot is not free, so it waits rather than \
         colliding (and is not dropped either: they are the user's words)"
    );
    assert!(
        app.session_of(child_buffer)
            .expect("child buffer")
            .conversation
            .turn_in_progress(),
        "the child's own turn is intact"
    );
}

/// A compose landing on an **idle** buffer must not seal a live turn belonging
/// to a different one.
///
/// `take_foreground_on`'s buffer comparison is the whole reason a generation
/// carries a buffer, and its doc says so at length. Deleting the comparison
/// left all 734 tests green — two reviewers mutation-verified that
/// independently.
#[tokio::test]
async fn composing_into_an_idle_buffer_leaves_another_buffers_turn_alone() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    let b_buffer = app.current;
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;

    app.submit("a live turn on A");
    let generation = app.turns().next().expect("A is streaming").id;

    // Replace B's contents — a composition landing somewhere else entirely.
    let (episode, turns) = load_full_session(app.store.as_ref(), &b).expect("B loads");
    let recall = app.unlock_turns(&turns).expect("unprotected");
    app.adopt_session(
        b_buffer,
        episode,
        turns,
        OnDisk::Absent,
        recall,
        Landing::Asked,
    );

    let still = app.turns().next().expect("A's turn survives");
    assert_eq!(still.id, generation, "the same generation");
    assert_eq!(still.buffer, a_buffer, "still A's");
}

/// Discarding one session's turn must not tear down **another** session's
/// decision.
///
/// `discard_foreground_on` cleared the tool round and the stream tail outside
/// its `if let`, so `:delete` on a session that owned no turn still wiped
/// whatever decision was on screen — dropping the other buffer's generation via
/// `abandon_tool_round` and leaving its conversation with a turn in progress
/// that could never be answered, sealed or resumed. Its sibling
/// `seal_foreground_on` had the scoping right all along.
#[tokio::test]
async fn discarding_one_sessions_turn_leaves_another_sessions_decision_alone() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    add_persisted_session(&mut app, "B0");
    let b_buffer = app.current;

    // B has a turn paused on a decision.
    model_call(&mut app, "tu-1", "rm -rf /tmp/shannon-test-absent-discard");
    assert!(app.shown_call().is_some(), "a decision is up on B");
    assert_eq!(app.turns().next().map(|g| g.buffer), Some(b_buffer));

    // Now discard A, which owns no generation at all.
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.end_work_on(a_buffer, Outcome::Deleted);

    assert!(
        app.shown_call().is_some(),
        "B's decision is still on screen and still answerable"
    );
    assert_eq!(
        app.turns().next().map(|g| g.buffer),
        Some(b_buffer),
        "and B's turn still exists to answer it into"
    );
}

/// **Every** focus mover re-aims the prompt, not just the ones the P4 tests
/// happened to drive.
///
/// `focus_moved` is the one door and three of the six movers bypassed it —
/// `<C-w>h/j/k/l`, `{count}<C-w>w`, and `<C-w>p` set `chat_dirty` by hand. So
/// P4's headline gesture was silently absent from the primary navigation keys,
/// while every P4 test drove `focus_pane` (the mouse path), which worked. The
/// comment on `focus_moved` asserted a count and was wrong about it.
///
/// Stated as the invariant rather than as a route, so it does not depend on
/// which pane a given layout puts above another: **after any focus move, if
/// the focused pane is a transcript, that is what the prompt commits to.**
///
/// **Every window command, with and without a count, settled as the loop
/// settles an event** (`settle_focus` re-aims at its end, where the guarantee
/// lives now). It drove four of the movers by direct call, unsettled — and
/// left out `{count}<C-w>w`, which the paragraph above names, `<C-w>h`/`l`
/// and `<C-w>W`; and a split, a close, a tab or `<C-^>` moves focus or what
/// it shows too (review stop 12, T2 6). The list is every variant but
/// `Quit`, held to the type by an exhaustive match.
#[tokio::test]
async fn every_focus_mover_re_aims_the_prompt() {
    use Direction::{Down, Left, Right, Up};
    use WindowCmd::*;
    let every = [
        Focus(Up),
        Focus(Down),
        Focus(Left),
        Focus(Right),
        FocusCycle,
        FocusCycleBack,
        FocusPrev,
        ToggleMark,
        ClearMarks,
        ResizeGrow,
        ResizeShrink,
        ResizeWider,
        ResizeNarrower,
        Equalize,
        Relocate(Up),
        Relocate(Left),
        Split,
        VSplit,
        Close,
        Only,
        Alternate,
        FollowLink,
        LinkBack,
        TabNext,
        TabPrevious,
        MoveToTab,
    ];
    // A variant added later fails to compile here until it is placed: in
    // the list above, or left out with `Quit`, which would end the program.
    for cmd in every {
        match cmd {
            Focus(_) | FocusCycle | FocusCycleBack | FocusPrev | ToggleMark | ClearMarks
            | ResizeGrow | ResizeShrink | ResizeWider | ResizeNarrower | Equalize | Relocate(_)
            | Split | VSplit | Close | Only | Alternate | FollowLink | LinkBack | TabNext
            | TabPrevious | MoveToTab => {}
            Quit => unreachable!("not walked: it ends the program"),
        }
    }
    let mut landed = std::collections::HashSet::new();
    for cmd in every {
        // Each count its own window for `{count}<C-w>w`: one count lands on
        // one pane every step, and that pane may be the one aimed at.
        for count in [None, Some(1), Some(2), Some(3)] {
            let dir = TempDir::new().unwrap();
            let mut app = app_with_n_turns(dir.path(), 1);
            let a = app.conversation().episode().id;
            let b = add_persisted_session(&mut app, "B0");
            app.set_focus(PaneKind::Output);
            app.open_by_hash(&a.to_hex()[..8]);
            app.split_command("output", SplitDir::Horizontal);
            app.open_by_hash(&b.to_hex()[..8]);
            app.split_command("output", SplitDir::Vertical);
            app.settle();

            // Walk with this gesture and check the invariant at every landing.
            for _ in 0..6 {
                let quits = app.apply_window(cmd, count, 120, 40);
                assert!(!quits, "{cmd:?} {count:?}: only `<C-w>q` ends the program");
                app.settle();
                let focused = app.workspace.focused();
                if app.workspace.kind_of(focused) == Some(PaneKind::Output) {
                    let live = app.live_session(focused);
                    landed.extend(live);
                    assert_eq!(
                        live,
                        Some(app.prompt_target()),
                        "{cmd:?} {count:?}: standing in a transcript must aim the prompt at it"
                    );
                }
            }
        }
    }
    assert!(
        landed.len() >= 2,
        "premise: the walks stood in both sessions' transcripts: {landed:?}"
    );
}

/// **A running child is watchable.** Point a pane at it and the reply arrives
/// there, live — the difference between a child being a number on the status
/// row and being a session you can read while it works ("agents as channels",
/// arriving through the buffer model rather than as its own mechanism).
#[tokio::test]
async fn a_running_child_can_be_opened_in_a_pane_and_streams_there() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let parent = app.conversation().episode().id;
    let child = app.spawn_child_job("say hi", None).expect("spawned");

    // Open the child WHILE it runs. It is not on disk as turns yet — only its
    // header is — so this can only work by finding the job's live buffer.
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.notices.clear(); // clear the spawn notice so a refusal would stand out
    app.open_by_hash(&child.to_hex()[..8]);
    assert!(app.notice().is_none(), "opened cleanly: {:?}", app.notice());
    assert_eq!(app.pane_session(pane).conversation.episode().id, child);

    let shown = |app: &App, pane| app.pane_session(pane).output.lines().join("\n");
    assert!(
        shown(&app, pane).contains("say hi"),
        "the briefing is there"
    );

    app.feed_job(child, StreamMsg::Chunk("working".into()));
    assert!(
        shown(&app, pane).contains("working"),
        "the reply streams into the open pane: {}",
        shown(&app, pane)
    );

    app.feed_job(child, StreamMsg::Complete);
    assert_eq!(app.running_children(), 0);
    assert_eq!(
        app.conversation().episode().id,
        child,
        "the prompt followed the pane we opened it in"
    );
    // The parent is still open in its own buffer, untouched by any of this.
    assert!(
        app.buffers
            .sessions()
            .any(|b| b.conversation.episode().id == parent),
        "the parent buffer survives"
    );
}

/// One backend slot, now that the prompt can be aimed at a running child: a
/// submit into a session a child is mid-turn on **queues**, and starts when the
/// child resolves. Both halves matter — queuing with nothing to drain it would
/// be a prompt that silently never sends.
#[tokio::test]
async fn a_prompt_aimed_at_a_running_child_queues_and_then_drains() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("say hi", None).expect("spawned");
    app.set_focus(PaneKind::Output);
    app.open_by_hash(&child.to_hex()[..8]); // foreground follows the pane

    app.submit("and then this");
    assert_eq!(app.pending.len(), 1, "queued behind the child's turn");
    let notice = app.notice().expect("says why");
    assert!(
        notice.contains("child"),
        "names what holds the slot: {notice}"
    );

    app.feed_job(child, StreamMsg::Complete);
    assert!(app.pending.is_empty(), "the queued prompt started");
    assert!(app.streaming(), "…as a foreground turn");
}

/// A child running on a **different** session must not block the foreground —
/// that is the entire point of a background child. The guard above narrows to
/// the foreground buffer for exactly this reason.
#[tokio::test]
async fn a_child_on_another_session_does_not_block_the_foreground() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.spawn_child_job("say hi", None).expect("spawned");

    app.submit("meanwhile, here");
    assert!(app.pending.is_empty(), "not queued — different session");
    assert!(app.streaming(), "started straight away");
}

/// **A turn on each session at once** — the sibling of the test above, and
/// now the same rule: one open turn per SESSION, no limit across sessions
/// (`buffers.md` §9, the flip; Steven, 10-20: "Sessions are independent…
/// We can't run multiple agents if that's true. So each agent is running
/// independently").
///
/// This was `only_one_foreground_turn_runs_at_a_time_across_sessions`, which
/// pinned §4's one foreground turn: a prompt typed at a second session while
/// the first answered QUEUED behind it, and the `:vfork` trial (10-15) ran
/// two agents' tasks one after the other. Inverted, as §9 said it would be.
/// A plain prompt is still a turn, never silently a job — that is `:Send`.
#[tokio::test]
async fn a_turn_runs_on_each_session_at_once() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());

    // A turn in flight on A, in the focused pane.
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    app.submit("ask A");
    assert!(app.streaming(), "premise: A is answering");
    let a_buffer = app.current;

    // Walk to the pane showing B — the aim follows focus — and submit there.
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.prompt_target();
    assert_ne!(a_buffer, b_buffer, "premise: a different session");

    app.submit("ask B");
    assert!(app.pending.is_empty(), "B's prompt started, not queued");
    assert_eq!(
        app.job_episodes().len(),
        0,
        "a plain prompt is never silently converted into a job — that is `:Send`"
    );
    let mut turns: Vec<BufferId> = app.turns().map(|g| g.buffer).collect();
    turns.sort();
    let mut want = vec![a_buffer, b_buffer];
    want.sort();
    assert_eq!(turns, want, "a turn on A and a turn on B");
}

/// A message from a generation that is over is **dropped by identity** — even
/// when a *later* generation is running to receive it.
///
/// `handle.abort()` is asynchronous and does not empty the channel, so a
/// cancelled task's queued messages still arrive. `Conversation` no-ops without
/// an open turn, but `Output::append_chunk` OPENS a block if none is open — so
/// one session's text appeared in another's transcript. The fix used to be
/// throwing the whole channel away and building a new one, which worked and
/// hid the question. The message carries its generation now, so the question
/// is answerable: this one is not ours, drop it.
///
/// **A later generation has to exist for this to measure anything.** The first
/// draft delivered the stale message with the slot empty, where "there is no
/// foreground" rejects it just as well — so removing the id comparison
/// entirely left the test passing.
#[tokio::test]
async fn a_message_from_a_finished_generation_is_dropped() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.open_by_hash(&a.to_hex()[..8]);

    app.submit("ask A something");
    let stale = app.turns().next().expect("a turn is in flight").id;
    app.feed_stream(StreamMsg::Complete); // that generation is over

    // A different session, and a live turn of its own to be confused with.
    app.open_by_hash(&b.to_hex()[..8]);
    app.submit("ask B something");
    let live = app.turns().next().expect("B has a turn").id;
    assert_ne!(stale, live, "two generations, two ids");

    // Exactly what the aborted task's queued `Chunk` looks like on arrival.
    app.dispatch_stream_msg(stale, StreamMsg::Chunk("A's late words".into()));

    let shown = app.output().lines().join("\n");
    assert!(
        !shown.contains("A's late words"),
        "B's live turn did not absorb A's leftovers: {shown}"
    );
}

/// A turn writes into **its own** session, not into whichever one is in front
/// when the chunk lands.
///
/// The other half of the same guarantee. Dropping stale messages is not enough
/// on its own: a *live* generation's chunks must still find their way home
/// after the user has moved on, or switching sessions mid-turn silently
/// truncates the reply you walked away from.
#[tokio::test]
async fn a_live_turn_keeps_writing_into_the_session_it_started_in() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;

    app.submit("ask A something");
    app.open_by_hash(&b.to_hex()[..8]); // look away mid-turn
    app.feed_stream(StreamMsg::Chunk("A's reply".into()));

    let b_shown = app.output().lines().join("\n");
    assert!(
        !b_shown.contains("A's reply"),
        "B's transcript is clean: {b_shown}"
    );
    let a_shown = app
        .session_of(a_buffer)
        .expect("A stays open")
        .output
        .lines()
        .join("\n");
    assert!(
        a_shown.contains("A's reply"),
        "and A got its own reply: {a_shown}"
    );
}

/// Queued prompts are the user's words: they go to the session they were
/// **typed for**, not to wherever you happen to be when the slot frees up.
///
/// They used to be bare text drained into `current`, so the queue had to be
/// thrown away whenever the foreground moved — losing the user's words in
/// order to avoid delivering them to a stranger. Both halves were wrong, and
/// they were the same missing field.
#[tokio::test]
async fn a_queued_prompt_goes_to_the_session_it_was_typed_for() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;

    app.submit("first");
    app.submit("second — queued behind it");
    assert_eq!(app.pending.len(), 1, "queued for A");

    app.open_by_hash(&b.to_hex()[..8]); // move away while it waits
    assert_eq!(app.pending.len(), 1, "still the user's, still queued");

    app.feed_stream(StreamMsg::Complete); // A's first turn finishes → drain

    let b_shown = app.output().lines().join("\n");
    assert!(
        !b_shown.contains("second"),
        "B never asked for it: {b_shown}"
    );
    let a_shown = app
        .session_of(a_buffer)
        .expect("A stays open")
        .output
        .lines()
        .join("\n");
    assert!(
        a_shown.contains("second"),
        "it continued the conversation it was written in: {a_shown}"
    );
}

/// An open tool round must not outlive its conversation. `:new` abandoned it
/// and `adopt_session` did not, so `:graph` surgery during a pending decision
/// left `tool_turn_open` set against a conversation that no longer existed —
/// `turn_in_flight()` true forever, every later prompt queuing in silence.
#[tokio::test]
async fn adopting_a_session_abandons_an_open_tool_round() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.park_on_decision();
    // Queued WITH an owner, which is how one really gets there: `gate_tool_call`
    // records who asked before it queues. An ownerless entry is a state the
    // system cannot produce, and building one here made this test blind to the
    // teardown being scoped by owner.
    let g = app.turns().next().expect("parked").id;
    app.track_tool_call("id-1", Some(g));
    app.queue_call_for_test(app.current, "id-1", "ls", false);

    // A composition lands OVER the buffer, replacing its session — the
    // defensive ending, which no command reaches since a clone took a buffer
    // of its own beside (10-21), as a checkout did before it (D3).
    app.supersede_for_test(app.current);

    assert!(!app.awaiting_decision(), "the round was abandoned");
    assert!(app.waiting_calls().is_empty());
    assert!(
        app.turn_in_flight().is_none(),
        "the slot is free — otherwise every later prompt queues in silence"
    );
}

/// A child's turns are filed under the child. The `Job` holds a `BufferId`, and
/// a buffer can be repointed underneath it (open the child, then check out from
/// that pane) — writing "the buffer's turns" into "the job's episode" would
/// file one session's history inside another's log.
///
/// The buffer must be repointed at a session that **has turns**. The first
/// version used `reset_foreground()`, which leaves an EMPTY conversation, so
/// `own_turns().skip(0)` was empty with or without the guard and the
/// "nothing filed" assertion held either way — it proved nothing about the
/// misfiling it is named for.
#[tokio::test]
async fn a_repointed_child_buffer_does_not_write_into_the_childs_log() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let other = app.conversation().episode().id; // a session WITH turns
    let child = app.spawn_child_job("say hi", None).expect("spawned");
    app.set_focus(PaneKind::Output);
    app.open_by_hash(&child.to_hex()[..8]); // a pane (and the foreground) on the child

    // Repoint THAT buffer — `:open` would merely find the other session's own
    // buffer and leave the child's alone. `new_session` replaces the current
    // buffer's conversation in place, which is the shape every `:graph` surgery
    // takes; then give it a turn, because turns are what would be misfiled.
    let _ = other;
    app.reset_foreground();
    app.conversation_mut().begin_turn("a turn of my own");
    app.conversation_mut().push_chunk("reply");
    app.conversation_mut().commit_turn();
    assert!(
        !app.conversation().episode().own_turns().is_empty(),
        "the child's buffer now holds turns that could be misfiled under it"
    );

    // The job is ABANDONED when its buffer is reset, so its messages never
    // arrive at all. This used to let the job run on and be caught later, at
    // persist time, by an episode-identity check — which stopped the disk write
    // but not the SCREEN write: chunks landed in the replacement session's
    // transcript. Cutting it at the reset is the stronger guarantee, and it is
    // why this test no longer expects a "NOT saved" notice: there is nothing
    // left to save or to refuse (Reviewers A and B).
    assert!(
        app.job_episodes().is_empty(),
        "resetting the buffer abandoned the job writing into it"
    );
    let before = app.output().lines().join("\n");
    app.feed_job(child, StreamMsg::Chunk("hi".into()));
    app.feed_job(child, StreamMsg::Complete);

    assert_eq!(
        app.output().lines().join("\n"),
        before,
        "a late chunk from the abandoned job did not reach the transcript"
    );
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let filed = g.episode(&child).map(|e| e.own_turns().len()).unwrap_or(0);
    assert_eq!(
        filed, 0,
        "and the other session's turns were NOT filed under the child"
    );
}

/// `:graph` and every id lookup must see a lazy session living in ANOTHER pane.
/// A clone is not on disk until continued, so a background buffer holding one
/// made it invisible to `:graph`, `:open`, `:delete` and `<Tab>` completion —
/// while that pane rendered it happily. `has_dependents` was widened for this
/// reason and the snapshot was not.
#[test]
fn the_graph_snapshot_sees_a_lazy_session_in_another_pane() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.clone_episode(original);
    let clone = app.conversation().episode().id;

    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Horizontal);
    app.open_by_hash(&original.to_hex()[..8]); // foreground → the original

    assert!(
        app.current_graph().episode_ids().contains(&clone),
        "the clone is still reachable from the graph"
    );
}

/// **A turn paused on a decision SURVIVES you leaving it** — on its own
/// session, with its own tokens.
///
/// The inverse of what this test asserted before, and deliberately so. Sealing
/// on a session switch was never the goal; it was the cheapest way to stop an
/// untagged stream from writing one session's reply into another's transcript.
/// A generation carries its buffer now, so leaving is an ordinary thing to do:
/// go and read something else, come back, answer the prompt.
///
/// Two guarantees are asserted together because they failed together. The
/// draft stays open **on A** (it used to be sealed, or — in the round-two
/// version — sealed from the wrong question), and A's tokens are billed to
/// **A** (`turn_usage` lived on `App`, so a tally could outlive its turn and
/// land on whatever session you had moved to).
#[tokio::test]
async fn a_turn_paused_on_a_decision_survives_a_session_switch() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;

    // A turn that streamed, asked for a tool, and is now waiting on a human.
    app.submit("do a thing");
    app.feed_stream(StreamMsg::Chunk("working".into()));
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 111,
        output_tokens: 222,
        ..Default::default()
    }));
    app.park_on_decision(); // what `Complete` does before the gate is answered
    assert!(app.conversation().turn_in_progress(), "a draft is open");

    app.open_by_hash(&b.to_hex()[..8]);

    let left = app.session_of(a_buffer).expect("A stays open");
    assert!(
        left.conversation.turn_in_progress(),
        "the turn is still A's, still open, still answerable"
    );
    assert_eq!(
        app.buffer().billing.total,
        Usage::default(),
        "B was not billed for a turn it never ran"
    );

    // And it finishes into A, not into whatever is in front.
    app.feed_stream(StreamMsg::Complete);
    let a_buf = app.session_of(a_buffer).expect("A stays open");
    assert!(
        !a_buf.conversation.turn_in_progress(),
        "the turn completed where it started"
    );
    assert_eq!(
        a_buf.billing.total,
        Usage {
            input_tokens: 111,
            output_tokens: 222,
            ..Default::default()
        },
        "and its tokens were billed to A"
    );
    assert_eq!(
        app.buffer().billing.total,
        Usage::default(),
        "B is still untouched"
    );
}

/// A tool call under a window a viewer has borrowed is answerable there.
///
/// Reachable in one step: `:help` takes over the only transcript pane, `:only`
/// closes the rest, and `fit_tool_pane` bailed silently when nothing showed
/// `Output`. `gate_tool_call` had already set `tool_prompt`, so the decision
/// existed with nowhere to answer it and `tool_turn_open` wedged the slot
/// forever — every later prompt queuing in silence.
///
/// The manual BORROWS the pane, so the conversation's window is still there
/// (`session_panes`), and the decision opens under it. A session in NO
/// window waits instead, said on the row, and `:decide` brings it
/// (`parallel::a_decision_in_no_window_waits_and_decide_brings_it`).
#[tokio::test]
async fn a_decision_under_a_borrowed_window_is_answered_there() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_help_command(None); // the only transcript pane now shows help
    app.only_command(false);
    assert!(!app.workspace.has(PaneKind::Output), "no transcript pane");

    // A command that actually gates. `ls -la` is read-only and auto-approves,
    // so it never creates a decision — the first version of this test passed
    // whether or not the pane was found, for that reason alone. A model's
    // call, in its turn: gated with no turn, then parked on a turn made for
    // it, the call answered could never resume a turn it was not in.
    model_call(&mut app, "tu-1", "rm -rf /tmp/shannon-test-absent-decision");
    assert!(app.awaiting_decision(), "premise: the turn waits on it");

    let pane = app.decision_pane_of(app.shown_buffer());
    assert!(pane.is_some(), "answerable, under the borrowed window");
    tool_key(&mut app, 'd');
    assert!(app.shown_call().is_none(), "and answered there");
    assert!(!app.awaiting_decision(), "the turn released");
}

/// **Leaving a session you are WATCHING must not destroy the child's turn.**
///
/// Round two changed `seal_foreground_turn` from "was a stream in flight" to
/// "does the conversation have a turn in progress". Both are the wrong
/// question, and the second is worse: a `Job` begins a turn on ITS buffer, so
/// after the documented workflow — `:child`, then `:open <child>` to watch it —
/// the child's turn is "in progress" on the foreground, and the next `:open`
/// seals it out from under the running job. Before the first chunk that
/// truncates the draft and DELETES THE BRIEFING; the job then streams into a
/// conversation with no draft, `Complete` commits nothing, and it is announced
/// as "finished" over an episode whose log is empty.
#[tokio::test]
async fn leaving_a_watched_child_does_not_seal_its_running_turn() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let other = app.conversation().episode().id;
    let child = app
        .spawn_child_job("compute the answer", None)
        .expect("spawned");
    app.set_focus(PaneKind::Output);
    app.open_by_hash(&child.to_hex()[..8]); // watch it; foreground = the child

    app.open_by_hash(&other.to_hex()[..8]); // and look away again

    let childs = app
        .buffers
        .sessions()
        .find(|b| b.conversation.episode().id == child)
        .expect("the child's buffer survives");
    assert!(
        childs.conversation.turn_in_progress(),
        "the running child's draft is untouched — the job still owns it"
    );

    // And it still completes into its own log, rather than reporting success
    // over an empty episode.
    app.feed_job(child, StreamMsg::Chunk("42".into()));
    app.feed_job(child, StreamMsg::Complete);
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    assert_eq!(
        g.episode(&child).map(|e| e.own_turns().len()).unwrap_or(0),
        1,
        "the child's work is on disk"
    );
}

/// **A distillation lands in the session it was started from**, even if you
/// switch away while it streams.
///
/// `S` streams a summary with the UI live, and `adopt_session` wrote into
/// `buffer_mut()` — the foreground — so `:open <other>` before it resolved
/// replaced THAT session's conversation, output, persist cursor and usage with
/// something composed from a graph it had nothing to do with. `Job` was given a
/// `BufferId` for exactly this reason; these were the two generations it did
/// not reach.
#[tokio::test]
async fn a_distillation_composes_into_the_session_it_started_from() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.open_by_hash(&a.to_hex()[..8]); // foreground = A

    // A distillation of A's turns, pending.
    let turns = app.conversation().episode().own_turns();
    let graph = app.current_graph();
    let target = SquashTarget::Range {
        episode: a,
        start: 0,
        end: 2,
        fork_turn: turns[0],
        config: app.conversation().episode().config.clone(),
    };
    let a_buffer = app.current;

    // Switch away, then commit it — the shape the summary stream arm produces.
    app.open_by_hash(&b.to_hex()[..8]);
    assert_ne!(app.current, a_buffer, "the foreground moved");
    let b_buffer = app.current;
    let b_before = app.conversation().episode().id;

    let text = to_editable(&[
        Message::user("distilled question"),
        Message::assistant("distilled answer"),
    ]);
    // The sources the range holds — `turns[..2]`, not all three: passing all
    // three was a state `S` cannot produce, and the compose now checks it.
    let composed_id = app.commit_summary_into(a_buffer, graph, target, &turns[..2], &text, None);
    assert!(
        composed_id.is_some(),
        "the compose itself must succeed, or this proves nothing: {:?}",
        app.notice()
    );

    // **B's buffer, addressed as B** — not "whatever is in the foreground".
    // Those were the same thing only because `reveal` used to no-op whenever
    // focus sat anywhere but a session pane, so this asserted a SUPERSET of
    // its subject and would have passed for a version that left the
    // composition invisible ([[feedback_assert_the_subject]]). The subject is
    // that the session I merely switched to was not written into.
    assert_eq!(
        app.session_of(b_buffer)
            .expect("still open")
            .conversation
            .episode()
            .id,
        b_before,
        "B — the session I merely switched to — is untouched"
    );
    let composed = app
        .session_of(a_buffer)
        .expect("A's buffer is still open")
        .conversation
        .episode()
        .id;
    assert_ne!(composed, a, "and A's buffer holds the composed result");
}

/// **One `:allow` that runs two sessions' calls names both** (review stop 8,
/// D 6): one call shown per session since A1.3, and the rebuild said the
/// first change alone — "it covers the open call, which runs" — the other
/// running unnamed.
#[tokio::test]
async fn an_allow_that_runs_two_sessions_calls_names_both() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("b1's first turn");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    // Inside where Shannon started, so the rule covers them; nothing runs
    // in a test (`no_real_execution`).
    manual_tool(&mut app, "mkdir -p shannon-test-allow-a");
    app.new_session_command();
    manual_tool(&mut app, "mkdir -p shannon-test-allow-b");
    assert_eq!(app.decisions.every_shown().count(), 2, "premise: one each");
    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    app.dispatch_command_for_test("allow mkdir:*");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("mkdir -p shannon-test-allow-a")
            && said.contains("mkdir -p shannon-test-allow-b"),
        "{said}"
    );
    assert_eq!(app.decisions.every_shown().count(), 0, "both ran");
    // The one off the aim is named by its session (review stop 8, E1 4: the
    // naming was held by no test).
    let off_aim = app
        .name_off_the_aim(BufferId(0))
        .expect("premise: b1 is off the aim");
    assert!(said.contains(&off_aim), "whose: {said}");
}

/// **One `:allow` that runs one of several sessions' calls names it, and
/// whose** (review stop 8, E1 4 / E2 3) — "it covers the open call, which
/// runs" was said with the call in front of you still asking, the one that
/// ran in another session, unnamed.
#[tokio::test]
async fn an_allow_that_runs_another_sessions_call_names_it() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("b1's first turn");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    manual_tool(&mut app, "mkdir -p shannon-test-allow-a");
    app.new_session_command();
    manual_tool(&mut app, "rm /tmp/shannon-test-x");
    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    app.dispatch_command_for_test("allow mkdir:*");
    let said = app.notice().unwrap_or_default().to_string();
    let off_aim = app
        .name_off_the_aim(BufferId(0))
        .expect("premise: b1 is off the aim");
    assert!(
        said.contains("`mkdir -p shannon-test-allow-a`") && said.contains(&off_aim),
        "{said}"
    );
    assert!(app.shown_call().is_some(), "premise: yours still asks");
}

/// **One `:allow` that changes no call names each that still asks, and
/// whose** (review stop 8, F 4) — the arm that stands kept the first by
/// `BufferId`: "the open call still asks — `/tmp/shannon-test-one` is
/// outside where Shannon started" explained the call in front of you with
/// another session's path, on no screen.
#[tokio::test]
async fn an_allow_that_changes_no_call_names_each_that_still_asks() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("b1's first turn");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    manual_tool(&mut app, "touch /tmp/shannon-test-one");
    app.new_session_command();
    manual_tool(&mut app, "rm /tmp/shannon-test-two");
    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    app.dispatch_command_for_test("allow echo:*");
    let said = app.notice().unwrap_or_default().to_string();
    assert_eq!(
        app.decisions.every_shown().count(),
        2,
        "premise: both still ask: {said}"
    );
    assert!(
        said.contains("the open call still asks") && said.contains("(`rm /tmp/shannon-test-two`)"),
        "the call in front of you, named: {said}"
    );
    let off_aim = app
        .name_off_the_aim(BufferId(0))
        .expect("premise: b1 is off the aim");
    assert!(
        said.contains("`touch /tmp/shannon-test-one`") && said.contains(&off_aim),
        "the other, named with whose: {said}"
    );
}

/// **A `:Send` waits on a call typed in its session, as a prompt does**
/// (review stop 8, A 9): `start_job_on` asked for work and a review apart,
/// and so started a turn on b1 while b1's `:tool` waited — a prompt to b1
/// was refused over it ("answer it first").
#[tokio::test]
async fn a_send_is_refused_over_a_call_typed_there() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("b1's first turn");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    assert!(app.shown_call().is_some(), "premise: b1's `:tool` asks");
    app.new_session_command();
    app.prompt_mut().insert_pasted("to b1");
    app.dispatch_command_for_test("Send b1");
    assert!(
        !app.generations.iter().any(|g| g.buffer == BufferId(0)),
        "no job on b1: {:?}",
        app.notice()
    );
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("a command is awaiting your decision"),
        "{said}"
    );
}
