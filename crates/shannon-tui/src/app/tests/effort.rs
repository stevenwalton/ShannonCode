//! **Effort** — `turn.effort` from the setting to the wire and into the
//! record. Every request path carrying it is
//! `every_request_carries_the_sessions_model_and_knobs`; this is what that
//! property cannot see: the seal, the model that takes none, and a turn whose
//! setting changes under it.
//!
//! Steven, 2026-10-04: "let's make sure that we have model effort enabled."

use super::*;
use shannon_core::{Effort, SendRequest};

type Sent = Arc<std::sync::Mutex<Vec<SendRequest>>>;

/// An app on a recording backend, with `turn.effort` set to `effort`.
fn recorded(effort: Option<Effort>) -> (App, Sent) {
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let sent = backend.requests.clone();
    let mut cfg = Config::default();
    cfg.turn.effort = effort;
    (test_app_backed(backend, Arc::new(cfg)), sent)
}

/// The effort on the newest request.
fn last_effort(sent: &Sent) -> Option<Effort> {
    sent.lock()
        .unwrap()
        .last()
        .expect("a request went out")
        .effort
}

/// The effort the newest turn of the session in front was sealed under.
fn sealed_effort(app: &App) -> Option<Effort> {
    let ep = app.buffer().conversation.episode().id;
    let graph = app.graph_reading(&[ep]).unwrap();
    graph
        .records(&ep)
        .and_then(|mut r| r.pop())
        .expect("a committed turn")
        .provenance
        .config
        .effort
}

/// **What is sent is what is recorded** — and a model with no effort control
/// is sent none and records none, though the setting stays for the run.
#[tokio::test]
async fn a_turn_is_sent_and_sealed_under_the_same_effort() {
    let (mut app, sent) = recorded(Some(Effort::Max));
    app.model_command("opus");
    app.submit("think it through");
    assert_eq!(last_effort(&sent), Some(Effort::Max), "on the wire");
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(sealed_effort(&app), Some(Effort::Max), "in the record");

    // Haiku 4.5 takes no effort: sending one would make every turn a 400.
    app.model_command("haiku-4-5");
    app.submit("quick one");
    assert_eq!(last_effort(&sent), None, "not on the wire");
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(sealed_effort(&app), None, "nor claimed by the record");
    assert_eq!(
        app.config.turn.effort,
        Some(Effort::Max),
        "the setting is the run's — the next model that takes it gets it"
    );
}

/// **A tool round asks for what its turn began under.** `:model` is refused
/// mid-turn, `:set effort` is not — and the round re-read the session's
/// settings, so it ran at the new effort inside a turn sealed as the old.
#[tokio::test]
async fn a_tool_round_keeps_the_effort_its_turn_began_under() {
    let (mut app, sent) = recorded(Some(Effort::Max));
    app.model_command("opus");
    app.start_turn("look around");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "mkdir build",
    )));
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.awaiting_decision(),
        "premise: the round waits on a decision"
    );

    app.set_command("effort=low", false);
    assert_eq!(
        app.config.turn.effort,
        Some(Effort::Low),
        "premise: the setting changed mid-turn"
    );
    let before = sent.lock().unwrap().len();
    // Deny: a denial is an answer, so the round resumes and nothing runs.
    tool_key(&mut app, 'd');
    assert!(
        sent.lock().unwrap().len() > before,
        "premise: the round resumed"
    );
    assert_eq!(last_effort(&sent), Some(Effort::Max), "the turn's own");
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(sealed_effort(&app), Some(Effort::Max));

    app.submit("and again");
    assert_eq!(
        last_effort(&sent),
        Some(Effort::Low),
        "the NEXT turn takes the new setting"
    );
}

/// **A screen over a waiting call changes only what comes after it**
/// (Steven, 10-26: "If a user with a pending decision changes via `:model`
/// do we change the model and then replay with the same decision? Can
/// we?"). Nothing is replayed: the call is what its model answered, and it
/// is answered in its turn, under the model and effort the turn began with.
/// `:models` over it: a new model for that session is refused whole, as
/// `:model` refuses it; the effort alone is set, for the next turn.
#[tokio::test]
async fn a_screen_over_a_waiting_call_changes_only_what_comes_after() {
    let (mut app, sent) = recorded(Some(Effort::Max));
    app.model_command("opus");
    let opus = shannon_core::backend::models::resolve("opus");
    app.start_turn("look around");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "mkdir build",
    )));
    app.feed_stream(StreamMsg::Complete);
    assert!(app.awaiting_decision(), "premise: a call waits on you");

    app.dispatch_command_for_test("models");
    assert!(app.setup().is_some(), "opened over it");
    on_screen(&mut app, "j\tl\n");
    assert_eq!(
        app.buffer().conversation.episode().config.model,
        opus,
        "the model kept"
    );
    assert_eq!(own(&app), None, "and nothing of the Enter applied");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("awaiting your decision"), "said why: {said}");

    app.dispatch_command_for_test("models");
    on_screen(&mut app, "\tl\n");
    let chosen = own(&app);
    assert!(
        chosen.is_some() && chosen != Some(Effort::Max),
        "premise: the effort alone set: {chosen:?}"
    );

    tool_key(&mut app, 'd');
    let round = (sent.lock().unwrap().last().cloned()).expect("the round went out");
    assert_eq!(
        round.model.as_deref(),
        Some(opus.as_str()),
        "the turn's model"
    );
    assert_eq!(round.effort, Some(Effort::Max), "the turn's effort");
    app.feed_stream(StreamMsg::Complete);
    app.submit("and again");
    assert_eq!(last_effort(&sent), chosen, "the next turn takes it");
}

/// **A child's first turn is sealed under what it was sent.** It was sealed
/// under the episode's preference — which never carries an effort — while
/// its request carried one.
#[tokio::test]
async fn a_childs_turn_is_sealed_under_the_effort_it_was_sent() {
    let (mut app, sent) = recorded(Some(Effort::High));
    app.model_command("opus");
    // A child hangs off a turn.
    app.submit("parent");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    assert_eq!(last_effort(&sent), Some(Effort::High), "premise: sent");
    app.feed_job(child, StreamMsg::Chunk("done".into()));
    app.feed_job(child, StreamMsg::Complete);
    let graph = app.graph_reading(&[child]).unwrap();
    let sealed = graph
        .records(&child)
        .and_then(|mut r| r.pop())
        .expect("the child's turn");
    assert_eq!(sealed.provenance.config.effort, Some(Effort::High));
}

/// **A child starts at its parent's own effort**, as at its model — not at
/// the run's default, which the parent's outranks.
#[tokio::test]
async fn a_child_starts_at_its_parents_effort() {
    let (mut app, sent) = recorded(Some(Effort::Low));
    app.model_command("opus");
    app.dispatch_command_for_test("effort max");
    app.submit("parent");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    assert_eq!(
        last_effort(&sent),
        Some(Effort::Max),
        "the parent's, not low"
    );
    let graph = app.current_graph();
    let config = &graph.episode(&child).expect("the child").config;
    assert_eq!(config.effort, Some(Effort::Max), "kept as the child's own");
}

/// `S`, run to the end on the session in front: marked whole, distilled,
/// its review up and composed as `$EDITOR`'s save composes it. `between`
/// runs while the distillation is in flight. The composed session's id.
async fn distil(app: &mut App, between: impl FnOnce(&mut App)) -> EpisodeId {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let episode = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    distill_reviewed(app);
    assert!(
        app.distilling().is_some(),
        "premise: `S` started: {:?}",
        app.notice()
    );
    between(app);
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    let buffer = app.current;
    let review = app
        .decisions
        .review(buffer)
        .expect("the draft waits for review");
    let text = to_editable(&[
        Message::user("instruction"),
        Message::assistant(review.work().draft.clone()),
    ]);
    app.compose_review(buffer, Some(text)).expect("composes")
}

/// **`S`: two configs, never one** (Reviewers, 10-04). The summary TURN is
/// sealed under what the distillation was sent — the run's effort and
/// ceiling here — and the composed SESSION keeps its base's own settings, as
/// they are when it lands: an `:effort` set while `S` ran carries over.
/// One value for both made the session adopt what was sent as its own.
#[tokio::test]
async fn a_distilled_session_keeps_its_own_and_its_summary_what_was_sent() {
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let sent = backend.requests.clone();
    let dir = TempDir::new().unwrap();
    let mut cfg = Config::default();
    cfg.turn.effort = Some(Effort::Xhigh);
    cfg.turn.max_output_tokens = 8_000;
    let mut app = test_app_with_store(
        backend,
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.model_command("opus");
    assert_eq!(own(&app), None, "premise: the session never chose");
    for n in 0..2 {
        app.conversation_mut().begin_turn(format!("q{n}"));
        app.conversation_mut().push_chunk(&format!("a{n}"));
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let composed = distil(&mut app, |_| {}).await;
    assert_eq!(
        last_effort(&sent),
        Some(Effort::Xhigh),
        "premise: sent at xhigh"
    );
    let graph = app.current_graph();
    let summary = &graph.placed(&composed)[0].provenance.config;
    assert_eq!(
        summary.effort,
        Some(Effort::Xhigh),
        "the summary: what was sent"
    );
    assert_eq!(summary.max_tokens, Some(8_000), "and its ceiling");
    let session = &graph.episode(&composed).expect("composed").config;
    assert_eq!(session.effort, None, "the session: still never chose");
    assert_eq!(session.max_tokens, None, "and no ceiling of its own");
    assert_eq!(
        session.model,
        shannon_core::backend::models::resolve("opus")
    );

    // An `:effort` set while it runs is the session's when it lands.
    let composed = distil(&mut app, |app| {
        app.dispatch_command_for_test("effort low");
    })
    .await;
    let graph = app.current_graph();
    let session = &graph.episode(&composed).expect("composed").config;
    assert_eq!(session.effort, Some(Effort::Low), "set mid-flight, kept");
}

/// **A turn no request produced records no effort** — a hand-typed `s`, a
/// `:return` result, a turn an edit adds — though the session it lands in is
/// at `max`. Sealed under the session's preference, each claimed `max` was
/// sent (Reviewers, 10-04); `TurnConfig::authored` is their one seal.
#[tokio::test]
async fn turns_nothing_sent_record_no_effort() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    // The effort the session's LAST turn records — after asserting it is the
    // turn the path made (`says`), or an older turn sealed before `:effort
    // max` would pass this vacuously.
    let last_effort_in = |app: &App, ep: EpisodeId, says: &str| {
        let g = app.graph_reading(&[ep]).expect("readable");
        let last = g.records(&ep).and_then(|mut r| r.pop()).expect("a turn");
        let words: Vec<&str> = last.messages.iter().map(|m| m.content.as_str()).collect();
        assert!(
            words.contains(&says),
            "premise: the path's own turn: {words:?}"
        );
        last.provenance.config.effort
    };

    // A turn an edit adds after the one it edits.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.dispatch_command_for_test("effort max");
    let episode = app.conversation().episode().id;
    let edited = to_editable(&[
        Message::user("Hello 1"),
        Message::assistant("reply 1"),
        Message::user("added"),
        Message::assistant("by hand"),
    ]);
    let branch = app
        .commit_slice_edit(episode, 1, 2, &edited)
        .unwrap_or_else(|| panic!("composes: {:?}", app.notice()));
    assert_eq!(
        last_effort_in(&app, branch, "added"),
        None,
        "an added turn was typed"
    );

    // A hand-typed `s`.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.dispatch_command_for_test("effort max");
    let episode = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    let plan = app.squash_plan().expect("the marked session resolves");
    let text = to_editable(&[Message::user("ask"), Message::assistant("typed")]);
    let squashed = app
        .commit_summary_into(
            app.current,
            plan.graph,
            plan.target,
            &plan.sources,
            &text,
            None,
        )
        .expect("composes");
    assert_eq!(
        last_effort_in(&app, squashed, "typed"),
        None,
        "a summary typed by hand"
    );
    let graph = app.current_graph();
    let session = &graph.episode(&squashed).expect("composed").config;
    assert_eq!(
        session.effort,
        Some(Effort::Max),
        "the session keeps its own"
    );

    // A `:return` result: assembled from the child's answer, not sent.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.dispatch_command_for_test("effort max");
    let child = app.spawn_child_job("compute it", None).expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("42".into()));
    app.feed_job(child, StreamMsg::Complete);
    app.return_result(child);
    let parent = app.conversation().episode().id;
    assert_eq!(
        last_effort_in(&app, parent, "42"),
        None,
        "a returned result"
    );
}

/// **With no key, a distilled session is not moved to the stub.** The
/// summary records the stub that wrote it; the session keeps the model it
/// asked for, which a key added later answers.
#[tokio::test]
async fn a_keyless_distilled_session_keeps_its_model() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    assert!(app.only_stub(), "premise: only the stub answers");
    app.model_command("opus");
    let composed = distil(&mut app, |_| {}).await;
    let graph = app.current_graph();
    let stub = shannon_core::backend::models::STUB;
    assert_eq!(graph.placed(&composed)[0].provenance.config.model, stub);
    let session = &graph.episode(&composed).expect("composed").config;
    assert_eq!(
        session.model,
        shannon_core::backend::models::resolve("opus")
    );
}

/// **A prefill `:continue`: what answers it now, and the session's own**.
/// The request (and the composed turn) take the resumed turn's config, its
/// effort re-filtered — with no key the stub answers, which takes none. The
/// composed SESSION keeps the continued one's preference, as it is when the
/// continuation lands: not the stub, and an `:effort` set meanwhile kept.
#[tokio::test]
async fn a_prefill_continue_asks_what_answers_now_and_keeps_the_sessions_own() {
    let mut cfg = Config::default();
    cfg.continue_.strategy = shannon_core::ContinueStrategy::Prefill;
    let mut app = test_app_with(cfg);
    assert!(app.only_stub(), "premise: only the stub answers");
    app.model_command("opus");
    let opus = shannon_core::backend::models::resolve("opus");
    let produced = TurnConfig {
        model: opus.clone(),
        max_tokens: None,
        effort: Some(Effort::Max),
    };
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut()
        .begin_turn_under("write a poem", Vec::new(), produced);
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(shannon_core::StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.continue_last_turn();
    assert!(app.continuing(), "premise: the prefill started");
    let config = app
        .generations
        .iter()
        .find_map(|g| match &g.work {
            Work::Continuation(work) => Some(work.config.clone()),
            _ => None,
        })
        .expect("the continuation's work");
    let stub = shannon_core::backend::models::STUB;
    assert_eq!(config.model, stub);
    assert_eq!(config.effort, None, "the stub takes none");

    app.dispatch_command_for_test("effort low");
    app.feed_continuation(StreamMsg::Chunk(" red".into()));
    app.feed_continuation(StreamMsg::Complete);
    assert!(!app.continuing(), "premise: it landed");
    let session = &app.conversation().episode().config;
    assert_eq!(session.model, opus, "the session's own model, not the stub");
    assert_eq!(session.effort, Some(Effort::Low), "set meanwhile, kept");
}

/// `:graph`'s inspector reads back the effort a turn was sent.
#[tokio::test]
async fn the_inspector_shows_the_effort_a_turn_was_sent() {
    let (mut app, _) = recorded(Some(Effort::Xhigh));
    app.model_command("opus");
    app.submit("hi");
    app.feed_stream(StreamMsg::Chunk("hello".into()));
    app.feed_stream(StreamMsg::Complete);
    let episode = app.buffer().conversation.episode().id;
    let turn = app.current_graph().placed(&episode)[0].turn;
    let mut view = app.build_graph_view();
    assert!(view.select_node(Node::Turn {
        episode,
        turn,
        index: 0
    }));
    let text: Vec<String> = crate::view::inspector_body(&view, false, "unix")
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    let row = text
        .iter()
        .find(|l| l.contains("effort"))
        .unwrap_or_else(|| panic!("an effort row: {text:?}"));
    assert!(row.contains("xhigh"), "{row}");

    // And the SESSION's row says its own effort — `default` until it chooses.
    let session_effort = |app: &App| -> String {
        let mut view = app.build_graph_view();
        assert!(view.select_node(Node::Episode(episode)));
        crate::view::inspector_body(&view, false, "unix")
            .iter()
            .map(crate::paint::PageLine::text)
            .find(|l| l.contains("effort"))
            .unwrap_or_default()
    };
    assert!(
        session_effort(&app).contains("default"),
        "{}",
        session_effort(&app)
    );
    app.dispatch_command_for_test("effort low");
    assert!(
        session_effort(&app).contains("low"),
        "{}",
        session_effort(&app)
    );
}

/// Keys on the open overlay, routed as the event loop routes them.
fn on_screen(app: &mut App, keys: &str) {
    for c in keys.chars() {
        let code = match c {
            '\n' => KeyCode::Enter,
            '\t' => KeyCode::Tab,
            '\x1b' => KeyCode::Esc,
            c => KeyCode::Char(c),
        };
        let event = Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        assert!(app.setup_event_for_test(&event), "the screen took `{c:?}`");
    }
}

/// The session in front's own effort.
fn own(app: &App) -> Option<Effort> {
    app.buffer().conversation.episode().config.effort
}

/// **`:effort <level>` is the session's**, as `:model` is (Steven, 10-04) —
/// it says when the session's model will not be sent it, refuses a word that
/// is not a level without a stale note beside the refusal, and `:set
/// effort=` sets the run's default, which a session's own outranks.
#[tokio::test]
async fn effort_is_the_sessions_and_says_where_it_does_not_reach() {
    let (mut app, _) = recorded(None);
    app.model_command("opus");
    app.dispatch_command_for_test("effort max");
    assert_eq!(own(&app), Some(Effort::Max), "the session's");
    assert_eq!(app.config.turn.effort, None, "not the run's");
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("takes no effort"), "opus takes it: {said}");

    app.model_command("haiku-4-5");
    app.dispatch_command_for_test("effort high");
    assert_eq!(own(&app), Some(Effort::High));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("haiku-4-5 takes no effort"), "{said}");

    app.dispatch_command_for_test("effort hgih");
    assert_eq!(own(&app), Some(Effort::High), "refused");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("is not an effort"), "and says why: {said}");
    assert!(!said.contains("takes no effort"), "and only why: {said}");

    // The rest of a line is not a second setting.
    let scrolloff = app.config.tui.scrolloff;
    app.dispatch_command_for_test("effort max tui.scrolloff=17");
    assert_eq!(own(&app), Some(Effort::High));
    assert_eq!(app.config.tui.scrolloff, scrolloff);

    app.dispatch_command_for_test("effort default");
    assert_eq!(own(&app), None, "follows the run's again");

    app.dispatch_command_for_test("effort low");
    app.dispatch_command_for_test("set effort=xhigh");
    assert_eq!(app.config.turn.effort, Some(Effort::Xhigh), "the default");
    assert_eq!(
        own(&app),
        Some(Effort::Low),
        "which a session's own outranks"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("keeps its own effort, low"), "{said}");
}

/// **A session's effort comes back with it** — what Steven saw: a resumed
/// session back on its model and without its effort, which had been a `:set`.
#[tokio::test]
async fn a_sessions_effort_comes_back_with_it() {
    let dir = TempDir::new().unwrap();
    let episode = {
        let mut app = app_with_n_turns(dir.path(), 1);
        app.dispatch_command_for_test("effort xhigh");
        assert_eq!(own(&app), Some(Effort::Xhigh), "premise: set");
        app.conversation().episode().id
    };
    let mut app = relaunched(dir.path());
    app.dispatch_command_for_test(&format!("open {}", episode.short_hex()));
    assert_eq!(
        app.buffer().conversation.episode().id,
        episode,
        "premise: opened"
    );
    assert_eq!(own(&app), Some(Effort::Xhigh));
}

/// **A bare `:effort` is vim's command-line window**: a panel at the
/// bottom, the session still on screen above it, one `:effort` line per
/// choice, the cursor on the session's; `<CR>` runs the line, `q` nothing.
#[tokio::test]
async fn a_bare_effort_opens_the_panel_on_the_sessions_line() {
    let (mut app, _) = recorded(None);
    app.model_command("opus");
    app.dispatch_command_for_test("effort xhigh");
    app.dispatch_command_for_test("effort");
    assert_eq!(
        app.setup().map(crate::setup::Setup::entry),
        Some(crate::setup::Entry::Effort)
    );
    let shown = screen(&mut app, 100);
    let rows: Vec<&str> = shown.lines().collect();
    let here = rows
        .iter()
        .position(|l| l.contains("▸:effort xhigh"))
        .unwrap_or_else(|| panic!("the cursor on the session's line:\n{shown}"));
    assert!(
        here > 20,
        "at the bottom of 30 rows, not a box in the middle:\n{shown}"
    );
    let session = app.buffer().conversation.episode().id.short_hex();
    assert!(
        rows[..here - 4].iter().any(|l| l.contains(&session)),
        "the session still readable above it:\n{shown}"
    );

    on_screen(&mut app, "j\n");
    assert!(app.setup().is_none(), "Enter closes it");
    assert_eq!(
        own(&app),
        Some(Effort::Max),
        "the line under the cursor ran"
    );

    app.dispatch_command_for_test("effort");
    on_screen(&mut app, "ggq");
    assert_eq!(own(&app), Some(Effort::Max), "q runs nothing");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("effort unchanged")
    );
}

/// **The panel's bar owns the status row** — the row's own account offered
/// the focused pane's keys, `^C:stop`, a decision's `a`/`d`, all of which
/// the panel takes — and a decision arriving behind it is said there.
#[tokio::test]
async fn the_panels_bar_owns_the_status_row() {
    let (mut app, _) = recorded(None);
    app.model_command("opus");
    app.dispatch_command_for_test("effort");
    let shown = screen(&mut app, 100);
    let status = shown.lines().last().expect("a status row");
    assert!(status.contains("q close · Enter set"), "{status}");
    assert!(
        !shown.contains("i:insert") && !shown.contains("^W:focus"),
        "no key the panel takes is offered:\n{shown}"
    );

    // A decision arrives while it is open: the bar says so.
    app.start_turn("look");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "mkdir build",
    )));
    app.feed_stream(StreamMsg::Complete);
    assert!(app.awaiting_decision(), "premise: a decision waits");
    let shown = screen(&mut app, 100);
    let status = shown.lines().last().expect("a status row");
    assert!(status.contains("a decision is waiting"), "{status}");

    // And opened over one, it says so from the start (Steven, 10-26: "I'd
    // open it up so these don't block").
    on_screen(&mut app, "q");
    app.dispatch_command_for_test("effort");
    assert!(app.setup().is_some(), "opened over a decision");
    let shown = screen(&mut app, 100);
    let status = shown.lines().last().expect("a status row");
    assert!(status.contains("a decision is waiting"), "{status}");
}

/// **A screen sets only the session it opened on.** Something put another
/// session in front while it was open (here `:new`, as a landing `S` review
/// or prefill `:continue` would): Enter refuses, and says why, rather than
/// setting a session the screen never named.
#[tokio::test]
async fn a_screen_sets_only_the_session_it_opened_on() {
    let (mut app, _) = recorded(None);
    app.model_command("opus");
    // A turn, so `:new` really moves (on a blank session it stays put).
    app.submit("hello");
    app.feed_stream(StreamMsg::Chunk("hi".into()));
    app.feed_stream(StreamMsg::Complete);
    let opened_on = app.buffer().conversation.episode().id;
    app.dispatch_command_for_test("effort");
    app.dispatch_command_for_test("new");
    assert_ne!(app.buffer().conversation.episode().id, opened_on, "premise");
    on_screen(&mut app, "G\n");
    assert_eq!(own(&app), None, "the new session was not set");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("changed while it was open"), "{said}");
}

/// **What `:effort` says**: the same level again is `unchanged` and writes
/// nothing; `default` on a model that takes none says none is sent (it said
/// the run's level "is kept for" the session); with no key, the stub.
#[tokio::test]
async fn effort_says_what_happened() {
    let (mut app, _) = recorded(Some(Effort::High));
    app.model_command("opus");
    app.dispatch_command_for_test("effort max");
    app.dispatch_command_for_test("effort max");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("max") && said.contains("unchanged"), "{said}");

    app.model_command("haiku-4-5");
    app.dispatch_command_for_test("effort default");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("haiku-4-5 takes none — sent none"), "{said}");
    assert!(!said.contains("kept for"), "{said}");

    let mut keyless = test_app();
    assert!(keyless.only_stub(), "premise: no key");
    keyless.model_command("opus");
    keyless.dispatch_command_for_test("effort max");
    let said = keyless.notice().unwrap_or_default();
    assert!(said.contains("no API key, so the stub answers"), "{said}");
}

/// **Nothing behind the panel looks like it takes keys** — the panel does,
/// all of them — so the pane that HAD focus is drawn without its focus
/// border (and no terminal cursor sits in it). Read off the rendered cells:
/// a role nobody renders is no signal.
#[tokio::test]
async fn the_panes_behind_the_panel_are_unfocused() {
    let (mut app, _) = recorded(None);
    let area = ratatui::layout::Rect::new(0, 0, 100, 30);
    let layout = app.workspace.frame_layout(area);
    let focused = layout
        .panes
        .iter()
        .find(|p| p.focused)
        .expect("a focused pane")
        .rect;
    let corner = |app: &mut App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        draw_frame(app, &mut terminal);
        terminal.backend().buffer()[(focused.x, focused.y)]
            .style()
            .fg
    };
    assert_eq!(
        corner(&mut app),
        Some(crate::palette::FOCUSED),
        "premise: focus shows"
    );
    app.dispatch_command_for_test("effort");
    assert!(
        focused.y < layout.status.y - 7,
        "premise: the corner is above the panel"
    );
    assert_ne!(
        corner(&mut app),
        Some(crate::palette::FOCUSED),
        "the panel has the keys"
    );
}

/// **`:models` applies each field through the command that owns it**, and
/// the status line then says what a turn will be sent.
#[tokio::test]
async fn models_applies_each_field_through_its_command() {
    let (mut app, sent) = recorded(None);
    app.model_command("haiku");
    app.dispatch_command_for_test("models");
    // haiku → sonnet; effort default → max; context the model's → 200k.
    on_screen(&mut app, "j\tlllll\tll");
    assert!(
        screen(&mut app, 100).contains("Enter: model claude-haiku-5-5 → claude-sonnet-5-5"),
        "said before it is done"
    );
    on_screen(&mut app, "\n");
    assert!(app.setup().is_none());
    assert_eq!(
        app.buffer().conversation.episode().config.model,
        shannon_core::backend::models::resolve("sonnet")
    );
    assert_eq!(own(&app), Some(Effort::Max), "the session's effort");
    assert_eq!(app.config.turn.effort, None, "not the run's");
    assert_eq!(
        app.config.turn.max_context_tokens, 200_000,
        "the run's budget"
    );
    // The notice holds the status row until the next moment.
    app.new_moment();
    let shown = screen(&mut app, 140);
    assert!(shown.contains("sonnet max"), "the status line:\n{shown}");

    app.submit("go");
    let req = sent.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (req.effort, req.model.as_deref()),
        (Some(Effort::Max), Some("claude-sonnet-5-5"))
    );
}

/// **Leaving changes nothing**, and a model switch mid-turn is refused as
/// `:model` refuses it — the screen has no door of its own.
#[tokio::test]
async fn models_leaves_clean_and_refuses_what_model_refuses() {
    let (mut app, _) = recorded(None);
    app.model_command("haiku-4-5");
    app.dispatch_command_for_test("models");
    on_screen(&mut app, "j\tl\x1b");
    assert!(app.setup().is_none());
    assert_eq!(app.config.turn.effort, None);
    let haiku = shannon_core::backend::models::resolve("haiku-4-5");
    assert_eq!(app.buffer().conversation.episode().config.model, haiku);
    assert!(app.notice().unwrap_or_default().contains("models closed"));

    // Mid-turn, an Enter that changes the model is refused WHOLE — its
    // effort too — rather than applying the half `:effort` would allow.
    app.start_turn("busy");
    app.dispatch_command_for_test("models");
    on_screen(&mut app, "j\tl\n");
    assert_eq!(
        app.buffer().conversation.episode().config.model,
        haiku,
        "a turn in flight keeps its model"
    );
    assert_eq!(own(&app), None, "and nothing of the Enter applied");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("models: a turn is in flight"),
        "the refusal says why: {said}"
    );
}

/// **A bare `:model` is `:models`** (Steven, 10-06), and opening it changes
/// nothing: Steven's earlier rule, "on a default `:model` with no argument
/// the model should not change from the current value". The hazard is a
/// keyless run writing the stub that answers over the session's preference.
#[tokio::test]
async fn a_bare_model_opens_models_and_sets_nothing() {
    let (mut app, _) = recorded(None);
    app.model_command("opus");
    let opus = shannon_core::backend::models::resolve("opus");
    for bare in ["", "   "] {
        app.model_command(bare);
        assert_eq!(
            app.setup().map(|s| s.entry()),
            Some(crate::setup::Entry::Models),
            "{bare:?} opens `:models`"
        );
        on_screen(&mut app, "\x1b");
        assert!(app.setup().is_none());
        assert_eq!(app.buffer().conversation.episode().config.model, opus);
    }
    app.dispatch_command_for_test("model");
    assert!(app.setup().is_some(), "and typed, through the dispatch");
    assert!(
        !screen(&mut app, 100).contains("no API key"),
        "a key answers"
    );
}

/// **With no key the screen says the stub answers** — what a bare `:model`
/// said before it opened the screen, and what the header alone would hide.
#[test]
fn a_bare_model_says_when_the_stub_answers() {
    let mut app = test_app();
    app.model_command("opus");
    app.setup = None; // the setup `:model opus` offers with no key
    app.model_command("");
    let shown = screen(&mut app, 100);
    assert!(
        shown.contains("no API key, so the stub answers, not opus"),
        "{shown}"
    );
}

/// **Every screen opens over a decision, and none over a y/N** — through
/// each of its doors. A decision is its pane's, answered there, and one
/// behind a screen is said on its legend or bar (Steven, 10-26: "I'd open
/// it up so these don't block"); a y/N takes the next key wherever you are,
/// which the screen would take (`refuse_over_a_confirm`).
#[tokio::test]
async fn every_screen_opens_over_a_decision_and_none_over_a_confirm() {
    let doors = ["models", "model", "effort", "setup"];
    let (mut app, _) = recorded(None);
    app.start_turn("look");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "mkdir build",
    )));
    app.feed_stream(StreamMsg::Complete);
    assert!(app.awaiting_decision(), "premise: a decision waits");
    for door in doors {
        app.dispatch_command_for_test(door);
        assert!(app.setup().is_some(), "`:{door}` over a decision");
        let shown = screen(&mut app, 100);
        assert!(
            shown.contains("a decision is waiting"),
            "`:{door}` says it waits:\n{shown}"
        );
        on_screen(&mut app, "\x1b");
        assert!(app.setup().is_none(), "premise: `:{door}` left");
    }
    assert!(app.awaiting_decision(), "and the decision still waits");

    let (mut app, _) = recorded(None);
    app.pending_confirm.push_back(ConfirmAction::DeleteAll);
    for door in doors {
        app.dispatch_command_for_test(door);
        assert!(app.setup().is_none(), "`:{door}` not over a y/N");
    }
    app.pending_confirm.clear();
    for door in doors {
        app.dispatch_command_for_test(door);
        assert!(app.setup().is_some(), "`:{door}` opens once answered");
        on_screen(&mut app, "\x1b");
    }
}

/// **Every request a session makes carries THAT session's model.**
///
/// Six sites re-read `episode().config.model` to build a request and two did
/// not ask at all: `S` and a prefill `:continue` handed `SendRequest::new` to
/// the backend bare, so `req.model` was `None` and the backend fell back to the
/// model it was *constructed* with. Invisible while every session ran the same
/// one; wrong the moment `:model` could differ. `:model opus` then `S`
/// distilled on the startup model, and a continuation generated the splice on
/// one model and spliced it into a turn attributed to another.
///
/// Written as **one property over every path** rather than two tests for the
/// two that were broken. The defect was an omission, and a test per known
/// omission cannot catch the next one — this fails for a seventh path that
/// forgets, which is the shape of the bug (`[[feedback_generalize_after_examples]]`).
///
/// **And the knobs beside the model** (10-04): the reply ceiling reached one
/// of these paths and the effort none, while every seal recorded the ceiling.
/// Each request is now built from the config it is sealed under
/// (`SendRequest::under`), and this is the property that says so — on a
/// model that takes effort, and on one that takes none, where EVERY path must
/// send none (one that sent it would turn each of its turns into a 400).
#[tokio::test]
async fn every_request_carries_the_sessions_model_and_knobs() {
    use shannon_core::Effort;
    const MODEL: &str = "claude-opus-5";
    let knobs = every_request_path(MODEL).await;
    assert!(
        knobs.iter().all(|(m, _, _)| m.as_deref() == Some(MODEL)),
        "a request with no model runs on whatever the BACKEND was built with, \
         which is not the session you are looking at: {knobs:?}"
    );
    assert!(
        knobs
            .iter()
            .all(|(_, effort, cap)| (*effort, *cap) == (Some(Effort::Max), Some(8_000))),
        "a knob the user set that a request did not carry: {knobs:?}"
    );

    let haiku = shannon_core::backend::models::resolve("haiku-4-5");
    let knobs = every_request_path(&haiku).await;
    assert!(
        knobs
            .iter()
            .all(|(m, effort, _)| m.as_deref() == Some(haiku.as_str()) && effort.is_none()),
        "a model that takes no effort is sent none, on every path: {knobs:?}"
    );
}
