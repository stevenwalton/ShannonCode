//! Teaching the gate: `[permissions]`, `:allow`/`:deny`/`:ask`, `:rules`, a policy revisited.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **A broad allow the user taught answers a shipped ask, never the shipped
/// deny** (Steven, 10-06: "`allow find *` should accept all flags … a user can
/// override"); a narrow one leaves the ask asking. Each by its DECISION: this
/// asserted only "a prompt appeared", which held for a deny and for an ask
/// alike (Reviewers A and C, round 18).
#[tokio::test]
async fn a_taught_allow_answers_a_shipped_ask_never_the_deny() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    app.dispatch_command_for_test("allow rm:*");
    model_call(&mut app, "tu_1", "rm -rf --no-preserve-root /tmp/x");
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Deny),
        "the one shipped deny still refuses under `allow rm:*`"
    );
    assert!(app.tool_results.is_empty(), "and nothing ran");

    let mut app = test_app();
    app.dispatch_command_for_test("allow find .");
    model_call(&mut app, "tu_1", "find . -delete");
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Ask),
        "`-delete` still asks under `allow find .`, which does not name it"
    );
    assert!(app.tool_results.is_empty(), "and nothing ran");

    // `allow find:*` answers the flag; the place it deletes is the path
    // rules' (10-13: as `allow rm *` with `rm -rf build`).
    let mut app = test_app();
    app.dispatch_command_for_test("allow find:*");
    model_call(&mut app, "tu_1", "find build -delete");
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Ask),
        "the place is not granted"
    );
    let mut app = test_app();
    app.dispatch_command_for_test("allow find:*");
    app.dispatch_command_for_test("allow :delete build");
    model_call(&mut app, "tu_1", "find build -delete");
    assert!(
        app.shown_call().is_none(),
        "the flag and the place answer it"
    );
}

/// **`A` never puts a credential on the `:` line** (Apiiro, 11-08: Claude
/// Code's "don't ask again" saved approved commands, passwords included, in
/// project files people commit; here `:config save` would). The pane offers
/// no rule and says why; `A` opens nothing and teaches nothing; the reason,
/// on the pane and in the notice, names where the value sat — never the
/// value. The command itself is still drawn whole: the user sees values.
#[tokio::test]
async fn a_offers_no_rule_holding_a_credential() {
    let mut app = test_app();
    model_call(
        &mut app,
        "tu_1",
        "docker login --password hunter2 registry.example.org",
    );
    let frame = frame_of(&mut app, 140, 40);
    let screen: String = (0..frame.area().height)
        .map(|y| {
            (0..frame.area().width)
                .map(|x| frame[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !screen.contains("A adds") && screen.contains("credential"),
        "the pane offers no rule, and says why:\n{screen}"
    );
    tool_key(&mut app, 'A');
    assert_eq!(app.cmdline.line(), None, "nothing opened to teach");
    assert!(app.session_rules.is_empty(), "nothing taught");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("the value of `--password`") && !notice.contains("hunter2"),
        "{notice:?}"
    );
    assert!(app.shown_call().is_some(), "the decision stays");
}

/// **A typed rule holding a credential is refused, whatever its verb** —
/// `:allow`, `:ask` and `:deny` are the one door into the session's rules
/// (`session_rules` has one writer), and so into what `:config save` writes
/// beside the project's code. Written into its flag (`--api-key=x`) the value
/// IS in the rule, unlike a proposal's `*`. The refusal says where it sat,
/// never the value; an ordinary rule is taught as before.
#[tokio::test]
async fn a_typed_rule_holding_a_credential_is_refused() {
    for line in [
        "allow docker login --password hunter2 registry.example.org *",
        shannon_core::fakes::DENY_PSQL_WITH_URL_PASSWORD,
        "ask deploy-tool --api-key=hunter2 *",
        // In the rule's comment, which no command reading sees: the
        // redactor over the whole text.
        shannon_core::fakes::ALLOW_WITH_URL_PASSWORD_IN_COMMENT,
    ] {
        let mut app = test_app();
        app.dispatch_command_for_test(line);
        assert!(app.session_rules.is_empty(), "taught: {line}");
        let notice = app.notice().unwrap_or_default();
        assert!(
            notice.contains("holds a credential") && !notice.contains("hunter2"),
            "{line}: {notice:?}"
        );
    }
    // A value marked a secret, typed into a rule: refused as one, however
    // ordinary it looks. (A PLAIN alias's value is no credential —
    // `a_deny_holds_against_a_handle_typed_in_an_edit` teaches one.)
    let mut app = test_app();
    let secret = "plainword7";
    app.mark_secret(secret, true).expect("marked");
    app.dispatch_command_for_test(&format!("allow deploy-tool {secret} *"));
    assert!(app.session_rules.is_empty(), "a marked value taught");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("a secret Shannon holds") && !notice.contains(secret),
        "{notice:?}"
    );
    // Behind a wide character, refused the same way: this panicked, its
    // message holding the value (review stop 17, B F1).
    app.dispatch_command_for_test(&format!("allow deploy-tool é{secret} *"));
    assert!(app.session_rules.is_empty(), "a marked value taught");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("a secret Shannon holds") && !notice.contains(secret),
        "{notice:?}"
    );
    let mut app = test_app();
    app.dispatch_command_for_test("allow cargo test *");
    assert_eq!(app.session_rules.len(), 1, "an ordinary rule is taught");
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("`:rules` lists the policy"), "{notice:?}");
}

/// **`redact = false` is about what leaves, not what a rule keeps** (review
/// stop 17, E1 L6; Steven, 11-09: "let's refuse both"). With the switch off,
/// a `--password`'s value was refused and a URL's password taught: the
/// named arm never asked the switch, the shaped arm did. Both are refused,
/// in a rule's words and in the comment no command reading sees.
#[tokio::test]
async fn a_rule_holding_a_credential_is_refused_with_redaction_off() {
    let mut app = test_app();
    let mut config = (*app.config).clone();
    config.secrets.redact = false;
    let _ = app.adopt_config(config);
    assert!(!app.config.secrets.redact, "premise: off");
    for line in [
        "allow deploy --password hunter2 *",
        shannon_core::fakes::DENY_PSQL_WITH_URL_PASSWORD,
        shannon_core::fakes::ALLOW_WITH_URL_PASSWORD_IN_COMMENT,
    ] {
        app.dispatch_command_for_test(line);
        assert!(app.session_rules.is_empty(), "taught: {line}");
        let notice = app.notice().unwrap_or_default();
        assert!(
            notice.contains("holds a credential") && !notice.contains("hunter2"),
            "{line}: {notice:?}"
        );
    }
}

/// **What the gate calls a credential is kept masked** (review stop 17, E1
/// M1): `A` refused to make a rule of `--password correcthorse9staple` while
/// the kept call held it, and so every later request. The call runs as typed;
/// what is kept, and so sent again, reads the value's place.
#[tokio::test]
async fn a_credential_after_its_flag_is_kept_masked() {
    let value = "correcthorse9staple";
    let line = format!("mysql --password {value} -h db.example.org");
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    assert!(
        app.scrub(&line).contains(value),
        "premise: free text has no position"
    );
    model_call(&mut app, "tu_1", &line);
    // Its round finished — denied, nothing run — the call is what the next
    // request carries, kept as it arrived.
    tool_key(&mut app, 'd');
    let sent = serde_json::to_string(&app.conversation().snapshot()).expect("json");
    assert!(
        sent.contains("tool_use"),
        "premise: the call is kept: {sent}"
    );
    assert!(
        sent.contains("mysql --password [redacted: named secret] -h db.example.org"),
        "{sent}"
    );
    assert!(!sent.contains(value), "{sent}");
}

/// **A refused rule leads with the way forward, and that way works** (review
/// stop 17, F M2). The remedy came last, past the cut on a narrow row, and
/// was to `:secret` the value, which refuses the same again as a value and,
/// as a handle, makes a rule that lets nothing run unasked.
#[tokio::test]
async fn a_refused_rule_says_how_to_write_it() {
    let mut app = test_app();
    app.dispatch_command_for_test("allow mysql --password hunter2 *");
    assert!(app.session_rules.is_empty(), "premise: refused");
    let notice = app.notice().unwrap_or_default().to_string();
    let forward = notice.find("write `*` in its place").expect("the way");
    let warning = notice.find("`:config save`").expect("and why");
    assert!(forward < warning, "the way first: {notice}");
    app.dispatch_command_for_test("allow mysql --password * *");
    assert_eq!(app.session_rules.len(), 1, "{:?}", app.notice());
}

/// **A rule's own grammar is no way around the door** (review stop 17, E1
/// M3): a line the shell cannot read was answered "no credential here", and
/// a held value found only by the whole-text pass was read off its `kinds`,
/// where a held value is not. In a comment no command reading sees, it is
/// the `masked` list that says so.
#[tokio::test]
async fn a_typed_rule_the_shell_cannot_read_is_still_asked() {
    let secret = "plainword7";
    for line in [
        "allow deploy --password hunter2 '".to_string(),
        format!("allow deploy {secret} '"),
        format!("allow cargo test * # {secret}"),
    ] {
        let mut app = test_app();
        app.mark_secret(secret, true).expect("marked");
        app.dispatch_command_for_test(&line);
        assert!(app.session_rules.is_empty(), "taught: {line}");
        let notice = app.notice().unwrap_or_default();
        assert!(
            notice.contains("holds a credential")
                && !notice.contains("hunter2")
                && !notice.contains(secret),
            "{line}: {notice:?}"
        );
    }
}

/// **`A` opens a rule to edit; Enter teaches it, and the rules it offers
/// answer this call** (Steven, 2026-09-30: the rule taught must be the
/// user's to edit). The keystroke alone grants nothing — Esc leaves the
/// decision open. A call that needs a program's form AND a place takes two
/// rules (Steven, 10-10: program rules are about flags and arguments, path
/// rules about access): the pane says both before the first `A`, the first
/// leaves the call asking for the second, and the second answers it.
#[tokio::test]
async fn a_opens_a_rule_to_edit_and_enter_teaches_it() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    let frame = frame_of(&mut app, 140, 40);
    let screen: String = {
        let buf = &frame;
        (0..buf.area().height)
            .map(|y| {
                (0..buf.area().width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        screen.contains("A adds, this session: allow rm -r -f")
            && screen.contains("then A adds:          allow :delete /tmp/shannon-test-absent"),
        "both rules said before the first `A`:\n{screen}"
    );
    tool_key(&mut app, 'A');
    assert_eq!(app.cmdline.line(), Some("allow rm -r -f"));
    assert!(
        app.shown_call().is_some(),
        "the keystroke alone grants nothing"
    );
    assert!(tool_result(&app).is_none(), "nothing ran");

    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.command_input(esc), None);
    assert!(app.shown_call().is_some(), "Esc: the decision stays");
    assert!(app.session_rules.is_empty(), "Esc: nothing taught");

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    tool_key(&mut app, 'A');
    let cmd = app.command_input(enter).expect("Enter runs the line");
    app.dispatch_command_for_test(&cmd);
    assert!(
        app.shown_call().is_some(),
        "the flags taught, the place asks"
    );
    tool_key(&mut app, 'A');
    assert_eq!(
        app.cmdline.line(),
        Some("allow :delete /tmp/shannon-test-absent")
    );
    let cmd = app.command_input(enter).expect("Enter runs the line");
    app.dispatch_command_for_test(&cmd);
    assert!(
        app.shown_call().is_none(),
        "the rules answered the open call"
    );
    settle(&mut app).await;
    assert!(tool_result(&app).is_some(), "it ran");

    // Taught: the same command runs without asking; a neighbour still asks.
    app.tool_results.clear();
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call().is_none(), "taught — no prompt");
    model_call(&mut app, "tu_3", "rm -rf /tmp/shannon-test-absent-2");
    assert!(
        app.shown_call().is_some(),
        "the rule names its operand, not its neighbours"
    );
}

/// A rule that does not cover the open call leaves it open — re-gated, and
/// said, with the reason it still asks — rather than answering it.
#[tokio::test]
async fn a_rule_that_does_not_cover_the_open_call_leaves_it_asking() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    app.dispatch_command_for_test("allow ls *");
    assert!(app.shown_call().is_some(), "still open");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("still asks") && notice.contains("outside where Shannon started"),
        "{notice:?}"
    );
    // What came of it leads: a row clips its end, and a command can be
    // hundreds of characters (review stop 17, F M8).
    let (outcome, command) = (notice.find("still asks"), notice.find("(`rm -rf"));
    assert!(outcome.is_some() && outcome < command, "{notice:?}");
    assert!(!notice.contains("lists the policy"), "{notice:?}");
    assert!(tool_result(&app).is_none(), "nothing ran");

    // A deny that covers it answers it: denied, nothing ran — and SAID, in
    // the line `:deny` answers with.
    app.dispatch_command_for_test("deny rm *");
    assert!(app.shown_call().is_none(), "answered");
    let (_, is_error) = tool_result(&app).expect("the call has its answer");
    assert!(is_error, "a denial");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("the open call is denied"),
        "{:?}",
        app.notice()
    );
}

/// **A call that asks whatever a rule allows says so** — at the legend, at
/// `A`, and after a rule is taught. Each used to say "your rule does not cover
/// it" (or blame a glob), which sent the user to widen a rule that could
/// never help (Reviewer F, round 16).
/// **A rule that asks says what lifts it** — not "no rule can stop this
/// asking", which read as permanent while `:unask` takes it out (review 20,
/// Reviewer F). A SHIPPED ask an allow answers (Steven, 10-06), so `A`
/// offers that allow, naming the flag; your own ask no allow outranks.
#[tokio::test]
async fn a_rule_that_asks_says_what_takes_it_out() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "find . -delete");
    assert!(
        app.shown_call().is_some(),
        "premise: the model's default asks"
    );
    tool_key(&mut app, 'A');
    // The form that asked: the flag that made `find` a deleter. (`.` is a
    // read of the directory, allowed on its own, so the rule does not name
    // it — the files `-delete` removes are not written in the line.)
    assert_eq!(app.cmdline.line(), Some("allow find -delete"));

    // Your own ask on the same form: no narrower allow exists, so what
    // takes the rule out is said instead.
    let mut app = test_app();
    app.dispatch_command_for_test("ask find:-delete");
    model_call(&mut app, "tu_1", "find . -delete");
    assert!(app.shown_call().is_some(), "premise: your ask");
    tool_key(&mut app, 'A');
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("no allow lifts this") && notice.contains("`:unask find:-delete`"),
        "{notice:?}"
    );
}

#[tokio::test]
async fn a_call_no_rule_can_answer_says_why_and_offers_no_rule() {
    // A privilege wrapper: a hard ask, which no allow lifts. (A credential
    // path is no longer one: a grant naming the place lifts its guard, and
    // `A` offers that grant.)
    let mut app = test_app();
    model_call(&mut app, "tu_1", "sudo cat /etc/hosts");
    assert!(app.shown_call().is_some(), "premise: sudo asks");
    assert!(
        !legend(&app).contains("A always"),
        "no `A` to offer: {}",
        legend(&app)
    );

    tool_key(&mut app, 'A');
    assert_eq!(app.cmdline.line(), None, "nothing to edit");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("no allow lifts this") && notice.contains("`sudo`"),
        "{notice:?}"
    );
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));

    app.dispatch_command_for_test("allow cat *");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("still asks") && notice.contains("`sudo`"),
        "{notice:?}"
    );
    assert!(!notice.contains("does not cover"), "{notice:?}");
}

/// **The legend shows the rule `A` would teach, and its scope**, before the
/// key is pressed — it lost "this session", and advertised `A` where the key
/// was refused (Reviewer F, round 16).
#[tokio::test]
async fn the_legend_shows_the_rule_a_would_teach() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    assert!(
        legend(&app).contains("A always, this session: allow rm -r -f"),
        "{}",
        legend(&app)
    );
}

/// **On a line of several commands, the one still asking is named**, and `A`
/// then proposes ITS rule.
#[tokio::test]
async fn teaching_one_stage_names_the_stage_still_asking() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/zz-a && rm -rf /tmp/zz-b");
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    // The first stage's flags, then its place.
    for want in ["allow rm -r -f", "allow :delete /tmp/zz-a"] {
        tool_key(&mut app, 'A');
        assert_eq!(app.cmdline.line(), Some(want));
        let cmd = app.command_input(enter).expect("Enter runs the line");
        app.dispatch_command_for_test(&cmd);
    }
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("`rm -rf /tmp/zz-b` still asks"),
        "{notice:?}"
    );
    tool_key(&mut app, 'A');
    assert_eq!(app.cmdline.line(), Some("allow :delete /tmp/zz-b"));
}

/// **A rule arriving by file answers the open call too** — through the same
/// door `:allow` uses. A `:source`d deny left the pane offering `a`, and `a`
/// ran a command the policy now denied (Reviewer A, round 16). And the model
/// is told the RULE denied it, not the user.
#[tokio::test]
async fn a_deny_arriving_by_source_answers_the_open_call() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call().is_some(), "premise: it asks");

    let file = dir.path().join("deny.toml");
    std::fs::write(&file, "[permissions]\nrules = [\"deny rm *\"]\n").unwrap();
    app.dispatch_command_for_test(&format!("source {}", file.display()));

    assert!(app.shown_call().is_none(), "answered by the new policy");
    let (content, is_error) = tool_result(&app).expect("the call has its answer");
    assert!(is_error, "a denial");
    assert!(
        content.contains("denied by rule") && content.contains("deny rm *"),
        "the model is told which rule: {content:?}"
    );
    assert!(
        content.contains("(your config)"),
        "and where it came from: {content:?}"
    );
    assert!(!content.contains("by the user"), "{content:?}");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("the open decision"),
        "and the user is told: {:?}",
        app.notice()
    );
}

/// **A policy change that leaves the verdict standing still refreshes the
/// pane.** `:ask touch …`, the model calls it, `:unask touch …`: the verdict
/// stays Ask (a write), so nothing re-gated — the pane cited the removed
/// rule, and `A` explained a stored cause by INDEX into the shrunken policy
/// and took Shannon down (Reviewers A, D, round 17).
#[tokio::test]
async fn a_rule_taken_out_under_an_open_decision_refreshes_it() {
    let mut app = test_app();
    app.dispatch_command_for_test("ask touch /tmp/zz-x");
    model_call(&mut app, "tu_1", "touch /tmp/zz-x");
    assert!(app.shown_call().is_some(), "premise: it asks");
    let shown = |app: &App| {
        let prompt = app.shown_call().unwrap();
        prompt
            .text()
            .lines()
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        shown(&app).contains("ask touch /tmp/zz-x"),
        "premise: cites the rule"
    );

    app.dispatch_command_for_test("unask touch /tmp/zz-x");
    assert!(app.shown_call().is_some(), "still asked: a write");
    assert!(
        !shown(&app).contains("ask touch /tmp/zz-x"),
        "the pane cites a rule that is gone:\n{}",
        shown(&app)
    );
    tool_key(&mut app, 'A');
    assert_eq!(
        app.cmdline.line(),
        Some("allow touch /tmp/zz-x"),
        "now a rule can answer it: {:?}",
        app.notice()
    );
}

/// **A deny pane is not answered by an unrelated rule.** The recheck after
/// `:allow` resolved every Deny it found, so an `:allow` for something else
/// closed the open deny and took its `e` away (Reviewer A, round 17).
#[tokio::test]
async fn an_unrelated_allow_leaves_an_open_deny_open() {
    let mut app = test_app();
    app.dispatch_command_for_test("deny touch /tmp/zz-y");
    model_call(&mut app, "tu_1", "touch /tmp/zz-y");
    let decision = app.shown_call().map(|p| p.decision());
    assert_eq!(
        decision,
        Some(shannon_core::permission::Decision::Deny),
        "premise: a deny pane"
    );
    app.dispatch_command_for_test("allow ls *");
    assert!(app.shown_call().is_some(), "the deny pane stays open");
    assert!(
        app.notice().unwrap_or_default().contains("is still denied"),
        "{:?}",
        app.notice()
    );
}

/// A change that leaves the verdict alone leaves the decision alone: `:set`
/// of something unrelated must not re-gate, resolve, or announce anything.
#[tokio::test]
async fn an_unrelated_setting_leaves_the_open_decision_alone() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    app.notices.clear();
    app.dispatch_command_for_test("set tui.prompt.height=7");
    assert!(app.shown_call().is_some(), "still open");
    assert!(
        !app.notice().unwrap_or_default().contains("open decision"),
        "{:?}",
        app.notice()
    );
}

/// A command denied by a standing rule offers NO allow key: the way past a
/// rule is to change it deliberately, not to answer a prompt.
#[tokio::test]
async fn a_denied_command_cannot_be_allowed_at_the_prompt() {
    let mut app = test_app();
    // The shipped deny (find's carvings ask since 2026-10-02).
    model_call(
        &mut app,
        "tu_1",
        "rm --no-preserve-root -rf /tmp/shannon-test-absent",
    );
    let prompt = app.shown_call().expect("gated");
    assert_eq!(prompt.decision(), shannon_core::permission::Decision::Deny);

    tool_key(&mut app, 'a');
    assert!(app.shown_call().is_some(), "`a` must not resolve a deny");
    tool_key(&mut app, 'A');
    assert!(app.shown_call().is_some(), "`A` must not resolve a deny");
    assert!(tool_result(&app).is_none(), "nothing ran");
    // The refusal is visible — a key that does nothing silently is a bug.
    assert!(app.notice().is_some(), "the refusal says why");

    // Denying still works.
    tool_key(&mut app, 'd');
    assert!(app.shown_call().is_none());
}

/// **`:config edit` applying a `[secrets]` change really changes the gate.**
///
/// `RuleSet` snapshots `[secrets]` at construction, so assigning `self.config`
/// left the permission engine on the old policy while the app reported "config
/// saved and applied". A user who reads the docs, decides `~/.ssh` should be a
/// hard boundary, sets `path_policy = "deny"` and is told it applied still had
/// every credential read merely `Ask` for the rest of the session — and `Ask`
/// is exactly the rung approval fatigue can spend (Reviewer E, executed).
///
/// Asserting on the **decision**, not on `app.config`: the config field was
/// always updated correctly, which is why this survived. The question is
/// whether anything derived from it was rebuilt.
#[tokio::test]
async fn editing_the_secrets_policy_changes_what_the_gate_decides() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let probe = "cat ~/.ssh/id_rsa";
    assert_eq!(
        app.rules.decide_line(probe).decision,
        shannon_core::permission::Decision::Ask,
        "premise: the shipped default asks about a credential read"
    );

    let path = dir.path().join("config.toml");
    app.save_config(&path, "[secrets]\npath_policy = \"deny\"\n");
    app.source_file(shannon_core::config::Origin::User, &path); // `:reload`

    assert_eq!(
        app.config.secrets.path_policy,
        shannon_core::PathPolicy::Deny,
        "premise: the config itself really did change"
    );
    assert_eq!(
        app.rules.decide_line(probe).decision,
        shannon_core::permission::Decision::Deny,
        "and the engine derived from it was rebuilt — saying `applied` is a \
         claim about every derived structure, not about the struct assigned"
    );
}

/// **The scratch directory launch vouches for** reaches each reader: the
/// rules approve it, and the model is told it (Steven, 10-06). Where Shannon
/// started is named from `App::start`, the one answer.
#[test]
fn the_scratch_directory_reaches_the_rules_and_the_model() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    let dir = TempDir::new().unwrap();
    let scratch = dir.path().display().to_string();
    let read = format!("cat {scratch}/notes.txt");
    assert_eq!(
        app.rules.decide_line(&read).decision,
        Decision::Ask,
        "premise: not yet approved"
    );
    app.set_scratch(vec![scratch.clone()]);
    assert_eq!(app.rules.decide_line(&read).decision, Decision::Allow);
    let req = app.tool_request(
        vec![Message::user("hi")],
        &app.sealing_for("m".into(), None),
        false,
    );
    let system = req.system.expect("a system prompt");
    assert!(system.contains(&scratch), "{system}");
    let start = app.start.as_ref().expect("a start").display().to_string();
    assert!(system.contains(&start), "{system}");
    // A rebuild keeps it (`rebuild_rules` reads `App::scratch`).
    app.dispatch_command_for_test("allow ls *");
    assert_eq!(app.rules.decide_line(&read).decision, Decision::Allow);
}

/// **A config edit cannot un-protect Shannon's own state** —
/// `SECURITY-2026-09-24-config-edit-drops-own-state.md`, as a test.
///
/// The registrations (config file, logs, episode store, API key file) used to
/// be pushed into `config.secrets.paths`. `:config edit` replaces the config
/// with the file as parsed, which never held them, and rebuilt the gate from
/// it — so a registered path went from `Ask` to `Allow` for the rest of the
/// session (probed: exactly that). Driven through `protect_paths`, the door
/// `main.rs` uses, and the real edit path. The path is outside every shipped
/// pattern, so the registration is the ONLY thing that can make it `Ask`.
#[test]
fn a_config_edit_keeps_shannons_own_paths_protected() {
    use shannon_core::permission::{By, Decision};
    let path = "/opt/creds/shannon-token";
    let mut app = test_app();
    // By the rule cited: a read outside where Shannon started asks of itself
    // (2026-10-05), so the decision alone no longer says the registration is
    // what holds it — the registered path is a shipped guard naming it.
    let guarded = |app: &App| {
        let v = app.rules.decide_line(&format!("cat {path}"));
        v.decision != Decision::Allow
            && v.stages().iter().any(|s| {
                s.acts.iter().any(|a| {
                    matches!(&a.by, By::Rules(cited) if cited.iter().any(|c| c.source.contains(path)))
                })
            })
    };
    assert!(!guarded(&app), "premise: no shipped pattern covers it");
    app.protect_paths(vec![path.to_string()]);
    assert!(guarded(&app), "premise: registered, it asks as one");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("shannon/config.toml");
    app.save_config(&file, "[tui]\nscrolloff = 2\n");
    app.source_file(shannon_core::config::Origin::User, &file); // `:reload`
    assert_eq!(app.config.tui.scrolloff, 2, "premise: it applied");
    assert!(
        guarded(&app),
        "after a config edit, Shannon's own paths must still ask"
    );
    assert!(
        !app.config.secrets.paths.iter().any(|p| p == path),
        "and they live apart from the user's config, not in it"
    );
}

/// **The redaction notice says where the value went, truthfully.** It said
/// "(kept on this machine)" while the output was scrubbed before anything
/// recorded it — a secret a command printed once was simply gone. Three cases,
/// three different facts, and a copy that could not be written must never
/// read as kept.
#[test]
fn the_redaction_notice_says_where_the_value_went() {
    let notice_for = |kept: Option<Result<String, String>>| {
        let mut app = test_app();
        app.handle_exec_outcome(shannon_core::ExecOutcome {
            tool_use_id: "toolu_01".to_string(),
            command: "aws iam create-access-key".to_string(),
            output: "[redacted: high-entropy string]".to_string(),
            code: Some(0),
            timed_out: false,
            truncated: false,
            spawn_error: None,
            redacted: vec!["high-entropy string".to_string()],
            spent: Vec::new(),
            attempts: 1,
            kept,
            left: shannon_core::LeftBehind::Nothing,
            file: None,
        });
        app.messages().collect::<Vec<_>>().join("\n")
    };

    let off = notice_for(None);
    assert!(off.contains("not stored anywhere"), "{off}");
    assert!(
        !off.contains("kept on this machine"),
        "the old false claim: {off}"
    );

    let kept = notice_for(Some(Ok("/state/redacted/1-toolu_01.txt".to_string())));
    assert!(
        kept.contains("unredacted copy kept at /state/redacted/1-toolu_01.txt"),
        "{kept}"
    );

    let failed = notice_for(Some(Err("permission denied".to_string())));
    assert!(
        failed.contains("NOT kept") && failed.contains("permission denied"),
        "{failed}"
    );
    assert!(
        !failed.contains("kept at"),
        "a failed copy must not read as kept: {failed}"
    );
}

/// The setting is read per run, like the rest of `[secrets]`: on, the executor
/// is handed the record directory; off (the default), it is handed none.
#[test]
fn keep_redacted_reaches_the_executor() {
    let app = test_app();
    assert_eq!(app.exec_config().keep_redacted_in, None, "off by default");

    let mut cfg = Config::default();
    cfg.secrets.keep_redacted = true;
    let app = test_app_with(cfg);
    assert!(
        shannon_core::Config::redacted_record_dir().is_some(),
        "premise: a resolvable directory, or `None == None` proves nothing"
    );
    assert_eq!(
        app.exec_config().keep_redacted_in,
        shannon_core::Config::redacted_record_dir(),
        "on: the one resolver's directory"
    );
}

// --- teaching the gate: [permissions], :allow / :deny / :ask, :rules --------

/// **A rule typed at `:allow` reaches the gate, for the session, and outlives a
/// config change** — the config is replaced by `:set`, `:reload` and `:config
/// edit`, which is why session rules live apart from it.
#[test]
fn a_session_rule_reaches_the_gate_and_survives_a_config_change() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    assert_eq!(
        verdict(&app, "uv run pytest -k x"),
        Decision::Ask,
        "premise"
    );
    app.dispatch_command_for_test("allow uv run pytest *");
    assert_eq!(verdict(&app, "uv run pytest -k x"), Decision::Allow);
    assert_eq!(
        verdict(&app, "uv run python -c 1"),
        Decision::Ask,
        "only what it names"
    );

    app.dispatch_command_for_test("set tui.scrolloff=9");
    assert_eq!(
        verdict(&app, "uv run pytest -k x"),
        Decision::Allow,
        "a config change kept the session's rule"
    );
    app.dispatch_command_for_test("deny uv run pytest *");
    assert_eq!(
        verdict(&app, "uv run pytest -k x"),
        Decision::Deny,
        "deny wins"
    );
}

/// **A rule typed on the `:` line keeps its `|`** — nothing between the
/// command line and the rule reader splits on it (vim's `:bar` is not
/// modeled), so `:deny frobnicate:-X|--method` is one rule naming both
/// spellings, as the manual promises.
#[test]
fn a_typed_rule_keeps_its_alternation() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    app.dispatch_command_for_test("deny frobnicate:-X|--method");
    for line in ["frobnicate -X DELETE", "frobnicate --method DELETE"] {
        assert_eq!(
            app.rules.decide_line(line).decision,
            Decision::Deny,
            "{line}"
        );
    }
}

/// **The gate knows where Shannon started** (`rules_for`): a rule's word
/// holds in a child of it, named absolutely, and not after a `cd` out — the
/// wiring of SECURITY-2026-10-03 #3, which only this path supplies.
#[test]
fn the_gate_knows_where_shannon_started() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    // About where Shannon started, not credentials — and the checkout may
    // itself lie under a credential path (every review worktree is under
    // `.claude/`), which asks first and hid the verdict (review 20, C).
    let mut cfg = (*app.config).clone();
    cfg.secrets.path_policy = shannon_core::PathPolicy::Allow;
    app.config = std::sync::Arc::new(cfg);
    app.rebuild_rules();
    app.dispatch_command_for_test("allow rm");
    app.dispatch_command_for_test("allow :delete passwd");
    assert_eq!(
        app.rules.decide_line("rm passwd").decision,
        Decision::Allow,
        "the file the rule names, where Shannon started"
    );
    // A path rule names ONE place, read from where Shannon started: after a
    // `cd` the same word is another file, and the grant does not reach it
    // (the old engine held a bare word anywhere inside).
    let here = std::env::current_dir().unwrap().display().to_string();
    let inside = format!("cd '{here}/sub' && rm passwd");
    assert_eq!(
        app.rules.decide_line(&inside).decision,
        Decision::Ask,
        "{inside}"
    );
    assert_eq!(
        app.rules.decide_line("cd /etc && rm passwd").decision,
        Decision::Ask
    );
}

/// A rule the gate cannot read is refused with its reason, and nothing is
/// added — never an inert rule that reads as a decision.
#[test]
fn a_malformed_session_rule_is_refused_and_adds_nothing() {
    let mut app = test_app();
    let before = app.rules.rules().len();
    app.dispatch_command_for_test("allow");
    assert!(
        app.notice().unwrap_or_default().contains("needs a rule"),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("deny /bin/rm");
    assert!(
        app.notice().unwrap_or_default().contains(":deny"),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.rules.rules().len(), before);
    // The count above cannot change — the gate skips a line it cannot read —
    // so it never measured "nothing added": the line itself must not be kept,
    // or `:rules` lists an inert rule as policy in force (Reviewer C, round
    // 16: storing before validating survived).
    assert!(app.session_rules.is_empty(), "{:?}", app.session_rules);
    assert!(!app.rules_listing().contains("deny /bin/rm"));
}

/// **A flag matched only as written is said where the rule is typed**:
/// `:deny gh api -X` stops `-X` and not `--method` (Reviewer F, round 16).
#[test]
fn a_session_rule_naming_a_flag_only_as_written_says_so() {
    let mut app = test_app();
    // (`gh` is modelled now, and `-X` resolves; a program Shannon does not
    // model keeps the spelling.)
    app.dispatch_command_for_test("deny frob --fast");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("matched only as written") && notice.contains("`frob`"),
        "{notice:?}"
    );
    app.dispatch_command_for_test("deny rm:-r");
    assert!(
        !app.notice().unwrap_or_default().contains("as written"),
        "a modeled flag needs no note: {:?}",
        app.notice()
    );
    // And where the rule is LISTED, on its own row (review 20, Reviewer C:
    // the `:rules` note was untested).
    let listing = app.rules_listing();
    let row = |rule: &str| {
        listing
            .lines()
            .find(|l| l.trim_start().starts_with(rule))
            .unwrap_or_else(|| panic!("`{rule}` is not listed:\n{listing}"))
            .to_string()
    };
    assert!(row("deny frob --fast").contains("matched only as written"));
    assert!(!row("deny rm:-r").contains("as written"));
}

/// `[permissions]` from the config reaches the gate, and `:rules` lists the
/// policy by source — a removed shipped rule under REMOVED, not SHIPPED.
#[test]
fn the_config_rules_reach_the_gate_and_rules_lists_them_by_source() {
    use shannon_core::permission::Decision;
    let mut cfg = Config::default();
    // A shipped guard removed: a write into the harness surface is then a
    // write inside the project, allowed by default; the other guards stand.
    cfg.permissions.rules = vec!["allow find .".into(), "allwo rm *".into()];
    cfg.permissions.remove = vec!["ask :write **/.git/hooks".into()];
    let mut app = test_app_with(cfg);
    assert_eq!(
        verdict(&app, "echo x > .git/hooks/pre-commit"),
        Decision::Allow,
        "removed, and allowed"
    );
    assert_eq!(
        verdict(&app, "echo x > .claude/settings.json"),
        Decision::Ask
    );
    assert_eq!(verdict(&app, "find . -exec id ;"), Decision::Ask);
    app.dispatch_command_for_test("ask git push *");

    let listing = app.rules_listing();
    let section = |title: &str| {
        let start = listing
            .find(title)
            .unwrap_or_else(|| panic!("{title}\n{listing}"));
        let rest = &listing[start + title.len()..];
        rest[..rest.find("\n\n").unwrap_or(rest.len())].to_string()
    };
    assert!(
        !section("SHIPPED GUARDS").contains(r"ask :write \c**/.git/hooks"),
        "{listing}"
    );
    assert!(
        section("SHIPPED GUARDS").contains("deny rm --no-preserve-root"),
        "{listing}"
    );
    assert!(
        section("REMOVED BY A CONFIG FILE").contains(r"ask :write \c**/.git/hooks   (your config)"),
        "{listing}"
    );
    assert!(
        section("YOUR CONFIG ([permissions] rules)").contains("allow find ."),
        "{listing}"
    );
    assert!(
        section("THIS SESSION").contains("ask git push *"),
        "{listing}"
    );
    // **What the gate holds, not what the file says**: a line the gate could
    // not read is not "policy in force" (Reviewer A, round 16).
    assert!(
        !section("YOUR CONFIG ([permissions] rules)").contains("allwo"),
        "{listing}"
    );
    assert!(section("NOT IN FORCE").contains("allwo rm *"), "{listing}");
    assert!(
        listing.contains("WHATEVER a rule allows"),
        "the overrides are named"
    );
    assert!(
        !section("SHIPPED ALLOWS").contains("allow find ."),
        "each rule under its own origin: {listing}"
    );

    // **A shipped ask takes `!` to come out**: without the guard a write
    // into `.claude/` is a write inside the project, which runs unasked
    // (Reviewer D, round 18). Steven: "`!` should override system."
    app.dispatch_command_for_test("unask :write **/.claude");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("could run unasked") && notice.contains("`:unask! :write **/.claude`"),
        "{notice:?}"
    );
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert_eq!(
        verdict(&app, "echo x > .claude/settings.json"),
        Decision::Ask,
        "still asks"
    );
    app.dispatch_command_for_test("unask! :write **/.claude");
    let listing = app.rules_listing();
    let removed = {
        let title = "REMOVED THIS SESSION";
        let start = listing.find(title).expect("the section");
        let rest = &listing[start + title.len()..];
        rest[..rest.find("\n\n").unwrap_or(rest.len())].to_string()
    };
    assert!(
        removed.contains(r"ask :write \c**/.claude") && removed.contains("(shipped)"),
        "{listing}"
    );
}

/// **A rule written across lines** (a config's `'''` string — a long regex
/// under `(?x)`, laid out to be read; Steven, 2026-10-03) is cited on ONE
/// line where there is one — the reason a call is denied, which the pane,
/// its refusals and the model's answer share — and listed by `:rules` as
/// written, nested under its first.
#[tokio::test]
async fn a_rule_across_lines_is_cited_on_one_and_listed_as_written() {
    let mut cfg = Config::default();
    cfg.permissions.rules = vec![
        // A blank line inside it, as a laid-out pattern may have.
        "deny re:(?x)\n   ^(?:.*/)?rm      # the program\n\n   (?:\\s|$)        # then a break\n"
            .into(),
    ];
    let mut app = test_app_with(cfg);
    model_call(&mut app, "tu_1", "/bin/rm -r /tmp/shannon-test-absent");
    app.notices.clear();
    tool_key(&mut app, 'a');
    let notice = app.notice().unwrap_or_default();
    // Each break and the indent around it is one space; the spaces inside a
    // line are the author's (outside `(?x)` they are the pattern's own).
    assert!(
        notice.contains(
            "denied by rule: deny re:(?x) ^(?:.*/)?rm      # the program (?:\\s|$)        \
             # then a break (your config)"
        ),
        "{notice:?}"
    );

    let listing = app.rules_listing();
    assert!(
        listing.contains(
            "  deny re:(?x)\n      ^(?:.*/)?rm      # the program\n      (?:\\s|$)        # then a break\n"
        ),
        "{listing}"
    );
}

/// **`:rules` colours each rule** (Steven, 10-11): its verb in the verdict's
/// own colour — `allow` green, `ask` yellow, `deny` red — and the rule after
/// it as shell; the page's prose stays plain. Read from the drawn cells, so a
/// role that paints as nothing fails here.
#[test]
fn the_rules_page_colours_each_verdict_and_its_rule() {
    let mut cfg = Config::default();
    cfg.permissions.rules = vec![
        "allow cat *".into(),
        "ask gh pr view --web".into(),
        "deny rm --recursive".into(),
    ];
    let mut app = test_app_with(cfg);
    app.dispatch_command_for_test("rules");
    app.dispatch_command_for_test("only");
    let buf = frame_of(&mut app, 140, 80);
    // The cell's fg where `needle` begins on the first row holding `row`.
    let fg = |row: &str, needle: &str| {
        let at = cell_of(&buf, row).unwrap_or_else(|| panic!("`{row}` is drawn"));
        let at = (cells_of(&buf, needle))
            .find(|c| c.y == at.y && c.x >= at.x)
            .unwrap_or_else(|| panic!("`{needle}` in `{row}`"));
        buf[at].style().fg
    };
    use crate::palette::{CAUTION, DANGER, SAFE};
    assert_eq!(fg("allow cat *", "allow"), Some(SAFE));
    assert_eq!(fg("ask gh pr view", "ask"), Some(CAUTION));
    assert_eq!(fg("deny rm --recursive", "deny"), Some(DANGER));
    // The rule as shell: the program and the flag each wear a role, not
    // the verb's colour and not plain.
    let plain = fg("The permission policy in force", "The");
    assert_eq!(
        plain,
        Some(ratatui::style::Color::Reset),
        "prose is no rule: plain"
    );
    for (row, word) in [
        ("allow cat *", "cat"),
        ("deny rm --recursive", "--recursive"),
    ] {
        let got = fg(row, word);
        assert!(
            got != plain && got != Some(SAFE) && got != Some(DANGER),
            "{word}: {got:?}"
        );
    }
    assert_ne!(
        fg("allow cat *", "cat"),
        fg("deny rm --recursive", "--recursive")
    );

    // Headings and their references (Steven, 10-11: "section headers should
    // have some color too. And paint the bracketed index"): a heading in the
    // listing's colour — never a verdict's, so it cannot read as one — and a
    // reference as the manual paints a literal; the prose's references too.
    use crate::palette::{HELP_CODE, LISTING_HEADING};
    for (row, word) in [
        ("SHIPPED", "SHIPPED"),
        ("YOUR CONFIG (", "YOUR"),
        ("THIS SESSION (", "THIS"),
        ("THIS SESSION (", "("),
    ] {
        assert_eq!(fg(row, word), Some(LISTING_HEADING), "{row} / {word}");
    }
    for (row, word) in [
        ("YOUR CONFIG (", "[permissions]"),
        ("THIS SESSION (", ":allow"),
        ("THIS SESSION (", ":ask"),
        ("FROM [memory]", "[skills]"),
        ("a word the gate cannot place", ":help"),
    ] {
        assert_eq!(fg(row, word), Some(HELP_CODE), "{row} / {word}");
    }
    assert!(![SAFE, CAUTION, DANGER].contains(&LISTING_HEADING));
}

/// **`:rules` is a live pane, like `:ls` and `:secrets`** (Steven,
/// 2026-10-03). It was a page handed to `$EDITOR`: frozen while the policy
/// changed, and the event loop with it. A rule taught beside it shows on the
/// next frame, and your place in the page holds.
#[test]
fn rules_is_a_live_pane() {
    let mut app = test_app();
    app.dispatch_command_for_test("rules");
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Rules))
        .expect("`:rules` opens a pane");
    app.clamp_text_panes(100, 30);
    let page = |app: &App| -> String {
        app.pane_listing(pane)
            .expect("content")
            .lines()
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        page(&app).contains("deny rm --no-preserve-root"),
        "{}",
        page(&app)
    );
    assert!(
        !page(&app).contains("deny gh api"),
        "premise: {}",
        page(&app)
    );
    app.text_pane_mut_for_test(pane)
        .expect("a text page")
        .goto_line(3, 0);
    let place = app.text_pane_pos_for_test(pane).expect("a text page").0;

    app.dispatch_command_for_test("deny gh api:-X|--method");
    app.clamp_text_panes(100, 30); // the next frame
    assert!(
        page(&app).contains("deny gh api:-X|--method"),
        "{}",
        page(&app)
    );
    assert_eq!(
        app.text_pane_pos_for_test(pane).expect("a text page").0,
        place,
        "the page changed under you and kept your place"
    );
}

/// **A rule comes out for the session, whoever wrote it, and the file is
/// untouched** (Steven, 2026-09-30) — and removing a shipped deny, which
/// LOOSENS the standing policy, takes `!`. Adding a rule already in force
/// says so; adding back one removed this session restores it.
#[test]
fn a_rule_comes_out_for_the_session_and_a_standing_deny_takes_a_bang() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    let doomed = "rm --no-preserve-root -rf /tmp/x";
    assert_eq!(
        verdict(&app, doomed),
        Decision::Deny,
        "premise: shipped deny"
    );

    app.dispatch_command_for_test("undeny rm:--no-preserve-root");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("loosens") && notice.contains("`:undeny! rm:--no-preserve-root`"),
        "{notice:?}"
    );
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert_eq!(verdict(&app, doomed), Decision::Deny, "nothing removed");

    app.dispatch_command_for_test("undeny! rm:--no-preserve-root");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("removed for this session (shipped)"),
        "{notice:?}"
    );
    // ASK, not merely "not deny": `rm` is Destructive, so the policy beneath
    // the removed rule asks — and "not deny" also held for an Allow, which
    // would be a removal that unleashed it (Reviewer C).
    assert_eq!(
        verdict(&app, doomed),
        Decision::Ask,
        "removed; the policy beneath asks"
    );

    // Back again, by teaching it.
    app.dispatch_command_for_test("deny rm:--no-preserve-root");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("back in force (shipped)"),
        "{:?}",
        app.notice()
    );
    assert_eq!(verdict(&app, doomed), Decision::Deny);
    assert!(app.session_rules.is_empty(), "restored, not copied");

    // An allow needs no `!`, and a second `:allow` is not a second rule.
    app.dispatch_command_for_test("allow ls *");
    app.dispatch_command_for_test("allow ls *");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("already in force (this session)"),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.session_rules.len(), 1);
    app.dispatch_command_for_test("unallow ls *");
    assert!(app.session_rules.is_empty(), "taken out");

    app.dispatch_command_for_test("unallow nothing *");
    assert!(
        app.notice().unwrap_or_default().contains("not in force"),
        "{:?}",
        app.notice()
    );
}

/// **"Back in force" only while a source still holds the rule.** Taken out for
/// the session and then its source gone (`:unsource`), a typed `:deny` said
/// "back in force (your config)" while nothing supplied it — and the deny the
/// user had just typed did not fire (Reviewers A, C, round 17).
#[tokio::test]
async fn a_rule_taught_again_after_its_source_went_is_this_sessions() {
    use shannon_core::permission::Decision;
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("deny.toml");
    std::fs::write(
        &file,
        "[permissions]\nrules = [\"deny ls /tmp/zz-secret\"]\n",
    )
    .unwrap();
    let mut app = test_app();
    app.dispatch_command_for_test(&format!("source {}", file.display()));
    assert_eq!(
        verdict(&app, "ls /tmp/zz-secret"),
        Decision::Deny,
        "premise"
    );
    app.dispatch_command_for_test("undeny! ls /tmp/zz-secret");
    assert_ne!(
        verdict(&app, "ls /tmp/zz-secret"),
        Decision::Deny,
        "premise: removed"
    );
    app.dispatch_command_for_test(&format!("unsource {}", file.display()));

    app.dispatch_command_for_test("deny ls /tmp/zz-secret");
    let notice = app.notice().unwrap_or_default();
    assert!(
        !notice.contains("back in force") && notice.contains("for this session"),
        "{notice:?}"
    );
    assert_eq!(
        verdict(&app, "ls /tmp/zz-secret"),
        Decision::Deny,
        "it fires"
    );
}

/// **`!` is force, needed to LOOSEN a rule this session did not add** — an
/// ask or a deny. Steven, 2026-09-30: "reserve the bang for allowing a deny
/// rule (removing a deny) that is in the config"; 2026-10-02, asks too: "`!`
/// should override system. It is a force." Your config's rule takes it as a
/// shipped one does; a rule taught this session comes out as freely as it
/// went in; removing an allow only tightens, so `:unallow!` is refused as
/// any verb that takes no `!` is.
#[test]
fn only_loosening_a_rule_this_session_did_not_add_takes_a_bang() {
    use shannon_core::permission::Decision;
    let mut cfg = Config::default();
    cfg.permissions.rules = vec!["deny curl *".into(), "allow ls *".into()];
    let mut app = test_app_with(cfg);
    let in_force = |app: &App, text: &str| app.rules.rules().iter().any(|r| r.source == text);
    assert_eq!(verdict(&app, "curl https://x"), Decision::Deny, "premise");

    app.dispatch_command_for_test("undeny curl *");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("(your config)") && notice.contains("`:undeny! curl *`"),
        "{notice:?}"
    );
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert!(in_force(&app, "deny curl *"), "your config's deny stays");

    app.dispatch_command_for_test("deny wget *");
    assert!(in_force(&app, "deny wget *"), "premise");
    app.dispatch_command_for_test("undeny wget *");
    assert!(!in_force(&app, "deny wget *"), "{:?}", app.notice());

    // A shipped ask, as a shipped deny.
    assert!(
        in_force(&app, r"ask :write \c**/.claude"),
        "premise: shipped ask"
    );
    app.dispatch_command_for_test("unask :write **/.claude");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("(shipped)") && notice.contains("`:unask! :write **/.claude`"),
        "{notice:?}"
    );
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert!(
        in_force(&app, r"ask :write \c**/.claude"),
        "the shipped ask stays"
    );
    // Named without its `\c`, as a rule is named by its words.
    app.dispatch_command_for_test("unask! :write **/.claude");
    assert!(
        !in_force(&app, r"ask :write \c**/.claude"),
        "{:?}",
        app.notice()
    );

    app.dispatch_command_for_test("ask git push *");
    assert!(in_force(&app, "ask git push *"), "premise");
    app.dispatch_command_for_test("unask git push *");
    assert!(!in_force(&app, "ask git push *"), "{:?}", app.notice());

    // Your config's allow: removing it only tightens.
    app.dispatch_command_for_test("unallow! ls *");
    assert!(
        app.notice().unwrap_or_default().contains("no `!` allowed"),
        "{:?}",
        app.notice()
    );
    assert!(
        in_force(&app, "allow ls *"),
        "a refused verb removes nothing"
    );
    app.dispatch_command_for_test("unallow ls *");
    assert!(!in_force(&app, "allow ls *"), "{:?}", app.notice());
}

/// **One reader for every source**: `#` starts a comment at `:deny` too, as
/// in a config file — `:deny git push * # never` read `#` and `never` as
/// operands, and never fired (Reviewer A, round 16).
#[test]
fn a_comment_in_a_session_rule_is_a_comment() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    app.dispatch_command_for_test("deny git push * # never");
    assert_eq!(verdict(&app, "git push origin main"), Decision::Deny);
    assert_eq!(app.session_rules[0].source, "deny git push *");
}
