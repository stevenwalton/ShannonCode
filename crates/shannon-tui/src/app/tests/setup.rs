//! `:setup`: the first-run flow and its writes.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **`:setup` opens the menu, and leaving it changes nothing** — and while
/// it is open, keys and pastes are its own: nothing reaches the prompt behind.
#[tokio::test]
async fn setup_opens_takes_the_keys_and_leaves_without_changing_anything() {
    let mut app = test_app();
    let model = app.conversation().episode().config.model.clone();
    app.open_setup();
    app.setup_event_for_test(&Event::Paste("hello prompt".into()));
    press_setup(&mut app, KeyCode::Char('i'));
    assert!(
        !app.prompt().text().contains("hello"),
        "a paste behind setup reached the prompt"
    );
    // `Ctrl-L` still redraws: setup leaves it to the app, and stays open.
    let redraw = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);
    assert!(
        !app.setup_event_for_test(&Event::Key(redraw)),
        "Ctrl-L was eaten"
    );
    // …and a resize still reaches the layout.
    assert!(
        !app.setup_event_for_test(&Event::Resize(80, 24)),
        "a resize was eaten"
    );
    assert!(app.setup().is_some());
    press_setup(&mut app, KeyCode::Esc);
    assert!(app.setup().is_none());
    assert_eq!(app.conversation().episode().config.model, model);
    assert!(app.only_stub());
    assert!(app.notice().is_some_and(|n| n.contains("nothing changed")));
}

/// **The key the API answers with can be saved — an `ANTHROPIC_API_KEY`
/// included** (Steven, 10-04): Save with nothing pasted writes the key in
/// use, owner-only; after that, setup says it is saved.
#[tokio::test]
async fn setup_saves_the_key_in_use_and_then_says_it_is_saved() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    let env_key = "sk-ant-api03-FROM-THE-ENVIRONMENT";
    app.set_key_in_use(
        env_key.into(),
        crate::keys::KeyFrom::Var("ANTHROPIC_API_KEY"),
    );
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('1'));
    let said = screen(&mut app, 100);
    assert!(
        said.contains("in use: the key from ANTHROPIC_API_KEY"),
        "{said}"
    );
    assert!(!said.contains("FROM-THE-ENVIRONMENT"), "{said}");
    press_setup(&mut app, KeyCode::Down);
    press_setup(&mut app, KeyCode::Enter);
    assert!(
        app.setup().is_none(),
        "saved and closed: {:?}",
        app.notice()
    );
    let saved = shannon_core::config::api_key_beside(&dir.path().join("config.toml"));
    assert_eq!(std::fs::read_to_string(&saved).unwrap().trim(), env_key);
    assert_eq!(
        std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // Now it is the saved key: saving it again would change nothing.
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('1'));
    assert!(screen(&mut app, 100).contains("saved there already"));
}

/// **The API path: the key is saved owner-only, never shown, and answers at
/// once** — no relaunch. Chosen from the menu, the session switches to it.
#[tokio::test]
async fn setup_saves_a_key_owner_only_and_the_api_answers_at_once() {
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    let secret = "sk-ant-api03-NEVER-SHOWN-ANYWHERE";
    // An old key file left readable: the new one must not inherit that.
    let saved = shannon_core::config::api_key_beside(&dir.path().join("config.toml"));
    std::fs::write(&saved, "old\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&saved, std::fs::Permissions::from_mode(0o644)).unwrap();
    let logs = captured_logs(|| {
        app.open_setup();
        press_setup(&mut app, KeyCode::Char('1'));
        app.setup_event_for_test(&Event::Paste(secret.into()));
        press_setup(&mut app, KeyCode::Enter);
    });
    assert!(app.setup().is_none(), "saved and closed");
    assert_eq!(std::fs::read_to_string(&saved).unwrap().trim(), secret);
    assert_eq!(
        std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!app.only_stub(), "the API answers now, without a relaunch");
    assert_eq!(
        app.conversation().episode().config.model,
        "recording-stub-default",
        "the session switched to the API's model"
    );
    let notices = app.notice().unwrap_or_default();
    assert!(!notices.contains("NEVER-SHOWN"), "{notices}");
    assert!(!logs.contains("NEVER-SHOWN"), "{logs}");
    assert!(
        !dir.path().join("config.toml").exists(),
        "the API needs no [tui] model: nothing to write"
    );
}

/// **Claude Code from the menu: checked, then written to `[tui] model`** —
/// that key alone, the user's comments kept — and the session switches.
#[tokio::test]
async fn setup_chooses_claude_code_and_writes_only_that_key() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "# mine, keep me\n[exec]\ntimeout_secs = 99 # mine too\n",
    )
    .unwrap();
    let mut app = app_configured_in(dir.path());
    app.claude_program = logged_in_claude(dir.path());
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('2'));
    press_setup(&mut app, KeyCode::Enter);
    assert!(
        trust_screen_up(&app),
        "ready, so Enter finished it — and asks about this directory now, not at the first send"
    );
    let written = std::fs::read_to_string(&config).unwrap();
    assert!(written.contains("# mine, keep me"), "{written}");
    assert!(
        written.contains("timeout_secs = 99 # mine too"),
        "{written}"
    );
    assert!(
        written.contains("model = \"claude-code/sonnet\""),
        "{written}"
    );
    assert_eq!(
        app.config.tui.model, "claude-code/sonnet",
        "taken in, as :reload would"
    );
    assert_eq!(
        app.conversation().episode().config.model,
        "claude-code/sonnet"
    );
    // Your file's now, not a `:set`: `:config save` does not write setup's
    // choice into a project's file (review 2026-10-13, A).
    let typed = app.config_stack.typed_for_project(&app.config);
    assert!(typed.values.is_empty(), "{typed:?}");
}

/// **Claude Code, then back to the API, lands on the API** (Steven, 10-04):
/// the API choice takes `[tui] model` OUT of the file, and the running value
/// must follow — taking the file in as `:reload` does is additive, kept the
/// removed `claude-code/sonnet`, and switched the session straight back to it.
#[tokio::test]
async fn setup_from_claude_code_back_to_the_api_lands_on_the_api() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("config.toml");
    let mut app = app_configured_in(dir.path());
    app.claude_program = logged_in_claude(dir.path());
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('2'));
    press_setup(&mut app, KeyCode::Enter);
    assert_eq!(
        app.conversation().episode().config.model,
        "claude-code/sonnet"
    );

    app.open_setup();
    press_setup(&mut app, KeyCode::Char('1'));
    app.setup_event_for_test(&Event::Paste("sk-ant-api03-x".into()));
    press_setup(&mut app, KeyCode::Enter);
    assert_eq!(
        app.config.tui.model, "",
        "the running value follows the file"
    );
    assert!(
        !std::fs::read_to_string(&config)
            .unwrap()
            .contains("model =")
    );
    assert_eq!(
        app.conversation().episode().config.model,
        "recording-stub-default",
        "the session is on the API's model, not the Claude Code one it left"
    );

    // And choosing the API again keeps the key in use: nothing to paste.
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('1'));
    press_setup(&mut app, KeyCode::Enter);
    assert!(app.setup().is_none(), "Enter kept the key in use");
}

/// **The session takes the CHOICE, whatever happened to the config** — with
/// `[tui] model` unwritable it switched to the old default the unchanged
/// config still named, under a notice saying the choice answered; and a switch
/// refused by a turn in flight is said, not announced as done (review 21,
/// Reviewer A).
#[tokio::test]
async fn setup_switches_the_session_by_the_choice_not_the_config() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app();
    // A config "file" that is a directory: unreadable, so nothing is written.
    app.user_config = Ok(dir.path().to_path_buf());
    app.claude_program = logged_in_claude(dir.path());
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('2'));
    press_setup(&mut app, KeyCode::Enter);
    assert_eq!(
        app.conversation().episode().config.model,
        "claude-code/sonnet",
        "the chosen provider, not the config's old default"
    );

    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    app.claude_program = logged_in_claude(dir.path());
    let before = app.conversation().episode().config.model.clone();
    app.fake_stream();
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('2'));
    press_setup(&mut app, KeyCode::Enter);
    assert_eq!(
        app.conversation().episode().config.model,
        before,
        "a turn is running"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("stays on"), "{said}");
    assert!(!said.contains("set up: Claude Code answers"), "{said}");
}

/// **Setup opened by a model switch makes THAT model work, and changes
/// nothing else** — no `[tui] model` written, the session keeps the model it
/// asked for (`Entry::Model`; review 21, Reviewer C: the Menu/Model split was
/// held by nothing).
#[tokio::test]
async fn setup_from_a_model_switch_writes_no_default() {
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    app.model_command("claude-code/opus");
    assert!(
        app.setup().is_some(),
        "premise: not installed, so setup opened"
    );
    app.claude_program = logged_in_claude(dir.path());
    press_setup(&mut app, KeyCode::Char('r'));
    press_setup(&mut app, KeyCode::Enter);
    assert!(trust_screen_up(&app), "set up; the trust question follows");
    assert!(
        !dir.path().join("config.toml").exists(),
        "no default written"
    );
    assert_eq!(
        app.conversation().episode().config.model,
        "claude-code/opus"
    );
}

/// **Leaving a setup that a model switch opened says where the session is** —
/// the switch already happened, so "nothing changed" would be false (review
/// 21, Reviewer C).
#[tokio::test]
async fn leaving_setup_from_a_model_switch_names_the_model_it_stays_on() {
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    app.model_command("claude-code/opus");
    assert!(
        app.setup().is_some(),
        "premise: not installed, so setup opened"
    );
    press_setup(&mut app, KeyCode::Esc);
    assert!(app.setup().is_none());
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("`claude-code/opus`, which is not set up"),
        "{said}"
    );
    assert!(!said.contains("nothing changed"), "{said}");
}

/// **A session already answering from the chosen provider keeps its model**
/// — choosing the API from the menu does not move an `opus` session to the
/// default (`answers_from`; review 21, Reviewer C).
#[tokio::test]
async fn setup_keeps_a_session_already_on_the_chosen_provider() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_backed(
        Arc::new(shannon_core::backend::RecordingStub::new()),
        Arc::new(Config::default()),
    );
    app.user_config = Ok(dir.path().join("config.toml"));
    app.model_command("opus");
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('1'));
    press_setup(&mut app, KeyCode::Enter);
    assert!(app.setup().is_none(), "the key in use was kept");
    assert_eq!(
        app.conversation().episode().config.model,
        shannon_core::backend::models::resolve("opus")
    );
}

/// **A key the API client cannot start with is never written** — the client
/// is built first; setup stays open and says nothing was saved (review 21,
/// Reviewer C).
#[tokio::test]
async fn a_key_the_api_client_refuses_is_not_saved() {
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    app.make_api = |_, _| Err(anyhow::anyhow!("no client"));
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('1'));
    app.setup_event_for_test(&Event::Paste("sk-ant-api03-x".into()));
    press_setup(&mut app, KeyCode::Enter);
    assert!(app.setup().is_some(), "still open, saying why");
    assert!(
        screen(&mut app, 100).contains("nothing saved"),
        "{}",
        screen(&mut app, 100)
    );
    let saved = shannon_core::config::api_key_beside(&dir.path().join("config.toml"));
    assert!(!saved.exists());
    assert!(app.only_stub());
}

/// The demo writes `stub`, and the session answers from it.
#[tokio::test]
async fn setup_demo_mode_is_the_stub_remembered() {
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    app.open_setup();
    press_setup(&mut app, KeyCode::Char('3'));
    assert!(app.setup().is_none());
    let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(written.contains("model = \"stub\""), "{written}");
    assert_eq!(
        app.conversation().episode().config.model,
        shannon_core::backend::models::STUB
    );
}

/// **A model that is not set up opens setup on what it needs** — and
/// finishing it makes THAT model work, changing nothing else: no
/// `[tui] model`, the session keeps the model it asked for.
#[tokio::test]
async fn a_model_switch_opens_setup_on_what_that_model_needs() {
    let dir = TempDir::new().unwrap();
    let mut app = app_configured_in(dir.path());
    app.model_command("opus");
    assert!(app.setup().is_some(), "opus needs a key, and there is none");
    app.setup_event_for_test(&Event::Paste("sk-ant-api03-x".into()));
    press_setup(&mut app, KeyCode::Enter);
    assert!(app.setup().is_none());
    assert!(!app.only_stub());
    assert_eq!(
        app.conversation().episode().config.model,
        shannon_core::backend::models::resolve("opus")
    );
    assert!(
        !dir.path().join("config.toml").exists(),
        "no default chosen"
    );

    // Claude Code: not installed here, so setup opens on it, saying so.
    let mut app = test_app();
    app.model_command("claude-code/sonnet");
    let shown = screen(&mut app, 100);
    assert!(shown.contains("not installed"), "{shown}");
    assert!(shown.contains("claude-code/sonnet"), "{shown}");
    // …and logged in, setup does not open: only the trust question.
    let dir = TempDir::new().unwrap();
    let mut app = test_app();
    app.claude_program = logged_in_claude(dir.path());
    app.model_command("claude-code/sonnet");
    assert!(trust_screen_up(&app));
}

/// **Setup is the whole frame while it is open** — what is drawn and what
/// takes keys ask one question, so nothing behind it may look answerable.
#[tokio::test]
async fn setup_is_drawn_over_everything() {
    let mut app = test_app();
    app.open_setup();
    let shown = screen(&mut app, 100);
    assert!(
        shown.contains("How should Shannon reach a model?"),
        "{shown}"
    );
    assert!(shown.contains("Fully featured."), "{shown}");
    assert!(shown.contains("Limited."), "{shown}");
    assert!(
        !shown.contains("NORMAL") && !shown.contains("INSERT"),
        "{shown}"
    );
    press_setup(&mut app, KeyCode::Esc);
    assert!(!screen(&mut app, 100).contains("How should Shannon reach a model?"));
}

/// **`[tui] model` edited in place**: the choice written, a comment kept, the
/// API's removal of a Claude Code default, an API model left alone, a file
/// that does not parse refused without quoting it.
#[test]
fn the_provider_is_written_to_tui_model_and_nothing_else() {
    use crate::app::setup_flow::with_provider_model;
    use crate::setup::Choice;
    let file = "# top\n[tui]\nmouse = true # keep\n";
    let cc = with_provider_model(file, Choice::ClaudeCode)
        .unwrap()
        .unwrap();
    assert!(
        cc.contains("# top") && cc.contains("mouse = true # keep"),
        "{cc}"
    );
    assert!(cc.contains("model = \"claude-code/sonnet\""), "{cc}");
    assert_eq!(
        with_provider_model(&cc, Choice::ClaudeCode).unwrap(),
        None,
        "already"
    );
    let opus = "[tui]\nmodel = \"claude-code/opus\"\n";
    assert_eq!(with_provider_model(opus, Choice::ClaudeCode).unwrap(), None);
    let api = with_provider_model(&cc, Choice::Api).unwrap().unwrap();
    assert!(!api.contains("model ="), "{api}");
    assert!(api.contains("mouse = true # keep"), "{api}");
    assert_eq!(
        with_provider_model("[tui]\nmodel = \"opus\"\n", Choice::Api).unwrap(),
        None,
        "an API model stays"
    );
    let demo = with_provider_model("", Choice::Demo).unwrap().unwrap();
    assert!(demo.contains("model = \"stub\""), "{demo}");
    let broken = "[[alias]]\nvalue = \"hunter2-secret\nunclosed";
    let err = with_provider_model(broken, Choice::Demo).unwrap_err();
    assert!(!err.contains("hunter2"), "{err}");
    // A `tui` that parses but is not a table is said, not a panic (review
    // 21, Reviewer B); an inline table is one.
    for odd in [
        "tui = \"x\"\n",
        "tui = [1]\n",
        "tui = 3\n",
        "[[tui]]\nmodel = \"x\"\n",
    ] {
        for choice in [Choice::ClaudeCode, Choice::Demo] {
            let err = with_provider_model(odd, choice).unwrap_err();
            assert!(err.contains("not a table"), "{odd:?}: {err}");
        }
    }
    let inline = with_provider_model("tui = { model = \"opus\" }\n", Choice::Demo)
        .unwrap()
        .unwrap();
    assert!(inline.contains("\"stub\""), "{inline}");
}
