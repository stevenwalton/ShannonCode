//! A project's config and models, and `:config save`.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **The user's program models, all three halves** (P6b): read at launch
/// from `programs/` beside your config, and every rule read by them — the
/// config's and `:allow`'s alike; read again on every config change, so
/// `:reload` takes an edited model FILE; and what they could not honour —
/// a misread example, a config rule only they refuse, which the config's
/// own review (by the shipped set) cannot see — said once.
#[test]
fn the_users_program_models_load_at_launch_and_on_reload() {
    use shannon_core::permission::{Decision, model::USER_DIR};
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("config.toml");
    let rules = ["allow bat *", "deny bat --language rust", "allow bat -p=x"];
    std::fs::write(&mine, format!("[permissions]\nrules = {rules:?}\n")).unwrap();
    let programs = dir.path().join(USER_DIR);
    std::fs::create_dir(&programs).unwrap();
    let model = |example: &str| {
        format!(
            "name = \"bat\"\neffect = \"read\"\nexamples.match = [\"{example}\"]\n\
             positional = [{{ at = \"rest\", role = \"read_file\" }}]\n\
             flag = [{{ names = [\"-p\"] }}, \
             {{ names = [\"-l\", \"--language\"], arity = 1, role = \"data\" }}]\n"
        )
    };
    let file = programs.join("bat.toml");
    std::fs::write(&file, model("bat -l rust a.txt")).unwrap();

    let mut config = Config::default();
    config.permissions.rules = rules.iter().map(|r| r.to_string()).collect();
    let mut app = test_app_with(config);
    let line = "bat -l rust a.txt";
    assert_eq!(
        app.rules.decide_line(line).decision,
        Decision::Allow,
        "premise: before launch, the shipped set — `rust` a subcommand word"
    );
    assert!(
        app.rules_listing().contains("matched only as written"),
        "premise: unmodelled, `--language` is matched as written"
    );
    app.set_config_stack(Default::default(), Some(mine));
    // Notes read by the models in force (review 2026-10-11, A): a flag the
    // user's model knows is no longer "matched only as written".
    assert!(
        !app.rules_listing().contains("matched only as written"),
        "{}",
        app.rules_listing()
    );
    let said = app.add_session_rule("allow", "bat -p").unwrap();
    assert!(!said.contains("note:"), "{said}");
    // A flag the model does not know is noted as THAT, not as a program
    // Shannon does not model.
    let said = app.add_session_rule("allow", "bat --frob").unwrap();
    assert!(said.contains("not a flag or subcommand"), "{said}");
    assert_eq!(
        app.rules.decide_line(line).decision,
        Decision::Deny,
        "launch read the model, and the rules by it"
    );
    let refused = |app: &App| {
        app.messages()
            .filter(|m| m.contains("allow bat -p=x") && m.contains("your model of `bat`"))
            .count()
    };
    assert_eq!(refused(&app), 1, "a rule only the model refuses, said");
    // Said once while it stands: a rebuild that holds the same refusal says
    // nothing (a message between, or `:messages` folds a repeat).
    // (A session rule rebuilds over the same config; a `:set` here would
    // rebuild from the stack, which this test leaves empty.)
    app.dispatch_command_for_test("trust");
    app.add_session_rule("ask", "bat cache").unwrap();
    assert_eq!(refused(&app), 1, "not again");
    let err = app.add_session_rule("allow", "bat -p=y").unwrap_err();
    assert!(
        err.contains("takes no value"),
        "`:allow` reads by them: {err}"
    );

    // Another message between, or `:messages` folds a repeat into the one
    // before it and a count could not see one.
    app.dispatch_command_for_test("trust");
    std::fs::write(&file, model("bat --frob a.txt")).unwrap();
    app.dispatch_command_for_test("reload");
    let misread = |app: &App| {
        app.messages()
            .filter(|m| m.contains(&format!("{USER_DIR}/bat.toml")) && m.contains("bat --frob"))
            .count()
    };
    assert_eq!(misread(&app), 1, "the edited file, taken at `:reload`");
    app.dispatch_command_for_test("set tui.scrolloff=3");
    app.dispatch_command_for_test("reload");
    assert_eq!(misread(&app), 1, "the same models say nothing");

    let named = |app: &App| {
        app.messages()
            .filter(|m| m.contains("changed:") && m.contains(&format!("{USER_DIR}/bat.toml")))
            .count()
    };
    assert_eq!(
        named(&app),
        1,
        "the edit is a change, named, not \"nothing changed\""
    );

    std::fs::remove_file(&file).unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(
        app.rules.decide_line(line).decision,
        Decision::Allow,
        "the model gone: the shipped set again"
    );
    assert_eq!(named(&app), 2, "and so is taking it away");

    // A session rule a model then refuses leaves force SAID, not silently
    // (review 2026-10-11, A).
    app.add_session_rule("allow", "bat -p=y")
        .expect("unmodelled, it reads");
    app.dispatch_command_for_test("trust");
    std::fs::write(&file, model("bat -l rust a.txt")).unwrap();
    app.dispatch_command_for_test("reload");
    assert!(
        app.messages()
            .any(|m| m.contains("the session's rule `allow bat -p=y`")
                && m.contains("takes no value")),
        "said where the gate is built"
    );

    // A rule removed this session stays removed when the models change and
    // every rule is read again (review 2026-10-11, A: it came back) —
    // removed while `bat` is unmodelled (`--language` a word as written),
    // read again by a model that knows it. (A message between each reload,
    // or `:messages` folds a repeated "changed" into the one before.)
    app.dispatch_command_for_test("trust");
    std::fs::remove_file(&file).unwrap();
    app.dispatch_command_for_test("reload");
    app.dispatch_command_for_test("trust");
    let in_force = |app: &App| {
        app.rules
            .rules()
            .iter()
            .any(|r| r.source == "deny bat --language rust")
    };
    assert!(in_force(&app), "premise");
    app.dispatch_command_for_test("undeny! bat --language rust");
    assert!(!in_force(&app), "removed");
    std::fs::write(&file, model("bat -p a.txt")).unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(named(&app), 5, "premise: the models changed");
    assert_eq!(
        app.rules.decide_line(line).decision,
        Decision::Allow,
        "still removed, read by the new models"
    );
    // And taught again, it is the same rule coming back, not a session copy.
    let said = app.add_session_rule("deny", "bat --language rust").unwrap();
    assert!(said.contains("back in force"), "{said}");
    assert_eq!(app.rules.decide_line(line).decision, Decision::Deny);
}

/// **A trusted project's rules are the user's own, listed apart** (6d,
/// Steven 10-13): read from the project's layer, named as its in `:rules`,
/// and deciding as yours do.
#[test]
fn a_projects_rules_are_listed_apart_and_decide() {
    use shannon_core::permission::Decision;
    let dir = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(dir.path());
    let mut stack = shannon_core::config::ConfigStack::default();
    let (config, _) = stack
        .source(
            shannon_core::config::Origin::Project,
            &path,
            "[permissions]\nrules = [\"allow :read /srv/notes\"]\n",
        )
        .expect("the project's file parses");
    let mut app = test_app_with(config);
    app.set_config_stack(stack, None);
    let listing = app.rules_listing();
    let (yours, project) = listing
        .split_once("THE PROJECT'S CONFIG")
        .expect("the project's section");
    assert!(
        project
            .lines()
            .take(3)
            .any(|l| l.contains("allow :read /srv/notes")),
        "{listing}"
    );
    assert!(!yours.contains("/srv/notes"), "not yours:\n{listing}");
    assert_eq!(
        app.rules.decide_line("cat /srv/notes/a").decision,
        Decision::Allow,
        "it decides as yours do"
    );
}

/// **The project's file follows its trust, and `:reload` keeps it over
/// yours** (6d, Steven 10-13). Only the trust at launch counted: a yes on
/// the trust screen left `.shannon/config.toml` unread until the next
/// launch, `:untrust` left its rules deciding, and `:reload` read yours
/// alone, back over the project's.
#[test]
fn the_projects_file_follows_trust_and_reload() {
    use shannon_core::permission::Decision;
    let start = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "[tui]\nscrolloff = 7\nmouse = false\n\
         [permissions]\nrules = [\"allow :read /srv/notes\"]\n",
    )
    .unwrap();
    let home = TempDir::new().unwrap();
    let mine = home.path().join("config.toml");
    std::fs::write(&mine, "[tui]\nscrolloff = 2\n").unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.user_config = Ok(mine);
    app.dispatch_command_for_test("reload");
    assert_eq!(app.config.tui.scrolloff, 2, "premise: yours alone");
    assert_eq!(verdict(&app, "cat /srv/notes/a"), Decision::Ask, "premise");

    // The screen's yes; `:trust` is the same `trust_dir`.
    app.answer_trust(start.path(), crate::setup::TrustAnswer::ThisRun, false);
    assert_eq!(app.config.tui.scrolloff, 7, "read, over yours");
    assert!(
        app.messages()
            .any(|m| m.contains("changes safety settings") && m.contains("`:untrust` takes it")),
        "its own verb: {:?}",
        app.messages().collect::<Vec<_>>()
    );
    assert!(
        app.messages()
            .any(|m| m.starts_with("these apply on relaunch: mouse")),
        "what only a relaunch applies, said after: {:?}",
        app.messages().collect::<Vec<_>>()
    );
    assert_eq!(
        verdict(&app, "cat /srv/notes/a"),
        Decision::Allow,
        "its rule decides"
    );

    app.dispatch_command_for_test("reload");
    assert_eq!(app.config.tui.scrolloff, 7, "a reload keeps it over yours");
    // An edit to the project's file is taken by `:reload`, and `:source` of
    // it is read as the project's — never a second copy beside it.
    std::fs::write(&path, "[tui]\nscrolloff = 9\n").unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(app.config.tui.scrolloff, 9, "its edit, taken");
    app.dispatch_command_for_test(&format!("source {}", path.display()));
    let origins: Vec<_> = app.config_stack.layers().map(|(o, _)| o).collect();
    assert_eq!(
        origins,
        [
            shannon_core::config::Origin::User,
            shannon_core::config::Origin::Project
        ]
    );

    app.dispatch_command_for_test(&format!("untrust {}", start.path().display()));
    assert_eq!(app.config_stack.project_file(), None, "taken off");
    assert_eq!(app.config.tui.scrolloff, 2, "yours again");
    assert_eq!(
        verdict(&app, "cat /srv/notes/a"),
        Decision::Ask,
        "its rule no longer decides"
    );
    assert!(
        app.messages()
            .any(|m| m.starts_with("took .shannon/config.toml off (changed:")),
        "said: {:?}",
        app.messages().collect::<Vec<_>>()
    );
}

/// **What the project's file changes about safety is kept once said** —
/// beside the trust, owner-only — for the next launch to say it only if it
/// is news (Steven, 10-19: "only the first time on enabling trust"; the
/// stack's half is `a_projects_safety_note_is_said_when_it_is_news`). From
/// both doors: a trust's read, and launch's stack. `:untrust` forgets it, so
/// a trust again says it again.
#[test]
fn what_a_projects_file_changes_is_kept_once_said() {
    use shannon_core::config::{ConfigStack, Origin};
    let start = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "[permissions]\nrules = [\"allow :read /srv/notes\"]\n";
    std::fs::write(&path, text).unwrap();
    let state = TempDir::new().unwrap();
    let file = state.path().join("trusted_dirs");
    let kept = |file: &std::path::Path| {
        let (trusted, _) = crate::trust::Trusted::load(Some(file.to_path_buf()));
        trusted.told(&path).map(str::to_string)
    };
    let mut app = test_app();
    app.set_trusted(crate::trust::Trusted::load(Some(file.clone())).0);
    app.start = Some(start.path().to_path_buf());

    app.dispatch_command_for_test(&format!("trust {}", start.path().display()));
    assert!(
        app.messages()
            .any(|m| m.contains("changes safety settings")),
        "said at the trust: {:?}",
        app.messages().collect::<Vec<_>>()
    );
    let said = app.config_stack.told().map(|(_, d)| d.to_string());
    assert!(said.is_some(), "premise: the stack said it");
    assert_eq!(kept(&file), said, "kept for the next launch");

    // Kept already: a later change of config writes nothing — so a state
    // directory gone read-only does not say, at every `:set`, that what was
    // told could not be kept (review stop 6, A 3: the early return's
    // deletion passed every test).
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |m| std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(m));
        mode(0o500).unwrap();
        app.set_command("tui.scrolloff=3", false);
        mode(0o700).unwrap();
        assert!(
            !app.messages().any(|m| m.contains("said again next launch")),
            "{:?}",
            app.messages().collect::<Vec<_>>()
        );
    }

    app.dispatch_command_for_test(&format!("untrust {}", start.path().display()));
    assert_eq!(kept(&file), None, "forgotten with the trust");

    // Launch's door: the stack it hands over was told already — at a start
    // launch trusts, as it reads a project's file only there.
    let elsewhere = TempDir::new().unwrap();
    let other = elsewhere.path().join("trusted_dirs");
    let mut app = test_app();
    let (mut trusted, _) = crate::trust::Trusted::load(Some(other.clone()));
    trusted
        .trust(start.path(), crate::trust::Lasting::Remembered)
        .unwrap();
    app.set_trusted(trusted);
    let mut stack = ConfigStack::default();
    stack.source(Origin::Project, &path, text).expect("parses");
    let said = stack.told().map(|(_, d)| d.to_string());
    app.set_config_stack(stack, None);
    assert_eq!(kept(&other), said, "kept at launch");
}

/// **What an untrusted start holds unread is said where it can be seen**
/// (Steven, 10-13: it never showed). On the API, at launch. On Claude Code
/// the trust screen is the question — and the key answering it no longer
/// wipes the launch notes it covered — then a yes says what it read in its
/// one sentence (said as a second notice, the row's cut hid it), and a no
/// says what stays unread.
#[test]
fn an_unread_project_is_said_where_it_is_seen() {
    let start = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "[permissions]\nrules = [\"allow :read /srv/notes\"]\n",
    )
    .unwrap();

    let mut api = test_app();
    api.start = Some(start.path().to_path_buf());
    api.ask_trust_up_front();
    assert!(
        api.notice()
            .is_some_and(|n| n.contains(".shannon/config.toml is not read")),
        "{:?}",
        api.notice()
    );
    // A launch that opened setup instead: left, it says so too (review
    // 2026-10-13, A: it never did).
    let mut api = test_app();
    api.start = Some(start.path().to_path_buf());
    api.open_setup();
    press_setup(&mut api, KeyCode::Esc);
    assert!(
        api.notice()
            .is_some_and(|n| n.contains(".shannon/config.toml is not read")),
        "{:?}",
        api.notice()
    );

    for (key, said, seen) in [
        (
            '3',
            "not trusted — .shannon/config.toml is not read",
            "not trusted — .shannon/config.toml is not read",
        ),
        (
            '2',
            "read .shannon/config.toml (changed: permissions.rules)",
            "read .shannon/config.toml",
        ),
    ] {
        let (mut app, _) = untrusted_claude_code_app();
        app.start = Some(start.path().to_path_buf());
        app.notices.clear();
        app.report_startup(&["a launch note".to_string()]);
        app.ask_trust_up_front();
        assert!(trust_screen_up(&app), "premise: the screen asks");
        // The screen is where it can be seen: it names what a yes applies.
        let shown = screen(&mut app, 100);
        assert!(
            shown.contains("This folder holds Shannon settings")
                && shown.contains(".shannon/config.toml"),
            "{shown}"
        );
        press_setup(&mut app, KeyCode::Char(key));
        let row = app.notice().unwrap_or_default();
        // The answer first, then the notes the screen covered (Steven,
        // 10-14: "the trust message should come first").
        assert!(row.starts_with(said), "{key}: {row}");
        assert!(
            row.contains(" · a launch note"),
            "kept under the screen: {row}"
        );
        // On the row as drawn at 80 columns, not only in the joined notice:
        // the cut hid it there (review 2026-10-13, C).
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24))
            .expect("a test terminal");
        draw_frame(&mut app, &mut terminal);
        let drawn = rendered_text(terminal.backend().buffer());
        assert!(drawn.contains(seen), "{key}: {row}\n{drawn}");
    }
}

/// **A project's models follow its trust, with no config file beside them**
/// (review 2026-10-13, A, B, C: deleting the gate passed every test). Not
/// trusted, `.shannon/models/` is not read; `:trust` reads it and says so;
/// `:untrust` drops it and says so.
#[test]
fn a_projects_models_follow_its_trust() {
    use shannon_core::permission::model::Source;
    let start = TempDir::new().unwrap();
    let models = shannon_core::config::project_models_dir(start.path());
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(
        models.join("bat.toml"),
        "name = \"bat\"\neffect = \"read\"\npositional = [{ at = \"rest\", role = \"read_file\" }]\n",
    )
    .unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    let source = |app: &App| app.models.get("bat").map(|m| m.source.clone());
    let _ = app.load_models();
    assert_eq!(source(&app), None, "not trusted: not read");

    app.dispatch_command_for_test(&format!("trust {}", start.path().display()));
    assert_eq!(source(&app), Some(Source::Project("bat.toml".into())));
    assert!(
        app.notice()
            .is_some_and(|n| n.starts_with("read .shannon/models/bat.toml — trusted")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test(&format!("untrust {}", start.path().display()));
    assert_eq!(source(&app), None, "dropped");
    assert!(
        app.notice()
            .is_some_and(|n| n.starts_with("took .shannon/models/bat.toml off — no longer")),
        "{:?}",
        app.notice()
    );
}

/// **A project's file that will not read makes the trust's sentence a
/// refusal** naming why — it said "NOT LOADED", then "its own settings
/// apply there" (review 2026-10-13, A). And `:source` of the project's
/// file after `:unsource` reads it as the project's, as `:reload` does —
/// it came back `Sourced`, and a reload then laid a second copy (A).
#[test]
fn the_projects_file_reads_one_way() {
    let start = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[tui\n").unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.dispatch_command_for_test(&format!("trust {}", start.path().display()));
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert!(
        app.notice()
            .is_some_and(|n| n.starts_with(".shannon/config.toml NOT LOADED (")
                && !n.contains("its own settings apply")),
        "{:?}",
        app.notice()
    );

    std::fs::write(&path, "[tui]\nscrolloff = 6\n").unwrap();
    app.dispatch_command_for_test(&format!("source {}", path.display()));
    app.dispatch_command_for_test(&format!("unsource {}", path.display()));
    app.dispatch_command_for_test(&format!("source {}", path.display()));
    let origins: Vec<_> = app.config_stack.layers().map(|(o, _)| o).collect();
    assert_eq!(origins, [shannon_core::config::Origin::Project]);
}

/// **`:reload` reads a project's file made since launch** in a trusted
/// start — by hand, or by `:config! save` before a `:trust` — not only one
/// loaded at launch.
#[test]
fn a_reload_reads_a_projects_file_made_since() {
    let start = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let mine = home.path().join("config.toml");
    std::fs::write(&mine, "[tui]\nscrolloff = 2\n").unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.user_config = Ok(mine.clone());
    app.trust_dir(start.path(), crate::trust::Lasting::ThisRun);
    assert_eq!(app.config_stack.project_file(), None, "premise: none yet");
    // A trusted start with no project file — the common case — reads
    // yours: the missing file is left out, never the reason to refuse.
    std::fs::write(&mine, "[tui]\nscrolloff = 3\n").unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(
        app.config.tui.scrolloff,
        3,
        "yours, read: {:?}",
        app.notice()
    );
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[tui]\nscrolloff = 6\n").unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(app.config.tui.scrolloff, 6, "read, over yours");
    assert!(app.config_stack.project_file().is_some());
    // Gone since: taken off and said; your edit is read all the same.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&mine, "[tui]\nscrolloff = 4\n").unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(app.config.tui.scrolloff, 4, "{:?}", app.notice());
    assert_eq!(app.config_stack.project_file(), None);
    assert!(
        app.messages().any(|m| m.contains("is gone — taken off")),
        "{:?}",
        app.messages().collect::<Vec<_>>()
    );

    // No config of yours (a normal state): the project's edit is read.
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.user_config = Ok(home.path().join("absent.toml"));
    app.trust_dir(start.path(), crate::trust::Lasting::ThisRun);
    std::fs::write(&path, "[tui]\nscrolloff = 8\n").unwrap();
    app.dispatch_command_for_test("reload");
    assert_eq!(app.config.tui.scrolloff, 8, "{:?}", app.notice());
}

/// **A rule holding a value marked since it was typed stays out of the
/// file** (review stop 17, E1 M2): the door asks at the moment of entry, and
/// what counts as a secret changes after it. The same text typed after the
/// mark was refused, and typed before it was written beside the project's
/// code. Left out, and said so without the value; the session keeps its
/// rule, and the rest is saved.
#[test]
fn config_save_leaves_out_a_rule_holding_a_value_marked_since() {
    let secret = "plainword7";
    let start = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.dispatch_command_for_test(&format!("allow deploy-tool {secret} *"));
    assert_eq!(
        app.session_rules.len(),
        1,
        "premise: taught before the mark"
    );
    app.dispatch_command_for_test("allow cargo test *");
    app.mark_secret(secret, true).expect("marked");
    app.dispatch_command_for_test("config! save");
    let written = std::fs::read_to_string(&path).expect("saved");
    assert!(written.contains("allow cargo test *"), "premise: {written}");
    assert!(!written.contains(secret), "{written}");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("not saved: a rule holding a credential (a secret Shannon holds)")
            && !notice.contains(secret),
        "{notice:?}"
    );
    assert_eq!(app.session_rules.len(), 2, "still this session's");
}

/// **`:config save` writes what this session set into the project's file**
/// (Steven, 10-13): a `:set` at its value, an `:allow` among its rules, a
/// shipped rule `:un…`'d in its `remove` — into the file as it stands,
/// comments kept — and reads it back, the rules from it now (listed once,
/// under the project). Not trusted, it is refused; `!` writes it anyway,
/// and says it is not read. Nothing set, nothing written; a `.shannon` that
/// is a link, never written through.
#[test]
fn config_save_writes_what_the_session_set() {
    use shannon_core::permission::Decision;
    let start = TempDir::new().unwrap();
    let path = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# the team's\n[permissions]\n\
         rules = [\"allow :read /srv/a # the team's\", \"allow :read /srv/b\"]\n",
    )
    .unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.trust_dir(start.path(), crate::trust::Lasting::ThisRun);
    app.dispatch_command_for_test("config! dump");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`!` is `:config! save`'s alone")),
        "a `!` that means nothing is refused, not dropped: {:?}",
        app.notice()
    );
    app.dispatch_command_for_test("config save");
    assert!(
        app.notice().is_some_and(|n| n.contains("nothing to save")),
        "{:?}",
        app.notice()
    );
    // Only what a project's file cannot hold: refused, and said why.
    let mine = TempDir::new().unwrap();
    app.dispatch_command_for_test(&format!("set memory.write={}", mine.path().display()));
    app.dispatch_command_for_test("config save");
    assert!(
        app.notice().is_some_and(|n| n.contains("nothing to save")
            && n.contains("memory.write (only your own config sets it)")),
        "{:?}",
        app.notice()
    );

    // A commented line is the project's rule, named as its.
    let listing = app.rules_listing();
    let (yours, _) = listing
        .split_once("THE PROJECT'S CONFIG")
        .expect("the project's section");
    assert!(!yours.contains("/srv/a"), "{listing}");

    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("allow cargo test *");
    app.dispatch_command_for_test("unask! :write **/.claude");
    app.dispatch_command_for_test("unallow :read /srv/a");
    app.dispatch_command_for_test("config save");
    let saved = std::fs::read_to_string(&path).unwrap();
    for want in [
        "# the team's",
        "allow :read /srv/b",
        "allow cargo test *",
        "scrolloff = 4",
    ] {
        assert!(saved.contains(want), "{want:?} in:\n{saved}");
    }
    assert!(
        !saved.contains("/srv/a"),
        "taken out, comment and all:\n{saved}"
    );
    let file = shannon_core::Config::parse(&saved).unwrap();
    // The shipped rule as `:rules` lists it (its `\c` with it).
    assert_eq!(file.permissions.remove, [r"ask :write \c**/.claude"]);
    assert!(
        app.messages().any(|m| m.contains("saved")
            && m.contains(
                "1 option (tui.scrolloff), 1 rule, 1 of its rules taken out, \
                 1 shipped rule removed"
            )),
        "{:?}",
        app.messages().collect::<Vec<_>>()
    );
    assert!(
        app.messages()
            .any(|m| m.starts_with("not saved: memory.write")),
        "{:?}",
        app.messages().collect::<Vec<_>>()
    );
    // Read back: the rules come from the file now, listed once; the
    // removals it says are its, not this session's.
    assert!(app.session_rules.is_empty(), "the session's moved into it");
    assert!(app.session_removed.is_empty(), "{:?}", app.session_removed);
    let listing = app.rules_listing();
    let (_, project) = listing
        .split_once("THE PROJECT'S CONFIG")
        .expect("the project's section");
    assert!(project.contains("allow cargo test *"), "{listing}");
    assert!(
        listing.contains(r"ask :write \c**/.claude   (the project's config)"),
        "the removal named by the file that holds it:\n{listing}"
    );
    assert_eq!(
        verdict(&app, "cargo test"),
        Decision::Allow,
        "still decides"
    );
    assert_eq!(verdict(&app, "cat /srv/a/x"), Decision::Ask, "gone from it");

    // A `config.toml` that is a link is never read through, nor replaced by
    // what it leads to.
    let linked_file = TempDir::new().unwrap();
    let private = linked_file.path().join("private.toml");
    std::fs::write(&private, "# private\n[tui]\nscrolloff = 9\n").unwrap();
    let dot = linked_file.path().join(".shannon");
    std::fs::create_dir_all(&dot).unwrap();
    std::os::unix::fs::symlink(&private, dot.join("config.toml")).unwrap();
    let mut app = test_app();
    app.start = Some(linked_file.path().to_path_buf());
    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("config! save");
    assert!(
        app.notice().is_some_and(|n| n.contains("is a link")),
        "{:?}",
        app.notice()
    );
    assert!(
        std::fs::symlink_metadata(dot.join("config.toml")).is_ok_and(|m| m.is_symlink()),
        "not replaced by a copy of what it leads to"
    );
    assert_eq!(
        std::fs::read_to_string(&private).unwrap(),
        "# private\n[tui]\nscrolloff = 9\n"
    );

    // Not trusted: refused; `!` writes it, and says it is not read. Then
    // trusted, its rules are listed once — the file's, not this session's.
    let elsewhere = TempDir::new().unwrap();
    let theirs = shannon_core::config::project_path(elsewhere.path());
    let mut app = test_app();
    app.start = Some(elsewhere.path().to_path_buf());
    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("allow cargo test *");
    app.dispatch_command_for_test("config save");
    assert!(!theirs.exists(), "refused, nothing written");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("not trusted") && n.contains("`:config! save`")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("config! save");
    assert!(
        std::fs::read_to_string(&theirs).is_ok_and(|t| t.contains("scrolloff = 4")),
        "written anyway"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("not read here until you `:trust .`")),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.session_rules.len(),
        1,
        "premise: not read, still the session's"
    );
    app.trust_dir(elsewhere.path(), crate::trust::Lasting::ThisRun);
    assert!(
        app.session_rules.is_empty(),
        "the file's now, listed once: {}",
        app.rules_listing()
    );

    // A `.shannon` that is a link is never written through.
    let linked = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    std::os::unix::fs::symlink(target.path(), linked.path().join(".shannon")).unwrap();
    let mut app = test_app();
    app.start = Some(linked.path().to_path_buf());
    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("config! save");
    assert!(
        !target.path().join("config.toml").exists(),
        "not written through the link"
    );
    assert!(
        app.notice().is_some_and(|n| n.contains("is a link")),
        "{:?}",
        app.notice()
    );
}

/// **`:config save <file>` writes the same into a file you name** (Steven,
/// 11-10: the README's "export your settings") — into the file as it
/// stands, comments kept, and nothing read back: the rules stay the
/// session's. Only what was added: a rule taken out of the project's own
/// entries is that file's to lose, said and not written. Trusted or not;
/// the project's own file by name is the bare form, its trust gate with it;
/// a file that is a link is refused and left a link; a relative name is
/// under the directory Shannon started in.
#[test]
fn config_save_to_a_file_writes_what_the_session_added() {
    let start = TempDir::new().unwrap();
    let project = shannon_core::config::project_path(start.path());
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    std::fs::write(
        &project,
        "[permissions]\nrules = [\"allow :read /srv/a\"]\n",
    )
    .unwrap();
    let out = TempDir::new().unwrap();
    let file = out.path().join("mine.toml");
    std::fs::write(&file, "# my settings\n[tui]\nsplitright = true\n").unwrap();
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.trust_dir(start.path(), crate::trust::Lasting::ThisRun);
    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("allow cargo test *");
    app.dispatch_command_for_test("unallow :read /srv/a");
    let project_was = std::fs::read_to_string(&project).unwrap();

    app.dispatch_command_for_test(&format!("config save {}", file.display()));

    let saved = std::fs::read_to_string(&file).expect("saved");
    for want in [
        "# my settings",
        "splitright = true",
        "scrolloff = 4",
        "allow cargo test *",
    ] {
        assert!(saved.contains(want), "{want:?} in:\n{saved}");
    }
    assert!(!saved.contains("/srv/a"), "the project's own:\n{saved}");
    shannon_core::Config::parse(&saved).expect("it reads as a config");
    assert_eq!(
        std::fs::read_to_string(&project).unwrap(),
        project_was,
        "the project's file untouched"
    );
    assert_eq!(app.session_rules.len(), 1, "nothing read back");
    let messages: Vec<&str> = app.messages().collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("saved") && m.contains("mine.toml")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.starts_with("not saved")
            && m.contains("`allow :read /srv/a` out of permissions.rules")),
        "{messages:?}"
    );

    // Not trusted: written all the same, under the start when relative.
    let elsewhere = TempDir::new().unwrap();
    let mut app = test_app();
    app.start = Some(elsewhere.path().to_path_buf());
    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("config save kept.toml");
    let kept = elsewhere.path().join("kept.toml");
    assert!(
        std::fs::read_to_string(&kept).is_ok_and(|t| t.contains("scrolloff = 4")),
        "{:?}",
        app.notice()
    );
    // The project's own file by name: the bare form, refused untrusted —
    // however it is spelt (V3 M1: through `..`, and by the real path of a
    // start reached through a link — macOS's `/var` is `/private/var`, where
    // a TempDir lives; compared as spelt, both were written past the gate).
    let theirs = shannon_core::config::project_path(elsewhere.path());
    let base = elsewhere.path().file_name().unwrap().to_string_lossy();
    let real = std::fs::canonicalize(elsewhere.path()).unwrap();
    for spelt in [
        theirs.display().to_string(),
        format!("../{base}/.shannon/config.toml"),
        format!("{}/.shannon/config.toml", real.display()),
    ] {
        app.dispatch_command_for_test(&format!("config save {spelt}"));
        assert!(!theirs.exists(), "{spelt}: refused, nothing written");
        assert!(
            app.notice().is_some_and(|n| n.contains("not trusted")),
            "{spelt}: {:?}",
            app.notice()
        );
    }

    // Only a take out of the project's own entries: nothing a file of your
    // naming can hold — refused, nothing written, not "saved …: " with an
    // empty `rules = []` (V3 M2: the `retain` held by no test).
    let mut app = test_app();
    app.start = Some(start.path().to_path_buf());
    app.trust_dir(start.path(), crate::trust::Lasting::ThisRun);
    app.dispatch_command_for_test("unallow :read /srv/a");
    let only = out.path().join("only.toml");
    app.dispatch_command_for_test(&format!("config save {}", only.display()));
    assert!(!only.exists(), "nothing written");
    assert!(
        app.notice().is_some_and(|n| n.contains("nothing to save")
            && n.contains("a file of your naming can hold")
            && n.contains("`allow :read /srv/a` out of permissions.rules")),
        "{:?}",
        app.notice()
    );

    // A link: refused, left a link, what it leads to unchanged.
    let real = out.path().join("real.toml");
    std::fs::write(&real, "# real\n").unwrap();
    let link = out.path().join("link.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    app.dispatch_command_for_test(&format!("config save {}", link.display()));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("is a link") && n.contains("name the one it points to")),
        "{:?}",
        app.notice()
    );
    assert!(std::fs::symlink_metadata(&link).is_ok_and(|m| m.is_symlink()));
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "# real\n");
}

/// **`:rules` lists what the gate reads, not only what was written** (P5):
/// the credential guards `[secrets]` asks for, Shannon's own state, the
/// scratch directory's grant — each as the line the decision reads. The
/// page listed only the rules, so the guard holding `cat ~/.ssh/config` was
/// nowhere on it.
#[test]
fn rules_lists_the_guards_and_grants_the_gate_reads() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    let scratch = TempDir::new().unwrap();
    // The gate's spelling of it, a root link taken (`/tmp` on macOS).
    let scratch = std::fs::canonicalize(scratch.path())
        .unwrap()
        .display()
        .to_string();
    app.protect_paths(vec!["/home/me/.config/shannon".into()]);
    app.set_scratch(vec![scratch.clone()]);
    let listing = app.rules_listing();
    let section = |title: &str| {
        let start = listing
            .find(title)
            .unwrap_or_else(|| panic!("{title}\n{listing}"));
        let rest = &listing[start + title.len()..];
        rest[..rest.find("\n\n").unwrap_or(rest.len())].to_string()
    };
    assert_eq!(verdict(&app, "cat ~/.ssh/config"), Decision::Ask, "premise");
    assert!(
        section("CREDENTIAL PATHS").contains("ask :* \\c~/.ssh\n"),
        "{listing}"
    );
    assert!(
        section("SHANNON'S OWN STATE").contains("ask :* \\c/home/me/.config/shannon"),
        "{listing}"
    );
    assert!(
        section("SHIPPED GRANTS").contains(&format!("allow :* {scratch}/*")),
        "{listing}"
    );
    assert!(
        section("SHIPPED ALLOWS").contains("allow re:^date"),
        "{listing}"
    );
    assert!(
        !section("SHIPPED GUARDS").contains("  allow "),
        "a guard is no allow: {listing}"
    );
}
