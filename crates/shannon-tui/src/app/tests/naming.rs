//! **Session names and descriptions** (`app/naming.rs`): what a name may be,
//! that every session verb reads it through the one door, where it shows,
//! that it outlives the run, and that it goes on with the work.

use super::*;
use proptest::prelude::*;

/// Two sessions on disk over `dir`, the second in front, one pane:
/// `(app, first, second)`. Named by its shape: it was `two_sessions`, the
/// name `app/tests.rs` gives two panes side by side (review stop 12, T4 6).
fn two_stored_sessions(dir: &std::path::Path) -> (App, EpisodeId, EpisodeId) {
    let mut app = app_with_n_turns(dir, 1);
    let first = app.conversation().episode().id;
    let second = add_persisted_session(&mut app, "Second");
    assert_eq!(app.conversation().episode().id, second, "premise: in front");
    (app, first, second)
}

fn name_of(app: &App, episode: EpisodeId) -> Option<String> {
    app.label_of(episode).name
}

/// **A short id read off a border is an id, even `b` and seven digits** — one
/// id in ~430; it read as buffer 4821390 and flaked the rename test below
/// (Steven, 10-20). A buffer number is `b` and at most six digits.
#[test]
fn a_short_id_of_b_and_digits_is_an_id() {
    assert_eq!(
        names_a_session("b4821390"),
        Some(SessionName::Id),
        "8 characters"
    );
    assert_eq!(names_a_session("b12345678"), Some(SessionName::Id));
    assert_eq!(
        names_a_session("b123456"),
        Some(SessionName::Buffer(123456)),
        "six digits"
    );
    assert_eq!(names_a_session("b2"), Some(SessionName::Buffer(2)));
}

/// **A name is one word that no other reading claims** — so a token is never
/// a name AND an id, a buffer, a pane or `%`, and the refusal says which
/// reading it would have collided with, at the moment of naming.
#[test]
fn a_name_is_one_word_that_no_other_reading_claims() {
    // Every script, with the marks its words are written with (review stop
    // 9's verification, G 7): Korean, Devanagari's virama, Thai's tone mark,
    // an accent combining — refused while a name had to be alphanumeric.
    for good in [
        "auth-fix",
        "v2.0",
        "notes_1",
        "abc",
        "café",
        "b",
        "bx2",
        "인증-수정",
        "क्षेत्र",
        "ไม่ได้",
        "cafe\u{301}",
    ] {
        assert_eq!(crate::app::naming::name_syntax(good), Ok(()), "{good}");
        assert_eq!(names_a_session(good), Some(SessionName::Name), "{good}");
    }
    let long = "n".repeat(49);
    for (bad, why) in [
        ("", "a character"),
        ("-x", "flag"),
        ("3", "pane number"),
        ("b3", "buffer"),
        ("cafe", "session id"),
        ("CAFE", "session id"),
        ("deadbeef", "session id"),
        ("%", "cannot be in a name"),
        ("a b", "cannot be in a name"),
        ("x!", "cannot be in a name"),
        ("x\u{202e}y", "cannot be in a name"),
        ("auth\u{200b}fix", "cannot be in a name"),
        ("a\u{2028}b", "cannot be in a name"),
        ("a/b", "cannot be in a name"),
        (long.as_str(), "at most"),
        ("0000", "session id"),
        ("all", "read first"),
        ("ball", "read first"),
    ] {
        let said = crate::app::naming::name_syntax(bad).expect_err(bad);
        assert!(said.contains(why), "{bad:?}: {said}");
    }
    // Every keyword a session command reads before any session, read off
    // the table rather than listed again here — a name `all` made `:delete
    // all` delete everything (review of 34033e8, Reviewers A and B).
    let keywords: Vec<&str> = crate::command::session_keywords().collect();
    assert!(
        keywords.contains(&"all") && keywords.contains(&"ball"),
        "premise: {keywords:?}"
    );
    for word in keywords {
        assert!(crate::app::naming::name_syntax(word).is_err(), "{word}");
    }
}

proptest! {
    /// **The readings are disjoint by construction**: whatever passes as a
    /// name reads as a name and as nothing else, and a short run of digits
    /// stays a pane's — four or more are an id, as they always were
    /// (`send_target`). Biased to the space where they meet — hex, digits,
    /// `b`, `%` — with digits alone drawn often enough to reach the boundary.
    #[test]
    fn a_name_never_reads_as_another_token(
        token in prop_oneof!["[0-9a-fA-Fb%._-]{0,8}", "[0-9]{1,6}", "\\PC{0,12}"]
    ) {
        if crate::app::naming::name_syntax(&token).is_ok() {
            prop_assert_eq!(names_a_session(&token), Some(SessionName::Name));
            prop_assert!(id_prefix(&token).is_err());
        }
        if token.chars().all(|c| c.is_ascii_digit()) {
            let pane = (1..4).contains(&token.len());
            let read = names_a_session(&token);
            prop_assert_eq!(read, (!pane && !token.is_empty()).then_some(SessionName::Id));
        }
    }
}

/// **Every session verb reads a name, and `%`, through the one door** —
/// `session_named` — and a confirmation names the session as its graph row
/// does, so `:delete auth-fix` is checked against `auth-fix` on screen.
#[tokio::test]
async fn every_session_verb_reads_a_name_and_percent() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = two_stored_sessions(dir.path());
    app.dispatch_command_for_test(&format!("rename {} auth-fix", first.short_hex()));
    assert_eq!(
        name_of(&app, first).as_deref(),
        Some("auth-fix"),
        "named by token"
    );
    assert_eq!(name_of(&app, second), None, "and only that one");

    assert_eq!(app.session_named("auth-fix"), Ok(first));
    assert_eq!(
        app.session_named("%"),
        Ok(second),
        "the session the prompt goes to"
    );
    let none = app.session_named("nope").expect_err("no such name");
    assert!(
        none.contains("no session is named `nope` — `:graph` shows every one"),
        "`:ls` lists this run's, not the one you asked for: {none}"
    );

    app.dispatch_command_for_test("delete auth-fix");
    let asked = app.confirm_prompt().expect("asks first").text();
    assert!(asked.contains("auth-fix"), "{asked}");
    app.pending_confirm.clear();

    app.dispatch_command_for_test("open auth-fix");
    assert_eq!(app.conversation().episode().id, first, "`:open` by name");
    assert_eq!(app.session_named("%"), Ok(first), "`%` follows the aim");

    // The aim, not the foreground: with the prompt aimed at the other
    // session, `%` is that one — what a bare verb would act on.
    let other = app
        .buffers
        .sessions()
        .find(|b| b.conversation.episode().id == second)
        .map(|b| b.id)
        .expect("still open, hidden");
    app.aim_prompt_at(other);
    assert_eq!(app.conversation().episode().id, first, "premise: in front");
    assert_eq!(app.session_named("%"), Ok(second), "the aim's");
}

/// **A question names a session as it was when asked, and its draw reads no
/// store** — the y/N row is drawn every frame, and naming a session no
/// buffer holds read its whole log each time (review of 34033e8, Reviewers A
/// and B). Proved by taking the log away after asking: the question still
/// says the name.
#[test]
fn a_delete_question_names_the_session_without_reading_the_store() {
    let dir = TempDir::new().unwrap();
    let first = {
        let (mut app, first, _) = two_stored_sessions(dir.path());
        app.dispatch_command_for_test(&format!("rename {} auth-fix", first.short_hex()));
        first
    };
    let mut app = relaunched(dir.path());
    assert!(
        app.buffers
            .sessions()
            .all(|b| b.conversation.episode().id != first),
        "premise: in no buffer"
    );
    app.dispatch_command_for_test("delete auth-fix");
    std::fs::remove_file(
        dir.path()
            .join("episodes")
            .join(format!("{}.jsonl", first.to_hex())),
    )
    .unwrap();
    let asked = app.confirm_prompt().expect("asks first").text();
    assert!(asked.contains("auth-fix"), "named as asked: {asked}");
}

/// **A name is one session's**: naming a second session what the first is
/// called is refused and changes neither; renaming, saying, and taking it
/// off all say what they did.
#[tokio::test]
async fn a_name_is_one_sessions() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = two_stored_sessions(dir.path());
    app.dispatch_command_for_test(&format!("rename {} auth-fix", first.short_hex()));
    app.notices.clear();
    app.dispatch_command_for_test("rename auth-fix");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("already names"), "{said}");
    assert_eq!(name_of(&app, second), None, "refused");
    assert_eq!(name_of(&app, first).as_deref(), Some("auth-fix"), "kept");

    app.dispatch_command_for_test("rename token-race");
    assert_eq!(name_of(&app, second).as_deref(), Some("token-race"));
    app.notices.clear();
    app.dispatch_command_for_test("rename refresh");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("renamed `token-race` → `refresh`"), "{said}");
    app.notices.clear();
    app.dispatch_command_for_test("rename refresh");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("is already `refresh`"),
        "the same name again: {said}"
    );
    assert_eq!(
        app.free_name(second, "refresh"),
        Ok(()),
        "a session's own name is free to it"
    );
    app.notices.clear();
    app.dispatch_command_for_test("rename");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("`refresh` names"), "bare says it: {said}");
    app.dispatch_command_for_test("rename!");
    assert_eq!(name_of(&app, second), None, "`!` takes it off");
    app.notices.clear();
    app.dispatch_command_for_test("rename cafe");
    assert!(
        app.notice().unwrap_or_default().contains("session id"),
        "a refusal says which reading it collides with"
    );

    // **And which of the two it is** (Steven, 10-14: "Is it because it
    // _looks_ like a session ID or because _it is_ an existing session"):
    // another session's id says whose; an id-shaped word no session has
    // says it only looks like one. Refused either way, by its shape.
    app.notices.clear();
    app.dispatch_command_for_test(&format!("rename {}", first.short_hex()));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "is the session id of auth-fix {}",
            first.short_hex()
        )),
        "{said}"
    );
    let ids = [first.to_hex(), second.to_hex()];
    let nobodys = "0123456789abcdef"
        .chars()
        .map(|c| c.to_string().repeat(8))
        .find(|t| ids.iter().all(|id| !id.starts_with(t.as_str())))
        .expect("two ids cannot begin with all sixteen");
    app.notices.clear();
    app.dispatch_command_for_test(&format!("rename {nobodys}"));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("looks like a session id"), "{said}");
    assert_eq!(name_of(&app, second), None, "refused, both ways");
}

/// **A name is read whole, never by its start** (Steven, 11-09: renames
/// collide — "`:rename working-on-some-project` and in another …
/// `working-on-some-other-project`"). An id abbreviates, as an id's start
/// is one id's; a name is a word a person chose, and two may begin alike,
/// so its start names neither, at any length — not even when one name is
/// the only one so begun. Tab still offers each by its start: completion
/// is the way to a long name, and what it fills in is whole.
#[tokio::test]
async fn a_name_is_read_whole_never_by_its_start() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = two_stored_sessions(dir.path());
    let (one, other) = ("working-on-some-project", "working-on-some-other-project");
    app.dispatch_command_for_test(&format!("rename {} {one}", first.short_hex()));
    app.dispatch_command_for_test(&format!("rename {} {other}", second.short_hex()));
    assert_eq!(name_of(&app, first).as_deref(), Some(one), "premise");
    assert_eq!(name_of(&app, second).as_deref(), Some(other), "premise");

    assert_eq!(app.session_named(one), Ok(first), "whole");
    assert_eq!(app.session_named(other), Ok(second), "whole");
    for start in [
        "working-on-some",
        "working-on-some-p",
        "working-on-some-projec",
    ] {
        let said = app.session_named(start).expect_err(start);
        assert!(
            said.contains(&format!("no session is named `{start}`")),
            "{said}"
        );
    }

    // A verb is refused, and acts on nothing.
    let front = app.conversation().episode().id;
    app.dispatch_command_for_test("open working-on-some-p");
    assert_eq!(app.conversation().episode().id, front, "nothing opened");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no session is named")),
        "{:?}",
        app.notice()
    );

    // Tab offers each by its start, and fills in the whole.
    type_keys(&mut app, ":open working-on-some<Tab>");
    assert_eq!(app.cmdline.line(), Some(format!("open {other}").as_str()));
    type_keys(&mut app, "<Tab>");
    assert_eq!(app.cmdline.line(), Some(format!("open {one}").as_str()));
    // `:open` from the `:` line wants a terminal; the line it filled in, run.
    let filled = app.cmdline.line().map(str::to_string).expect("filled");
    type_keys(&mut app, "<Esc>");
    app.dispatch_command_for_test(&filled);
    assert_eq!(
        app.conversation().episode().id,
        first,
        "the whole name opens it"
    );
}

/// **A name outlives the run** — on the session's header, read back by a
/// relaunch, for one open in front and for one renamed while in no buffer.
#[tokio::test]
async fn a_name_and_description_come_back_after_a_relaunch() {
    let dir = TempDir::new().unwrap();
    let (first, second) = {
        let (mut app, first, second) = two_stored_sessions(dir.path());
        app.dispatch_command_for_test("rename here");
        app.dispatch_command_for_test("description the token refresh race");
        (first, second)
    };
    // A relaunch starts fresh, so both are in no buffer: what it reads, and
    // what the rename below writes, goes through the store half of the door.
    let mut app = relaunched(dir.path());
    let label = app.label_of(second);
    assert_eq!(label.name.as_deref(), Some("here"));
    assert_eq!(label.description.as_deref(), Some("the token refresh race"));

    app.dispatch_command_for_test(&format!("rename {} there", first.short_hex()));
    assert_eq!(
        name_of(&app, first).as_deref(),
        Some("there"),
        "not open, still named"
    );
    let app = relaunched(dir.path());
    assert_eq!(
        name_of(&app, first).as_deref(),
        Some("there"),
        "and on disk"
    );
}

/// **With `:graph` focused, a bare `:rename` names what it has selected**,
/// as bare `:delete` and `:clone` act on it — not the session behind it.
#[tokio::test]
async fn rename_in_the_graph_names_its_selection() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = two_stored_sessions(dir.path());
    open_graph(&mut app);
    assert!(
        app.graph_view_mut()
            .expect("focused")
            .select_node(Node::Episode(first)),
        "premise: a row"
    );
    app.dispatch_command_for_test("rename picked");
    assert_eq!(name_of(&app, first).as_deref(), Some("picked"));
    assert_eq!(name_of(&app, second), None, "not the one in front");
    app.notices.clear();
    let shown = screen(&mut app, 120);
    assert!(
        shown.contains("picked"),
        "the tree's row shows it at once:\n{shown}"
    );
}

/// **A description is one line of plain text**, said back bare, taken off
/// with `!` — and the inspector shows it, and the name, only when there is
/// one (Steven, 10-14: "only shown when they have values").
#[tokio::test]
async fn the_inspector_shows_a_name_and_description_only_when_given() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, second) = two_stored_sessions(dir.path());
    let fields = |app: &App| -> Vec<String> {
        let mut view = app.build_graph_view();
        assert!(view.select_node(Node::Episode(second)), "premise: a row");
        crate::view::inspector_body(&view, true, "unix")
            .iter()
            .map(crate::paint::PageLine::text)
            .collect()
    };
    let has = |lines: &[String], key: &str| lines.iter().any(|l| l.trim_start().starts_with(key));
    let before = fields(&app);
    assert!(
        !has(&before, "name ") && !has(&before, "about "),
        "{before:?}"
    );

    app.dispatch_command_for_test("rename auth-fix");
    app.notices.clear();
    for bad in ["bad\u{7}bell", "looks\u{202e}reversed"] {
        app.notices.clear();
        app.dispatch_command_for_test(&format!("description {bad}"));
        assert!(
            app.notice().unwrap_or_default().contains("plain text"),
            "a control or bidi character is refused: {bad:?}"
        );
    }
    app.notices.clear();
    app.dispatch_command_for_test(&format!("description {}", "é".repeat(201)));
    assert!(
        app.notice().unwrap_or_default().contains("at most 200"),
        "the bound counts characters, not bytes"
    );
    app.dispatch_command_for_test(&format!("description {}", "é".repeat(200)));
    assert_eq!(
        app.label_of(second).description.map(|d| d.chars().count()),
        Some(200),
        "and holds at it"
    );
    app.dispatch_command_for_test("description the token refresh race");
    let after = fields(&app);
    // First, above the id (Steven, 10-14: "put the name and description at
    // the top, above ID"): the heading, then the name, the description, the
    // id.
    let line_of = |key: &str| after.iter().position(|l| l.trim_start().starts_with(key));
    assert_eq!(
        (line_of("name "), line_of("about "), line_of("id ")),
        (Some(1), Some(2), Some(3)),
        "{after:?}"
    );
    assert!(
        after
            .iter()
            .any(|l| l.contains("name") && l.contains("auth-fix")),
        "{after:?}"
    );
    assert!(
        after
            .iter()
            .any(|l| l.contains("about") && l.contains("the token refresh race")),
        "{after:?}"
    );
    app.dispatch_command_for_test("description!");
    assert!(!has(&fields(&app), "about "), "taken off");
}

/// **The name shows where the id did** — the pane's border, `:graph`'s row
/// in both styles (the name first, the id after it: Steven, 10-14), on the
/// drawn screen, and `<Tab>` offers it first.
#[tokio::test]
async fn a_name_shows_on_the_border_the_tree_and_completion() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = two_stored_sessions(dir.path());
    app.dispatch_command_for_test("rename auth-fix");
    // The notice names the id it acted on, rightly; the border is the subject.
    app.notices.clear();
    let shown = screen(&mut app, 120);
    assert!(shown.contains(" auth-fix "), "the border:\n{shown}");
    assert!(
        !shown.contains(&second.short_hex()),
        "the name, not the id:\n{shown}"
    );

    app.dispatch_command_for_test(&format!("rename {} older", first.short_hex()));
    open_graph(&mut app);
    app.notices.clear();
    let git = screen(&mut app, 120);
    assert!(
        git.contains(&format!("auth-fix {}  (HEAD", second.short_hex())),
        "the name leads, the id after it, HEAD in the decoration:\n{git}"
    );
    assert!(
        git.contains(&format!("older {}  (root", first.short_hex())),
        "a named row that is not HEAD's leads with its name too:\n{git}"
    );

    // The inspector names a turn's session as a sentence does: name and id.
    let turn = app.current_graph().full_turns(&second)[0];
    let mut view = app.build_graph_view();
    assert!(
        view.select_node(Node::Turn {
            episode: second,
            turn,
            index: 0,
        }),
        "premise: the turn row"
    );
    let page: Vec<String> = crate::view::inspector_body(&view, true, "unix")
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    let tag = format!("auth-fix {}", second.short_hex());
    assert!(
        page.iter()
            .any(|l| l.contains("session") && l.contains(&tag)),
        "{page:?}"
    );

    app.dispatch_command_for_test("set tui.graph_style=tree");
    app.dispatch_command_for_test("q");
    open_graph(&mut app);
    let tree = screen(&mut app, 120);
    assert!(
        tree.contains(&format!("auth-fix {}  root", second.short_hex())),
        "{tree}"
    );

    // `hidden` drops a NAMED session's id, at once in the tree already open
    // — the tree style's labels are built with it, so the `:set` rebuilds
    // the tree; a turn row keeps its id, having no name.
    // The tree's rows as drawn (`graph_row_texts`, by the draw's own
    // functions): the screen would also hold the inspector, which names the
    // selected turn's session with its id.
    app.dispatch_command_for_test("set tui.graph_ids=hidden");
    let rows = crate::view::graph_row_texts(
        app.graph_view().expect("the tree, still focused"),
        crate::view::GraphLook::of(&app.config.tui),
        app.theme(),
    );
    let named = rows
        .iter()
        .find(|r| r.contains("auth-fix"))
        .expect("the named row");
    assert!(named.contains("auth-fix  root"), "{named}");
    assert!(
        !named.contains(&second.short_hex()),
        "the named session's id is gone from its row: {named}"
    );
    let turn = app.current_graph().full_turns(&second)[0];
    assert!(
        rows.iter().any(|r| r.contains(&turn.short_hex())),
        "a turn's stays: {rows:?}"
    );

    let offered = app.matching_short_ids(crate::command::IdSpace::Episodes, "au");
    assert_eq!(
        offered.first().map(String::as_str),
        Some("auth-fix"),
        "{offered:?}"
    );
}

/// **A name goes on with the work** — a squash of a session's own turns
/// REPLACES it, so the result takes the name and is written at once (a quit
/// cannot lose it), and the original stays, unnamed; a composition beside
/// its sources (a straight merge's squash-merge form) takes none.
#[tokio::test]
async fn a_replacing_squash_carries_the_name_and_writes_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let original = app.conversation().episode().id;
    app.dispatch_command_for_test("rename auth-fix");
    let graph = app.current_graph();
    let config = graph.episode(&original).unwrap().config.clone();
    let turns = graph.full_turns(&original);
    let summary = Turn::seal(
        vec![Message::user("S"), Message::assistant("summary")],
        config.clone(),
        std::time::SystemTime::UNIX_EPOCH,
    );
    // Its marks where they were: this test asks of names, not marks.
    let same: &dyn Fn(usize) -> Option<usize> = &Some;
    let id = app
        .adopt_composed(
            app.current,
            graph,
            &Squash {
                episode: original,
                start: 0,
                end: 2,
                summary: vec![summary],
            },
            Anchor {
                base: original,
                turn: turns[0],
                index: 0,
            },
            config,
            Some((original, same)),
            Landing::Asked,
        )
        .expect("composed")
        .0;
    assert_eq!(
        app.conversation().episode().id,
        id,
        "premise: in the result"
    );
    assert_eq!(
        name_of(&app, id).as_deref(),
        Some("auth-fix"),
        "the name went on"
    );
    assert_eq!(name_of(&app, original), None, "and left the original");
    assert!(
        matches!(app.store.episode_len(&id), Ok(Some(_))),
        "the result is written at once"
    );
    assert_eq!(
        app.session_named("auth-fix"),
        Ok(id),
        "one session answers it"
    );
}

/// **`:resume` is Claude Code's `/resume`** (Steven, 10-14): bare, the
/// sessions to choose from — the graph, focused, so `<Enter>` opens one;
/// with a name, that session in front. A turn's id is refused toward
/// `:open`, never checked out by a verb that says "resume".
#[tokio::test]
async fn resume_is_the_graph_bare_and_opens_a_session_by_name() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = two_stored_sessions(dir.path());
    app.dispatch_command_for_test(&format!("rename {} auth-fix", first.short_hex()));
    app.dispatch_command_for_test("resume");
    assert_eq!(
        app.focused_viewer(),
        Some(PaneKind::Graph),
        "bare: the graph"
    );
    app.dispatch_command_for_test("q");
    app.dispatch_command_for_test("resume auth-fix");
    assert_eq!(app.conversation().episode().id, first, "by name, in front");

    let turn = app.current_graph().full_turns(&second)[0];
    app.notices.clear();
    app.dispatch_command_for_test(&format!("resume {}", turn.short_hex()));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("`:open`"), "{said}");
    assert_eq!(
        app.conversation().episode().id,
        first,
        "nothing checked out"
    );

    // The pointer only where it is true: an id that names no turn either.
    app.notices.clear();
    app.dispatch_command_for_test("resume fffffffe");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("no session matches") && !said.contains("`:open`"),
        "{said}"
    );
}
