//! Trusting a directory, and what trust decides.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **The first prompt to Claude Code in a directory asks to trust it** — on
/// the trust screen. No sends nothing and leaves the message in the prompt;
/// yes trusts the directory and sends it; after that, nothing asks (Steven,
/// 10-04: "shannon should ask its own trust questions per directory").
#[tokio::test]
async fn the_first_claude_code_prompt_in_a_directory_asks_to_trust_it() {
    let (mut app, provider) = untrusted_claude_code_app();
    let here = std::env::current_dir().unwrap();
    type_into(&mut app, "hello there");
    app.submit_prompt();
    assert!(trust_screen_up(&app));
    assert_eq!(requests_sent(&provider), 0, "asked before anything starts");

    press_setup(&mut app, KeyCode::Char('3'));
    assert!(!trust_screen_up(&app), "answered");
    assert_eq!(requests_sent(&provider), 0, "no: nothing sent");
    assert_eq!(app.prompt().text(), "hello there", "the message waits");
    assert!(app.notice().is_some_and(|n| n.contains("not sent")));
    // Esc and Ctrl-C, the reflexive keys, are the safe answer: no, said as
    // no — not "setup closed".
    for leave in [
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        app.submit_prompt();
        app.notices.clear();
        app.setup_event_for_test(&Event::Key(leave));
        assert_eq!(requests_sent(&provider), 0, "{leave:?}: nothing sent");
        assert!(!app.trusted.covers(&here));
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("not sent") && !said.contains("setup closed"),
            "{leave:?}: {said:?}"
        );
    }

    app.submit_prompt();
    press_setup(&mut app, KeyCode::Enter); // the first answer: remember
    assert_eq!(requests_sent(&provider), 1, "trusted, then sent");
    assert!(app.prompt().text().is_empty());
    assert!(app.trusted.covers(&here));
    assert!(app.notice().is_some_and(|n| n.contains("trusted")));

    type_into(&mut app, "and again");
    app.submit_prompt();
    assert!(!trust_screen_up(&app), "trusted: not asked again");
}

/// **"This time only" trusts the directory for this run, never kept** — and
/// sends the prompt that waited, as the remembered yes does.
#[tokio::test]
async fn trusting_this_time_only_keeps_nothing() {
    let (mut app, provider) = untrusted_claude_code_app();
    let here = std::env::current_dir().unwrap();
    type_into(&mut app, "hello");
    app.submit_prompt();
    press_setup(&mut app, KeyCode::Char('2'));
    assert_eq!(requests_sent(&provider), 1, "trusted, then sent");
    assert!(app.trusted.covers(&here));
    assert!(app.trusted.dirs().is_empty(), "nothing kept");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("until Shannon quits")),
        "{:?}",
        app.notice()
    );
}

/// **Trust is asked before anything is sent** — at launch, and when `:model`
/// or setup brings Claude Code in (Steven, 10-04: "mimic Claude Code and
/// Copilot that they ask before entering the TUI"). Yes trusts and sends
/// NOTHING — a draft in the prompt was never submitted; no leaves it to the
/// send-time ask, which still comes.
#[tokio::test]
async fn trust_is_asked_up_front() {
    let (mut app, provider) = untrusted_claude_code_app();
    type_into(&mut app, "a draft, not sent");
    app.ask_trust_up_front();
    assert!(trust_screen_up(&app));
    press_setup(&mut app, KeyCode::Char('1'));
    assert!(app.trusted.covers(&std::env::current_dir().unwrap()));
    assert_eq!(requests_sent(&provider), 0, "yes trusted; it sent nothing");
    assert_eq!(app.prompt().text(), "a draft, not sent");

    let (mut app, provider) = untrusted_claude_code_app();
    app.ask_trust_up_front();
    press_setup(&mut app, KeyCode::Char('q'));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("sending asks again")),
        "{:?}",
        app.notice()
    );
    type_into(&mut app, "hello");
    app.submit_prompt();
    assert!(trust_screen_up(&app), "and sending does");
    assert_eq!(requests_sent(&provider), 0);

    let mut api = test_app();
    api.ask_trust_up_front();
    assert!(!trust_screen_up(&api), "the API: nothing to ask");

    // A setup screen already up is answered first: the trust question never
    // replaces it (a half-pasted key would go with it).
    let (mut app, _) = untrusted_claude_code_app();
    app.open_setup();
    app.ask_trust_up_front();
    assert!(app.setup().is_some() && !trust_screen_up(&app));
}

/// **`:model` to Claude Code asks about this directory** when `claude` is
/// ready — the switch is the moment the question becomes true.
#[tokio::test]
async fn switching_to_claude_code_asks_about_this_directory() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app();
    app.claude_program = logged_in_claude(dir.path());
    app.model_command("claude-code/sonnet");
    assert!(
        trust_screen_up(&app),
        "ready, so the trust screen, not setup"
    );
    let shown = screen(&mut app, 100);
    for want in [
        "Do you trust the files in this folder?",
        "Yes, and remember",
        "Yes, this time only",
        "No",
        "Shannon Code — trust",
    ] {
        assert!(shown.contains(want), "{want:?} in {shown}");
    }
    // Whole, however deep: a folder wider than the box goes on across rows
    // (`setup::fold`), so read it as the rows run on.
    let here = crate::app::config_file::tilde(&std::env::current_dir().unwrap());
    // Spaces aside on both sides: a fold at a space in the folder's own name
    // drops it at the row's end (review of f50fda5, Reviewer A).
    let unspaced = |text: &str| text.replace(char::is_whitespace, "");
    let run_on: String = shown
        .lines()
        .map(|row| unspaced(row).trim_matches('│').to_string())
        .collect();
    assert!(
        run_on.contains(&unspaced(&here)),
        "names the folder: {shown}"
    );
}

/// **Claude Code never starts in an untrusted directory** — whatever starts
/// it: a turn that never met the question (a `:continue`, a job, a queued
/// prompt) is refused at the door every request leaves through, and the
/// refusal says how to trust. A session on the API is never asked.
#[tokio::test]
async fn claude_code_never_starts_in_an_untrusted_directory() {
    let (mut app, provider) = untrusted_claude_code_app();
    app.start_turn("past the question");
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), app.stream_rx.recv())
        .await
        .expect("the refusal arrives")
        .expect("a message");
    app.drain_stream(first);
    assert_eq!(requests_sent(&provider), 0, "nothing reached Claude Code");
    let shown = format!(
        "{} {}",
        app.output().lines().join("\n"),
        app.notice().unwrap_or_default()
    );
    assert!(shown.contains("`:trust` trusts it"), "{shown}");

    let mut api = test_app();
    type_into(&mut api, "hi");
    api.submit_prompt();
    assert!(
        api.pending_confirm.is_empty(),
        "not Claude Code: no question"
    );
}

/// **`:trust` says what is trusted and trusts; `:untrust` stops** — each
/// saying what happened.
#[tokio::test]
async fn trust_and_untrust_say_what_they_did() {
    let (mut app, _) = untrusted_claude_code_app();
    app.trust_command("");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no directory is trusted"))
    );
    app.trust_command(".");
    let here = std::env::current_dir().unwrap();
    assert!(app.trusted.covers(&here));
    app.trust_command("");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("this directory is"))
    );
    app.untrust_command("");
    assert!(!app.trusted.covers(&here));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no longer trusted"))
    );
    app.untrust_command("");
    assert!(app.notice().is_some_and(|n| n.contains("was not trusted")));
}

/// **Trust is asked of where Shannon started** (`App::start`) — the
/// directory the hook check reads, not a second reading of the process's:
/// the two must agree on where Claude Code runs.
#[test]
fn trust_is_asked_of_where_shannon_started() {
    let mut app = test_app();
    let dir = TempDir::new().unwrap();
    app.start = Some(dir.path().to_path_buf());
    let why = app
        .claude_code_refusal(Some("claude-code/sonnet"))
        .expect("a fresh directory is not trusted");
    let name = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(why.contains(&name), "names where it started: {why}");
}
