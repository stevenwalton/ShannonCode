//! The model's and the stub's honesty: what is said about who answered.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **With no API key, nothing may claim a real model is answering.**
///
/// Steven, dogfooding 2026-08-25: `cargo run` with no key on disk falls back
/// to the canned stub — correctly, and it says so at startup. But `:model
/// haiku` then put `haiku` in the pane title *and* on the status row, and the
/// stub went on answering. Every signal on screen agreed with each other and
/// with nothing that was happening; the only way to find out was to read the
/// reply and notice it was canned.
///
/// Both halves are asserted against a **live-backend app** as well, because
/// "never says haiku" passes trivially if the display simply stopped showing
/// models ([[feedback_assert_the_subject]]).
#[test]
fn a_stub_only_process_does_not_claim_a_real_model_answers() {
    let mut app = test_app();
    assert!(
        app.only_stub(),
        "premise: test_app is backed by the canned stub"
    );

    app.notices.clear();
    app.model_command("haiku-4-5");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("no API key"),
        "the notice must say the stub still answers: {said:?}"
    );
    // …and how to fix it: setup, open on the key the model needs (it used to
    // say "set ANTHROPIC_API_KEY, then restart"; a saved key answers at once).
    assert!(said.contains("`:setup`"), "{said:?}");
    assert!(
        app.setup().is_some(),
        "setup opened on what `haiku` needs: a key"
    );
    // Leaving it changes nothing — the stub still answers, below.
    app.setup_event_for_test(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.setup().is_none());
    assert!(app.only_stub());
    // **Assert the RENDERED row and title, not the pure function.** The first
    // version of this test called `answering_model` directly — and reverting
    // BOTH display sites to `episode.config.model` left all 1017 tests green
    // (Reviewer C). The reported symptom was verbatim "put `haiku` in the pane
    // title *and* on the status row", so those are the two things to read.
    use ratatui::{Terminal, backend::TestBackend};
    // Clear the notice first: it OWNS the status row while it is current, and
    // it legitimately names the model that is not answering ("no API key for
    // claude-haiku-…"). What is under test is the row's ordinary state.
    app.notices.clear();
    let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let screen = rendered_text(terminal.backend().buffer());
    let row = screen.lines().last().expect("a status row").to_string();
    assert!(
        row.contains("stub"),
        "the status row names what will answer:\n{row}"
    );
    assert!(!row.contains("haiku"), "…and not what cannot:\n{row}");
    assert!(
        !screen.contains("haiku"),
        "nor anywhere else on the frame — the pane title said it too:\n{screen}"
    );

    // The inspector is the third surface, and it is drawn from its own
    // function rather than the frame above.
    let insp = crate::view::inspector_body(&app.build_graph_view(), true, "unix");
    let insp: String = insp.iter().map(crate::paint::PageLine::text).collect();
    assert!(
        !insp.contains("haiku"),
        "the inspector sits beside `:graph` and said it too:\n{insp}"
    );

    assert_eq!(
        app.answering_model("claude-haiku-4-5-20251001"),
        shannon_core::backend::models::STUB,
        "and the predicate underneath agrees"
    );

    // A live backend: the same command must be plain, or the caveat above
    // would just be a sentence this app always emits.
    let live = test_app_backed(
        std::sync::Arc::new(shannon_core::backend::HangingBackend::new()),
        std::sync::Arc::new(Config::default()),
    );
    assert!(!live.only_stub(), "premise: HangingBackend is not the stub");
    assert_eq!(
        live.answering_model("claude-haiku-4-5-20251001"),
        "claude-haiku-4-5-20251001",
        "with a real backend the session's model is the one shown"
    );
}

/// **`:model` stops reporting a typo as a success.**
///
/// Found by sweeping every documented command with `tools/drive`:
/// `:model nonesuch` answered "model: nonesuch — for 8d581d15, from the next
/// turn", indistinguishable from `:model opus`. The name really is set, and
/// that is deliberate — `resolve` passes an unknown name through so a model
/// released after this build still works — but the only other signal the user
/// gets is a 400 on the next turn, which names the API rather than the command
/// that caused it.
#[test]
fn setting_an_unknown_model_says_it_is_unknown() {
    // **A live backend, or the stub caveat swallows the one under test.**
    // `test_app` is stub-backed, so `:model opus` there reports "the stub still
    // answers" and the unknown-alias branch is unreachable — the fixture would
    // omit the axis ([[feedback_fixture_omits_the_axis]]).
    let mut app = test_app_backed(
        std::sync::Arc::new(shannon_core::backend::HangingBackend::new()),
        std::sync::Arc::new(Config::default()),
    );
    assert!(!app.only_stub(), "premise: this backend is not the stub");

    app.notices.clear();
    app.model_command("opus");
    let known = app.notice().unwrap_or_default();
    assert!(
        known.contains("claude-opus-5") && !known.contains("not a known alias"),
        "premise: a real alias must NOT carry the caveat, or the caveat says \
         nothing: {known:?}"
    );

    app.notices.clear();
    app.model_command("nonesuch");
    let unknown = app.notice().unwrap_or_default();
    assert!(
        unknown.contains("not a known alias"),
        "a name we do not recognise must say so: {unknown:?}"
    );
    assert!(
        unknown.contains("nonesuch"),
        "…while still reporting that it was set — the door stays open: {unknown:?}"
    );
}

/// **The cap in effect is recorded, so the inspector stops contradicting itself.**
///
/// `TurnConfig.max_tokens` was `None` at all four seal sites, always — while
/// the cap was really applied, from a different place. Set `[turn]
/// max_output_tokens`, let a turn hit it, and the inspector showed two adjacent
/// lines disagreeing: `max tok  —` above `stop reason  max_tokens (:continue
/// resumes)` (Reviewer D).
///
/// It is provenance, not identity, so it is deliberately **not** hashed — see
/// `TurnId::compute` and `a_stored_turns_id_never_changes`.
#[tokio::test]
async fn a_turn_records_the_output_cap_that_was_in_effect() {
    let mut cfg = Config::default();
    cfg.turn.max_output_tokens = 4096;
    let mut app = test_app_with(cfg);

    app.submit("capped");
    app.feed_stream(StreamMsg::Complete);

    let ep = app.buffer().conversation.episode().id;
    let graph = app.graph_reading(&[ep]).unwrap();
    let sealed = graph
        .records(&ep)
        .and_then(|mut r| r.pop())
        .expect("a committed turn");
    assert_eq!(
        sealed.provenance.config.max_tokens,
        Some(4096),
        "the ceiling that was really sent"
    );

    // And with no cap configured, still honestly nothing.
    let mut plain = test_app();
    plain.submit("uncapped");
    plain.feed_stream(StreamMsg::Complete);
    let ep = plain.buffer().conversation.episode().id;
    let graph = plain.graph_reading(&[ep]).unwrap();
    let sealed = graph
        .records(&ep)
        .and_then(|mut r| r.pop())
        .expect("a committed turn");
    assert_eq!(
        sealed.provenance.config.max_tokens, None,
        "`0` means the model's own maximum, and `—` is then the truth"
    );
}

/// **A turn records what PRODUCED it, not what was asked for.**
///
/// Steven's rule, and the reason it is not a nit: "If they think they're
/// talking to model x and are actually talking to model y then that's a big
/// failure on our part." With no API key the backend is the canned stub for
/// every model, so `:model haiku` followed by a send used to persist a stub
/// turn labelled `claude-haiku-4-5-20251001` — and unlike a status row, an
/// on-disk record is never re-derived, so the lie outlived the session.
///
/// The other half is the one that would be easy to break while fixing this:
/// **the preference must survive.** `episode.config.model` is what `:model`
/// set and what a later run WITH a key honours; only the turn's provenance
/// follows reality.
#[tokio::test]
async fn a_turn_is_sealed_under_the_model_that_answered_it() {
    let mut app = test_app();
    assert!(app.only_stub(), "premise: nothing but the stub can answer");

    app.model_command("haiku-4-5");
    assert_eq!(
        app.buffer().conversation.episode().config.model,
        "claude-haiku-4-5-20251001",
        "premise: the session ASKS for haiku"
    );

    app.submit("what answered this?");
    app.feed_stream(StreamMsg::Complete);

    let ep = app.buffer().conversation.episode().id;
    let graph = app.graph_reading(&[ep]).unwrap();
    let sealed = graph
        .records(&ep)
        .and_then(|mut r| r.pop())
        .expect("a committed turn");
    assert_eq!(
        sealed.provenance.config.model,
        shannon_core::backend::models::STUB,
        "the record names what produced it"
    );

    assert_eq!(
        app.buffer().conversation.episode().config.model,
        "claude-haiku-4-5-20251001",
        "…and the preference is still what the user asked for"
    );
}

#[test]
fn committed_turns_record_the_resolved_backend_model() {
    // The model fix: the stub's sentinel is threaded into the active config,
    // so a committed turn's provenance carries the real model (and the
    // episode's forward config too) — not the "unspecified" placeholder.
    let mut app = test_app(); // CannedStub → model "canned-stub"
    app.conversation_mut().begin_turn("hi");
    app.conversation_mut().push_chunk("there");
    app.conversation_mut().commit_turn();

    let turn = app.conversation().head_turn().expect("a committed turn");
    assert_eq!(turn.provenance.config.model, "canned-stub");
    assert_eq!(app.conversation().episode().config.model, "canned-stub");
}
