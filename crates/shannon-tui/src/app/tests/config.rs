//! `:config edit`, `:set`, `:source`: what a setting change reaches.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- `:config edit` -------------------------------------------------------
//
// The editor half is a subprocess; these cover the half that decides what the
// edited text does to the user's machine.

/// A config that does not parse must be **saved** (the edit is the user's work)
/// — and the user must be told, on screen, that `:reload` would refuse it.
/// Before this was fixed, every outcome went only to the log: you came back
/// from `$EDITOR`, saw nothing, and had every reason to believe your typo had
/// taken effect.
#[test]
fn a_config_that_does_not_parse_is_saved_and_says_reload_would_refuse_it() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shannon/config.toml");

    let before = Arc::clone(&app.config);
    app.save_config(&path, "this is not = = toml");

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "this is not = = toml",
        "the edit is the user's work — never discard it"
    );
    assert!(Arc::ptr_eq(&before, &app.config), "saving never applies");
    let notice = app.notice().expect("the outcome must reach the screen");
    assert!(
        notice.contains("saved") && notice.contains("does not parse"),
        "the notice must name both halves, got: {notice}"
    );
}

/// **Saving does not apply; `:reload` does** (Steven: editing must not swap
/// live state mid-session). The save says how to apply it, in a notice that
/// survives an 80-column row; the reload says what it changed and which of
/// those only a relaunch applies — named, not the whole relaunch list recited.
#[test]
fn a_saved_config_applies_on_reload_and_says_what_changed() {
    use shannon_core::config::Origin;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shannon/config.toml");
    assert!(app.config.tui.mouse, "premise: the default");

    app.save_config(&path, "[tui]\nmouse = false\nscrolloff = 2\n");
    assert!(app.config.tui.mouse, "saved is not applied");
    let notice = app.notice().expect("the outcome must reach the screen");
    assert!(notice.contains(":reload"), "says how to apply it: {notice}");
    assert!(notice.chars().count() <= 78, "fits 80 columns: {notice}");

    app.source_file(Origin::User, &path);
    assert!(!app.config.tui.mouse, "`:reload` applied it");
    assert_eq!(app.config.tui.scrolloff, 2);
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("changed: tui.mouse, tui.scrolloff"),
        "names what changed:\n{said}"
    );
    assert!(
        said.contains("these apply on relaunch: mouse") && !said.contains("keymaps"),
        "names the relaunch-only setting it touched, and only that:\n{said}"
    );
}

/// **`:config edit` says what the parse had to say**, which it used to drop:
/// `parse_ready` called `clamp` and discarded the notes, and never looked at
/// unknown keys — so a `nan` threshold was reset, and a typo'd top-level key
/// ignored, while the notice said the config applied.
#[test]
fn config_edit_shows_what_it_could_not_use_as_written() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shannon/config.toml");

    app.save_config(&path, "tuii = 3\n[secrets]\nentropy_bits = nan\n");

    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(said.contains("config saved"), "premise: it saved:\n{said}");
    assert!(
        said.contains("unknown key `tuii`"),
        "the typo is named:\n{said}"
    );
    assert!(
        said.contains("secrets.entropy_bits NaN"),
        "the reset is named:\n{said}"
    );
}

/// The notice's relaunch list and the manual's are ONE list: a setting added to
/// the startup-only set must be named in both, or the manual says it applies
/// live while the notice says it does not.
#[test]
fn the_relaunch_list_is_the_manuals() {
    // Whitespace collapsed, so re-wrapping the manual cannot break this and a
    // changed list still does.
    let manual = crate::help::compose(&crate::command::commands_chapter())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let list = crate::app::config_file::relaunch_list();
    // Terminated by the dash that follows it: unterminated, a list missing
    // its last entry was still a prefix of the sentence (Reviewer C).
    assert!(
        manual.contains(&format!("These need a relaunch: {list} —")),
        "`:help config` must name exactly `{list}`"
    );
}

/// The settings that describe a launch are the manual's too — `LAUNCH_KEYS`
/// was tied to no sentence, so dropping one passed (Reviewer C).
#[test]
fn the_launch_list_is_the_manuals() {
    let manual = crate::help::compose(&crate::command::commands_chapter())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let list = crate::app::config_file::launch_list();
    assert!(
        manual.contains(&format!("except that {list} describe a launch.")),
        "`:help config` must name exactly {list}"
    );
}

/// `secrets.extra` is documented as the place to paste a database password
/// verbatim, and `:config edit` is the command that *creates* the file — so the
/// file it creates must not be world-readable. (The mechanism is tested in
/// `private_file`; this pins that `:config edit` actually goes through it.)
#[cfg(unix)]
#[test]
fn the_config_written_by_config_edit_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shannon/config.toml");

    app.save_config(&path, "[tui]\nmouse = true\n");

    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600, "config file must be owner-only");
    assert_eq!(
        mode(path.parent().unwrap()),
        0o700,
        "config dir must be owner-only"
    );
}

/// **A reloaded prompt height resizes the prompt, at once** (Steven: height 50,
/// `:reload` said "changed: tui.prompt.height", and nothing moved — the value
/// was only read at launch and by `<C-w>=`). Measured on the drawn layout.
#[test]
fn a_reloaded_prompt_height_resizes_the_prompt() {
    let mut app = test_app();
    draw_at(&mut app, 80, 40);
    let prompt = app
        .workspace
        .first_of(PaneKind::Prompt)
        .expect("a prompt pane");
    let height = |app: &App| app.pane_content_size(prompt, Rect::new(0, 0, 80, 40)).1;
    assert_ne!(height(&app), 17, "premise: not already that tall");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    app.save_config(&path, "[tui.prompt]\nheight = 17\n");
    assert_ne!(height(&app), 17, "saving applies nothing");
    app.source_file(shannon_core::config::Origin::User, &path);
    assert_eq!(height(&app), 17, "`:reload` resized the prompt");
}

/// **`:source`, `:unsource` and `:scriptnames` through the real dispatch** —
/// the stack's rules are tested in core; this pins that the commands reach
/// them and say what they did.
#[test]
fn source_and_unsource_a_file_through_the_command_line() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj.toml");
    std::fs::write(&proj, "[tui]\nscrolloff = 1\nsplitbelow = true\n").unwrap();
    let default = app.config.tui.scrolloff;
    let said = |app: &App| app.messages().collect::<Vec<_>>().join("\n");

    app.dispatch_command_for_test("source");
    assert!(
        app.notice().is_some_and(|n| n.contains("needs a file")),
        "a bare `:source` is refused, not guessed at: {:?}",
        app.notice()
    );

    app.dispatch_command_for_test(&format!("so {}", proj.display()));
    assert_eq!(app.config.tui.scrolloff, 1, "`:so` applied it");
    assert!(
        said(&app).contains("changed: tui.scrolloff, tui.splitbelow"),
        "{}",
        said(&app)
    );

    let listing = scriptnames(&mut app);
    assert!(
        listing.contains("sourced") && listing.contains("proj.toml"),
        "{listing}"
    );

    app.dispatch_command_for_test(&format!("unsource {}", proj.display()));
    assert_eq!(app.config.tui.scrolloff, default, "back to what remains");
    assert!(!app.config.tui.splitbelow);
    app.dispatch_command_for_test(&format!("uns {}", proj.display()));
    assert!(
        app.notice().is_some_and(|n| n.contains("is not loaded")),
        "unsourcing twice says so: {:?}",
        app.notice()
    );
}

/// **A parse error names the line, never quotes it** — on screen or in the
/// log. The rendered TOML error quotes the offending line, which can be an
/// `[[alias]] value`. `:config edit` logged it whole (`error = %e`), and the
/// screen got it too — and every notice is ALSO logged (`App::post`), scrubbed
/// by a redactor that does not catch a low-entropy value like this one. Both
/// the save and the `:source` paths, through the log capture: asserting on
/// `describe_parse_error` would test the function, not the call sites.
#[test]
fn a_config_parse_error_never_puts_the_line_in_the_log() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shannon/config.toml");
    let text = "[[alias]]\nname = \"db\"\nvalue = \"canary-9QxT\" junk\n";

    let logs = captured_logs(|| app.save_config(&path, text));
    assert!(
        logs.contains("does not parse"),
        "premise: it logged:\n{logs}"
    );
    assert!(
        !logs.contains("canary-9QxT"),
        "the line leaked to the log:\n{logs}"
    );
    assert!(
        app.notice().is_some_and(|n| n.contains("line 3")),
        "the screen says WHERE: {:?}",
        app.notice()
    );

    let logs = captured_logs(|| app.source_file(shannon_core::config::Origin::User, &path));
    assert!(logs.contains("not sourced"), "premise: it logged:\n{logs}");
    assert!(
        !logs.contains("canary-9QxT"),
        "the line leaked to the log:\n{logs}"
    );
}

/// A file that does not parse is refused by `:source` and changes nothing —
/// the running config, the gate, and the stack.
#[test]
fn a_bad_file_is_not_sourced() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, "[tui]\nscrolof = 1\n").unwrap();
    let before = Arc::clone(&app.config);
    app.dispatch_command_for_test(&format!("source {}", bad.display()));
    assert!(Arc::ptr_eq(&before, &app.config), "nothing applied");
    assert!(
        app.notice().is_some_and(|n| n.contains("NOT sourced")),
        "{:?}",
        app.notice()
    );
    let listing = scriptnames(&mut app);
    assert!(
        listing.contains("no config files"),
        "and no layer was left behind: {listing}"
    );
}

/// **The `:scriptnames` pane is live, titled and legended** (review
/// 2026-10-13, C: none of the three was asserted): a file sourced or taken
/// out while it is open shows at the next frame.
#[test]
fn the_scriptnames_pane_is_live() {
    use shannon_core::config::Origin;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("extra.toml");
    std::fs::write(&file, "[tui]\nscrolloff = 3\n").unwrap();
    assert!(scriptnames(&mut app).contains("no config files"), "premise");
    app.source_file(Origin::Sourced, &file);
    let shown = scripts_pane(&mut app);
    assert!(
        shown.contains("1: sourced") && shown.contains("extra.toml"),
        "{shown}"
    );
    // The next key's moment: the `:source` notice gives the row back to
    // the legend.
    app.notices.clear();
    let drawn = text_of(&mut app, 100, 30);
    // The row points at the page that lists `:so` and `:uns`, as every
    // page's does (Steven, 10-14: minimal rows).
    assert!(
        drawn.contains(" scriptnames ") && drawn.contains("help `:help scriptnames`"),
        "its title and its row:\n{drawn}"
    );
    app.dispatch_command_for_test(&format!("unsource {}", file.display()));
    assert!(scripts_pane(&mut app).contains("no config files"));
}

/// **The test dispatch starts a new moment, as the Enter that runs a command
/// does** — so what a command's assertion reads is what THAT command said.
/// Without it, notices piled up across dispatches, and an assertion that
/// `:scriptnames` named a file passed on the `:source` notice before it.
#[test]
fn each_dispatched_command_speaks_for_itself() {
    let mut app = test_app();
    app.dispatch_command_for_test("source");
    assert!(
        app.notice().is_some_and(|n| n.contains("needs a file")),
        "premise"
    );
    app.dispatch_command_for_test("trust");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("trusted"), "premise: it speaks: {said}");
    assert!(
        !said.contains("needs a file"),
        "the last command's notice stayed: {said}"
    );
}

/// **`:source` reads through the one config reader**: what is not a small
/// regular file is refused with its reason, and nothing changes. The reader's
/// own rules (a pipe, `/dev/zero`, the cap) are tested in core; this pins
/// that `:source` walks through it — `/dev/stdin` froze the app through a
/// private `read_to_string`.
#[test]
fn source_refuses_what_is_not_a_config_file_and_says_why() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let before = Arc::clone(&app.config);
    let huge = dir.path().join("huge.toml");
    std::fs::write(
        &huge,
        "#".repeat(shannon_core::config::MAX_CONFIG_BYTES as usize + 1),
    )
    .unwrap();
    for (path, why) in [
        (dir.path().to_path_buf(), "it is a directory"),
        (dir.path().join("absent.toml"), "no such file"),
        (huge, "larger than a config file"),
    ] {
        app.dispatch_command_for_test(&format!("source {}", path.display()));
        assert!(
            app.notice()
                .is_some_and(|n| n.contains("NOT sourced") && n.contains(why)),
            "{why}: {:?}",
            app.notice()
        );
    }
    assert!(Arc::ptr_eq(&before, &app.config), "nothing applied");
}

/// **`:reload` through the real dispatch** — your file comes in as YOURS (no
/// safety heads-up, `user` in `:scriptnames`), and `:source` of the same
/// file, spelled another way, is the same verb (Reviewers A and C: `:reload`
/// was only ever tested by calling `source_file` directly, and `:source` of
/// your own config was treated as someone else's).
#[test]
fn reload_and_sourcing_your_own_file_are_one_verb() {
    let mut app = test_app();
    app.dispatch_command_for_test("reload");
    assert!(
        app.messages()
            .last()
            .is_some_and(|m| m.contains("no config path")),
        "no path, said why: {:?}",
        app.messages().last()
    );

    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("config.toml");
    std::fs::write(&mine, "[secrets]\nredact = false\n").unwrap();
    app.user_config = Ok(mine.clone());

    app.dispatch_command_for_test("reload");
    assert!(!app.config.secrets.redact, "`:reload` applied your file");
    app.dispatch_command_for_test(&format!("source {}/./config.toml", dir.path().display()));
    let listed = scriptnames(&mut app);
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        !said.contains("safety settings"),
        "your own file is not called out:\n{said}"
    );
    assert!(
        listed.contains("user") && !listed.contains("sourced"),
        "both loads are yours: {listed}"
    );
}

/// **A sourced file that changes a safety setting says so on screen** — and
/// still applies (Steven: local wins, "but also serve a warning message").
/// Your own file, reloaded, is not called out for the same kind of change.
#[test]
fn sourcing_a_file_that_changes_a_safety_setting_says_so() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj.toml");
    std::fs::write(&proj, "[secrets]\nredact = false\n").unwrap();
    assert!(app.config.secrets.redact, "premise: the default");

    app.dispatch_command_for_test(&format!("so {}", proj.display()));
    assert!(!app.config.secrets.redact, "it applied: local wins");
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("changes safety settings: secrets.redact"),
        "the heads-up names it:\n{said}"
    );

    let mut app = test_app();
    let mine = dir.path().join("mine.toml");
    std::fs::write(&mine, "[exec]\naudit = \"off\"\n").unwrap();
    app.source_file(shannon_core::config::Origin::User, &mine);
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("changed: exec.audit") && !said.contains("safety settings"),
        "your own file changes it without the heads-up:\n{said}"
    );
}

/// **The log says what was loaded and what it changed** (Steven: "make sure
/// that in the logs we're showing what's being loaded") — for a file put on
/// and taken off, by name. Never a value: `secrets.extra` is where a literal
/// credential is documented to go.
#[test]
fn loading_and_unloading_a_config_file_is_logged_with_what_it_changed() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj.toml");
    std::fs::write(
        &proj,
        "[tui]\nscrolloff = 1\n[secrets]\nextra = [\"canary-7fQz-literal\"]\n",
    )
    .unwrap();

    let logs = captured_logs(|| {
        app.dispatch_command_for_test(&format!("so {}", proj.display()));
    });
    let loaded = logs
        .lines()
        .find(|l| l.contains("config layer loaded"))
        .unwrap_or_else(|| panic!("the load was logged:\n{logs}"));
    assert!(
        loaded.contains("proj.toml")
            && loaded.contains("tui.scrolloff")
            && loaded.contains("secrets.extra"),
        "names the file and what it changed: {loaded}"
    );
    assert!(
        !logs.contains("canary-7fQz"),
        "a value reached the log:\n{logs}"
    );

    let logs = captured_logs(|| {
        app.dispatch_command_for_test(&format!("unsource {}", proj.display()));
    });
    let unloaded = logs
        .lines()
        .find(|l| l.contains("config layer unloaded"))
        .unwrap_or_else(|| panic!("the unload was logged:\n{logs}"));
    assert!(
        unloaded.contains("proj.toml") && unloaded.contains("tui.scrolloff"),
        "names the file and what going away changed: {unloaded}"
    );
    assert!(
        !logs.contains("canary-7fQz"),
        "a value reached the log:\n{logs}"
    );
}

// --- `:set` ---------------------------------------------------------------

/// **Steven's own example: `:set tui.prompt.height=15` applies at once** —
/// measured on the drawn layout — and says the value you got.
#[test]
fn set_applies_at_once_and_resizes_the_prompt() {
    let mut app = test_app();
    draw_at(&mut app, 80, 40);
    let prompt = app
        .workspace
        .first_of(PaneKind::Prompt)
        .expect("a prompt pane");
    let height = |app: &App| app.pane_content_size(prompt, Rect::new(0, 0, 80, 40)).1;
    assert_ne!(height(&app), 15, "premise: not already that tall");

    app.dispatch_command_for_test("set tui.prompt.height=15");
    assert_eq!(app.config.tui.prompt.height, 15);
    assert_eq!(height(&app), 15, "resized at once");
    assert_eq!(app.notice().as_deref(), Some("tui.prompt.height=15"));
}

/// **The echo is the value you GOT**: a height the config clamps shows as
/// clamped, with the note saying why — not as what you typed.
#[test]
fn set_echoes_the_value_after_the_config_adjusts_it() {
    let mut app = test_app();
    app.dispatch_command_for_test("se tui.prompt.height=500");
    let max = shannon_core::config::MAX_PROMPT_HEIGHT;
    assert_eq!(app.config.tui.prompt.height, max);
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains(&format!("tui.prompt.height={max}")) && !said.contains("=500"),
        "{said}"
    );
    assert!(said.contains("500"), "the adjustment is named:\n{said}");
}

/// **A refused `:set` changes nothing and says why, on screen** — and a
/// credential typed at it never reaches the messages or the log.
#[test]
fn a_refused_set_changes_nothing_and_says_why() {
    let mut app = test_app();
    let before = Arc::clone(&app.config);
    for (line, why) in [
        ("set tui.scrolloff=many", "takes a number"),
        ("set scrolloff=3", "unknown option"),
        // serde's own sentence, which lists the names `[tui]` does have.
        (
            "set tui.scrolof=3",
            "unknown field `scrolof`, expected one of",
        ),
        ("set tui.resume=sometimes", "sometimes"),
    ] {
        app.dispatch_command_for_test(line);
        assert!(Arc::ptr_eq(&before, &app.config), "`{line}` applied");
        assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
        let said = app.messages().last().unwrap_or_default().to_string();
        assert!(said.contains(why), "`{line}`: {said}");
    }

    let logs = captured_logs(|| {
        app.dispatch_command_for_test("set secrets.extra=canary-5Kd2-literal");
    });
    assert!(
        Arc::ptr_eq(&before, &app.config),
        "a credential is not :set"
    );
    assert!(
        app.notice().is_some_and(|n| n.contains("credentials")),
        "{:?}",
        app.notice()
    );
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(!said.contains("canary-5Kd2"), "echoed:\n{said}");
    assert!(!logs.contains("canary-5Kd2"), "logged:\n{logs}");
}

/// **A project's rules are ADDED to yours, and your deny still holds** —
/// through the real `:source` and the real gate. Before rules stacked, the
/// project's `rules = [...]` REPLACED yours, so its `allow rm *` made
/// `rm -r` run where your config denied it (Steven, 2026-10-03: "rules
/// should stack"; strictest wins).
#[test]
fn a_projects_rules_add_to_yours_and_your_deny_holds() {
    use shannon_core::config::Origin;
    use shannon_core::permission::Decision;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("config.toml");
    std::fs::write(&mine, "[permissions]\nrules = [\"deny rm:-r\"]\n").unwrap();
    app.source_file(Origin::User, &mine);
    assert_eq!(
        app.rules.decide_line("rm -r build").decision,
        Decision::Deny,
        "premise: your deny is in force"
    );

    // The program AND the place: a delete inside the project needs a place
    // granted (Q7), which a program allow is not.
    let proj = dir.path().join("proj.toml");
    std::fs::write(
        &proj,
        "[permissions]\nrules = [\"allow rm *\", \"allow :delete build\"]\n",
    )
    .unwrap();
    app.dispatch_command_for_test(&format!("source {}", proj.display()));
    assert_eq!(
        app.rules.decide_line("rm build/x").decision,
        Decision::Allow,
        "the project's rules are in force"
    );
    assert_eq!(
        app.rules.decide_line("rm -r build/x").decision,
        Decision::Deny,
        "and yours still is: the more specific rule decides"
    );
}

/// **A credential in your config never reaches the screen's history or the
/// log through a config command** — every route the review found (Reviewers
/// A and B, 2026-09-29), driven through the real dispatch. The canaries are
/// under 8 characters on purpose: the log scrub ignores literals that short,
/// so a leak cannot be hidden by it and the test measures the route itself.
#[test]
fn a_credential_in_config_never_reaches_messages_or_log() {
    use shannon_core::config::Origin;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("config.toml");
    std::fs::write(
        &mine,
        "[secrets]\nextra = [\"c4n4ry7\"]\n\n[plugins.gh]\ntoken = \"t0k3n5x\"\n",
    )
    .unwrap();
    app.source_file(Origin::User, &mine);
    assert_eq!(app.config.secrets.extra, ["c4n4ry7"], "premise: loaded");

    let mut logs = String::new();
    for line in [
        "set secrets?",
        "set secrets",
        "set secrets&",
        "verbose set secrets?",
        "verbose set secrets&",
        "set",
        "verbose set",
        "set plugins.gh.token?",
        "set secrets.extra =typ3d7x",
        "sett secrets.extra=typ3d7x",
    ] {
        logs.push_str(&captured_logs(|| {
            app.dispatch_command_for_test(line);
        }));
    }
    let CmdEffect::View { text, .. } = app.dispatch_command("set all") else {
        panic!("`:set all` opens the viewer");
    };
    let said = app.messages().collect::<Vec<_>>().join("\n");
    for canary in ["c4n4ry7", "t0k3n5x", "typ3d7x"] {
        assert!(!said.contains(canary), "`{canary}` on screen:\n{said}");
        assert!(!logs.contains(canary), "`{canary}` in the log:\n{logs}");
        assert!(!text.contains(canary), "`{canary}` in `:set all`");
    }
    assert_eq!(
        app.config.secrets.extra,
        ["c4n4ry7"],
        "no `:set` emptied the literal-secret scrub"
    );
}

/// **A config that does not parse names the line and never quotes the
/// value** — serde's type error quotes it, and the natural mistake puts a
/// password there: a string where the list belongs (Reviewer B). Through
/// `:source` and through saving from `:config edit`.
#[test]
fn a_misshapen_secret_is_never_quoted_by_the_parse_error() {
    use shannon_core::config::Origin;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let text = "[secrets]\nextra = \"c4n4ry7\"\n";
    std::fs::write(&path, text).unwrap();

    let mut logs = captured_logs(|| app.source_file(Origin::User, &path));
    logs.push_str(&captured_logs(|| app.save_config(&path, text)));
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("line 2: invalid type: string, expected"),
        "{said}"
    );
    assert!(!said.contains("c4n4ry7"), "on screen:\n{said}");
    assert!(!logs.contains("c4n4ry7"), "in the log:\n{logs}");
}

/// **`:verbose set` names the environment where it set a value** — the
/// effort `CLAUDE_CODE_EFFORT_LEVEL` gave, laid as launch lays it — not as
/// a `:set` you never typed (review 2026-10-13, A, C).
#[test]
fn verbose_names_the_environment() {
    let mut config = Config::default();
    let mut stack = shannon_core::config::ConfigStack::default();
    let said = stack.start_effort(&mut config, Some(std::ffi::OsStr::new("high")));
    assert!(said.is_some(), "premise: the variable was taken");
    let mut app = test_app_with(config);
    app.set_config_stack(stack, None);
    app.dispatch_command_for_test("verbose set turn.effort?");
    let said = app.messages().last().unwrap_or_default().to_string();
    assert!(
        said.contains("(CLAUDE_CODE_EFFORT_LEVEL high") && !said.contains(":set"),
        "{said}"
    );
}

/// **`:reload` wins over an earlier `:set`** (Steven: "it is like re-sourcing
/// a file"), and `:verbose set x?` shows every layer that sets the option,
/// top first, each with its own value, down to the default — not only "last
/// set from the command line", which you already knew (Steven).
#[test]
fn reload_wins_over_set_and_verbose_shows_the_layers() {
    use shannon_core::config::Origin;
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[tui]\nscrolloff = 3\n").unwrap();
    app.source_file(Origin::User, &path);
    let default = shannon_core::config::DEFAULT_SCROLLOFF;

    app.dispatch_command_for_test("set tui.scrolloff=9 tui.splitbelow");
    assert_eq!(app.config.tui.scrolloff, 9, "the :set wins over the file");
    app.dispatch_command_for_test("verbose set tui.scrolloff?");
    let said = app.messages().last().unwrap_or_default().to_string();
    assert!(
        said.starts_with("tui.scrolloff=9 (:set 9, ")
            && said.ends_with(&format!("config.toml 3, default {default})")),
        "the :set, the file under it, the default: {said}"
    );

    app.source_file(Origin::User, &path);
    assert_eq!(app.config.tui.scrolloff, 3, "`:reload` wins");
    assert!(
        app.config.tui.splitbelow,
        "what the file does not name stays"
    );
    app.dispatch_command_for_test("verb set tui.scrolloff? tui.mouse?");
    let said = app.messages().last().unwrap_or_default().to_string();
    assert!(
        said.starts_with("tui.scrolloff=3 (") && said.contains("config.toml 3, :set 9, "),
        "the reloaded file on top, the :set under it: {said}"
    );
    assert!(said.ends_with("tui.mouse=true (default true)"), "{said}");

    app.dispatch_command_for_test("verbose help");
    assert!(
        app.messages()
            .last()
            .is_some_and(|m| m.contains("`:verbose` works with `:set`")),
        "`:verbose` of something it cannot say more about is refused, by name: {:?}",
        app.messages().last()
    );
}

/// **`tui.set_report`** (Steven: "a silent, echo, and verbose option"):
/// `quiet` says nothing after a change — but still answers `x?`, which you
/// asked for — and `verbose` echoes the `:verbose` form.
#[test]
fn set_report_chooses_what_a_set_says() {
    let mut app = test_app();
    app.dispatch_command_for_test("set tui.set_report=quiet");
    let before = app.messages().count();
    app.dispatch_command_for_test("set tui.scrolloff=2");
    assert_eq!(app.config.tui.scrolloff, 2, "applied");
    assert_eq!(app.messages().count(), before, "and nothing said");
    app.dispatch_command_for_test("set tui.scrolloff?");
    assert_eq!(
        app.messages().last(),
        Some("tui.scrolloff=2"),
        "a query is answered"
    );

    app.dispatch_command_for_test("set tui.set_report=verbose tui.scrolloff=4");
    let default = shannon_core::config::DEFAULT_SCROLLOFF;
    assert_eq!(
        app.messages().last(),
        Some(
            format!(
                "tui.scrolloff=4 (:set 4, default {default})  ·  \
                 tui.set_report=verbose (:set verbose, default default)"
            )
            .as_str()
        )
    );

    app.dispatch_command_for_test("set tui.set_report=default tui.scrolloff=6");
    assert_eq!(
        app.messages().last(),
        Some("tui.scrolloff=6  ·  tui.set_report=default")
    );
}

/// **`:verbose set <Tab>` completes as `:set <Tab>` does** (Steven: "it
/// didn't tab complete after verbose") — a modifier's argument is a command
/// line, completed as if typed alone, with what came before kept.
#[test]
fn a_modifier_completes_the_command_it_carries() {
    for line in ["verbose set tui.pro", "verb se tui.pro"] {
        let c = crate::command::complete(line);
        assert!(
            c.candidates.iter().any(|w| w == "tui.prompt.height"),
            "`{line}`: {:?}",
            c.candidates
        );
        assert_eq!(format!("{}{}", c.stem, c.word), line, "the line is kept");
    }
    let c = crate::command::complete("verbose ");
    assert_eq!(c.stem, "verbose ");
    assert!(
        c.candidates.iter().any(|w| w == "set"),
        "{:?}",
        c.candidates
    );
    let c = crate::command::complete("verbose");
    assert_eq!(c.candidates, ["verbose"], "the name itself still completes");
}

/// **`[cache]` applies at once**: a change reaches the backend, the one place
/// that decides every request's `cache_control` (Reviewers A and C: it was
/// built with the launch policy and never told). The wire half is pinned in
/// core (`a_changed_cache_policy_reaches_the_next_request`).
#[test]
fn a_cache_change_reaches_the_backend() {
    use shannon_core::config::{CacheConfig, CacheTtl};
    struct Watch(std::sync::Mutex<Option<CacheConfig>>, CannedStub);
    impl Backend for Watch {
        fn send_message(
            &self,
            req: shannon_core::backend::SendRequest,
            tx: tokio::sync::mpsc::UnboundedSender<shannon_core::StreamMsg>,
        ) -> tokio::task::JoinHandle<()> {
            self.1.send_message(req, tx)
        }
        fn model(&self) -> &str {
            self.1.model()
        }
        fn set_cache(&self, cache: CacheConfig) {
            *self.0.lock().unwrap() = Some(cache);
        }
    }
    let watch = Arc::new(Watch(std::sync::Mutex::new(None), CannedStub::new()));
    let mut app = test_app_backed(watch.clone(), Arc::new(Config::default()));

    app.dispatch_command_for_test("set tui.scrolloff=2");
    assert_eq!(
        *watch.0.lock().unwrap(),
        None,
        "only a cache change is handed over"
    );
    app.dispatch_command_for_test("set nocache.enabled cache.ttl=1h");
    assert_eq!(
        *watch.0.lock().unwrap(),
        Some(CacheConfig {
            enabled: false,
            ttl: CacheTtl::OneHour
        })
    );
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(!said.contains("read only at launch"), "it is live: {said}");
}

/// **A setting read only at launch says so when `:set`** — a `:set` cannot
/// reach it and is gone when you quit, so silence would be a false promise.
#[test]
fn a_launch_only_setting_says_so_when_set() {
    let mut app = test_app();
    app.dispatch_command_for_test("set tui.resume=last");
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("tui.resume is read only at launch — this `:set` changes nothing now")
            && said.contains("forgotten when you quit"),
        "says it does nothing now and will not survive — never 'takes effect at launch', \
         which reads as the next one (Steven): {said}"
    );
    assert!(!said.contains("take effect"), "{said}");
    assert_eq!(
        app.messages().last(),
        Some("tui.resume=last"),
        "the echo is the value now held: {said}"
    );
    // Steven's sequence: a config saying `fresh`, then `:set tui.resume=last`.
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[tui]\nresume = \"fresh\"\n").unwrap();
    app.source_file(shannon_core::config::Origin::User, &path);
    app.dispatch_command_for_test("set tui.resume=last");
    assert_eq!(app.messages().last(), Some("tui.resume=last"));
    let mut app = test_app();
    app.dispatch_command_for_test("set notui.mouse");
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(said.contains("tui.mouse is read only at launch"), "{said}");
    let mut app = test_app();
    app.dispatch_command_for_test("set notui.mouse tui.resume=last");
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("tui.mouse, tui.resume are read only at launch"),
        "{said}"
    );
    let mut app = test_app();
    app.dispatch_command_for_test("set tui.scrolloff=2");
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(!said.contains("launch"), "a live setting: {said}");
}

/// A bare `:set` lists what you changed, as vim's does; `:set all` is every
/// option, in the viewer; Tab offers the option names.
#[test]
fn bare_set_lists_changes_and_set_all_opens_the_viewer() {
    let mut app = test_app();
    app.dispatch_command_for_test("set");
    assert!(
        app.notice().is_some_and(|n| n.contains("at its default")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("set tui.scrolloff=9");
    app.dispatch_command_for_test("set");
    assert_eq!(app.messages().last(), Some("tui.scrolloff=9"));

    match app.dispatch_command("set all") {
        CmdEffect::View { label, text, .. } => {
            assert_eq!(label, ":set all");
            assert!(text.contains("\ntui.scrolloff=9\n"), "{text}");
            assert!(
                !text.contains("secrets.extra"),
                "credentials are not listed"
            );
        }
        _ => panic!("`:set all` opens the viewer"),
    }

    let offered = crate::command::complete("set tui.pro").candidates;
    assert!(
        offered.iter().any(|c| c == "tui.prompt.height"),
        "{offered:?}"
    );

    // A credential a FILE set is still not `:set`'s to show.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("creds.toml");
    std::fs::write(&file, "[secrets]\nextra = [\"canary-8Hw1-literal\"]\n").unwrap();
    app.source_file(shannon_core::config::Origin::User, &file);
    app.dispatch_command_for_test("set");
    let listed = app.messages().last().unwrap_or_default().to_string();
    assert!(
        listed.contains("tui.scrolloff=9") && !listed.contains("secrets.extra"),
        "{listed}"
    );
}

/// The manual's list of safety settings IS the one the note uses: a section
/// added to `SAFETY_KEYS` must be named in `:help`, or the manual under-reports
/// what gets called out.
#[test]
fn the_safety_list_is_the_manuals() {
    let manual = crate::help::compose(&crate::command::commands_chapter())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let list = shannon_core::config::SAFETY_KEYS.join(", ");
    assert!(
        manual.contains(&format!("Safety settings are those under: {list}.")),
        "`:help` must name exactly `{list}`"
    );
}
