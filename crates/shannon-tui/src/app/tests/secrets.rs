//! Secrets: lent, marked, vaulted, recalled — and never shown or sent.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- lent secrets (P3) ---------------------------------------------------

/// A model that is not told the handles cannot spend a secret at all, so the
/// tool description is load-bearing rather than documentation. It must also
/// say the value is unavailable — a model that thinks it could look will
/// waste a turn trying. Read off the request: the test built the
/// description itself, so a turn sent the bare one passed it (review stop
/// 12, T2 3).
#[tokio::test]
async fn the_model_is_told_which_secrets_it_may_spend() {
    let mut aliases = shannon_core::Aliases::default();
    let handle =
        (aliases.insert("gh", shannon_core::fakes::GITHUB_TOKEN, "test", true)).expect("mint");
    let (mut app, backend) = recorded_holding(aliases);
    app.submit("go");
    let desc = sent_tool_description(&backend);
    assert!(desc.contains(&handle), "{desc}");
    assert!(desc.contains("USE but never SEE"), "{desc}");
    assert!(
        !desc.contains("ghp_pretend"),
        "the VALUE never appears: {desc}"
    );

    // And an empty vault leaves the description exactly as it was — no
    // dangling paragraph about a feature nobody is using.
    let (mut plain, backend) = recorded_holding(shannon_core::Aliases::default());
    plain.submit("go");
    assert_eq!(
        sent_tool_description(&backend),
        shannon_core::ToolDef::foreground()[0].description
    );
}

/// A command carrying a handle always opens the gate, and the explanation
/// names the secret. This is the confused-deputy guard reaching the screen —
/// a refusal only the log knows about reads as a dead key.
#[test]
fn spending_a_secret_opens_the_gate_and_says_which() {
    let (mut app, handle) = app_with_secret("gh", shannon_core::fakes::GITHUB_TOKEN);
    app.arrive_and_gate_for_test(app.foreground_id(), "t1", &format!("echo {handle}"));
    let prompt = app.shown_call().expect("the gate opened");
    let text = prompt
        .text()
        .lines()
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("spends"), "{text}");
    assert!(text.contains(&handle), "{text}");
}

/// The user is told at startup when a declared secret is not actually set.
/// Silence here looks exactly like the feature being broken, and sends
/// someone hunting in the wrong place.
#[test]
fn an_unset_declared_secret_is_announced() {
    let mut cfg = Config::default();
    cfg.secrets.env = vec!["SHANNON_TEST_DEFINITELY_UNSET_VAR".to_string()];
    let app = test_app_with(cfg);
    let notice = app.notice().expect("a notice was raised");
    assert!(
        notice.contains("SHANNON_TEST_DEFINITELY_UNSET_VAR"),
        "{notice}"
    );
    assert!(notice.contains("not set"), "{notice}");
    assert_eq!(app.aliases.secrets().count(), 0, "nothing was lent");
}

/// `:alias` pairs each handle with **the value it stands for**, its source
/// and whether it is a secret. Secrets are hidden from the model, not from
/// the user, who reads here what they hid (Steven, 2026-10-01). One ROW per
/// entry, its columns asserted on that row: a substring of the whole listing
/// would pass with one entry's value on another's line.
#[test]
fn the_secrets_listing_pairs_each_handle_with_its_value() {
    let row = |listed: &str, handle: &str| -> String {
        listed
            .lines()
            .find(|l| l.contains(handle))
            .unwrap_or_else(|| panic!("no row for {handle}: {listed}"))
            .to_string()
    };
    let (app, handle) = app_with_secret("gh", shannon_core::fakes::GITHUB_TOKEN);
    let listed = app.aliases_listing();
    let gh = row(&listed, &handle);
    assert!(
        gh.contains(" secret ") && gh.contains(" test "),
        "kind and source: {gh}"
    );
    assert!(gh.ends_with(shannon_core::fakes::GITHUB_TOKEN), "{gh}");

    // A plain alias is listed too, as an alias, with its value.
    let (app, handle) = app_holding("repo", "/Users/me/proj", false);
    let listed = app.aliases_listing();
    let repo = row(&listed, &handle);
    assert!(
        repo.contains(" alias ") && !repo.contains("secret"),
        "{repo}"
    );
    assert!(repo.ends_with("/Users/me/proj"), "{repo}");

    // A default session is not empty — `$HOME` is aliased out of the box.
    assert!(test_app().aliases_listing().contains("$HOME"));
}

/// **`:alias` is a live pane**, as `:ls` is: opened by the command, and a
/// secret marked while it is open appears in it without a keypress — with
/// its value, its source, and `--no-spend` said where it applies. It was one
/// notice line the status row cut off after two entries (Steven,
/// 2026-10-01). The value is in the PANE only: a notice is logged.
#[test]
fn alias_opens_a_live_pane_that_shows_a_new_secret() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("secrets");
    let pane = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Aliases))
    );
    let shown = |app: &App| -> Vec<String> {
        app.pane_listing(pane)
            .expect("a listing")
            .lines()
            .iter()
            .map(|l| l.text())
            .collect()
    };
    assert!(
        !shown(&app).iter().any(|l| l.contains("marked by you")),
        "premise"
    );

    let handle = app
        .mark_secret("correct horse battery", false)
        .expect("marked");
    app.before_frame(100, 40);

    let row = shown(&app)
        .into_iter()
        .find(|l| l.contains(&handle))
        .expect("the new secret has a row");
    assert!(
        row.contains("no (--no-spend)") && row.contains("marked by you"),
        "{row}"
    );
    assert!(
        row.ends_with("correct horse battery"),
        "beside its handle: {row}"
    );

    // Opened again with the secret held: the pane, never a notice.
    app.dispatch_command_for_test("secrets");
    assert!(
        !app.messages().any(|m| m.contains("correct horse")),
        "a value reached the notices, which the log records"
    );
}

/// A plain alias resolves **at the gate**, so the human reads the command
/// that will really run — and, critically, so does the analyzer.
#[test]
fn a_plain_alias_is_resolved_before_the_human_sees_it() {
    let (mut app, handle) = app_holding("repo", "/tmp/shannon-alias-probe", false);
    app.arrive_and_gate_for_test(app.foreground_id(), "t1", &format!("rm -rf {handle}/build"));
    let prompt = app.shown_call().expect("gated: rm is destructive");
    assert_eq!(prompt.command(), "rm -rf /tmp/shannon-alias-probe/build");
    assert!(!prompt.command().contains("{{a:"), "no placeholder shown");
}

/// **Why gate-time.** If the alias were resolved after analysis, the operand
/// would be an unresolved `{{…}}` token and a path-scoped `deny` naming the
/// real directory would never match — an alias would quietly launder a path
/// past a rule. Resolving first closes that.
#[test]
fn an_alias_cannot_launder_a_path_past_a_deny_rule() {
    let (mut app, handle) = app_holding("etc", "/etc", false);
    app.rules = shannon_core::permission::RuleSet::defaults().with_aliases(app.aliases.clone());
    app.rules
        .extend_from_lines("deny rm /etc/*")
        .expect("rule parses");
    // Premise: the rule really does fire on the spelled-out path.
    assert_eq!(
        app.rules.decide_line("rm /etc/passwd").decision,
        shannon_core::permission::Decision::Deny
    );
    // And it still fires when the path arrives via an alias.
    let expanded = app
        .aliases
        .expand_at_gate(&format!("rm {handle}/passwd"))
        .unwrap();
    assert_eq!(
        app.rules.decide_line(&expanded.text).decision,
        shannon_core::permission::Decision::Deny,
        "the alias must not weaken the rule"
    );
}

/// **A handle this session never minted is refused on the record, with
/// why** — the record holding what the model was told under it, as a
/// reopened turn draws the stored result (Steven, 11-01: a refusal's reason
/// was in the reopened record only).
#[tokio::test]
async fn an_unknown_handle_is_refused_on_the_record_with_why() {
    let mut app = test_app();
    model_call(&mut app, "tu_x", "rm -rf {{a:bogus}}/");
    let record = (app.output().spans())
        .find(|s| s.role == crate::output::BlockRole::Tool)
        .map(|s| s.lines.to_vec())
        .expect("its record");
    assert!(
        record[0].starts_with("⚙ REFUSED — nothing ran"),
        "{record:#?}"
    );
    let (_, told, refused) = (tool_results(&app).into_iter())
        .find(|(id, _, _)| id == "tu_x")
        .expect("answered");
    assert!(refused, "premise: refused");
    for line in told.lines() {
        assert!(
            record.iter().any(|l| l.ends_with(line)),
            "{line:?}: {record:#?}"
        );
    }
}

/// **`:analyze` sees the same line the gate would.**
///
/// The test above proves the *gate* cannot be laundered by an alias. `:analyze`
/// is the preview a user consults to decide what to trust, and it passed its
/// argument RAW — so it could show a benign verdict for a command the gate
/// would refuse, which is a trust inversion in the one surface whose whole job
/// is to be trusted (Reviewer E).
///
/// Asserted on the rendered report, because that is what the user reads, and
/// on the resolved path appearing in it — a report that still says `{{a:etc}}`
/// is one the gate never saw.
#[test]
fn analyze_resolves_aliases_the_way_the_gate_does() {
    let (mut app, handle) = app_holding("etc", "/etc", false);
    app.rules = shannon_core::permission::RuleSet::defaults().with_aliases(app.aliases.clone());
    app.rules
        .extend_from_lines("deny rm /etc/*")
        .expect("rule parses");
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();

    app.open_analyze_command(&format!("rm {handle}/passwd"));

    let shown = app
        .pane_page(here)
        .expect("a report")
        .text()
        .lines()
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        shown.contains("/etc/passwd"),
        "the preview analysed the resolved line, not the alias text: {shown}"
    );
    assert!(
        !shown.contains(&handle),
        "so the handle is gone by the time the analyzer sees it: {shown}"
    );
}

/// A secret is NOT resolved at the gate: the value must never be displayed.
/// Same axis as above, opposite answer.
#[test]
fn a_secret_is_still_a_placeholder_at_the_gate() {
    let (mut app, handle) = app_with_secret("gh", shannon_core::fakes::GITHUB_TOKEN);
    app.arrive_and_gate_for_test(app.foreground_id(), "t1", &format!("echo {handle}"));
    let prompt = app.shown_call().expect("gated");
    assert_eq!(prompt.command(), format!("echo {handle}"));
    assert!(!prompt.command().contains("ghp_pretend"), "never shown");
}

/// An unknown alias refuses visibly rather than running something else. A
/// refusal only the log knows about reads as a dead key.
#[test]
fn an_unknown_alias_refuses_and_says_so() {
    let mut app = test_app();
    app.arrive_and_gate_for_test(app.foreground_id(), "t1", "rm -rf {{a:typo}}/build");
    assert!(app.shown_call().is_none(), "nothing to approve");
    let notice = app.notice().expect("told the user");
    assert!(
        notice.contains("not an alias this session holds"),
        "{notice}"
    );
    assert!(app.running_tools.is_empty(), "nothing was started");
}

/// `$HOME` is aliased to `~` **by default**, because an absolute
/// `/Users/you/...` is machine-specific and true nowhere else: sending it
/// teaches the model a fact that does not travel. Privacy and token savings
/// come along free. (Steven, 2026-07-26.)
#[test]
fn home_is_aliased_to_a_portable_tilde_by_default() {
    let app = test_app();
    let home = std::env::var("HOME").expect("a home directory");
    let home = home.trim_end_matches('/');

    // Outbound: the absolute path leaves as `~`, and the username with it.
    let out = app.scrub(&format!("compiling {home}/proj/src/main.rs"));
    assert_eq!(out, "compiling ~/proj/src/main.rs", "{out}");
    assert!(out.len() < format!("compiling {home}/proj/src/main.rs").len());

    // Inbound: `~` resolves before the gate analyses anything — including
    // inside quotes, where bash would NOT have expanded it. That case only
    // exists because our own masking put `~` in front of the model.
    let back = app
        .aliases
        .expand_at_exec("cat \"~/My Documents/x\"")
        .expect("resolves");
    assert_eq!(back.text, format!("cat \"{home}/My Documents/x\""));
}

/// Turning it off is honoured — someone may prefer absolute paths.
#[test]
fn home_aliasing_can_be_switched_off() {
    let mut cfg = Config::default();
    cfg.secrets.alias_home = false;
    let app = test_app_with(cfg);
    let home = std::env::var("HOME").expect("a home directory");
    let line = format!("compiling {}/proj/x.rs", home.trim_end_matches('/'));
    assert_eq!(app.scrub(&line), line);
}

/// **The display rule:** expand at the gate exactly what a human cannot
/// read, and nothing else. A minted `{{a:repo}}` means nothing without
/// us, so it resolves; `~` is universally legible, so expanding it would
/// trade a short portable path for a long machine-specific one and buy the
/// reader nothing.
#[tokio::test]
async fn the_gate_expands_only_what_a_human_cannot_read() {
    let (mut app, minted) = app_holding("repo", "/tmp/shannon-display-probe", false);
    // As the model's call arrives: the gate called by hand skipped the claim
    // and the showing (review stop 12, T4 5).
    model_call(&mut app, "t1", &format!("rm -rf ~/logs {minted}/build"));
    let shown = app.shown_call().expect("gated").command();
    assert_eq!(
        shown, "rm -rf ~/logs /tmp/shannon-display-probe/build",
        "{shown}"
    );
}

/// ...and the shell still gets the real path, because `~` resolves at exec.
/// Display and execution differ here on purpose, and only where the two
/// forms mean the same thing to a reader.
#[test]
fn the_tilde_the_human_read_still_resolves_before_running() {
    let app = test_app();
    let home = std::env::var("HOME").expect("a home directory");
    let home = home.trim_end_matches('/');
    let ran = app
        .aliases
        .expand_at_exec("rm -rf ~/logs")
        .expect("resolves");
    assert_eq!(ran.text, format!("rm -rf {home}/logs"));
}

// --- security review fixes ------------------------------------------------

/// **F2.** A remembered `A` grant was a third source of allow authority that
/// consulted neither override, and one `A` on a secret-spending call spent
/// the credential with no pane and no keypress, forever. `A` writes a rule
/// now, so the same guarantee is asked of a rule: a taught allow covering the
/// command still cannot spend a secret unattended.
#[test]
fn a_taught_allow_cannot_auto_run_a_secret_spend() {
    let (mut app, handle) = app_with_secret("gh", shannon_core::fakes::GITHUB_TOKEN);
    let command = format!("TOKEN={handle} env");
    app.dispatch_command_for_test("allow env *");
    app.arrive_and_gate_for_test(app.foreground_id(), "t1", &command);
    assert!(app.shown_call().is_some(), "it still faces a human");
    assert!(app.running_tools.is_empty(), "and nothing ran");
}

/// The same for a sensitive path spelled absolutely — which reads as a
/// perfectly "stable" command, so `A` was offered for it.
#[test]
fn a_taught_allow_cannot_auto_run_a_credential_read() {
    let mut app = test_app();
    let home = std::env::var("HOME").expect("a home directory");
    let command = format!("cat {}/.ssh/id_rsa", home.trim_end_matches('/'));
    app.dispatch_command_for_test("allow cat *");
    app.arrive_and_gate_for_test(app.foreground_id(), "t1", &command);
    assert!(app.shown_call().is_some(), "it still faces a human");
    assert!(app.running_tools.is_empty(), "and nothing ran");
}

/// Belt and braces: `A` has no rule where the answer must be asked again
/// every time, so it never proposes one, and the legend does not offer it.
#[test]
fn always_allow_is_not_offered_for_a_reason_that_must_be_rechecked() {
    let (app, handle) = app_with_secret("gh", shannon_core::fakes::GITHUB_TOKEN);
    let spend =
        crate::permission_view::ToolPrompt::of("t", &format!("curl -H {handle} x"), &app.rules);
    assert!(spend.proposal().is_err(), "no standing rule for a spend");
    let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(spend.resolve(&a).is_some(), "allow-once is still offered");

    // An ordinary destructive command still offers it.
    let plain = crate::permission_view::ToolPrompt::of("t", "rm -r /tmp/x", &app.rules);
    assert_eq!(plain.proposal().as_deref(), Ok("rm -r"));
}

/// **F5.** The edit path is the one place a HUMAN types a handle, and it
/// used to gate without expanding — so `:alias` advertised handles that
/// silently did not work, and an unminted one was never refused there.
///
/// **Driven as `e` and `$EDITOR`'s return** (`apply_edit_for_test`). This
/// called `resolve_for_gate` and `re_gate` itself, so the edit path's own
/// resolve could be bypassed with the suite green (review stop 12, T2 2).
#[tokio::test]
async fn an_edited_command_gets_the_same_expansion_as_the_models() {
    let (mut app, handle) = app_holding("repo", "/tmp/shannon-edit-probe", false);
    model_call(&mut app, "t1", "rm -rf /tmp/shannon-edit-probe/old");
    assert!(app.shown_call().is_some(), "premise: gated");
    let (seed, _) = app.edit_seed_for_test().expect("`e` opens the call");
    app.apply_edit_for_test(&seed, &format!("rm -rf {handle}/new"));
    assert_eq!(
        app.shown_call().expect("still asking").command(),
        "rm -rf /tmp/shannon-edit-probe/new",
        "re-gated on what the handle stands for"
    );

    // An unminted one is refused, said, and the call is left as it was.
    let (seed, _) = app.edit_seed_for_test().expect("`e` opens it again");
    app.apply_edit_for_test(&seed, "rm -rf {{a:typo}}/x");
    let notice = app.notice().map(|n| n.to_string()).unwrap_or_default();
    assert!(
        notice.contains("not an alias this session holds"),
        "{notice}"
    );
    assert_eq!(
        app.shown_call().expect("still asking").command(),
        "rm -rf /tmp/shannon-edit-probe/new"
    );
}

/// **A deny holds against a handle typed in an edit** — decided on what
/// the handle stands for: raw, `cat {{etc.…}}/shadow` names no `/etc`, and
/// a `deny cat /etc/*` passes it by (T2 2, probed).
#[tokio::test]
async fn a_deny_holds_against_a_handle_typed_in_an_edit() {
    let (mut app, handle) = app_holding("etc", "/etc", false);
    app.dispatch_command_for_test("deny cat /etc/*");
    model_call(&mut app, "t1", "rm -rf /tmp/x");
    let (seed, _) = app.edit_seed_for_test().expect("`e` opens the call");
    app.apply_edit_for_test(&seed, &format!("cat {handle}/shadow"));
    let shown = (app.shown_call()).map(|p| (p.command().to_string(), p.decision()));
    assert_eq!(
        shown,
        Some((
            "cat /etc/shadow".to_string(),
            shannon_core::permission::Decision::Deny
        ))
    );
}

// --- user-marked secrets --------------------------------------------------

/// **The property the whole feature exists for**, and it is a property of the
/// BOUNDARY rather than of the buffer: what leaves carries the handle, what
/// the user sees carries their own words.
#[tokio::test]
async fn a_marked_value_leaves_as_a_handle_but_stays_on_screen() {
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).expect("marked");

    app.submit(&format!("my password is {secret} ok"));

    // What LEFT — the conversation is both the wire and the store.
    let sent = app
        .conversation()
        .messages()
        .iter()
        .rev()
        .find(|m| m.role == shannon_core::Role::User)
        .expect("a user message")
        .content
        .clone();
    assert!(!sent.contains(secret), "the value left: {sent}");
    assert_eq!(sent, format!("my password is {handle} ok"));
}

/// Marking does not touch the buffer at all, so there is nothing to undo and
/// no editing key can "un-protect" a value by accident. That footgun — `u`
/// and `<C-r>` toggling protection — is gone by construction.
#[test]
fn marking_leaves_the_prompt_alone() {
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    app.prompt_mut()
        .insert_pasted(&format!("my password is {secret}"));
    let before = app.prompt().text();
    app.mark_secret(secret, true).expect("marked");
    assert_eq!(app.prompt().text(), before, "the prompt is untouched");
}

/// **A sentence is a secret like any other** (Steven asked, 2026-10-01).
/// `:secret` takes the rest of the line verbatim — spaces, punctuation,
/// non-ASCII — and only `--` is needed before text that starts with a dash,
/// which the flag grammar would otherwise read as a flag. Through the command,
/// to the wire: the tests above mark single tokens through `mark_secret`.
#[tokio::test]
async fn a_whole_sentence_can_be_a_secret() {
    for (typed, value) in [
        (
            "secret the eagle lands at noon, café #3!",
            "the eagle lands at noon, café #3!",
        ),
        (
            "secret -- -first a dash, then words",
            "-first a dash, then words",
        ),
    ] {
        let mut app = test_app();
        app.prompt_mut()
            .insert_pasted(&format!("tell them: {value} (ok)"));
        app.notices.clear();
        app.dispatch_command_for_test(typed);
        assert!(
            app.notice().unwrap_or_default().contains("marked"),
            "`:{typed}`: {:?}",
            app.notice()
        );
        let text = app.prompt_mut().take();
        app.submit(&text);
        let sent = app
            .conversation()
            .messages()
            .iter()
            .rev()
            .find(|m| m.role == shannon_core::Role::User)
            .expect("a user message")
            .content
            .clone();
        assert!(
            !sent.contains(value),
            "`:{typed}`: the sentence left: {sent}"
        );
        assert!(
            sent.starts_with("tell them: ") && sent.ends_with(" (ok)"),
            "{sent}"
        );
    }
}

/// **A value can be marked before it is typed.** `:secret hunter2` on an
/// empty prompt was refused (Steven, 2026-10-01); it marks now, says the
/// text is not in the prompt yet — the visible half of the old refusal, which
/// guarded a typo'd mark — and masks the value when it is typed later.
#[tokio::test]
async fn a_secret_can_be_marked_before_it_is_typed() {
    let mut app = test_app();
    assert!(app.prompt().text().is_empty(), "premise: nothing typed");
    app.notices.clear();
    app.dispatch_command_for_test("secret hunter2");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("marked") && notice.contains("not in the prompt yet"),
        "{notice}"
    );
    assert!(
        !notice.contains("hunter2"),
        "the notice names the handle only: {notice}"
    );
    // And what the mark cannot reach, said where it is made (review stop 14,
    // Reviewer E F3; Steven: "just do (d)").
    assert!(
        notice.contains("a copy in a file can still reach the model"),
        "{notice}"
    );

    app.submit("the password is hunter2");
    let sent = app
        .conversation()
        .messages()
        .iter()
        .rev()
        .find(|m| m.role == shannon_core::Role::User)
        .expect("a user message")
        .content
        .clone();
    assert!(!sent.contains("hunter2"), "masked when typed later: {sent}");

    // Present in the prompt: no "not yet" clause.
    let mut app = test_app();
    app.prompt_mut().insert_pasted("pw: swordfish");
    app.notices.clear();
    app.dispatch_command_for_test("secret swordfish");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("marked") && !notice.contains("not in the prompt"),
        "{notice}"
    );
}

/// A short password is still a password. Refusing `hunter2` because seven
/// characters is inconvenient for us was imposing our implementation on the
/// user's situation.
#[test]
fn a_short_password_can_be_marked() {
    let mut app = test_app();
    assert!(app.mark_secret("hunter2", true).is_ok());
}

/// **Saying what did not leave.** A styled value stops being evidence the
/// moment it scrolls off; the count survives, and it is what a user can
/// actually check. Silence would make a protected message indistinguishable
/// from one where the protection quietly stopped working.
#[tokio::test]
async fn submitting_says_how_many_secrets_were_withheld() {
    let mut app = test_app();
    app.mark_secret("hunter2beornot2be", true).unwrap();
    app.submit("my password is hunter2beornot2be");
    let notice = app.notice().expect("said something");
    assert!(notice.contains("1 secret withheld"), "{notice}");
    assert!(notice.contains("the value stayed here"), "{notice}");

    let mut app = test_app();
    app.mark_secret("hunter2beornot2be", true).unwrap();
    app.mark_secret("another-long-secret-value", true).unwrap();
    app.submit("hunter2beornot2be and another-long-secret-value");
    assert!(app.notice().unwrap().contains("2 secrets withheld"));

    let mut app = test_app();
    app.submit("an ordinary message");
    assert!(
        !app.notice().unwrap_or_default().contains("withheld"),
        "no claim when nothing was withheld"
    );

    // **A plain alias is a spelling, not a secret withheld** (Steven,
    // 10-23, image 7: "1 secret withheld (home)" in a conversation holding
    // no secret). The home's path still leaves as `~`; nothing is claimed.
    let home = std::env::var("HOME").expect("a home to name");
    let mut app = test_app();
    app.submit(&format!("look in {home}/proj"));
    let sent = (app.conversation().messages().last())
        .map(|m| m.content.clone())
        .unwrap_or_default();
    assert!(sent.contains("~/proj"), "premise: the home masked: {sent}");
    assert!(
        !app.notice().unwrap_or_default().contains("withheld"),
        "{:?}",
        app.notice()
    );
}

/// The name follows what it looks like, and carries **no decoration**.
/// `{{s:secret-1.2864}}` said "secret" twice and then numbered the only one
/// there was; an index earns its place only when there is something to tell
/// apart.
#[test]
fn a_marked_secret_is_named_for_what_it_is_and_nothing_more() {
    let mut app = test_app();
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    assert!(
        app.mark_secret(token, true)
            .unwrap()
            .starts_with("{{github-token."),
        "named after its kind"
    );

    let mut app = test_app();
    let first = app.mark_secret("some-long-passphrase-here", true).unwrap();
    assert!(first.starts_with("{{secret."), "no index for the only one");

    // A second of the same kind is where an index starts earning its place.
    let second = app.mark_secret("a-different-passphrase", true).unwrap();
    assert!(second.starts_with("{{secret-2."), "{second}");
    assert_ne!(first, second);
}

/// **The inbound half of the boundary, on the stub.** The model is sent
/// handles, so it echoes handles back — and the offline stub's echo shows the
/// value AND the handle, `value (handle)`, so a test sees what crossed the
/// wire (Steven, 2026-10-02; 10-19: "only in the stub"). A real model's reply
/// shows the value alone (`a_reply_shows_a_held_secret_as_its_value`). The
/// test app answers with the stub, so every straddle test here is the
/// stub's.
// `tokio::test`, because the straddle buffer now lives on a generation and
// `fake_stream` installs one (which spawns).
#[tokio::test]
async fn a_handle_in_the_reply_is_shown_with_its_value() {
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();

    let id = app.fake_stream();
    let shown = app.unmask_stream(id, &format!("you said {handle} to me"));
    assert_eq!(shown, format!("you said {secret} ({handle}) to me"));
}

/// **A reply shows a held secret as its value** (Steven, 10-19: "In chats we
/// should just show `hunter2` (the secret) in the marked text. No need to
/// show the hash") — the same colour as the user's own line, which says it
/// travelled as its handle. Live, as it streams in a session Claude Code
/// answers, and replayed from a turn sealed under a real model; the
/// stub's echo alone keeps `value (handle)`.
#[tokio::test]
async fn a_reply_shows_a_held_secret_as_its_value() {
    use shannon_core::graph::Turn;
    use shannon_core::message::Message;
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();
    assert!(app.switch_model(app.current, "claude-code/sonnet", "claude-code/sonnet"));
    let id = app.fake_stream();
    let shown = app.unmask_stream(id, &format!("you said {handle} to me"));
    assert_eq!(shown, format!("you said {secret} to me"), "live");

    let replayed = |model: &str| {
        let turn = Turn::seal(
            vec![
                Message::user("hi"),
                Message::assistant(format!("got {handle}")),
            ],
            shannon_core::TurnConfig {
                model: model.to_string(),
                max_tokens: None,
                effort: None,
            },
            std::time::SystemTime::UNIX_EPOCH,
        );
        let mut output = Output::new();
        App::replay_turn(&app.aliases, None, &mut output, &turn);
        output.lines().join("\n")
    };
    assert!(
        replayed("sonnet").contains(&format!("got {secret}\n")),
        "replayed"
    );
    assert!(!replayed("sonnet").contains(&handle), "no handle beside it");
    let stub = shannon_core::backend::models::STUB;
    assert!(
        replayed(stub).contains(&format!("got {secret} ({handle})")),
        "the stub's"
    );
}

/// **A value you marked secret is drawn as one on a call's pane** (review
/// stop 8, E2 5) — the pane drew it as plain text, the one surface where a
/// line holding it is approved; Claude Code's line holding it is one the
/// model wrote itself (the gate reads handles only), so `ui.secret.leaked`.
#[tokio::test]
async fn a_secret_in_a_calls_line_is_drawn_as_one_on_its_pane() {
    use crate::style::Ink;
    use ratatui::style::Modifier;
    let secret = shannon_core::fakes::PLACEHOLDER_KEY_1;
    let (mut app, _provider) = provider_app();
    app.mark_secret(secret, true).unwrap();
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": format!("echo {secret} > /tmp/shannon-test-x")}),
    );
    assert!(app.shown_call().is_some(), "premise: it asks");
    let buf = &frame_of(&mut app, 140, 40);
    let n = secret.chars().count();
    let styles: Vec<ratatui::style::Style> = (0..40u16)
        .flat_map(|y| {
            let row: Vec<&str> = (0..140u16).map(|x| buf[(x, y)].symbol()).collect();
            (0..row.len().saturating_sub(n))
                .filter(|&x| row[x..x + n].concat() == secret)
                .map(|x| buf[(x as u16, y)].style())
                .collect::<Vec<_>>()
        })
        .collect();
    let leaked = app.theme().ui(Ink::SecretLeaked);
    assert!(!styles.is_empty(), "premise: the value is on the pane");
    for style in styles {
        assert_eq!(style.fg, leaked.fg, "drawn as a secret the model wrote");
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }
}

/// **A range over a secret on a call's pane stays a range** — reversed, in
/// the secret's colour. The secret's colouring replaced the cell's style and
/// took the selection's reverse with it, where the prompt and the transcript
/// patch theirs over a secret (Reviewer D, F3).
#[tokio::test]
async fn a_range_over_a_secret_on_a_calls_pane_stays_reversed() {
    use crate::style::Ink;
    use ratatui::style::Modifier;
    let secret = shannon_core::fakes::PLACEHOLDER_KEY_2;
    let (mut app, _provider) = provider_app();
    app.mark_secret(secret, true).unwrap();
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": format!("echo {secret} > /tmp/shannon-test-x")}),
    );
    app.set_focus(PaneKind::Tool);
    type_keys(&mut app, "ggVG");
    let buf = &frame_of(&mut app, 140, 40);
    let n = secret.chars().count();
    let cells: Vec<ratatui::style::Style> = (0..40u16)
        .flat_map(|y| {
            let row: Vec<&str> = (0..140u16).map(|x| buf[(x, y)].symbol()).collect();
            (0..row.len().saturating_sub(n))
                .filter(|&x| row[x..x + n].concat() == secret)
                .flat_map(|x| (x..x + n).map(move |x| buf[(x as u16, y)].style()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(!cells.is_empty(), "premise: the value is on the pane");
    let leaked = app.theme().ui(Ink::SecretLeaked);
    for style in cells {
        assert_eq!(style.fg, leaked.fg, "still drawn as a secret");
        assert!(
            style.add_modifier.contains(Modifier::REVERSED),
            "and inside the range"
        );
    }
}

/// **A value the model wrote itself is drawn apart** (Steven, 10-19: "an
/// underline along with a color"; review stop 6, F 5). Put back from its
/// handle, a held value wears the secret's colour; written by the model —
/// it had the value, told before it was marked — it is underlined in
/// `ui.secret.leaked`. Your own line, which holds the value as you typed
/// it, is never a leak. Live, and replayed from the stored turn.
#[tokio::test]
async fn a_value_the_model_wrote_itself_is_drawn_apart() {
    use crate::style::Ink;
    use ratatui::style::Modifier;
    use shannon_core::graph::Turn;
    use shannon_core::message::Message;
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();
    app.start_turn(&format!("mine is {secret}"));
    app.feed_stream(StreamMsg::Chunk(format!(
        "put back: {handle} / wrote: {secret}"
    )));
    let buf = &frame_of(&mut app, 140, 40);
    // The style of each occurrence of the value, top to bottom.
    let styles: Vec<ratatui::style::Style> = (0..40u16)
        .flat_map(|y| {
            let row: Vec<&str> = (0..140u16).map(|x| buf[(x, y)].symbol()).collect();
            let n = secret.chars().count();
            (0..row.len().saturating_sub(n))
                .filter(|&x| row[x..x + n].concat() == secret)
                .map(|x| buf[(x as u16, y)].style())
                .collect::<Vec<_>>()
        })
        .collect();
    let theme = app.theme();
    let (held, leaked) = (theme.ui(Ink::Secret), theme.ui(Ink::SecretLeaked));
    assert_ne!(held.fg, leaked.fg, "premise: two colours");
    assert_eq!(styles.len(), 3, "yours, put back, written: {styles:?}");
    for (what, style) in [("your line", styles[0]), ("put back", styles[1])] {
        assert_eq!(style.fg, held.fg, "{what}");
        assert!(!style.add_modifier.contains(Modifier::UNDERLINED), "{what}");
    }
    assert_eq!(styles[2].fg, leaked.fg, "written by the model");
    assert!(styles[2].add_modifier.contains(Modifier::UNDERLINED));

    // Replayed: the same span kept, from the stored text.
    let turn = Turn::seal(
        vec![
            Message::user("hi"),
            Message::assistant(format!("put back: {handle} / wrote: {secret}")),
        ],
        shannon_core::TurnConfig {
            model: "sonnet".to_string(),
            max_tokens: None,
            effort: None,
        },
        std::time::SystemTime::UNIX_EPOCH,
    );
    let mut output = Output::new();
    App::replay_turn(&app.aliases, None, &mut output, &turn);
    let vp = crate::viewport::Viewport::default();
    let window = output.visible_window(&vp, &crate::output::Shut::NONE, 140, 40);
    let at = window
        .rows
        .iter()
        .position(|r| r.starts_with("put back"))
        .expect("the reply");
    let shown = output.shown_at(window.origins[at].0).expect("the model's");
    let from = "put back: ".chars().count();
    assert_eq!(
        shown,
        vec![(from, from + secret.chars().count())],
        "the put-back alone"
    );
}

/// A handle can straddle a chunk boundary, and half of one resolves to
/// nothing — so the tail is held until it completes.
// `tokio::test`, because the straddle buffer now lives on a generation and
// `fake_stream` installs one (which spawns).
#[tokio::test]
async fn a_handle_split_across_chunks_still_resolves() {
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();
    let (head, tail) = handle.split_at(6);

    let id = app.fake_stream();
    let first = app.unmask_stream(id, &format!("you said {head}"));
    assert_eq!(first, "you said ", "the partial handle is held back");
    let second = app.unmask_stream(id, &format!("{tail} to me"));
    assert_eq!(second, format!("{secret} ({handle}) to me"));
}

/// **A plain alias in a reply shows its value, cut or whole** (Steven,
/// 10-19: "secrets and aliases follow the same rule"; review stop 6, C 8).
/// With no secret held the stream passed every chunk through raw, so a reply
/// naming `{{a:repo}}` showed the handle; and only a secret's handle was held
/// back across a cut.
#[tokio::test]
async fn a_plain_alias_in_a_reply_shows_its_value() {
    let mut app = test_app();
    let mut table = (*app.aliases).clone();
    let handle = table.insert("repo", "/src/repo", "config", false).unwrap();
    app.aliases = std::sync::Arc::new(table);
    assert_eq!(app.aliases.secrets().count(), 0, "premise: no secret held");
    assert!(app.switch_model(app.current, "claude-code/sonnet", "claude-code/sonnet"));
    let (head, tail) = handle.split_at(5);
    let id = app.fake_stream();
    let first = app.unmask_stream(id, &format!("cd {head}"));
    assert_eq!(first, "cd ", "the partial handle is held back");
    let second = app.unmask_stream(id, &format!("{tail} now"));
    assert_eq!(second, "/src/repo now");
}

/// **…including a split INSIDE the opening `{{`** — every split point, not the
/// one the first test happened to pick. The hold looked for a whole `{{`, so a
/// chunk ending in a lone `{` showed it at once and the next chunk's `{secret…`
/// never resolved: the stub's 4-char chunks put `:\n\n{` at the boundary of
/// "Got it. You said:\n\n{{…", and the reply showed the raw handle (Steven,
/// 10-02 smoke). A model's tokens can split there just as well.
#[tokio::test]
async fn a_handle_split_at_any_point_still_resolves() {
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();
    for at in 1..handle.len() {
        let (head, tail) = handle.split_at(at);
        let id = app.fake_stream();
        let shown = app.unmask_stream(id, &format!("said:\n\n{head}"))
            + &app.unmask_stream(id, &format!("{tail} ok"));
        assert_eq!(
            shown,
            format!("said:\n\n{secret} ({handle}) ok"),
            "split at {at}"
        );
    }
    // Only what could still BECOME a held handle waits: prose braces that
    // cannot are shown at once, not held until the stream ends.
    let id = app.fake_stream();
    assert_eq!(app.unmask_stream(id, "a map {{x"), "a map {{x");
    // And a whole handle ending a chunk resolves now, not a chunk later — with
    // a LONGER handle also held, or the length cut alone would decide it.
    let longer = app.mark_secret("a-second-value-notreal", true).unwrap();
    assert!(longer.len() > handle.len(), "premise: {longer} vs {handle}");
    let id = app.fake_stream();
    assert_eq!(
        app.unmask_stream(id, &format!("use {handle}")),
        format!("use {secret} ({handle})")
    );
}

/// **Streamed in any chunk size, the display is the whole text displayed at
/// once** — braces doubled around a handle (`{{hunter2}}` typed becomes
/// `{{{{secret.…}}}}` on the wire), prose braces, two handles touching, a lone
/// `{` and `}`. Asked because a smoke run showed an extra blank line after a
/// `{{hunter2}}` echo: this says whether the display could add one.
#[tokio::test]
async fn any_chunking_displays_as_the_whole_text_does() {
    let mut app = test_app();
    let a = app.mark_secret("hunter2beornot2be", true).unwrap();
    let b = app.mark_secret("a-second-value-notreal", true).unwrap();
    let text =
        format!("Got it. You said:\n\nthis is a test {{{{{a}}}}}\n\n{a}{b} {{ x }} {{{{y}}}} {{\n");
    // The stub's rendering, as `unmask_stream` gives it on the test app.
    let whole = (app.aliases).unmask_for_display(&text, shannon_core::Unmasked::WithHandle);
    let chars: Vec<char> = text.chars().collect();
    for size in 1..=8 {
        let id = app.fake_stream();
        let shown: String = chars
            .chunks(size)
            .map(|c| app.unmask_stream(id, &c.iter().collect::<String>()))
            .collect();
        assert_eq!(shown, whole, "chunks of {size}");
    }
}

/// **Two generations streaming at once do not splice each other's held text.**
///
/// The straddle buffer holds anything after an unclosed `{{`, because half a
/// handle resolves to nothing. It lived on `App` — safe only while exactly one
/// thing unmasked, which was true until the chunk handler was unified behind
/// `Sink` and jobs started unmasking too. Reviewer D predicted this precisely
/// ("fix the job path's display without moving this and you get N jobs sharing
/// one straddle buffer") and I walked into it two commits later.
///
/// The failure is text bleeding between sessions: the foreground holds `…{{sec`
/// and a child's next chunk arrives, so the parent's partial word is spliced
/// into the child's transcript and lost from the parent's. That is the class
/// the entire buffer model exists to prevent.
#[tokio::test]
async fn a_concurrent_job_does_not_steal_the_foregrounds_held_text() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();
    let (head, tail) = handle.split_at(6);

    // A child streaming into its own buffer, concurrently.
    let child = app.spawn_child_job("go", None).expect("spawned");

    // The foreground begins a handle and holds the partial.
    app.submit("ask something");
    let held = app.turns().next().expect("a turn").id;
    let first = app.unmask_stream(held, &format!("you said {head}"));
    assert_eq!(
        first, "you said ",
        "premise: the partial handle is held back"
    );

    // The child streams meanwhile. Its chunk must not pick up the foreground's
    // held bytes, and must not consume them.
    app.feed_job(child, StreamMsg::Chunk("child text".into()));
    // The child's own buffer, or the test fails — it fell back to the
    // foreground's, and probed for "you said", which is never held (the held
    // bytes are the handle's head): half of it measured nothing (review stop
    // 12, T2 5).
    let child_buffer = (app.generations.iter())
        .find_map(|g| matches!(&g.work, Work::Job(_)).then_some(g.buffer))
        .expect("the child's buffer");
    let child_shown = (app.session_of(child_buffer).expect("open").output.lines()).join("\n");
    assert!(
        child_shown.contains("child text"),
        "premise: the child's chunk is in its own transcript: {child_shown}"
    );
    assert!(
        !child_shown.contains(head),
        "the child's transcript did not absorb the foreground's held bytes: {child_shown}"
    );

    // And the foreground's handle still completes, from its own tail.
    let second = app.unmask_stream(held, &format!("{tail} to me"));
    assert_eq!(
        second,
        format!("{secret} ({handle}) to me"),
        "the foreground's own tail survived a concurrent stream"
    );
}

/// **A reopened turn keeps its rounds apart**, as the live view does: each
/// assistant message a paragraph, never two run together ("in sequence.I'm
/// unable to run that", Steven's demos, 2026-10-03). Its trailing newline
/// kept, as the live view draws it — a blank row before the turn's own
/// (trimmed, a reopened turn drew a row fewer than it drew live, and a kept
/// mark lands by rows: `a_reopened_turn_draws_the_blocks_it_drew_live`).
#[test]
fn a_reopened_turn_keeps_its_rounds_apart() {
    use shannon_core::graph::Turn;
    use shannon_core::message::Message;
    let turn = Turn::seal(
        vec![
            Message::user("run it"),
            Message::assistant("I'll run it."),
            Message::assistant(""),
            Message::assistant("I can't run that.\n"),
        ],
        shannon_core::TurnConfig {
            model: "stub".to_string(),
            max_tokens: None,
            effort: None,
        },
        std::time::SystemTime::UNIX_EPOCH,
    );
    let mut output = Output::new();
    App::replay_turn(
        &shannon_core::Aliases::default(),
        Some(crate::permission_view::Breaks::default()),
        &mut output,
        &turn,
    );
    assert_eq!(
        output.lines(),
        [
            "",
            "> run it",
            "",
            "",
            "I'll run it.",
            "",
            "I can't run that.",
            "",
            ""
        ]
    );
}

/// **A notification Claude Code's harness delivered reopens as what it
/// says** — and the turn keeps the XML the model read (image 90).
#[test]
fn a_reopened_task_notification_shows_as_what_it_says() {
    use shannon_core::graph::Turn;
    use shannon_core::message::Message;
    let prompt = "<task-notification>\n<task-id>t1</task-id>\n<status>completed</status>\n\
                  <summary>Workflow done</summary>\n<result>all clear</result>\n\
                  </task-notification>";
    let turn = Turn::seal(
        vec![Message::user(prompt), Message::assistant("Noted.")],
        shannon_core::TurnConfig {
            model: "stub".to_string(),
            max_tokens: None,
            effort: None,
        },
        std::time::SystemTime::UNIX_EPOCH,
    );
    let mut output = Output::new();
    App::replay_turn(
        &shannon_core::Aliases::default(),
        Some(crate::permission_view::Breaks::default()),
        &mut output,
        &turn,
    );
    assert_eq!(
        output.lines(),
        [
            "",
            "> ⚙ task t1 completed: Workflow done",
            "all clear",
            "",
            "",
            "Noted.",
            ""
        ]
    );
    assert_eq!(
        turn.messages[0].content, prompt,
        "the record is the model's"
    );
}

/// **A reopened turn shows the commands it ran**, each as the record that
/// closed it live read — `ran`, the output under it — in the order they
/// ran, between the text around them. Replay drew only the text: every call
/// vanished on `:open` (Steven, 10-05: "we also lost the tool calls").
#[test]
fn a_reopened_turn_shows_the_commands_it_ran() {
    use shannon_core::graph::Turn;
    use shannon_core::message::{Block, Message};
    let call = |id: &str, command: &str| Block::ToolUse {
        id: id.into(),
        name: shannon_core::BASH_TOOL.into(),
        input: serde_json::json!({ "command": command }),
    };
    let turn = Turn::seal(
        vec![
            Message::user("list"),
            Message {
                role: Role::Assistant,
                content: "Listing.".into(),
                blocks: vec![call("t1", "ls"), call("t2", "pwd")],
            },
            Message {
                role: Role::User,
                content: String::new(),
                blocks: vec![Block::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "a.txt\nb.txt".into(),
                    is_error: false,
                }],
            },
            Message::assistant("done"),
        ],
        shannon_core::TurnConfig {
            model: "stub".to_string(),
            max_tokens: None,
            effort: None,
        },
        std::time::SystemTime::UNIX_EPOCH,
    );
    let mut output = Output::new();
    App::replay_turn(
        &shannon_core::Aliases::default(),
        Some(crate::permission_view::Breaks::default()),
        &mut output,
        &turn,
    );
    // Each command a ```bash block, its output a block of its own (Steven,
    // 10-05, image 93).
    assert_eq!(
        output.lines(),
        [
            "",
            "> list",
            "",
            "",
            "Listing.",
            "",
            "⚙ ran:",
            "  ```bash",
            "  ls",
            "  ```",
            "  ```output",
            "  a.txt",
            "  b.txt",
            "  ```",
            "",
            "⚙ no result recorded:",
            "  ```bash",
            "  pwd",
            "  ```",
            "",
            "done",
            ""
        ]
    );
}

/// **One content, one rendering** — a session shows the same thing while you
/// type into it and after you `:open` back into it.
///
/// The live paths resolve handles for display; `replay_turn` could not, because
/// as an associated fn it had no way to reach `self.aliases` — the narrow copy
/// with the narrower signature, mechanically (Reviewer D). So a session you
/// typed a secret into showed your words as you typed and `{{secret.…}}` when
/// you came back to it, within one run.
///
/// Across *runs* the table is never persisted: a resumed transcript shows
/// what recall put back from the vault (the handles its turns recorded
/// holding) and nothing else — see
/// `a_marked_secret_comes_back_when_its_session_is_resumed`.
#[tokio::test]
async fn reopening_a_session_renders_a_secret_the_way_it_was_typed() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).unwrap();

    // A real turn: `submit` masks the value to a handle for storage and shows
    // the user's words LITERALLY (Steven, 10-02: "the input should still be
    // the literal input"); the reply names the handle, and shows it as
    // `value (handle)`. The replay must reproduce both lines.
    app.submit(&format!("the token is {secret}"));
    app.feed_stream(StreamMsg::Chunk(format!("noted {handle}")));
    app.feed_stream(StreamMsg::Complete);
    let episode = app.conversation().episode().id;
    assert!(
        app.conversation()
            .messages()
            .iter()
            .any(|m| m.role == Role::User && m.content.contains(&handle)),
        "premise: the STORED prompt carries the handle, not the value"
    );
    let typed = format!("> the token is {secret}");
    let reply = format!("noted {secret} ({handle})");
    let check = |shown: &[String], when: &str| {
        assert!(
            shown.contains(&typed),
            "{when}, the user line as typed: {shown:?}"
        );
        assert!(
            shown.contains(&reply),
            "{when}, the reply with both: {shown:?}"
        );
    };
    check(&app.output().lines(), "live");

    // Clone it. `adopt_session` re-renders from the stored turns, which is the
    // replay path — `:open` would find the still-live buffer instead, because
    // buffers are never closed.
    app.clone_by_hash(&episode.to_hex()[..8]);
    check(&app.output().lines(), "replayed");
}

/// **By default a reopened conversation shows its secret, and nothing asks
/// for a password** (Steven, 10-23: with `password_protect` off, the
/// default, "a resumed message does not use keychain to prompt the user for
/// the password" and "the chat still shows `hunter2` and NOT
/// `{{secret.06ea}}`"). The mark is kept in the plain store; the handle the
/// turn recorded is recalled from it as the history loads, and the history
/// is drawn with it — your line as typed, the reply's handle as its value.
#[tokio::test]
async fn a_kept_secret_shows_as_itself_in_a_later_run() {
    let dir = TempDir::new().unwrap();
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let store = || Box::new(JsonlStore::new(dir.path()).unwrap()) as Box<dyn shannon_core::Store>;
    let secret = "hunter2-in-a-later-run";
    let handle = {
        let mut app = test_app_with_vault(Config::default(), store(), vault.clone());
        let handle = app.mark_secret(secret, true).expect("marked");
        app.submit(&format!("the token is {secret}"));
        app.feed_stream(StreamMsg::Chunk(format!("noted {handle}")));
        app.feed_stream(StreamMsg::Complete);
        handle
    };
    assert_eq!(vault.len(), 1, "kept");

    let mut cfg = Config::default();
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let app = test_app_with_vault(cfg, store(), vault);
    let shown = app.output().lines();
    assert!(
        shown.contains(&format!("> the token is {secret}")),
        "your line, as typed: {shown:?}"
    );
    assert!(
        shown.iter().any(|l| l.contains(&format!("noted {secret}"))),
        "the reply's handle, as its value: {shown:?}"
    );
    assert!(
        !shown
            .iter()
            .any(|l| l.contains(&handle) && !l.contains(secret)),
        "no bare handle: {shown:?}"
    );
}

/// **Steven's 10-23 sequence, which lost his secret, keeps it now**: a
/// secret marked and kept, then `:set secrets.password_protect=true`, then
/// a relaunch on the default config. `069099a`'s `:set` moved the secret
/// into the Keychain for good while the setting itself died with the run,
/// so the relaunch looked in the plain store and showed the handle. The
/// setting is your config file's alone now — `:set` refuses it — and
/// nothing the refusal touched moved.
#[tokio::test]
async fn setting_password_protect_with_set_moves_nothing() {
    let dir = TempDir::new().unwrap();
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let store = || Box::new(JsonlStore::new(dir.path()).unwrap()) as Box<dyn shannon_core::Store>;
    let secret = "hunter2-through-a-set";
    {
        let mut app = test_app_with_vault(Config::default(), store(), vault.clone());
        let handle = app.mark_secret(secret, true).expect("marked");
        commit_a_turn_naming(&mut app, &handle);
        for line in [
            "secrets.password_protect=true",
            "secrets.password_protect",
            "invsecrets.password_protect",
        ] {
            app.dispatch_command_for_test(&format!("set {line}"));
            let said = app.notice().unwrap_or_default().to_string();
            assert_eq!(
                app.notice_kind(),
                Some(crate::app::NoticeKind::Refusal),
                "{line}: {said}"
            );
            assert!(said.contains("config file"), "{line}: {said}");
        }
        assert!(!app.config.secrets.password_protect, "not set");
    }
    assert_eq!(vault.len(), 1, "still kept where the next run looks");

    let mut cfg = Config::default();
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let app = test_app_with_vault(cfg, store(), vault);
    let shown = app.output().lines();
    assert!(
        shown.iter().any(|l| l.contains(secret)),
        "shown as itself after the relaunch: {shown:?}"
    );
}

/// **A user's line is the literal input** — a secret in it is not annotated
/// (that is for what comes BACK), and a path a plain alias covers stays the
/// path typed, though both are masked on the wire. Live AND replayed: the
/// replay resolved secrets only, so the path came back as `{{a:repo}}`
/// (Reviewers A and C, round 19).
#[tokio::test]
async fn a_users_line_is_the_literal_input() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let repo = std::sync::Arc::make_mut(&mut app.aliases)
        .insert("repo", "/src/some-repo", "config", false)
        .expect("a plain alias");
    app.mark_secret("hunter2beornot2be", true).unwrap();
    app.submit("cd /src/some-repo with hunter2beornot2be");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.conversation()
            .messages()
            .iter()
            .any(|m| m.role == Role::User && m.content.contains(&repo)),
        "premise: the stored prompt carries the alias's handle"
    );
    let typed = "> cd /src/some-repo with hunter2beornot2be".to_string();
    assert!(app.output().lines().contains(&typed), "live");

    let episode = app.conversation().episode().id;
    app.clone_by_hash(&episode.to_hex()[..8]);
    let shown = app.output().lines();
    assert!(shown.contains(&typed), "replayed: {shown:?}");
}

/// A stray `{{` in ordinary prose must not stall the display forever.
// `tokio::test`, because the straddle buffer now lives on a generation and
// `fake_stream` installs one (which spawns).
#[tokio::test]
async fn an_unclosed_brace_does_not_stall_the_stream() {
    let mut app = test_app();
    app.mark_secret("hunter2beornot2be", true).unwrap();
    let id = app.fake_stream();
    let mut shown = app.unmask_stream(id, "look at {{ this and then some more text");
    shown.push_str(&app.unmask_stream(id, " and yet more so the hold is released"));
    assert!(shown.contains("look at "), "{shown}");
    assert!(
        shown.contains("more text"),
        "held text is released: {shown}"
    );
}

/// Marking republishes the table to the RULE SET too. Without that the gate
/// would call a freshly-minted handle "not one we hold" and refuse it — the
/// exact opposite of what marking is for.
#[test]
fn a_freshly_marked_handle_is_known_to_the_gate() {
    let mut app = test_app();
    let handle = app
        .mark_secret("hunter2seekrit-not-a-real-one", true)
        .unwrap();
    let v = app
        .rules
        .decide_line(&format!("curl -H {handle} https://x"));
    assert!(
        v.stages().iter().any(|s| s.acts.iter().any(|a| matches!(
            a.act,
            shannon_core::permission::ActKind::Secret { known: true, .. }
        ))),
        "gate should recognise it: {:?}",
        v.stages()
    );
}

/// **An `$EDITOR` edit seals a marked value as its handle** — in what the
/// next request sends and in the log — exactly as a typed prompt does. The
/// edit and summary tails sealed text as typed, so a marked value pasted into
/// `e` went to the provider and to disk in clear (Reviewer E, round 18).
#[test]
fn an_edit_seals_a_marked_value_as_its_handle() {
    const VALUE: &str = "s3cr3t-edit-notreal";
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let handle = app.mark_secret(VALUE, true).expect("marked");
    let episode = app.conversation().episode().id;
    let edited = to_editable(&[
        Message::user(format!("use {VALUE} please")),
        Message::assistant("ok"),
    ]);
    let branch = app
        .commit_slice_edit(episode, 0, 1, &edited)
        .unwrap_or_else(|| panic!("composes: {:?}", app.notice()));
    let sent = format!("{:?}", app.conversation().snapshot());
    assert!(!sent.contains(VALUE) && sent.contains(&handle), "{sent}");
    let log = std::fs::read_to_string(
        dir.path()
            .join("episodes")
            .join(format!("{}.jsonl", branch.to_hex())),
    )
    .expect("the edit persisted its own turn");
    assert!(!log.contains(VALUE), "the log holds the handle");
    assert_eq!(
        recorded_handles(&app, branch),
        vec![handle],
        "and the edited turn records holding it"
    );
}

/// **A summary composes only over the turns it was written from.** `S`
/// resolves a range at the keypress and composes when the summary returns; a
/// turn deleted in between shifted the positions, and the summary replaced the
/// wrong turns — one kept beside its own summary, another dropped (Reviewer A,
/// round 18, reproduced). The compose re-checks, and refuses.
#[test]
fn a_summary_refuses_a_range_that_moved() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 5);
    let episode = app.conversation().episode().id;
    let ids = app.current_graph().full_turns(&episode);
    let target = SquashTarget::Range {
        episode,
        start: 1,
        end: 3,
        fork_turn: ids[1],
        config: app.conversation().episode().config.clone(),
    };
    let sources = ids[1..3].to_vec();
    app.remove_turns_from(episode, &[0])
        .expect("a turn deleted meanwhile");

    let text = to_editable(&[Message::user("gist"), Message::assistant("of it")]);
    let graph = app.current_graph();
    assert_eq!(
        app.commit_summary_into(app.current, graph, target, &sources, &text, None),
        None,
        "refused, not composed over the wrong turns"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("changed since the summary was asked for")),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.conversation().episode().id, episode, "nothing replaced");
}

/// The same for an `s`/`S` summary, the other `$EDITOR` tail.
#[test]
fn a_summary_seals_a_marked_value_as_its_handle() {
    const VALUE: &str = "s3cr3t-summary-notreal";
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    let handle = app.mark_secret(VALUE, true).expect("marked");

    let text = to_editable(&[
        Message::user(format!("the gist, with {VALUE}")),
        Message::assistant("summarized"),
    ]);
    let graph = app.current_graph();
    let (b, fork, fork_index, config) = app.merge_target(&graph, branch).unwrap();
    let target = SquashTarget::Branch {
        base: b,
        branch,
        fork_turn: fork,
        fork_index,
        config,
    };
    let summary = app
        .commit_summary_into(app.current, graph, target, &[], &text, None)
        .expect("composes");
    let sent = format!("{:?}", app.conversation().snapshot());
    assert!(!sent.contains(VALUE) && sent.contains(&handle), "{sent}");
    assert_eq!(
        recorded_handles(&app, summary),
        vec![handle],
        "and the summary turn records holding it"
    );
}

/// **No request carries a value we hold, whatever put it in the history.** A
/// value typed before it was marked sits in the history in clear and went out
/// with every later request; the outbound door now masks every message. And
/// marking it says it already left — the highlight it gets from now on is not
/// protection it had (Reviewer E, round 18).
#[tokio::test]
async fn a_request_never_carries_a_value_we_hold() {
    const VALUE: &str = "s3cr3t-before-notreal";
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let backend = Arc::new(RecordingBackend { sent: sent.clone() });
    let mut app = test_app_backed(backend, Arc::new(Config::default()));
    app.conversation_mut()
        .begin_turn(format!("my key is {VALUE}"));
    app.conversation_mut().push_chunk("noted");
    app.conversation_mut().commit_turn();

    app.dispatch_command_for_test(&format!("secret {VALUE}"));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("already sent in clear")),
        "{:?}",
        app.notice()
    );
    app.submit("and now?");
    let request = sent.lock().unwrap().last().cloned().expect("a request");
    let wire = format!("{:?}", request.messages);
    assert!(!wire.contains(VALUE), "the door masked it: {wire}");
}

/// A command with a credential's shape in it (`OPENAI_API_KEY=…`, a fake),
/// which the redactor takes out and no secret Shannon holds — so the
/// outbound door, which masks held values alone, lets it through.
fn command_with_a_key(app: &App) -> (String, &'static str) {
    let value = "Zk3pQ9vX2mL7tR4wB8nY";
    let command = format!("echo OPENAI_API_KEY={value}");
    assert!(
        !app.scrub(&command).contains(value),
        "premise: the redactor catches it"
    );
    (command, value)
}

/// Every call's input in `messages`, as JSON text — the calls alone: the
/// recording executor answers with the line it was given, unscrubbed, where
/// a real run's output is scrubbed at capture (`exec.rs`).
fn calls_in(messages: &[shannon_core::Message]) -> String {
    let calls = messages.iter().flat_map(|m| &m.blocks);
    (calls.filter_map(|b| match b {
        shannon_core::Block::ToolUse { input, .. } => Some(input.to_string()),
        _ => None,
    }))
    .collect::<Vec<_>>()
    .join("\n")
}

/// **The model's own call is kept scrubbed, and runs as it asked** — kept on
/// the message that asked, so in the store, and in every request after it.
/// The door masks only what Shannon holds: a credential the model typed
/// went back with each later request and sat in the store, where Claude
/// Code's calls were scrubbed (`keep_tool_use`; the 11-08 trace, F5).
#[tokio::test]
async fn a_models_call_is_kept_scrubbed_and_runs_as_asked() {
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    let (command, value) = command_with_a_key(&app);
    app.submit("go");
    model_call(&mut app, "tu_1", &command);
    settle(&mut app).await;
    assert_eq!(exec.ran(), [command], "run as the model asked it");

    let kept = calls_in(app.conversation().messages());
    assert!(kept.contains("OPENAI_API_KEY"), "premise: kept: {kept}");
    assert!(!kept.contains(value), "kept: {kept}");
    let requests = backend.requests.lock().unwrap();
    let resent = calls_in(&requests.last().expect("a request").messages);
    assert!(
        resent.contains("OPENAI_API_KEY"),
        "premise: re-sent: {resent}"
    );
    assert!(!resent.contains(value), "re-sent: {resent}");
}

/// **And a child's**, through its own gate (`gate_job_tool_call`): kept by
/// the same door, run as it asked, unattended.
#[tokio::test]
async fn a_childs_call_is_kept_scrubbed_and_runs_as_asked() {
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    let (command, value) = command_with_a_key(&app);
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("ch-1", &command)),
    );
    app.feed_job(child, StreamMsg::Complete);
    settle(&mut app).await;
    assert_eq!(exec.ran(), [command], "run as the child asked it");

    let kept: Vec<String> = (app.buffers.sessions())
        .map(|b| calls_in(b.conversation.messages()))
        .collect();
    assert!(
        kept.iter().any(|k| k.contains("OPENAI_API_KEY")),
        "premise: the child's call is kept: {kept:?}"
    );
    assert!(kept.iter().all(|k| !k.contains(value)), "kept: {kept:?}");
    let requests = backend.requests.lock().unwrap();
    let resent = calls_in(&requests.last().expect("a request").messages);
    assert!(
        resent.contains("OPENAI_API_KEY"),
        "premise: re-sent: {resent}"
    );
    assert!(!resent.contains(value), "re-sent: {resent}");
}

/// **The demo bug, at the pane: a wrapped secret is marked on every row** —
/// here in the transcript drawn beside a focused prompt. The per-row rule in
/// `row_secrets` was tested on windows a test built itself, so reverting the
/// pane to per-row matching, or losing the block's line offset, left the suite
/// green (Reviewers A and C). And the two renderings, read off the cells: the
/// user's line marks the value it shows literally; a reply naming the handle
/// marks `value (handle)` — both halves (Steven, 10-02: "both highlighted").
#[tokio::test]
async fn a_wrapped_secret_is_marked_whole_in_the_transcript() {
    let (mut app, marked) = transcript_with_a_late_wrapped_secret();
    assert_eq!(
        app.focus(),
        PaneKind::Prompt,
        "premise: the transcript is unfocused"
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    assert_eq!(secret_cells(&terminal), marked);
}

/// The same in the focused chat, which frames the rows around a cursor and
/// computes their origins its own way.
#[tokio::test]
async fn a_wrapped_secret_is_marked_whole_in_the_focused_chat() {
    let (mut app, marked) = transcript_with_a_late_wrapped_secret();
    app.set_focus(PaneKind::Output);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    assert_eq!(secret_cells(&terminal), marked);
}

/// And in the prompt, where it was first reported: pasted on the SECOND line,
/// so the row's origin must carry the line, too.
#[test]
fn a_wrapped_secret_is_marked_whole_in_the_prompt() {
    let mut app = test_app();
    app.prompt_mut()
        .insert_pasted(&format!("a first line\nthe key {WRAPPING_SECRET}"));
    app.mark_secret(WRAPPING_SECRET, true).expect("marked");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    assert_eq!(secret_cells(&terminal), WRAPPING_SECRET);
}

/// **`--no-spend` closes the one channel that cannot otherwise be closed.**
/// A spendable secret can be *computed on* by an approved command whose
/// output the model reads, so it can derive facts a few bits at a time.
/// Spending is the default because it is the common case; this is the
/// control for when it is not.
#[tokio::test]
async fn a_no_spend_secret_is_masked_but_never_expanded() {
    let (mut app, backend) = recorded_holding(shannon_core::Aliases::default());
    let handle = app.mark_secret("hunter2beornot2be", false).expect("marked");

    // Still masked on the way out — that half is unchanged.
    let (outbound, hit) = app.aliases.mask("my password is hunter2beornot2be");
    assert_eq!(outbound, format!("my password is {handle}"));
    assert_eq!(hit.len(), 1);

    // But a command cannot turn it back into the value.
    let err = app
        .aliases
        .expand_at_exec(&format!("echo {handle}"))
        .expect_err("must refuse");
    assert!(err.contains("not lent for use"), "{err}");

    // And the model is told it exists but cannot be used, so it does not
    // waste a turn trying — on the request it is sent.
    app.submit("go");
    let desc = sent_tool_description(&backend);
    assert!(desc.contains(&handle), "{desc}");
    assert!(desc.contains("cannot be used in a command"), "{desc}");
}

/// The default is the common case: a marked secret CAN be spent.
#[test]
fn a_marked_secret_is_spendable_by_default() {
    let mut app = test_app();
    let handle = app.mark_secret("hunter2beornot2be", true).unwrap();
    let e = app
        .aliases
        .expand_at_exec(&format!("echo {handle}"))
        .expect("spendable");
    assert!(e.text.contains("hunter2beornot2be"));
    assert_eq!(e.spent, vec!["secret".to_string()]);
}

/// **Paste is the 90% case**, and the one moment we can be sure the user
/// knows what the text is. It offers rather than acting: a paste that
/// silently became a handle would be a surprise, and surprise is how a
/// protection gets switched off.
#[test]
fn pasting_a_credential_offers_to_keep_it_off_the_wire() {
    let mut app = test_app();
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    app.handle_paste(token);

    // The text landed, and an offer is pending — not an edit already made.
    assert!(
        app.prompt().text().contains(token),
        "paste is not swallowed"
    );
    let prompt = app.confirm_prompt().expect("offered").text();
    assert!(prompt.contains("github token"), "names the risk: {prompt}");

    // Accepting replaces it with a handle.
    type_keys(&mut app, "y");
    // The buffer keeps what they pasted — we own the display, so there is no
    // reason to show a placeholder where their own paste was. The table now
    // holds it, and the masking happens on the way out.
    assert!(
        app.prompt().text().contains(token),
        "prompt keeps the truth"
    );
    assert_eq!(app.aliases.secrets().count(), 1);
    let (outbound, _) = app.aliases.mask(&app.prompt().text());
    assert!(
        !outbound.contains(token),
        "but it does not leave: {outbound}"
    );
}

/// Declining leaves the text exactly as pasted. The offer must be an offer.
#[test]
fn declining_the_offer_leaves_the_paste_alone() {
    let mut app = test_app();
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    app.handle_paste(token);
    type_keys(&mut app, "n");
    assert_eq!(app.prompt().text(), token);
    assert_eq!(app.aliases.secrets().count(), 0);
}

/// Ordinary pasted text must not ask. An offer that fires on prose is an
/// offer people learn to dismiss, and then it is worth nothing.
#[test]
fn pasting_ordinary_text_does_not_ask() {
    let mut app = test_app();
    for text in [
        "the quick brown fox jumps over the lazy dog",
        "src/main.rs:42: cannot find type `Foo`",
        "hf_model_config-v2",
        "Xk7pQ2mZ9vL4nR8tW1yB3cF6hJ0sD5gA and some prose after it",
    ] {
        app.prompt_mut().take();
        app.handle_paste(text);
        assert!(app.confirm_prompt().is_none(), "false offer on: {text}");
    }
}

/// A child's briefing is masked like any other outbound text. It was not:
/// `Aliases::mask` had two call sites and the child spawn was not one, so
/// `:child <secret>` left the machine and landed in the child's log in
/// cleartext — while the same words typed as a prompt left as a handle.
/// (Reviewer D, mutation-verified.)
#[tokio::test]
async fn a_child_briefing_masks_secrets_like_a_prompt_does() {
    let dir = TempDir::new().unwrap();
    let (mut app, handle) = app_with_secret("token", "hunter2beornot2be");
    app.store = Box::new(JsonlStore::new(dir.path()).unwrap());
    app.conversation_mut().begin_turn("seed");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    let child = app
        .spawn_child_job("the password is hunter2beornot2be", None)
        .expect("spawned");

    let sent = app
        .buffers
        .sessions()
        .find(|b| b.conversation.episode().id == child)
        .expect("the child's buffer")
        .conversation
        .messages()
        .last()
        .unwrap()
        .content
        .clone();
    assert!(
        !sent.contains("hunter2beornot2be"),
        "the value must not reach the wire or the child's log: {sent}"
    );
    assert!(sent.contains(&handle), "it travels as its handle: {sent}");
}

/// **`CTRL-C` on the credential question vaults it, as `Esc` does** — the
/// dismissal key takes the SAFE answer (Steven, 10-16, review stop 1).
///
/// It first cancelled with "nothing done" while the pasted key stayed in the
/// prompt to leave verbatim on the next `<Enter>`; then it said so ("SENT
/// AS-IS"), which was true and still the dangerous branch for the key you
/// press to get out.
#[test]
fn ctrl_c_on_a_pasted_credential_vaults_it() {
    let mut app = test_app();
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    app.handle_paste(token);
    assert!(app.confirm_prompt().is_some(), "premise: offered");
    app.interrupt();
    assert!(app.pending_confirm.is_empty(), "answered");
    assert_eq!(app.aliases.secrets().count(), 1, "vaulted");
    let (outbound, _) = app.aliases.mask(&app.prompt().text());
    assert!(
        !outbound.contains(token),
        "and it does not leave: {outbound}"
    );
}

/// **`Esc` on the credential question vaults it** — dismissal is the SAFE
/// answer, whichever way round the question is (Reviewer F). The arm moved
/// into `answer_question` in phase 2 and nothing held it: deleting it left
/// the suite green while a reflexive `Esc` would send the token as typed
/// (review 2026-10-16, Reviewer B F1).
#[test]
fn esc_on_a_pasted_credential_vaults_it() {
    let mut app = test_app();
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    app.handle_paste(token);
    assert!(app.confirm_prompt().is_some(), "premise: offered");
    type_keys(&mut app, "<Esc>");
    assert!(app.pending_confirm.is_empty(), "answered");
    assert_eq!(
        app.aliases.secrets().count(),
        1,
        "Esc is the SAFE answer: vaulted"
    );
    let (outbound, _) = app.aliases.mask(&app.prompt().text());
    assert!(
        !outbound.contains(token),
        "and it does not leave: {outbound}"
    );
}

/// **A command's output shows a held handle as a reply does** — here on the
/// test app's stub, `value (handle)` (a real model's shows the value alone:
/// `a_claude_code_session_shows_a_held_secret_as_its_value`) — while the
/// `tool_result` the model gets keeps the bare handle (Reviewer A, round 19).
#[tokio::test]
async fn on_the_stub_a_commands_output_shows_a_value_with_its_handle() {
    const VALUE: &str = "s3cret-output-notreal";
    let mut app = test_app();
    let handle = app.mark_secret(VALUE, true).unwrap();
    model_call(&mut app, "toolu_out", "printenv TOKEN");
    app.handle_exec_outcome(shannon_core::ExecOutcome {
        tool_use_id: "toolu_out".to_string(),
        command: "printenv TOKEN".to_string(),
        output: format!("token={handle}"),
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
    });
    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains(&format!("token={VALUE} ({handle})")),
        "{shown}"
    );
    let (sent, _) = tool_result_for(&app, "toolu_out").expect("answered");
    assert!(
        sent.contains(&format!("token={handle}")) && !sent.contains(VALUE),
        "the model's copy keeps the handle: {sent}"
    );
}

/// **`:secret <Tab>` completes from text already typed** — the prompt first,
/// then the chat the prompt is aimed at, latest mention first — and stops
/// offering a value once it is marked (Steven, 2026-10-01). Through the real
/// `:` line and Tab key.
#[test]
fn secret_completes_from_the_prompt_then_the_latest_chat() {
    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let completed = |app: &mut App, typed: &str| -> String {
        type_command(app, typed);
        app.command_input(tab);
        let line = app.cmdline.line().unwrap_or_default().to_string();
        app.command_input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        line
    };

    let mut app = test_app();
    app.buffer_mut().output.begin_turn("first try hunter-old");
    app.buffer_mut().output.begin_turn("then hunter-new");
    assert_eq!(
        completed(&mut app, "secret hun"),
        "secret hunter-new",
        "the chat's latest mention first"
    );

    app.prompt_mut().insert_pasted("my password is hunter2 ok");
    assert_eq!(
        completed(&mut app, "secret hun"),
        "secret hunter2",
        "the prompt before the chat"
    );

    app.mark_secret("hunter2", true).expect("marked");
    assert_eq!(
        completed(&mut app, "secret hun"),
        "secret hunter-new",
        "a value already marked is not offered again"
    );
}

/// **Every way a turn is sealed records the handles it held** — through the
/// production doors, not a test helper: a seal site that forgot would leave
/// its turns unrestorable on resume (the safe failure, but a failure).
#[tokio::test]
async fn every_seal_records_the_handles_it_held() {
    const VALUE: &str = "s3cret-seal-notreal";
    let mut app = test_app();
    let handle = app.mark_secret(VALUE, true).unwrap();
    let head = |app: &App| {
        app.conversation()
            .head_turn()
            .and_then(|t| t.provenance.handles)
    };

    app.submit(&format!("use {VALUE}"));
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(head(&app), Some(vec![handle.clone()]), "a finished turn");

    app.submit(&format!("again {VALUE}"));
    app.feed_stream(StreamMsg::Chunk("partial".into()));
    app.feed_stream(StreamMsg::Error(BackendError::RateLimited));
    // The premise first: a turn never sealed leaves the last one at the head,
    // holding the same handle (review stop 12, T4 7).
    assert!(
        (app.conversation().head_turn())
            .is_some_and(|t| t.messages.iter().any(|m| m.content.contains("partial"))),
        "premise: the interrupted turn is sealed, at the head"
    );
    assert_eq!(
        head(&app),
        Some(vec![handle.clone()]),
        "an interrupted turn"
    );

    let child_handles = |app: &App, child: EpisodeId| {
        app.current_graph()
            .placed(&child)
            .last()
            .and_then(|p| p.provenance.handles.clone())
    };
    let finished = app
        .spawn_child_job(&format!("with {VALUE}"), None)
        .expect("spawned");
    app.feed_job(finished, StreamMsg::Chunk("done".into()));
    app.feed_job(finished, StreamMsg::Complete);
    assert_eq!(
        child_handles(&app, finished),
        Some(vec![handle.clone()]),
        "a child's"
    );

    let child = app
        .spawn_child_job(&format!("and {VALUE}"), None)
        .expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("half".into()));
    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));
    let graph = app.current_graph();
    assert!(
        (graph.placed(&child).last())
            .and_then(|p| graph.messages(&p.turn))
            .is_some_and(|ms| ms.iter().any(|m| m.content.contains("half"))),
        "premise: the child's interrupted turn is sealed, its last"
    );
    assert_eq!(
        child_handles(&app, child),
        Some(vec![handle.clone()]),
        "a child's interrupted"
    );

    // `r`: the result is a SECOND production of the child's content, with
    // provenance of its own — read that occurrence, not the child's.
    app.return_result(finished);
    let returned = app.conversation().episode().id;
    assert_eq!(
        child_handles(&app, returned),
        Some(vec![handle]),
        "a returned result: {:?}",
        app.notice()
    );
}

/// **A composition in a LATER run records what its sources recorded** — the
/// summary, the edit, the return and the continuation alike. The second run
/// here cannot restore the secret (its vault is empty: a keychain locked or
/// denied this launch), so its table never holds the handle; every compose
/// recorded only what the table held, sealed `Some([])` over text still
/// naming the handle, and that secret could never come back in any run after
/// (Reviewer A, round 19). The sources' records vouch for it instead.
#[tokio::test]
async fn a_later_runs_composition_records_what_its_sources_recorded() {
    use shannon_core::{ContinueStrategy, StopReason};
    let dir = TempDir::new().unwrap();
    let store = || Box::new(JsonlStore::new(dir.path()).unwrap()) as Box<dyn shannon_core::Store>;
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let (handle, episode, child) = {
        let vault = Arc::new(shannon_core::MemoryVault::default());
        let mut app = test_app_with_vault(cfg.clone(), store(), vault);
        let handle = app.mark_secret("s3cret-later-notreal", true).unwrap();
        let episode = commit_a_turn_naming(&mut app, &handle);
        let child = app
            .spawn_child_job(&format!("with {handle}"), None)
            .expect("spawned");
        app.feed_job(child, StreamMsg::Chunk(format!("done with {handle}")));
        app.feed_job(child, StreamMsg::Complete);
        // Last, so the second run resumes onto a turn `:continue` can extend.
        let prompt = format!("a poem about {handle}");
        app.output_mut().begin_turn(&prompt);
        app.conversation_mut().begin_turn(prompt);
        app.feed_stream(StreamMsg::Chunk(format!("Roses for {handle} are")));
        app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
        app.feed_stream(StreamMsg::Complete);
        (handle, episode, child)
    };
    let later = || {
        let vault = Arc::new(shannon_core::MemoryVault::default());
        test_app_with_vault(cfg.clone(), store(), vault)
    };
    let premise = |app: &App| assert!(!app.aliases.knows(&handle), "premise: never held");
    // The composed turn's OWN occurrence. A summary or edit of turn 0 slices
    // in run 1's turn 1, which records the handle itself — so a flattened
    // list passed with the composed turn recording nothing.
    let at = |app: &App, episode: EpisodeId, index: usize| {
        app.current_graph().placed(&episode)[index]
            .provenance
            .handles
            .clone()
    };
    let held = Some(vec![handle.clone()]);

    let mut app = later();
    premise(&app);
    let ids = app.current_graph().full_turns(&episode);
    let target = SquashTarget::Range {
        episode,
        start: 0,
        end: 1,
        fork_turn: ids[0],
        config: app.conversation().episode().config.clone(),
    };
    let text = to_editable(&[
        Message::user(format!("the gist of {handle}")),
        Message::assistant("ok"),
    ]);
    let graph = app.current_graph();
    let summary = app
        .commit_summary_into(app.current, graph, target, &ids[..1], &text, None)
        .unwrap_or_else(|| panic!("summarised: {:?}", app.notice()));
    assert_eq!(at(&app, summary, 0), held, "a summary");

    let mut app = later();
    let edited = to_editable(&[
        Message::user(format!("the password is {handle}, edited")),
        Message::assistant("noted"),
    ]);
    let edit = app
        .commit_slice_edit(episode, 0, 1, &edited)
        .unwrap_or_else(|| panic!("edited: {:?}", app.notice()));
    assert_eq!(at(&app, edit, 0), held, "an edit");

    let mut app = later();
    app.return_result(child);
    let returned = app.conversation().episode().id;
    assert_ne!(returned, episode, "premise: returned: {:?}", app.notice());
    let last = app.current_graph().placed(&returned).len() - 1;
    assert_eq!(at(&app, returned, last), held, "a return");

    let mut app = later();
    premise(&app);
    // The session whose last turn was cut off, by name: a run above that
    // composed has moved `HEAD` (a composition owning new content is
    // written, and so registered, at once — 10-16), and `resume = last`
    // follows it.
    app.open_by_hash(&episode.to_hex()[..8]);
    app.continue_last_turn();
    assert!(app.continuing(), "premise: {:?}", app.notice());
    app.feed_continuation(StreamMsg::Chunk(" red".into()));
    app.feed_continuation(StreamMsg::Stopped(StopReason::EndTurn));
    app.feed_continuation(StreamMsg::Complete);
    let continued = app.conversation().episode().id;
    let last = app.current_graph().placed(&continued).len() - 1;
    assert_eq!(at(&app, continued, last), held, "a continuation");
}

/// The one seal that composes outside the conversation's doors: a `prefill`
/// continuation seals `partial + continuation` by hand, and records what it
/// held like every other.
#[tokio::test]
async fn a_continuation_records_the_handles_it_held() {
    use shannon_core::{ContinueStrategy, StopReason};
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    let mut app = test_app_backed(test_backend(), Arc::new(cfg));
    let handle = app.mark_secret("s3cret-continue-notreal", true).unwrap();
    let prompt = format!("a poem about {handle}");
    app.output_mut().begin_turn(&prompt);
    app.conversation_mut().begin_turn(prompt.clone());
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);

    app.continue_last_turn();
    assert!(app.continuing(), "premise: prefill streams off-transcript");
    app.feed_continuation(StreamMsg::Chunk(" red".into()));
    app.feed_continuation(StreamMsg::Stopped(StopReason::EndTurn));
    app.feed_continuation(StreamMsg::Complete);

    let composed = app.conversation().episode().id;
    assert_eq!(branch_msgs(&app)[1], "Roses are red", "premise: composed");
    assert_eq!(recorded_handles(&app, composed), vec![handle]);
}

/// **Recall asks the vault only about what a turn RECORDS it held** — the
/// full fix for the planted-handle oracle (Reviewer E, round 18). A turn that
/// recorded nothing held is not asked about even when its own words name a
/// kept handle (a guess the user pasted, or a reply an injection steered); a
/// turn that recorded one restores it even when it appears only in a tool's
/// output — the case the shape-based mitigation had to give up.
#[test]
fn recall_asks_only_what_a_turn_recorded_holding() {
    use shannon_core::{Kept, Role, Secret, Vault as _};
    let vault = shannon_core::MemoryVault::default();
    let kept = "{{secret.3c7a}}";
    vault
        .keep(
            kept,
            &Kept {
                name: "secret".into(),
                value: Secret::new("the-real-value-notreal"),
                spendable: true,
            },
        )
        .unwrap();
    let cfg = TurnConfig {
        model: "claude-haiku".into(),
        max_tokens: None,
        effort: None,
    };
    let seal = |messages: Vec<Message>| {
        Turn::seal(messages, cfg.clone(), std::time::SystemTime::UNIX_EPOCH)
    };
    let table = shannon_core::Aliases::default();

    let guessed = seal(vec![
        Message::user(format!("try {kept}")),
        Message::assistant(format!("using {kept}")),
    ])
    .with_handles(Vec::new());
    assert_eq!(
        recall_secrets(&table, &vault, &[guessed]).restored,
        0,
        "named, but never held: not asked"
    );

    let printed = seal(vec![
        Message::user("show me"),
        Message {
            role: Role::User,
            content: String::new(),
            blocks: vec![shannon_core::Block::ToolResult {
                tool_use_id: "t1".into(),
                content: format!("value: {kept}"),
                is_error: false,
            }],
        },
    ])
    .with_handles(vec![kept.to_string()]);
    assert_eq!(
        recall_secrets(&table, &vault, &[printed]).restored,
        1,
        "recorded as held: restored, wherever it appears"
    );
}

/// **A turn sealed before handles were recorded (legacy) never yields a handle
/// from a tool's output, and asks the vault a bounded number of times.** A
/// tool's output is whoever wrote the file or
/// page: one 64 KiB result holds 4096 handle guesses, a miss was silent and a
/// hit restored another project's kept secret into this session and told the
/// model about it (Reviewer E, round 18, verified). The handle in the user's
/// own words still comes back; past the cap, nothing is asked, and it says so.
#[test]
fn recall_ignores_handles_in_tool_output_and_caps_its_asks() {
    use shannon_core::{Kept, Role, Secret, Vault as _};
    let vault = shannon_core::MemoryVault::default();
    let kept = "{{secret.3c7a}}";
    vault
        .keep(
            kept,
            &Kept {
                name: "secret".into(),
                value: Secret::new("the-real-value-notreal"),
                spendable: true,
            },
        )
        .unwrap();
    let cfg = TurnConfig {
        model: "claude-haiku".into(),
        max_tokens: None,
        effort: None,
    };
    // Legacy: a log written before `handles` existed reads back as `None` —
    // the only way to get one, since a turn sealed now records `Some`.
    let turn = |messages: Vec<Message>| {
        let mut t = Turn::seal(messages, cfg.clone(), std::time::SystemTime::UNIX_EPOCH);
        t.provenance.handles = None;
        t
    };
    let table = shannon_core::Aliases::default();

    let planted = turn(vec![
        Message::user("read the file"),
        Message {
            role: Role::User,
            content: String::new(),
            blocks: vec![shannon_core::Block::ToolResult {
                tool_use_id: "t1".into(),
                content: format!("a page that says {kept}"),
                is_error: false,
            }],
        },
    ]);
    assert_eq!(
        recall_secrets(&table, &vault, &[planted]).restored,
        0,
        "never from a tool's output"
    );

    let own = turn(vec![
        Message::user(format!("use {kept}")),
        Message::assistant("ok"),
    ]);
    assert_eq!(
        recall_secrets(&table, &vault, &[own]).restored,
        1,
        "the user's own words"
    );

    let guesses: String = (0..RECALL_LOOKUPS)
        .map(|i| format!("{{{{secret.{i:04x}}}}} "))
        .collect();
    let crowded = turn(vec![
        Message::user(format!("{guesses}{kept}")),
        Message::assistant("ok"),
    ]);
    let recall = recall_secrets(&table, &vault, std::slice::from_ref(&crowded));
    assert_eq!(
        recall.restored, 0,
        "the kept one is past the cap: not asked"
    );
    assert_eq!(recall.skipped, 1);
    assert!(
        recall
            .said("the test vault")
            .is_some_and(|s| s.contains("not looked up")),
        "and it says so"
    );

    // A RECORDED handle was held, so it is no guess: asked ahead of the
    // scanned ones and never capped (Reviewers A and B, round 19).
    let recording = |handles: Vec<String>| {
        let mut t = turn(vec![Message::user("x"), Message::assistant("ok")]);
        t.provenance.handles = Some(handles);
        t
    };
    let recall = recall_secrets(
        &table,
        &vault,
        &[crowded, recording(vec![kept.to_string()])],
    );
    assert_eq!(
        (recall.restored, recall.skipped),
        (1, 0),
        "recorded later, still asked first"
    );
    let many: Vec<String> = (0..2 * RECALL_LOOKUPS)
        .map(|i| format!("{{{{other.{i:04x}}}}}"))
        .chain(std::iter::once(kept.to_string()))
        .collect();
    let recall = recall_secrets(&table, &vault, &[recording(many)]);
    assert_eq!(
        (recall.restored, recall.skipped),
        (1, 0),
        "a recorded list is never capped"
    );
}

/// **A marked secret comes back when its session is resumed** (Steven,
/// 2026-10-01: "we have to be able to load the secrets back into the
/// session"). Kept under its handle when marked; recalled by that exact handle
/// when the history is loaded — at launch, and by `:open`. A handle planted in
/// the history that nobody kept stays inert: recall never goes by name.
#[test]
fn a_marked_secret_comes_back_when_its_session_is_resumed() {
    let dir = TempDir::new().unwrap();
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let store = || Box::new(JsonlStore::new(dir.path()).unwrap()) as Box<dyn shannon_core::Store>;
    let planted = "{{github-token.ffff}}";
    let (handle, episode) = {
        let mut app = test_app_with_vault(Config::default(), store(), vault.clone());
        let handle = app
            .mark_secret("correct horse battery staple", true)
            .expect("marked");
        assert_eq!(vault.len(), 1, "premise: kept when marked");
        let episode = commit_a_turn_naming(&mut app, &format!("{handle} and {planted}"));
        (handle, episode)
    };

    let mut cfg = Config::default();
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let app = test_app_with_vault(cfg, store(), vault.clone());
    assert_eq!(
        app.conversation().episode().id,
        episode,
        "premise: resumed it"
    );
    // What made it come back: the turn RECORDED the handle it held — the
    // planted one beside it was never held, so it was never recorded.
    let on_disk = store().load_episode(&episode).unwrap().turns;
    assert_eq!(
        on_disk[0].provenance.handles,
        Some(vec![handle.clone()]),
        "the held handle, recorded and kept on disk"
    );
    let run = app
        .aliases
        .expand_at_exec(&format!("echo {handle}"))
        .expect("the old handle resolves again");
    assert_eq!(run.text, "echo correct horse battery staple");
    assert!(
        !app.aliases.knows(planted),
        "a handle nobody kept stays inert"
    );
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("restored 1 kept secret"),
        "{:?}",
        app.notice()
    );

    // The other doors: a fresh launch, then `:open` on that session…
    let mut app = test_app_with_vault(Config::default(), store(), vault.clone());
    assert!(
        !app.aliases.knows(&handle),
        "premise: a fresh launch holds none"
    );
    app.open_command(&episode.to_hex()[..8]);
    assert!(app.aliases.knows(&handle), "`:open` restores it too");

    // …and a composition from the graph: `:clone` of a session this launch
    // never opened lands its history in a buffer through `adopt_session`.
    let mut app = test_app_with_vault(Config::default(), store(), vault);
    assert!(
        !app.aliases.knows(&handle),
        "premise: a fresh launch holds none"
    );
    app.clone_episode(episode);
    assert!(app.aliases.knows(&handle), "a composition restores it too");
}

/// **`:nosecret` takes a secret back out** (Steven, 2026-10-02) — by its
/// value or by its handle: off the table, out of the vault, so no later run
/// restores it, and the notice never quotes a value it was given.
#[test]
fn nosecret_takes_a_secret_out_of_the_table_and_the_vault() {
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let mut app = test_app_with_vault(
        Config::default(),
        Box::new(shannon_core::MemoryStore::new()),
        vault.clone(),
    );
    let pw = app.mark_secret("hunter2", true).expect("marked");
    let token = app.mark_secret("s3cr3t-token", true).expect("marked");
    assert_eq!(vault.len(), 2, "premise: both kept");

    app.dispatch_command_for_test("nosecret hunter2");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("{pw} is no longer a secret")),
        "what changed leads, so truncation cannot cut it: {said:?}"
    );
    assert!(
        !said.contains("hunter2"),
        "the value is never echoed: {said:?}"
    );
    assert!(!app.aliases.knows(&pw), "off the table");
    assert_eq!(vault.len(), 1, "out of the vault");

    app.dispatch_command_for_test(&format!("nosecret {token}"));
    assert!(!app.aliases.knows(&token), "by handle too");
    assert_eq!(vault.len(), 0);

    // Nothing by that name: refused, and the typed text not repeated back.
    app.dispatch_command_for_test("nosecret hunter3");
    let said = app.notice().unwrap_or_default();
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert!(!said.contains("hunter3"), "{said:?}");

    // Nothing at all: the usage, not "no secret here is that".
    app.dispatch_command_for_test("nosecret");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("needs a handle or the value"), "{said:?}");
}

/// `<Tab>` after `:nosecret` offers each secret by handle and by value.
#[test]
fn nosecret_completes_handles_and_values() {
    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let mut app = test_app();
    let handle = app.mark_secret("hunter2", true).expect("marked");
    type_command(&mut app, "nosecret hun");
    app.command_input(tab);
    assert_eq!(app.cmdline.line(), Some("nosecret hunter2"));
    app.command_input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    type_command(&mut app, "nosecret {{");
    app.command_input(tab);
    assert_eq!(
        app.cmdline.line().map(str::to_string),
        Some(format!("nosecret {handle}"))
    );

    // A bare Tab offers handles only: one reflexive press must not list every
    // value on the screen (Reviewer F, round 18).
    app.command_input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.mark_secret("s3cond-notreal", true).expect("marked");
    type_command(&mut app, "nosecret ");
    app.command_input(tab);
    let (shown, _) = app.cmdline.menu().expect("two handles, a menu");
    assert!(
        shown.iter().all(|c| c.starts_with("{{")),
        "handles, no values: {shown:?}"
    );
}

/// **Marking a value already held answers with its handle**, rather than
/// minting a second one that `:nosecret` by value would then leave behind. A
/// different `--no-spend` is refused with the way to change it.
#[test]
fn marking_a_held_value_again_answers_with_its_handle() {
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let mut app = test_app_with_vault(
        Config::default(),
        Box::new(shannon_core::MemoryStore::new()),
        vault.clone(),
    );
    let first = app.mark_secret("hunter2", true).expect("marked");
    assert_eq!(app.mark_secret("hunter2", true), Ok(first.clone()));
    assert_eq!(app.aliases.secrets().count(), 1, "one handle for one value");
    assert_eq!(vault.len(), 1, "and one keychain item");
    let refused = app
        .mark_secret("hunter2", false)
        .expect_err("a capability change");
    assert!(
        refused.contains(&first) && !refused.contains("hunter2"),
        "{refused}"
    );
}

/// **A value lent AND marked comes off whole, and the notice says which one
/// comes back.** Two secrets held one value; `:nosecret` by value took the
/// first and said the value was no longer masked while the second still
/// masked it. And a lent secret is lent again next launch — the vault's
/// silent "forgot it" is no promise about one it never held.
#[test]
fn nosecret_takes_every_secret_holding_a_value_and_says_which_returns() {
    // With a store, and with none — where forgetting asks nothing and the
    // lent one's return must still be said (review stop 6, C 2).
    nosecret_says_which_returns(test_app_with(Config::default()));
    let mut storeless = test_app_with(Config::default());
    storeless.vault = Arc::new(shannon_core::NoVault);
    nosecret_says_which_returns(storeless);
}

/// `:nosecret` republishes the table to the RULE SET, the mirror of
/// `a_freshly_marked_handle_is_known_to_the_gate`: the gate holds its own
/// copy, and a stale one went on calling a forgotten handle "one we minted".
#[test]
fn a_forgotten_handle_is_unknown_to_the_gate() {
    let mut app = test_app();
    let handle = app.mark_secret("hunter2forget-not-real", true).unwrap();
    app.dispatch_command_for_test(&format!("nosecret {handle}"));
    let v = app
        .rules
        .decide_line(&format!("curl -H {handle} https://x"));
    assert!(
        v.stages().iter().any(|s| s.acts.iter().any(|a| matches!(
            a.act,
            shannon_core::permission::ActKind::Secret { known: false, .. }
        ))),
        "the gate must call it unknown now: {:?}",
        v.stages()
    );
}

/// **Password protection is off by default, and a file that turns it on
/// moves nothing while sealing is unbuilt** — it says so, rather than read
/// as done (the plan's P5 seals there). The secret stays where it was, and
/// the next mark is kept there too.
#[test]
fn password_protect_from_a_file_says_it_is_not_built_and_moves_nothing() {
    assert!(
        !Config::default().secrets.password_protect,
        "off by default"
    );
    let dir = TempDir::new().unwrap();
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let mut app = test_app_with_vault(
        Config::default(),
        Box::new(shannon_core::MemoryStore::new()),
        vault.clone(),
    );
    let first = app.mark_secret("hunter2", true).expect("marked");
    let file = dir.path().join("protect.toml");
    std::fs::write(&file, "[secrets]\npassword_protect = true\n").unwrap();
    app.dispatch_command_for_test(&format!("source {}", file.display()));
    assert!(
        app.config.secrets.password_protect,
        "premise: the file set it"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("not built yet"), "{said}");
    let second = app.mark_secret("s3cr3t-token", true).expect("marked");
    for handle in [&first, &second] {
        assert!(
            shannon_core::Vault::recall(vault.as_ref(), handle)
                .unwrap()
                .is_some(),
            "{handle} kept where it always is"
        );
    }
}

/// **`:nosecret {{handle}}` reaches a copy that no session here holds** —
/// one kept by a session not open now, which nothing brings back into this
/// one: the store is asked by the handle (review stop 6, F 2: "no secret
/// here is that", and the copy stayed). A word that is not a handle (it may
/// be a value) asks nothing.
#[test]
fn nosecret_takes_a_kept_copy_no_session_holds() {
    use shannon_core::Vault as _;
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let handle = "{{secret.f00d}}";
    let kept = shannon_core::Kept {
        name: "secret".into(),
        value: shannon_core::Secret::new("hunter2-elsewhere"),
        spendable: true,
    };
    vault.keep(handle, &kept).unwrap();
    let mut app = test_app_with_vault(
        Config::default(),
        Box::new(shannon_core::MemoryStore::new()),
        vault.clone(),
    );
    assert!(
        !app.aliases.knows(handle),
        "premise: no session here holds it"
    );
    assert_eq!(vault.len(), 1, "premise: the copy is kept");

    app.dispatch_command_for_test("nosecret hunter2-elsewhere");
    assert_eq!(vault.len(), 1, "a value is not a handle: nothing asked");

    app.dispatch_command_for_test(&format!("nosecret {handle}"));
    let said = app.notice().unwrap_or_default();
    assert_eq!(vault.len(), 0, "taken out: {said:?}");
    assert!(said.contains("no copy of it is kept now"), "{said:?}");
}

/// **A later mark never overwrites an earlier run's** under a shared handle.
/// The nonce is 16 bits, so over months the same name meets the same handle;
/// keeping a second value there would make an old session's commands spend
/// the new one. The mark takes the next free name instead.
#[test]
fn a_mark_skips_a_name_whose_handle_was_kept_for_another_value() {
    use shannon_core::Vault as _;
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let mut app = test_app_with_vault(
        Config::default(),
        Box::new(shannon_core::MemoryStore::new()),
        vault.clone(),
    );
    // Seeds this run's table, so the handle computed below is the one a mark
    // named `secret-2` would really get.
    let first = app
        .mark_secret("the first value here", true)
        .expect("marked");
    assert!(first.starts_with("{{secret."), "premise: {first}");
    let next = (*app.aliases).clone().secret_handle("secret-2").unwrap();
    let earlier = shannon_core::Kept {
        name: "secret-2".into(),
        value: shannon_core::Secret::new("an earlier run's value"),
        spendable: true,
    };
    vault.keep(&next, &earlier).unwrap();

    let handle = app
        .mark_secret("the second value here", true)
        .expect("marked");
    assert!(
        handle.starts_with("{{secret-3."),
        "skipped the kept name: {handle}"
    );
    let still = vault.recall(&next).unwrap().expect("still kept");
    assert_eq!(
        still.value.expose(),
        "an earlier run's value",
        "not overwritten"
    );
}

/// **A kept handle this run holds for ANOTHER value is not restored** — a
/// 16-bit collision with a secret lent this run. Restoring would make the old
/// history's handle silently mean a different secret; it is said instead.
#[test]
fn a_kept_handle_held_this_run_for_another_value_is_not_restored() {
    use shannon_core::Vault as _;
    let vault = Arc::new(shannon_core::MemoryVault::default());
    let mut app = test_app_with_vault(
        Config::default(),
        Box::new(shannon_core::MemoryStore::new()),
        vault.clone(),
    );
    // Quote-free values: a quote is refused at splice time, which is not
    // what this test is about.
    let handle = app.mark_secret("this run value", true).expect("marked");
    let other = shannon_core::Kept {
        name: "secret".into(),
        value: shannon_core::Secret::new("another run value"),
        spendable: true,
    };
    vault.keep(&handle, &other).unwrap();
    let episode = commit_a_turn_naming(&mut app, &handle);
    let (_, turns) = load_full_session(app.store.as_ref(), &episode).expect("loads");

    app.notices.clear();
    let recall = app.unlock_turns(&turns).expect("unprotected");
    app.adopt_recall(recall);

    let run = app.aliases.expand_at_exec(&handle).expect("resolves");
    assert_eq!(run.text, "this run value", "this run's meaning stands");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("not restored") && notice.contains(&handle),
        "{notice}"
    );
}

/// The value the display-scrub tests below hold, marked on `app`: a plain
/// word no pattern knows, so only the mark can mask it.
const HELD_WORD: &str = "plainword7";

/// **A held value in a call that is no command line is masked on its
/// records** (review stop 17, E1 H1): `given_call`'s scrub is the only one a
/// `Write` passes, and deleting it left the suite green.
#[tokio::test]
async fn a_held_value_in_a_file_call_is_masked_on_its_records() {
    let (mut app, _provider) = provider_app();
    app.mark_secret(HELD_WORD, true).expect("marked");
    let asking = |n: u8| {
        serde_json::json!({
            "file_path": format!("/tmp/shannon-test-absent-{n}/{HELD_WORD}.txt"),
            "content": format!("the word is {HELD_WORD}\n"),
        })
    };
    provider_asks(&mut app, "tu_1", "Write", asking(1));
    assert!(app.shown_call().is_some(), "premise: it asks");
    tool_key(&mut app, 'a');
    provider_asks(&mut app, "tu_2", "Write", asking(2));
    tool_key(&mut app, 'd');
    let shown = app.output().lines().join("\n");
    assert!(shown.contains("DENIED"), "premise: recorded: {shown}");
    assert!(!shown.contains(HELD_WORD), "{shown}");
}

/// The "what actually ran" line of the note on screen. It is a command's
/// output, so a held handle draws as its value (`:help`; the stub's `value
/// (handle)`): the handle is the sign the note was scrubbed, and an unscrubbed
/// one holds the bare value.
fn note_on_screen(app: &App) -> String {
    let shown = app.output().lines().join("\n");
    (shown.lines())
        .find(|line| line.contains("what actually ran:"))
        .unwrap_or_else(|| panic!("the note, on screen: {shown}"))
        .to_string()
}

/// **A held value typed into an edit is masked in the note the screen shows**
/// (review stop 17, E1 H1): Claude Code's note and Shannon's own each draw
/// their copy through a scrub of their own, apart from the copy sent.
#[tokio::test]
async fn a_held_value_typed_into_an_edit_is_masked_on_the_screen() {
    use shannon_core::Block;
    let edited = format!("ls /tmp/{HELD_WORD}");
    let input = serde_json::json!({"command": "ls /tmp/a", "description": "d"});
    let (mut app, _provider) = provider_app();
    app.mark_secret(HELD_WORD, true).expect("marked");
    provider_asks(&mut app, "tu_1", "Bash", input.clone());
    let seed = app.shown_call().expect("asks").command().to_string();
    app.apply_edit_for_test(&seed, &edited);
    tool_key(&mut app, 'a');
    app.feed_stream(StreamMsg::ToolRecord {
        call: Block::ToolUse {
            id: "tu_1".into(),
            name: "Bash".into(),
            input,
        },
        result: Block::ToolResult {
            tool_use_id: "tu_1".into(),
            content: "out\n".into(),
            is_error: false,
        },
    });
    let note = note_on_screen(&app);
    assert!(note.contains("{{secret."), "{note}");

    let mut app = test_app();
    app.mark_secret(HELD_WORD, true).expect("marked");
    model_call(&mut app, "tu_1", "ls ~/src");
    let seed = (app.shown_call().expect("premise: it asks"))
        .command()
        .to_string();
    app.apply_edit_for_test(&seed, &edited);
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    let note = note_on_screen(&app);
    assert!(note.contains("{{secret."), "{note}");
}

/// **A job held on a call does not say its held value** (review stop 17, E1
/// H1).
#[tokio::test]
async fn a_held_job_masks_a_held_value_in_its_notice() {
    let mut app = test_app();
    app.mark_secret(HELD_WORD, true).expect("marked");
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look at something", None)
        .expect("spawns");
    let jobs = app.generation_ids_for_test();
    app.notices.clear();
    app.gate_job_tool_call_for_test(jobs[0], &format!("mkdir {HELD_WORD}"));
    let notice = app.notice().unwrap_or_default().to_string();
    assert!(notice.contains("held on"), "premise: {notice}");
    assert!(!notice.contains(HELD_WORD), "{notice}");
}

/// **A rule said over an open call masks a held value** (review stop 17, E1
/// H1): `revisit_open_decision` names the call it re-read.
#[tokio::test]
async fn a_rule_said_over_an_open_call_masks_a_held_value() {
    let mut app = test_app();
    app.mark_secret(HELD_WORD, true).expect("marked");
    let command = format!("rm -rf /tmp/shannon-test-absent-{HELD_WORD}");
    model_call(&mut app, "tu_1", &command);
    app.dispatch_command_for_test("allow ls *");
    assert!(app.shown_call().is_some(), "premise: still open");
    let notice = app.notice().unwrap_or_default().to_string();
    assert!(
        notice.contains("still asks") && notice.contains("rm -rf"),
        "premise: the call is named: {notice}"
    );
    assert!(!notice.contains(HELD_WORD), "{notice}");
}

/// **The pane waiting on a running call masks a held value** (review stop
/// 17, E1 H1).
#[tokio::test]
async fn the_waiting_pane_masks_a_held_value() {
    let mut app = test_app();
    app.mark_secret(HELD_WORD, true).expect("marked");
    let command = format!("rm -rf /tmp/shannon-test-absent-{HELD_WORD}");
    model_call(&mut app, "tu_1", &command);
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
    tool_key(&mut app, 'a');
    let (_, body) = app.wait_text_for_test().expect("it waits");
    assert!(body.contains("running  rm -rf"), "premise: {body}");
    assert!(!body.contains(HELD_WORD), "{body}");
}
