//! **Claude Code's conversations in `:graph`** — listed below the tree, read
//! and never changed; `<Enter>` imports a copy (Steven, 10-04: "I don't see
//! our session in `:graph`. Let's get that going. And make sure we don't let
//! `shannon` delete `claude code` sessions. Only shannon sessions.")
//!
//! Every test points the app at a throwaway projects directory: the test
//! sandbox leaves `claude_projects` unset, so nothing here can reach the
//! user's `~/.claude`.

use super::*;
use crate::graph_view::Entry;
use crate::rows::RowSelect;
use shannon_core::claude_import;

/// A conversation Claude Code would write: one prompt, one reply, a title.
fn conversation(prompt: &str, reply: &str, title: &str) -> String {
    let lines = [
        serde_json::json!({
            "type": "user", "uuid": "u1", "parentUuid": null, "isSidechain": false,
            "timestamp": "01", "cwd": "/work",
            "message": {"role": "user", "content": prompt}
        }),
        serde_json::json!({
            "type": "assistant", "uuid": "a1", "parentUuid": "u1", "isSidechain": false,
            "timestamp": "02", "cwd": "/work",
            "message": {"id": "M1", "model": "claude-x", "role": "assistant",
                        "content": [{"type": "text", "text": reply}]}
        }),
        serde_json::json!({"type": "custom-title", "customTitle": title}),
    ];
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// A projects directory with this directory's conversations — `ids` paired
/// with titles, the first written last (newest) — and the directory they
/// are in.
fn projects_with(conversations: &[(&str, &str)]) -> (TempDir, std::path::PathBuf) {
    let projects = TempDir::new().unwrap();
    let here = projects_in(projects.path(), conversations);
    (projects, here)
}

/// [`projects_with`] at `projects`, which need not exist yet — one inside
/// another temporary directory, so a directory ABOVE Claude Code's can hold
/// a file too.
fn projects_in(projects: &std::path::Path, conversations: &[(&str, &str)]) -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap();
    // Claude Code's own naming of a directory, for an ASCII path.
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let here = projects.join(name);
    std::fs::create_dir_all(&here).unwrap();
    for (id, title) in conversations.iter().rev() {
        let path = here.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            conversation(&format!("prompt of {title}"), "a reply", title),
        )
        .unwrap();
        // Distinct times, so "newest first" has an order to keep.
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        claude_import::project_dir(projects, &cwd),
        Some(here.clone()),
        "premise: Claude Code's name for this directory"
    );
    here
}

/// An app with one Shannon session and `:graph` open over it, Claude Code's
/// conversations listed below.
fn graph_over(projects: &std::path::Path, dir: &std::path::Path) -> App {
    let mut app = app_with_n_turns(dir, 1);
    reading_claude(&mut app, projects);
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    // The reading's arrival is an event, and the loop settles each: left
    // unsettled, the first key's settle saw the open and cancelled the chord
    // that key began (`gg`, `mf`; review stop 12, Tier 3).
    app.set_focus(PaneKind::Graph);
    app.settle();
    app
}

/// The cursor onto the first conversation: the row after the heading.
fn onto_first_conversation(app: &mut App) {
    let view = app.graph_view_mut().unwrap();
    let id = view.claude().unwrap().found[0].id.clone();
    assert!(view.select_claude(&id), "premise: listed");
}

#[test]
fn graph_lists_this_directorys_claude_code_conversations_newest_first() {
    let (projects, _) = projects_with(&[("cc-new", "Our session"), ("cc-old", "Older one")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let section = app
        .graph_view()
        .and_then(GraphView::claude)
        .cloned()
        .unwrap();
    let ids: Vec<&str> = section.found.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["cc-new", "cc-old"]);

    let shown = text_of(&mut app, 120, 30);
    for want in ["Claude Code", "2 here", "Our session", "Older one"] {
        assert!(shown.contains(want), "{want:?} in {shown}");
    }
    // The heading is a rule across the pane: the section reads as apart.
    let heading = shown
        .lines()
        .find(|l| l.contains("── Claude Code"))
        .unwrap_or_else(|| panic!("a rule heads the section: {shown}"));
    assert!(
        heading.contains("2 here ──────────"),
        "the rule runs on toward the pane's edge: {heading:?}"
    );
}

/// **A split tree lists Claude Code's conversations too** — `:sg` and
/// `<C-w>s` build through `:graph`'s own door (`content_for`). Two
/// doors built a tree, and only `:graph`'s added the section, so a split
/// never listed them (`buffers.md` §10.1).
#[test]
fn a_split_tree_lists_claude_codes_conversations_too() {
    let (projects, _) = projects_with(&[("cc-new", "Our session")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    for split in ["graph", ""] {
        app.workspace.focus_kind(PaneKind::Graph);
        app.split_command(split, SplitDir::Vertical);
        app.await_claude_reads();
        let pane = app.workspace.focused();
        assert_eq!(
            app.workspace.kind_of(pane),
            Some(PaneKind::Graph),
            "{split:?}"
        );
        let ids: Vec<String> = app
            .pane_graph(pane)
            .and_then(GraphView::claude)
            .map(|c| c.found.iter().map(|f| f.id.clone()).collect())
            .unwrap_or_default();
        assert_eq!(ids, ["cc-new"], "`:split {split}` lists them");
    }
}

/// **A conversation Shannon itself ran in Claude Code is not listed** — it is
/// a Shannon session's already, in the tree (Steven, 10-04: "those claude
/// code sessions there are effectively shannon sessions"). Filed to resume
/// or only noted (a summary's, a failed turn's): either is Shannon's.
#[test]
fn a_conversation_shannon_started_is_not_listed_again() {
    let (projects, _) = projects_with(&[
        ("cc-mine", "Shannon's"),
        ("cc-summary", "Summarize this conversation"),
        ("cc-theirs", "Claude's"),
    ]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let history = app.conversation().messages().to_vec();
    app.claude_sessions
        .file(&history, "cc-mine".into())
        .unwrap();
    app.claude_sessions.note("cc-summary").unwrap();
    reading_claude(&mut app, projects.path());
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    let section = app
        .graph_view()
        .and_then(GraphView::claude)
        .cloned()
        .unwrap();
    let ids: Vec<&str> = section.found.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["cc-theirs"]);
}

/// **`d` on a Claude Code conversation never deletes it** — refused, with
/// why, and no question asked; the file is untouched.
#[test]
fn d_never_deletes_a_claude_code_conversation() {
    let (projects, here) = projects_with(&[("cc-one", "Keep me")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let file = here.join("cc-one.jsonl");
    let before = std::fs::read(&file).unwrap();
    onto_first_conversation(&mut app);
    type_keys(&mut app, "d");
    assert!(app.pending_confirm.is_empty(), "nothing asked");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("never changes or deletes")),
        "{:?}",
        app.notice()
    );
    assert_eq!(std::fs::read(&file).unwrap(), before, "untouched");
}

/// **A range over everything deletes Shannon's sessions and passes Claude
/// Code's by** — and the question says so, so a range drawn over them does
/// not read as deleting them.
#[test]
fn a_range_over_everything_leaves_claude_code_conversations() {
    let (projects, here) = projects_with(&[("cc-a", "A"), ("cc-b", "B")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let session = app.conversation().episode().id;
    type_keys(&mut app, "ggVG");
    assert_eq!(app.graph_view().map(GraphView::claude_chosen), Some(2));
    type_keys(&mut app, "d");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains("2 Claude Code conversations stay"),
        "{asked}"
    );
    type_keys(&mut app, "y");
    assert!(
        !app.current_graph().episode_ids().contains(&session),
        "Shannon's went"
    );
    for id in ["cc-a", "cc-b"] {
        assert!(here.join(format!("{id}.jsonl")).exists(), "{id} stayed");
    }
}

/// **`<Enter>` imports a copy** — a Shannon session holding the prompt, in
/// the tree's window with the tree kept, as a session's `<Enter>` opens
/// (V4b Q1), and Claude Code's file unchanged.
#[test]
fn enter_imports_a_claude_code_conversation_as_a_copy() {
    let (projects, here) = projects_with(&[("cc-in", "Bring me")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let file = here.join("cc-in.jsonl");
    let before = std::fs::read(&file).unwrap();
    let tree = app.workspace.focused();
    onto_first_conversation(&mut app);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Output),
        "in its window"
    );
    assert!(keeps_a_tree(&app, tree), "the tree kept, for <C-^>");
    assert_eq!(
        branch_msgs(&app).first().map(String::as_str),
        Some("prompt of Bring me")
    );
    assert_eq!(std::fs::read(&file).unwrap(), before, "only read");
}

/// **`<Enter>` on a conversation in a tree of its own, beside a blank
/// session, opens the copy in the tree's window only** (V4b verify, M1, its
/// probe). The aimed session's blank buffer was taken over for it, "as
/// nobody could tell" — but the transcript beside the tree showed that
/// buffer and switched to the import too, where a Shannon session's `<CR>`
/// from the same tree leaves it alone.
#[test]
fn enter_on_a_conversation_beside_a_blank_session_leaves_it_alone() {
    let (projects, _) = projects_with(&[("cc-in", "Bring me")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.set_focus(PaneKind::Output);
    let transcript = app.workspace.focused();
    app.new_session_command();
    let blank = app.pane_session(transcript).id;
    assert_eq!(
        app.prompt_target(),
        blank,
        "premise: a blank session, aimed at"
    );
    app.dispatch_command_for_test("sgraph");
    app.await_claude_reads();
    let tree = app.workspace.focused();
    assert_ne!(tree, transcript, "premise: a tree of its own");
    onto_first_conversation(&mut app);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        branch_msgs(&app).first().map(String::as_str),
        Some("prompt of Bring me"),
        "the copy, aimed at"
    );
    assert_eq!(
        app.prompt_target(),
        app.pane_session(tree).id,
        "in the tree's window"
    );
    assert_eq!(
        app.pane_session(transcript).id,
        blank,
        "the blank session's pane keeps it"
    );
    assert_ne!(app.pane_session(tree).id, blank, "a buffer of its own");
}

/// **A lent tree's `<Enter>` on a conversation, over a blank session, lands
/// in that session's buffer** (V4b verify, M2 4) — beside the session
/// beneath the tree, which nobody could tell apart from the copy taking its
/// place, not beside the aim, here another pane's.
#[test]
fn enter_in_a_lent_tree_over_a_blank_session_lands_in_it() {
    let (projects, _) = projects_with(&[("cc-in", "Bring me")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("output", SplitDir::Vertical);
    let second = app.workspace.focused();
    app.new_session_command();
    let blank = app.pane_session(second).id;
    app.open_graph_command();
    app.await_claude_reads();
    assert!(
        app.window_of(second).is_some_and(Window::has_beneath),
        "premise: lent over the blank session"
    );
    app.focus_pane(first);
    app.focus_pane(second);
    assert_ne!(
        app.prompt_target(),
        blank,
        "premise: aimed at the other pane"
    );
    onto_first_conversation(&mut app);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.pane_session(second).id,
        blank,
        "the blank session's own buffer"
    );
    assert_eq!(
        branch_msgs(&app).first().map(String::as_str),
        Some("prompt of Bring me")
    );
}

/// **`<Enter>` on the Claude Code heading says it opens nothing** — it
/// closed the tree silently, and once `<Enter>` kept the tree it did nothing
/// at all, a dead key (V4b verify, LOW).
#[test]
fn enter_on_the_claude_code_heading_says_it_opens_nothing() {
    let (projects, _) = projects_with(&[("cc-in", "Bring me")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    assert_eq!(
        app.graph_view().and_then(GraphView::selected_entry),
        Some(crate::graph_view::Entry::ClaudeHeading),
        "premise: on the heading"
    );
    app.notices.clear();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("not the heading"), "{said}");
    assert_eq!(app.focus(), PaneKind::Graph, "still in the tree");
}

/// **`<Enter>` on a conversation in a LENT graph imports where the graph
/// was** (review 2026-10-16, Reviewer A F3, its probe 5). `:graph` over A's
/// pane, `<C-w>w` to B's and back: the aim followed B and not the graph, the
/// landing saw focus already in place and never re-aimed, and the import —
/// which lands at the aim — replaced B in the OTHER pane, while a session row
/// in the same spot landed here. One key, two landings.
#[test]
fn enter_in_a_lent_graph_imports_where_the_graph_was() {
    let (projects, _) = projects_with(&[("cc-in", "Bring me")]);
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    reading_claude(&mut app, projects.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    app.open_graph_command();
    app.await_claude_reads();
    assert!(
        app.window_of(left).is_some_and(Window::has_beneath),
        "premise: the graph borrowed A's pane"
    );
    app.focus_pane(right);
    app.focus_pane(left);
    assert_eq!(app.focus(), PaneKind::Graph, "premise: back in the graph");
    assert_eq!(
        app.prompt_target(),
        b_buffer,
        "premise: the aim stayed on B"
    );

    onto_first_conversation(&mut app);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let first = |app: &App, pane| {
        app.pane_session(pane)
            .conversation
            .messages()
            .first()
            .map(|m| m.content.clone())
    };
    assert_eq!(
        first(&app, left).as_deref(),
        Some("prompt of Bring me"),
        "imported where the graph was"
    );
    assert_eq!(
        app.pane_session(right).conversation.episode().id,
        b,
        "the other pane keeps its session"
    );
}

/// **Only this directory's conversations, and `a` says so** (Steven, 10-21:
/// "Only what's also in the same path. Let's keep it simple") — another
/// project's conversation never appears; `a`, which listed every
/// directory's, is retired, and says where the verb went rather than
/// reading as a dead key.
#[test]
fn only_this_directorys_conversations_and_a_says_so() {
    let (projects, _) = projects_with(&[("cc-here", "Here")]);
    let elsewhere = projects.path().join("-somewhere-else");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(
        elsewhere.join("cc-there.jsonl"),
        conversation("p", "r", "There"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let ids = |app: &App| -> Vec<String> {
        let section = app.graph_view().and_then(GraphView::claude).unwrap();
        section.found.iter().map(|f| f.id.clone()).collect()
    };
    assert_eq!(ids(&app), ["cc-here"]);
    app.notices.clear();
    type_keys(&mut app, "a");
    assert_eq!(ids(&app), ["cc-here"], "still this directory's only");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no longer lists every directory"), "{said}");
}

/// `<Space>` and `mf` on a Claude Code row say why — not a dead key.
#[test]
fn marking_a_claude_code_row_says_why() {
    let (projects, _) = projects_with(&[("cc-m", "M")]);
    let dir = TempDir::new().unwrap();
    for mark in [" ", "mf"] {
        let mut app = graph_over(projects.path(), dir.path());
        onto_first_conversation(&mut app);
        app.notices.clear();
        type_keys(&mut app, mark);
        assert!(
            app.notice().is_some_and(|n| n.contains("not marked")),
            "{mark:?}: {:?}",
            app.notice()
        );
        assert_eq!(marked(&app), Some(0));
    }
}

/// **A rebuild keeps the listing and the cursor on it** — a background
/// event must not re-read every file, nor throw the cursor back up the tree.
#[test]
fn a_rebuild_keeps_the_listing_and_the_cursor() {
    let (projects, here) = projects_with(&[("cc-r", "R")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    // Gone from disk: a re-read would drop it; a carried listing keeps it.
    std::fs::remove_file(here.join("cc-r.jsonl")).unwrap();
    app.refresh_graph_views();
    let view = app.graph_view().unwrap();
    assert_eq!(view.selected_claude().map(|f| f.id.as_str()), Some("cc-r"));
}

/// **The inspector describes a Claude Code row** — what it is, and that
/// Shannon only reads it.
#[test]
fn the_inspector_describes_a_claude_code_conversation() {
    let (projects, _) = projects_with(&[("cc-i", "Inspect me")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    let view = app.graph_view().unwrap();
    let text: String = crate::view::inspector_body(view, false, "unix")
        .iter()
        .map(|l| l.text() + "\n")
        .collect();
    for want in [
        "Claude Code conversation",
        "cc-i",
        "Inspect me",
        "does not delete",
    ] {
        assert!(text.contains(want), "{want:?} in {text}");
    }
}

/// **A range reaching into the section acts on the tree rows in it only** —
/// and a range wholly inside it is not "no range": it falls through to
/// neither the marks nor the cursor.
#[test]
fn a_range_into_the_section_acts_on_its_tree_rows_only() {
    let (projects, _) = projects_with(&[("cc-x", "X")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let view = app.graph_view_mut().unwrap();
    let tree = view.visible_len() - 2; // heading + one conversation
    view.select_row(tree - 1); // the tree's last row
    view.start_selection();
    view.select_bottom();
    assert_eq!(view.acts_on().len(), 1, "the tree's one row in the range");
    assert_eq!(view.claude_chosen(), 1);
    // Wholly inside the section, with a mark elsewhere: still not the mark.
    view.clear_selection();
    view.select_row(0);
    view.toggle_mark();
    view.select_bottom();
    view.start_selection();
    assert!(
        view.acts_on().is_empty(),
        "not the mark: {:?}",
        view.acts_on()
    );
}

/// **A conversation Shannon continues wears its own mark, in its own colour**
/// (Steven, 10-16: "indicating we've continued that conversation (so it can't
/// get lost in a fold)") — `⧓` in `ui.graph.continued` by default, where one
/// not continued keeps `◇` in the tool colour. Drawn, not asserted on the
/// role: a role mapped to a colour is its own place to be wrong.
#[test]
fn a_continued_conversation_wears_its_own_mark_and_colour() {
    use crate::style::Ink;
    let (projects, _) = projects_with(&[("cc-went-on", "Went on"), ("cc-alone", "Alone")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.import_command("cc-went-on");
    assert!(
        app.conversation().episode().inherited.is_some(),
        "premise: imported, its source recorded: {:?}",
        app.notice()
    );
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);

    let buf = frame_of(&mut app, 120, 30);
    // The style of `mark` on the first row that shows `id` and holds one.
    let mark_on = |id: &str, mark: &str| {
        (cells_of(&buf, id))
            .find_map(|at| cells_of(&buf, mark).find(|c| c.y == at.y))
            .map(|at| buf[at].style())
    };
    let went_on = mark_on("cc-went-", "⧓").expect("the continued mark on its row");
    let alone = mark_on("cc-alone", "◇").expect("the plain mark on the other");
    assert_eq!(went_on.fg, app.theme().ui(Ink::GraphContinued).fg);
    assert_eq!(alone.fg, app.theme().ui(Ink::Tool).fg);
    assert_ne!(
        went_on.fg, alone.fg,
        "told apart by colour as well as shape"
    );
}

/// **A conversation's turns are rows, read when it is opened** (Steven,
/// 10-16: "I know you can see each turn in Claude Code too, so there's no
/// reason to represent this all as a single node"). Listed, it is folded
/// and unread; `l` reads it and its turns are under it — rows of Claude
/// Code's, no nodes, so marking one says why, as on the conversation, and
/// the reading outlives a rebuild and a second `:graph`.
#[test]
fn l_reads_a_conversation_and_its_turns_are_rows() {
    let (projects, _) = projects_with(&[("cc-open", "Open me")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    // Examined for older imports before: opening `:graph` reads nothing.
    app.claude_sessions.check("cc-open").unwrap();
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    assert!(
        !app.graph_view()
            .unwrap()
            .claude()
            .unwrap()
            .read
            .contains_key("cc-open"),
        "premise: unread"
    );
    assert!(
        !screen(&mut app, 120).contains("prompt of Open me"),
        "premise: folded"
    );

    type_keys(&mut app, "l");
    app.await_claude_reads();
    assert!(
        screen(&mut app, 120).contains("prompt of Open me"),
        "its turn"
    );
    type_keys(&mut app, "j");
    let view = app.graph_view().unwrap();
    assert_eq!(
        view.selected_node(),
        None,
        "a turn of Claude Code's: no node"
    );
    assert_eq!(
        view.selected_claude().map(|f| f.id.as_str()),
        Some("cc-open"),
        "its conversation, for <Enter>"
    );
    app.notices.clear();
    type_keys(&mut app, " ");
    assert!(
        app.notice().is_some_and(|n| n.contains("not marked")),
        "{:?}",
        app.notice()
    );

    // Off the conversation, so only its fold — not the cursor's reveal —
    // can keep it open through a rebuild.
    app.graph_view_mut().unwrap().select_top();
    app.refresh_graph_views();
    assert!(
        app.graph_view().unwrap().selected_claude().is_none(),
        "premise: the cursor is off it"
    );
    assert!(
        screen(&mut app, 120).contains("prompt of Open me"),
        "a rebuild keeps it open"
    );
    // A second `:graph`, and `l` again: from what was kept, not the file —
    // the very reading, unchanged since.
    let kept = |app: &App| {
        let kept = app.claude_reads.kept();
        assert_eq!(kept.len(), 1, "one file, kept");
        std::sync::Arc::clone(&kept[0])
    };
    let first = kept(&app);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    assert!(
        screen(&mut app, 120).contains("prompt of Open me"),
        "open again"
    );
    assert!(
        std::sync::Arc::ptr_eq(&first, &kept(&app)),
        "read once, kept since"
    );
}

/// **A conversation written since it was read is read again** — the reading
/// is kept only while the file is the time and size its listing says. While
/// it is read again the reading kept is shown, the row saying so; then the
/// new one. (An assertion here that a new `:graph` offered nothing stale held
/// whatever the code did — a listing reads only what a session continues —
/// review stop 4, C M3.)
#[test]
fn a_conversation_written_since_it_was_read_is_read_again() {
    let (projects, here) = projects_with(&[("cc-grow", "Grows")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let file = here.join("cc-grow.jsonl");
    let mut text = std::fs::read_to_string(&file).unwrap();
    for line in [
        serde_json::json!({
            "type": "user", "uuid": "u2", "parentUuid": "a1", "isSidechain": false,
            "timestamp": "03", "cwd": "/work",
            "message": {"role": "user", "content": "and a second question"}
        }),
        serde_json::json!({
            "type": "assistant", "uuid": "a2", "parentUuid": "u2", "isSidechain": false,
            "timestamp": "04", "cwd": "/work",
            "message": {"id": "M2", "model": "claude-x", "role": "assistant",
                        "content": [{"type": "text", "text": "a second reply"}]}
        }),
    ] {
        text.push_str(&format!("{line}\n"));
    }
    std::fs::write(&file, text).unwrap();

    app.open_graph_command();
    app.await_claude_reads();
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    let meanwhile = screen(&mut app, 120);
    assert!(
        meanwhile.contains("prompt of Grows"),
        "as kept: {meanwhile}"
    );
    assert!(meanwhile.contains("reading…"), "and said: {meanwhile}");
    assert!(!meanwhile.contains("and a second question"), "{meanwhile}");
    app.await_claude_reads();
    assert!(
        screen(&mut app, 120).contains("and a second question"),
        "read again"
    );
}

/// **A session continued from Claude Code hangs under the conversation, at
/// the turn it parted at** — a session of Steven's (10-16, image 48):
/// imported from a Claude Code conversation, continued, and drawn among
/// Shannon's roots, "not connected to the … root". Now below the heading, under the
/// conversation's turn, the imported turn drawn once (the conversation's),
/// the session's own turn under it, its row saying what it continues — and
/// `:graph` landing on that turn, where you are.
///
/// **The conversation's turns closed until `l`** (Steven, 10-18: 900 turns
/// opened at once "shows too much all at once"): the session under its row
/// meanwhile, none of its turns drawn; `l` opens them, and the session is at
/// the turn it parted at.
#[tokio::test]
async fn a_continued_session_hangs_under_its_conversation() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    app.output_mut().begin_turn("Continuing the test, say bye");
    app.conversation_mut()
        .begin_turn("Continuing the test, say bye");
    app.feed_stream(StreamMsg::Chunk("Bye.".into()));
    app.feed_stream(StreamMsg::Complete);
    let continued = app.conversation().episode().id;
    assert!(
        app.conversation().episode().inherited.is_some(),
        "premise: an import, its source recorded"
    );
    // Examined for older imports before: nothing but the section's own
    // reading of what a session continues can place it.
    app.claude_sessions.check("cc-hello").unwrap();
    app.set_focus(PaneKind::Output);
    // A frame, as the loop draws one between the reply and the key: the
    // window riding its end takes the reply's lines, its cursor with them.
    frame_of(&mut app, 120, 40);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    let view = app.graph_view().unwrap();
    assert!(
        matches!(view.selected_node(), Some(Node::Turn { episode, .. }) if episode == continued),
        "landed on its own turn: {:?}",
        view.selected_node()
    );

    let closed = screen(&mut app, 120);
    assert!(
        closed.contains("continues cc-hello") && closed.contains("Continuing the test"),
        "the session, in sight: {closed}"
    );
    assert!(
        !closed.contains("prompt of Test greeting"),
        "none of its turns: {closed}"
    );
    let its_row = |shown: &str| {
        shown
            .lines()
            .find(|l| l.contains("Test greeting") && !l.contains("prompt of"))
            .map(str::to_string)
            .unwrap_or_else(|| panic!("its row in {shown}"))
    };
    assert!(its_row(&closed).contains('⋯'), "it holds more: {closed}");

    onto_row(&mut app, |e| matches!(e, Some(Entry::Conversation(_))));
    type_keys(&mut app, "l");
    // Kept open through a refresh, as every fold the user sets is: one runs
    // on every background event, into a fresh view.
    app.refresh_graph_views();
    let shown = screen(&mut app, 120);
    assert!(!its_row(&shown).contains('⋯'), "all shown: {shown}");
    let line_of = |words: &str| {
        shown
            .lines()
            .position(|l| l.contains(words))
            .unwrap_or_else(|| panic!("{words:?} in {shown}"))
    };
    let heading = line_of("── Claude Code");
    let parted = line_of("prompt of Test greeting");
    // Its decoration, not its id: the inspector beside names it too.
    let session = line_of("continues cc-hello");
    let own = line_of("Continuing the test");
    assert!(
        heading < parted && parted < session && session < own,
        "{shown}"
    );
    assert!(
        shown
            .lines()
            .nth(session)
            .unwrap()
            .contains(&continued.short_hex()),
        "{shown}"
    );
    assert_eq!(
        shown.matches("prompt of Test greeting").count(),
        1,
        "the imported turn drawn once, the conversation's: {shown}"
    );
}

/// **`:graph` opens on the turn you were reading in a continued import
/// too** — the first of its own two, not its last: the tree it lands in
/// is rebuilt with Claude Code's section, where the session hangs, and the
/// turn read is kept through it (review stop 15).
#[tokio::test]
async fn a_continued_session_opens_the_tree_on_the_turn_read() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    for (ask, reply) in [("Continuing, say bye", "Bye."), ("And again", "Again.")] {
        app.output_mut().begin_turn(ask);
        app.conversation_mut().begin_turn(ask);
        app.feed_stream(StreamMsg::Chunk(reply.into()));
        app.feed_stream(StreamMsg::Complete);
    }
    let continued = app.conversation().episode().id;
    app.claude_sessions.check("cc-hello").unwrap();
    app.set_focus(PaneKind::Output);
    let frame = Rect::new(0, 0, 120, 40);
    frame_of(&mut app, frame.width, frame.height);
    let pane = app.workspace.focused();
    let line = super::position_marks::line_reading(&app, pane, "Bye.");
    type_keys_in(&mut app, frame, &format!("{}G", line + 1));
    let (_, read) = app.turn_read().expect("premise: a turn read");
    app.open_graph_command();
    app.await_claude_reads();
    let selected = app.graph_view().and_then(|v| v.selected_node());
    assert!(
        matches!(selected, Some(Node::Turn { episode, index, .. })
            if episode == continued && index == read),
        "turn {read}: {selected:?}"
    );
}

/// What `:graph` draws, split at the inspector's border: the graph pane's
/// text and the inspector's, each on its own — a word in one must not count
/// for the other.
fn panes_drawn(app: &mut App) -> (String, String) {
    let shown = screen(app, 120);
    let first = shown.lines().next().unwrap_or_default();
    let border = first
        .find("┌ inspect")
        .map(|byte| first[..byte].chars().count())
        .expect("an inspector beside the graph");
    let (mut graph, mut inspector) = (String::new(), String::new());
    for line in shown.lines() {
        graph.extend(line.chars().take(border).chain(['\n']));
        inspector.extend(line.chars().skip(border).chain(['\n']));
    }
    (graph, inspector)
}

/// **A Claude Code turn's row and its inspector page show it scrubbed** — as
/// an import scrubs its words on the way in: they are Claude Code's file as
/// its model saw them, and a row or a page showing them as they are would be
/// the way round the import's guard. The prompt's key, the reply's, and a
/// tool result's — the last drawn nowhere yet, so held to on the reading the
/// view keeps (review stop 3, B 2: the reply's scrub could be deleted with
/// every test green). Each pane on its own (C 7).
#[test]
fn the_inspector_on_a_claude_code_turn_shows_it_scrubbed() {
    let [in_prompt, in_reply, in_result] = [
        "sk-ant-api03-abcdefghijklmnop",
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        "AKIAIOSFODNN7EXAMPLE",
    ];
    let (projects, here) = projects_with(&[("cc-said", "Said things")]);
    let entry = claude_entry;
    let lines = [
        entry(
            "user",
            "1",
            None,
            serde_json::json!({"role": "user", "content": format!("the key is {in_prompt}")}),
        ),
        entry(
            "assistant",
            "2",
            Some("1"),
            serde_json::json!({"id": "M1", "model": "claude-x", "role": "assistant",
                "content": [{"type": "tool_use", "id": "T1", "name": "Bash",
                             "input": {"command": "cat notes"}}]}),
        ),
        entry(
            "user",
            "3",
            Some("2"),
            serde_json::json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "T1", "content": format!("aws {in_result}")}
            ]}),
        ),
        entry(
            "assistant",
            "4",
            Some("3"),
            serde_json::json!({"id": "M2", "model": "claude-x", "role": "assistant",
                "content": [{"type": "text", "text": format!("Noted; yours is {in_reply}")}]}),
        ),
    ];
    std::fs::write(here.join("cc-said.jsonl"), lines.concat()).unwrap();
    // Both ways a reading enters a view: read by `l`, and read as `:graph`
    // opens because a session continues it.
    for continued in [false, true] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 1);
        reading_claude(&mut app, projects.path());
        if continued {
            app.import_command("cc-said");
        }
        app.set_focus(PaneKind::Output);
        app.open_graph_command();
        app.await_claude_reads();
        app.workspace.focus_kind(PaneKind::Graph);
        onto_first_conversation(&mut app);
        let unread = app.graph_view().unwrap().unread_selected().is_some();
        assert_eq!(unread, !continued, "premise: which door it comes by");
        // Read either way; its turns shown only once opened.
        type_keys(&mut app, "lj");
        assert!(
            app.graph_view().unwrap().selected_claude_turn().is_some(),
            "continued={continued}: premise: on its turn"
        );
        let (graph, inspector) = panes_drawn(&mut app);
        assert!(graph.contains("the key is"), "premise: its row: {graph}");
        for want in [
            "Claude Code turn",
            "Prompt",
            "the key is",
            "Noted; yours is",
        ] {
            assert!(inspector.contains(want), "{want:?} in {inspector}");
        }
        for value in [in_prompt, in_reply, in_result] {
            assert!(
                !graph.contains(value),
                "continued={continued}: the row: {graph}"
            );
            assert!(
                !inspector.contains(value),
                "continued={continued}: the page: {inspector}"
            );
        }
        let held = format!("{:?}", app.graph_view().unwrap().claude().unwrap().read);
        for value in [in_prompt, in_reply, in_result] {
            assert!(
                !held.contains(value),
                "continued={continued}: what the view holds"
            );
        }
    }
}

/// **A turn read again is painted again** — the inspector's page is keyed by
/// the turn's words, not its place: a conversation still being written,
/// read again, has new words at the same place, and the page went on showing
/// the old (review stop 3, A F6, B 3, C 2 — and with them a value since
/// marked secret).
#[test]
fn a_turn_read_again_is_painted_again() {
    let (projects, here) = projects_with(&[("cc-live", "Live")]);
    let file = here.join("cc-live.jsonl");
    std::fs::write(&file, conversation("a question", "FIRST REPLY", "Live")).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "lj");
    let (_, page) = panes_drawn(&mut app);
    assert!(page.contains("FIRST REPLY"), "premise: {page}");

    std::fs::write(
        &file,
        conversation("a question", "SECOND REPLY, longer", "Live"),
    )
    .unwrap();
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    // Read again off the UI thread: meanwhile the reading kept is shown, the
    // row saying so — then the new words, on the page left open.
    type_keys(&mut app, "lj");
    let (graph, page) = panes_drawn(&mut app);
    assert!(page.contains("FIRST REPLY"), "meanwhile, as kept: {page}");
    assert!(graph.contains("reading…"), "and said: {graph}");
    app.await_claude_reads();
    let (graph, page) = panes_drawn(&mut app);
    assert!(
        page.contains("SECOND REPLY") && !page.contains("FIRST REPLY"),
        "{page}"
    );
    assert!(!graph.contains("reading…"), "read: {graph}");
}

/// **A conversation not yet opened says it holds more** — its row trails
/// the mark a folded session's does: `l` opening it was a key nothing on
/// screen offered (review stop 3, A F10, C 9). Opened, the mark goes.
#[test]
fn an_unread_conversation_says_it_holds_more() {
    let (projects, _) = projects_with(&[("cc-more", "Holds more")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let row = |graph: &str| {
        graph
            .lines()
            .find(|l| l.contains("Holds more"))
            .unwrap_or_else(|| panic!("its row: {graph}"))
            .to_string()
    };
    let (graph, _) = panes_drawn(&mut app);
    assert!(row(&graph).contains('⋯'), "{graph}");
    // The match for older imports read it — and the view, which did not ask,
    // was not handed it: what a search finds would turn on what happened to
    // be read.
    assert_eq!(app.claude_reads.kept().len(), 1, "premise: read, kept");
    let section = app.graph_view().unwrap().claude().unwrap();
    assert!(!section.read.contains_key("cc-more"), "not pressed on it");
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let (graph, _) = panes_drawn(&mut app);
    assert!(!row(&graph).contains('⋯'), "opened: {graph}");
}

/// **An import made with `:graph` open in another pane hangs in sight** —
/// the refresh reads the conversation a session has come to continue, and a
/// fold nobody set follows its default: the conversation, closed while
/// nothing continued it, opens to what hangs from it (review stop 3, A F5:
/// the session vanished into the fold) — its turns closed (Steven, 10-18).
#[tokio::test]
async fn an_import_made_with_graph_open_elsewhere_hangs_in_sight() {
    let (projects, _) = projects_with(&[("cc-late", "Late")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.claude_sessions.check("cc-late").unwrap();
    app.split_command("output", SplitDir::Vertical);
    let graph_pane = app.workspace.focused();
    app.open_graph_command();
    app.await_claude_reads();
    let other = app
        .session_panes(Over::ThisTab)
        .find(|&p| p != graph_pane)
        .expect("the other transcript");
    app.workspace.focus_pane(other);
    app.focus_moved();
    app.import_command("cc-late");
    let imported = app.conversation().episode().id;
    assert!(
        app.conversation().episode().inherited.is_some(),
        "premise: continued"
    );
    app.refresh_graph_views();
    app.await_claude_reads();

    let view = app.pane_graph(graph_pane).expect("the graph, still open");
    let rows = view.tree_rows(0, view.visible_len());
    let at = |entry: Entry| rows.iter().position(|r| r.entry == entry);
    let conversation = at(Entry::Conversation(0)).expect("its conversation");
    let session = at(Entry::Node(Node::Episode(imported))).expect("the session in sight");
    assert!(conversation < session, "under its conversation");
    assert_eq!(rows[session].depth, 1, "from its row, its turns closed");
}

/// **A conversation's title is scrubbed where it is drawn** — Claude Code's
/// title can be the user's last prompt, word for word (`lastPrompt`), and it
/// is drawn on the row, on the conversation's page and the turn's, and
/// searched (review stop 3, B 1 and C 1).
#[test]
fn a_conversations_title_is_scrubbed_where_it_is_drawn() {
    let value = "sk-ant-api03-abcdefghijklmnop";
    let (projects, here) = projects_with(&[("cc-titled", "Titled")]);
    std::fs::write(
        here.join("cc-titled.jsonl"),
        conversation("hello", "hi", &format!("deploy with {value}")),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    let (graph, inspector) = panes_drawn(&mut app);
    assert!(graph.contains("deploy with"), "premise: the title: {graph}");
    assert!(
        inspector.contains("deploy with"),
        "premise: its page: {inspector}"
    );
    assert!(!graph.contains(value) && !inspector.contains(value));
    type_keys(&mut app, "lj");
    let (_, page) = panes_drawn(&mut app);
    assert!(
        page.contains("deploy with") && !page.contains(value),
        "{page}"
    );
    let look = crate::view::GraphLook::of(&app.config.tui);
    let searched =
        crate::view::graph_row_texts(app.graph_view().unwrap(), look, app.theme()).join("\n");
    assert!(searched.contains("deploy with") && !searched.contains(value));
}

/// **…scrubbed whole, then cut** — a credential across the title's cut kept
/// its start, `sk-ant-`, which no matcher knows for one (review stop 4, E 4).
#[test]
fn a_title_is_scrubbed_before_it_is_cut() {
    let value = "sk-ant-api03-abcdefghijklmnop";
    let title = format!("{} {value}", "x".repeat(72));
    let (projects, here) = projects_with(&[("cc-titled", "Titled")]);
    std::fs::write(
        here.join("cc-titled.jsonl"),
        conversation("hello", "hi", &title),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let app = graph_over(projects.path(), dir.path());
    let look = crate::view::GraphLook::of(&app.config.tui);
    let searched =
        crate::view::graph_row_texts(app.graph_view().unwrap(), look, app.theme()).join("\n");
    assert!(
        searched.contains(&"x".repeat(72)),
        "premise: the title: {searched}"
    );
    assert!(!searched.contains("sk-ant"), "{searched}");
}

/// **Another Claude Code turn is another page** — the inspector starts it at
/// the top, as it does a Shannon node's. Every Claude Code row is no node, so
/// by the node one turn's page was the next one's, and a reader scrolled
/// down the first stayed scrolled down the second.
#[test]
fn another_claude_code_turn_is_a_new_inspector_page() {
    use ratatui::{Terminal, backend::TestBackend};
    let (projects, here) = projects_with(&[("cc-long", "Long")]);
    let long = |what: &str| {
        (0..60)
            .map(|i| format!("{what} line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let entry = claude_entry;
    let reply = |id: &str| {
        serde_json::json!({"id": id, "model": "claude-x", "role": "assistant",
                           "content": [{"type": "text", "text": "ok"}]})
    };
    let lines = [
        entry(
            "user",
            "1",
            None,
            serde_json::json!({"role": "user", "content": long("first")}),
        ),
        entry("assistant", "2", Some("1"), reply("M1")),
        entry(
            "user",
            "3",
            Some("2"),
            serde_json::json!({"role": "user", "content": long("second")}),
        ),
        entry("assistant", "4", Some("3"), reply("M2")),
    ];
    std::fs::write(here.join("cc-long.jsonl"), lines.concat()).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "lj");
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let inspector = app
        .workspace
        .pane_ids()
        .into_iter()
        .find(|&p| app.workspace.kind_of(p) == Some(PaneKind::Inspector))
        .expect("an inspector");
    app.workspace.focus_pane(inspector);
    app.handle_text_pane_key_for_test(
        PaneKind::Inspector,
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        (80, 24),
    );
    draw_frame(&mut app, &mut terminal);
    assert!(
        app.text_pane_pos_for_test(inspector).unwrap().0 > 0,
        "premise: the reader is down the first turn's page"
    );

    app.workspace.focus_kind(PaneKind::Graph);
    type_keys(&mut app, "j");
    draw_frame(&mut app, &mut terminal);
    assert_eq!(
        app.text_pane_pos_for_test(inspector).map(|p| p.0),
        Some(0),
        "the next turn's page, from its top"
    );
}

/// **A conversation Shannon started stays listed when a session continues
/// it** (Steven, 10-16, decision 4: Steven's `5f6e7d8c`, a transcript
/// Shannon ran, then imported and continued) — it is that session's root;
/// hidden, the session would hang under nothing.
#[test]
fn a_conversation_shannon_started_and_a_session_continues_is_listed() {
    let (projects, _) = projects_with(&[("cc-ours", "Ours, continued")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.claude_sessions.note("cc-ours").unwrap();
    app.import_command("cc-ours");
    assert!(
        app.conversation().episode().inherited.is_some(),
        "premise: continued"
    );
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    let section = app.graph_view().unwrap().claude().unwrap();
    assert!(
        section.found.iter().any(|f| f.id == "cc-ours"),
        "listed, Shannon's though it is"
    );
}

/// Write `<id>.jsonl` in `here`: a turn, a compaction, its summary straight
/// followed by a reply (an auto-compaction mid-task: no prompt between).
fn write_compacted(here: &std::path::Path, id: &str) {
    let lines = [
        serde_json::json!({
            "type": "user", "uuid": "u1", "parentUuid": null, "isSidechain": false,
            "timestamp": "01", "cwd": "/work",
            "message": {"role": "user", "content": "before the compaction"}
        }),
        serde_json::json!({
            "type": "assistant", "uuid": "a1", "parentUuid": "u1", "isSidechain": false,
            "timestamp": "02", "cwd": "/work",
            "message": {"id": "M1", "model": "claude-x", "role": "assistant",
                        "content": [{"type": "text", "text": "a reply"}]}
        }),
        serde_json::json!({
            "type": "system", "subtype": "compact_boundary", "uuid": "b1",
            "parentUuid": null, "logicalParentUuid": "a1", "isSidechain": false,
            "timestamp": "03", "cwd": "/work"
        }),
        serde_json::json!({
            "type": "user", "uuid": "s1", "parentUuid": "b1", "isSidechain": false,
            "isCompactSummary": true, "timestamp": "04", "cwd": "/work",
            "message": {"role": "user", "content": "the summary of what came before"}
        }),
        serde_json::json!({
            "type": "assistant", "uuid": "a2", "parentUuid": "s1", "isSidechain": false,
            "timestamp": "05", "cwd": "/work",
            "message": {"id": "M2", "model": "claude-x", "role": "assistant",
                        "content": [{"type": "text", "text": "after"}]}
        }),
    ];
    std::fs::write(
        here.join(format!("{id}.jsonl")),
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
}

/// **Where Claude Code compacted, its turn wears `⊚`** — the summary is the
/// turn that went on (`Scope::Lineage` keeps it), marked as a Shannon
/// summary is.
#[test]
fn a_compaction_in_a_conversation_wears_its_mark() {
    let (projects, here) = projects_with(&[("cc-comp", "Compacted")]);
    write_compacted(&here, "cc-comp");
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let shown = screen(&mut app, 120);
    let row = |words: &str| {
        shown
            .lines()
            .find(|l| l.contains(words))
            .unwrap_or_else(|| panic!("{words:?} in {shown}"))
            .to_string()
    };
    assert!(row("the summary of what").contains('⊚'), "{shown}");
    assert!(!row("before the compaction").contains('⊚'), "{shown}");
}

/// **The `tree` style draws a Claude Code turn as the `git` style does** —
/// its `⊚` where Claude Code compacted, and its prompt's first words though
/// a blank line opens it (review stop 3, C 6: the tree style was never drawn
/// for one, and its snippet read the blank line, `""`).
#[test]
fn the_tree_style_draws_a_claude_code_turn() {
    let (projects, here) = projects_with(&[("cc-comp", "Compacted"), ("cc-blank", "Blank")]);
    write_compacted(&here, "cc-comp");
    std::fs::write(
        here.join("cc-blank.jsonl"),
        conversation("\n\nwords after a blank line", "ok", "Blank"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    std::sync::Arc::make_mut(&mut app.config).tui.graph_style =
        shannon_core::config::GraphStyle::Tree;
    reading_claude(&mut app, projects.path());
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    for id in ["cc-comp", "cc-blank"] {
        assert!(app.graph_view_mut().unwrap().select_claude(id), "premise");
        type_keys(&mut app, "l");
        app.await_claude_reads();
    }
    let (graph, _) = panes_drawn(&mut app);
    let row = |words: &str| {
        graph
            .lines()
            .find(|l| l.contains(words))
            .unwrap_or_else(|| panic!("{words:?} in {graph}"))
            .to_string()
    };
    assert!(row("the summary of what").contains('⊚'), "{graph}");
    assert!(!row("before the compaction").contains('⊚'), "{graph}");
    assert!(
        row("words after a blank line").contains("\"words after"),
        "{graph}"
    );
}

/// **A range of a conversation's turns is that conversation, once; marked,
/// it says why nothing is** — `claude_chosen` counted conversation rows
/// only, so `d`'s question passed turns over unsaid, and `mf` over a range
/// of Claude Code's rows alone marked nothing and said nothing (review stop
/// 3, A F9, C 10).
#[test]
fn a_range_of_a_conversations_turns_counts_it_once_and_marks_nothing() {
    let (projects, here) = projects_with(&[("cc-two", "Two turns")]);
    write_compacted(&here, "cc-two");
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "lj");
    let view = app.graph_view_mut().unwrap();
    view.start_selection();
    view.select_down();
    assert!(
        view.selected_claude_turn().is_some(),
        "premise: on its second turn"
    );
    assert_eq!(
        view.claude_chosen(),
        1,
        "one conversation, two of its turns"
    );

    app.notices.clear();
    type_keys(&mut app, "mf");
    assert!(
        app.notice().is_some_and(|n| n.contains("not marked")),
        "{:?}",
        app.notice()
    );
    assert_eq!(marked(&app), Some(0));
}

/// **`:import --all` records where it parted only when it can say so turn
/// for turn** — `--all` drops each compaction's summary and joins the reply
/// after one into the turn before, so its turns are not the run `:graph`
/// reads: placed by them, the session hung before the compaction it holds
/// (review stop 3, A F3, C 3). Over a compaction no place is recorded and
/// the session hangs from the conversation's row; with none in what it
/// read, it is placed as a resume-path import is.
#[test]
fn an_import_of_everything_is_placed_only_where_it_can_be() {
    for (compacted, placed) in [(true, false), (false, true)] {
        let (projects, here) = projects_with(&[("cc-all", "All of it")]);
        if compacted {
            write_compacted(&here, "cc-all");
        }
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 1);
        reading_claude(&mut app, projects.path());
        app.import_command("--all cc-all");
        let inherited = app.conversation().episode().inherited.clone();
        let inherited = inherited.expect("premise: imported");
        assert_eq!(
            inherited.at.is_some(),
            placed,
            "compacted={compacted}: {inherited:?}"
        );
        // …and stays so: the match for older imports read "no place" as "an
        // import from before the record" and placed it before the compaction
        // it holds, its later turns drawn as its own (review stop 4, A 2).
        let id = app.conversation().episode().id;
        app.open_graph_command();
        app.await_claude_reads();
        let stored = app.store.load_episode(&id).unwrap().episode.inherited;
        assert_eq!(
            stored,
            Some(inherited),
            "compacted={compacted}: as recorded"
        );
    }
}

/// **An import placed nowhere is not placed by the match for older
/// imports** — `--all` past a compaction records no turn on purpose, and the
/// match read the missing turn as "an import from before the record": two
/// turns matched, then the summary's, so it was placed at the second, BEFORE
/// the compaction it holds, its later turns drawn as its own (review stop 4,
/// A 2: `{turns: 3, at: None}` became `{turns: 2, at: "u2"}`).
#[test]
fn an_import_placed_nowhere_is_not_placed_by_the_match() {
    let (projects, here) = projects_with(&[("cc-twice", "Twice")]);
    let entry = |kind: &str, uuid: &str, parent: Option<&str>, more: serde_json::Value| {
        let mut line = serde_json::json!({"type": kind, "uuid": uuid, "parentUuid": parent,
            "isSidechain": false, "timestamp": uuid, "cwd": "/work"});
        line.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        format!("{line}\n")
    };
    let said = |text: &str| serde_json::json!({"message": {"role": "user", "content": text}});
    let reply = |id: &str, text: &str| {
        serde_json::json!({"message": {"id": id, "model": "claude-x", "role": "assistant",
            "content": [{"type": "text", "text": text}]}})
    };
    let text = [
        entry("user", "u1", None, said("one")),
        entry("assistant", "a1", Some("u1"), reply("M1", "first")),
        entry("user", "u2", Some("a1"), said("two")),
        entry("assistant", "a2", Some("u2"), reply("M2", "second")),
        entry(
            "system",
            "b1",
            None,
            serde_json::json!({"subtype": "compact_boundary", "logicalParentUuid": "a2"}),
        ),
        entry(
            "user",
            "s1",
            Some("b1"),
            serde_json::json!({"isCompactSummary": true,
                "message": {"role": "user", "content": "the summary"}}),
        ),
        entry("user", "u3", Some("s1"), said("three")),
        entry("assistant", "a3", Some("u3"), reply("M3", "third")),
    ]
    .concat();
    std::fs::write(here.join("cc-twice.jsonl"), text).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.import_command("--all cc-twice");
    let id = app.conversation().episode().id;
    let recorded = app.conversation().episode().inherited.clone();
    assert_eq!(
        recorded.as_ref().map(|i| (i.turns, i.at.clone())),
        Some((3, None)),
        "premise: placed nowhere"
    );
    app.open_graph_command();
    app.await_claude_reads();
    let stored = app.store.load_episode(&id).unwrap().episode.inherited;
    assert_eq!(stored, recorded, "as recorded");
}

/// The cursor onto the first row, from the top, that `is` says yes to.
fn onto_row(app: &mut App, is: impl Fn(Option<Entry>) -> bool) {
    let view = app.graph_view_mut().unwrap();
    view.select_top();
    for _ in 0..view.visible_len() {
        if is(view.selected_entry()) {
            return;
        }
        view.select_down();
    }
    panic!("no such row");
}

/// **`d` under a conversation acts on the row it is on** — Steven, 10-17
/// (images 59–61): "So I think this is a off-by-one error? Or perhaps the
/// linkage is off-by-one?" Probed: it is not. The session's own turn asks
/// as its turn 2 of 2 — its turn 1 is its copy of Claude Code's, drawn
/// once, as Claude Code's row above it — and goes alone, the session still
/// placed where it parted; Claude Code's turn is refused and its file left
/// as it was; the session's row asks for the session.
#[tokio::test]
async fn d_under_a_conversation_acts_on_the_row_it_is_on() {
    let (projects, here) = projects_with(&[("cc-hello", "Test greeting")]);
    let file = here.join("cc-hello.jsonl");
    let before = std::fs::read(&file).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let elsewhere = app.conversation().episode().id;
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    app.output_mut().begin_turn("Continuing the test, say bye");
    app.conversation_mut()
        .begin_turn("Continuing the test, say bye");
    app.feed_stream(StreamMsg::Chunk("Bye.".into()));
    app.feed_stream(StreamMsg::Complete);
    let continued = app.conversation().episode().id;
    let [imported, own] = app.current_graph().full_turns(&continued)[..] else {
        panic!("premise: the imported turn, then its own");
    };
    app.claude_sessions.check("cc-hello").unwrap();
    // Standing in another session, as in the images.
    app.switch_session(elsewhere);
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);

    onto_row(
        &mut app,
        |e| matches!(e, Some(Entry::Node(Node::Turn { episode, .. })) if episode == continued),
    );
    type_keys(&mut app, "d");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains(&format!("turn {} (2/2)", own.short_hex())),
        "{asked}"
    );
    type_keys(&mut app, "y");
    let graph = app.current_graph();
    assert_eq!(
        graph.full_turns(&continued),
        [imported],
        "its own went alone"
    );
    let inherited = graph.episode(&continued).and_then(|e| e.inherited.clone());
    assert!(
        inherited.is_some_and(|i| i.turns == 1 && i.at.is_some()),
        "still placed where it parted"
    );
    // **The cursor lands on the row now in its place**, as vim's `dd`
    // (Steven, 10-17) — nothing follows the turn, so the one above: the
    // session's row. It jumped to the session you stand in (image 61).
    let at = |app: &mut App| app.graph_view_mut().unwrap().selected_entry();
    assert_eq!(at(&mut app), Some(Entry::Node(Node::Episode(continued))));

    onto_row(&mut app, |e| matches!(e, Some(Entry::Conversation(_))));
    type_keys(&mut app, "l"); // its turns
    onto_row(&mut app, |e| matches!(e, Some(Entry::ClaudeTurn { .. })));
    app.notices.clear();
    type_keys(&mut app, "d");
    assert!(app.pending_confirm.is_empty(), "nothing asked");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("never changes or deletes")),
        "{:?}",
        app.notice()
    );

    onto_row(&mut app, |e| {
        e == Some(Entry::Node(Node::Episode(continued)))
    });
    type_keys(&mut app, "d");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains(&continued.short_hex()) && !asked.contains("turn"),
        "the session, not a turn of it: {asked}"
    );
    type_keys(&mut app, "y");
    assert!(
        !app.current_graph().episode_ids().contains(&continued),
        "the session went"
    );
    assert_eq!(
        at(&mut app),
        Some(Entry::Conversation(0)),
        "nothing continues it now, so it closes over the place — the cursor on it"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before,
        "Claude Code's, as it was"
    );
}

/// **`<Enter>` on a turn imports up to it** (Steven, 10-17, A F8: "it should
/// import up to that turn") — a copy of the conversation as it stood there,
/// placed at that turn, the note saying which; Claude Code's file only read.
#[test]
fn enter_on_a_claude_code_turn_imports_up_to_it() {
    let (projects, here) = projects_with(&[("cc-three", "Three turns")]);
    let file = here.join("cc-three.jsonl");
    append_turns(&file, &[(2, "a1"), (3, "a2")]);
    let before = std::fs::read(&file).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let sessions = app.current_graph().episode_ids().len();
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    onto_row(&mut app, |e| {
        matches!(e, Some(Entry::ClaudeTurn { turn: 1, .. }))
    });
    app.notices.clear();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app.current_graph().episode_ids().len(),
        sessions + 1,
        "a copy"
    );
    assert_eq!(
        branch_msgs(&app),
        ["prompt of Three turns", "a reply", "question 2", "answer 2"],
        "up to the second turn, not the third"
    );
    let inherited = app.conversation().episode().inherited.clone();
    assert!(
        inherited.is_some_and(|i| i.turns == 2 && i.at.as_deref() == Some("u2")),
        "placed at the turn it was opened on"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("up to its turn 2 of 3")),
        "{:?}",
        app.notice()
    );
    assert_eq!(std::fs::read(&file).unwrap(), before, "only read");
}

/// Turn `n` of a Claude Code conversation — `question n`, `answer n`, entries
/// `u{n}` and `a{n}` — appended to `file` under `parent`, for each.
fn append_turns(file: &std::path::Path, turns: &[(usize, &str)]) {
    let mut text = std::fs::read_to_string(file).unwrap();
    for &(n, parent) in turns {
        for line in [
            serde_json::json!({
                "type": "user", "uuid": format!("u{n}"), "parentUuid": parent,
                "isSidechain": false, "timestamp": format!("0{n}a"), "cwd": "/work",
                "message": {"role": "user", "content": format!("question {n}")}
            }),
            serde_json::json!({
                "type": "assistant", "uuid": format!("a{n}"), "parentUuid": format!("u{n}"),
                "isSidechain": false, "timestamp": format!("0{n}b"), "cwd": "/work",
                "message": {"id": format!("M{n}"), "model": "claude-x", "role": "assistant",
                            "content": [{"type": "text", "text": format!("answer {n}")}]}
            }),
        ] {
            text.push_str(&format!("{line}\n"));
        }
    }
    std::fs::write(file, text).unwrap();
}

/// **`<Enter>` on the last turn shown imports what was shown** — the file
/// gone on since (Claude Code still running it): the copy took every turn
/// after it and said nothing of them, where any other turn's stops at it
/// (review stop 4, A 3: one verb, two behaviours, chosen by whether the
/// reading was current).
#[test]
fn enter_on_the_last_turn_shown_imports_what_was_shown() {
    let (projects, here) = projects_with(&[("cc-three", "Three turns")]);
    let file = here.join("cc-three.jsonl");
    append_turns(&file, &[(2, "a1")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    onto_row(&mut app, |e| {
        matches!(e, Some(Entry::ClaudeTurn { turn: 1, .. }))
    });
    append_turns(&file, &[(3, "a2")]);
    app.notices.clear();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        branch_msgs(&app),
        ["prompt of Three turns", "a reply", "question 2", "answer 2"],
        "what was shown, not the turn since"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("up to its turn 2 of 2")),
        "{:?}",
        app.notice()
    );
}

/// **`<Enter>` on a conversation a session goes on with opens that session**
/// (Steven, 10-17, A F8: "open the continuing session") — it made another
/// copy, a second session where the user meant the one hanging there. Of
/// several, the most recently active — not the newest made — and it says
/// there are others.
#[tokio::test]
async fn enter_on_a_continued_conversation_opens_the_session_going_on_with_it() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let elsewhere = app.conversation().episode().id;
    reading_claude(&mut app, projects.path());
    let go_on = |app: &mut App, said: &str| {
        // Distinct times, so "most recently active" has an order.
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.output_mut().begin_turn(said);
        app.conversation_mut().begin_turn(said);
        app.feed_stream(StreamMsg::Chunk("Ok.".into()));
        app.feed_stream(StreamMsg::Complete);
    };
    app.import_command("cc-hello");
    go_on(&mut app, "the first goes on");
    let first = app.conversation().episode().id;
    app.import_command("cc-hello");
    go_on(&mut app, "the second goes on");
    let second = app.conversation().episode().id;
    assert_ne!(first, second, "premise: two sessions go on with it");
    // The first made, the latest active.
    app.switch_session(first);
    go_on(&mut app, "the first again");
    app.claude_sessions.check("cc-hello").unwrap();
    app.switch_session(elsewhere);
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    let sessions = app.current_graph().episode_ids().len();
    onto_first_conversation(&mut app);
    assert_eq!(
        app.conversation().episode().id,
        elsewhere,
        "premise: standing elsewhere"
    );

    app.notices.clear();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.conversation().episode().id,
        first,
        "the most recently active"
    );
    assert_eq!(
        app.current_graph().episode_ids().len(),
        sessions,
        "no copy made"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("most recently active of 2 sessions")),
        "{:?}",
        app.notice()
    );
}

/// **A secret marked since a conversation was read is masked in it — and
/// never shown as it was meanwhile.** The reading kept was scrubbed before
/// the mark: read again off the UI thread, it is scrubbed under it. What a
/// view shows while that runs is a reading kept from before only if it was
/// scrubbed as the secrets now are — offered as "what was kept", the one
/// from before the mark showed the value just marked.
#[test]
fn a_secret_marked_since_a_reading_is_masked_in_it() {
    let (projects, here) = projects_with(&[("cc-mark", "Marked later")]);
    let reply = "the password is hunter2";
    std::fs::write(
        here.join("cc-mark.jsonl"),
        conversation("a question", reply, "Marked later"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    type_keys(&mut app, "j");
    let (_, page) = panes_drawn(&mut app);
    assert!(
        page.contains("hunter2"),
        "premise: no pattern knows it: {page}"
    );

    app.mark_secret("hunter2", true).expect("marked");
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    let held = format!("{:?}", app.graph_view().unwrap().claude().unwrap().read);
    assert!(!held.contains("hunter2"), "meanwhile, not as kept: {held}");
    app.await_claude_reads();
    type_keys(&mut app, "j");
    let (graph, page) = panes_drawn(&mut app);
    assert!(
        page.contains("the password is"),
        "premise: its page: {page}"
    );
    assert!(!page.contains("hunter2"), "the page: {page}");
    assert!(!graph.contains("hunter2"), "the rows: {graph}");
}

/// What the tree in `pane` holds read of its Claude Code section, as text.
fn read_in(app: &App, pane: crate::wm::PaneId) -> String {
    let section = (app.window_of(pane))
        .and_then(|w| w.tree())
        .and_then(GraphView::claude)
        .expect("a tree with a section");
    format!("{:?}", section.read)
}

/// **A secret marked reaches the tree shown at once, and a `:sp` copy of
/// it** — what was open stays open, read again under the mark, and never
/// shown as it was meanwhile (review stop 10, E 2: only a new `:graph` was
/// scrubbed; the open view kept the value until a reading landed, and a
/// split copied it).
#[test]
fn a_secret_marked_reaches_the_tree_shown_and_keeps_what_was_open() {
    let (projects, here) = projects_with(&[("cc-mark", "Marked later")]);
    std::fs::write(
        here.join("cc-mark.jsonl"),
        conversation("a question", "the password is hunter2", "Marked later"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let pane = app.workspace.focused();
    assert!(
        read_in(&app, pane).contains("hunter2"),
        "premise: no pattern knows it"
    );

    app.mark_secret("hunter2", true).expect("marked");
    let meanwhile = read_in(&app, pane);
    assert!(!meanwhile.contains("hunter2"), "at the mark: {meanwhile}");
    app.await_claude_reads();
    let again = read_in(&app, pane);
    assert!(
        again.contains("the password is"),
        "open, read again: {again}"
    );
    assert!(!again.contains("hunter2"), "under the mark: {again}");

    app.dispatch_command_for_test("sp");
    let copy = app.workspace.focused();
    assert_ne!(copy, pane, "premise: a new window");
    let copied = read_in(&app, copy);
    assert!(
        copied.contains("the password is"),
        "premise: copied: {copied}"
    );
    assert!(!copied.contains("hunter2"), "the copy: {copied}");
}

/// **A bare split of a tree lists nothing** — its copy starts where the
/// window is (`Window::place`) and is built as a tree set aside comes back
/// (`view_back`); it was built by the door a new tree takes, its listing
/// read and thrown away for the window's section — and, the directory
/// unreadable, said on the status row though the split worked (review stop
/// 10, D 1, probe P1).
#[test]
fn a_split_of_a_tree_lists_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    app.settle();
    assert_eq!(section(&app).found.len(), 1, "premise: listed");
    let before = app.notice();
    std::fs::set_permissions(&here, std::fs::Permissions::from_mode(0o000)).unwrap();
    app.split_command("", SplitDir::Horizontal);
    let after = app.notice();
    std::fs::set_permissions(&here, std::fs::Permissions::from_mode(0o755)).unwrap();
    let copy = app.workspace.focused();
    assert_eq!(
        (app.pane_graph(copy).and_then(GraphView::claude)).map(|c| c.found.len()),
        Some(1),
        "premise: the copy shows the section it was split from"
    );
    assert!(
        !after
            .as_deref()
            .is_some_and(|n| n.contains("Claude Code's conversations")),
        "a split read the directory and said so — before {before:?}, after {after:?}"
    );
}

/// **A tree set aside comes back scrubbed under a mark made meanwhile** —
/// `:help` in the graph's pane, a mark, `:q`: the tree was built from the
/// store and its section carried as read, the value just marked shown again
/// (review stop 10, E 2, probed). Listed again as it comes back, what was
/// open asked for again (`App::current_section`).
#[test]
fn a_tree_set_aside_comes_back_scrubbed_under_a_mark_made_meanwhile() {
    let (projects, here) = projects_with(&[("cc-mark", "Marked later")]);
    std::fs::write(
        here.join("cc-mark.jsonl"),
        conversation("a question", "the password is hunter2", "Marked later"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let pane = app.workspace.focused();
    assert!(read_in(&app, pane).contains("hunter2"), "premise: read");
    app.open_help_command(None);
    assert!(
        app.window_of(pane).and_then(|w| w.tree()).is_none(),
        "premise: the tree set aside"
    );

    app.mark_secret("hunter2", true).expect("marked");
    app.dispatch_command_for_test("q");
    let back = read_in(&app, pane);
    assert!(!back.contains("hunter2"), "brought back: {back}");
    app.await_claude_reads();
    let again = read_in(&app, pane);
    assert!(
        again.contains("the password is"),
        "open, read again: {again}"
    );
    assert!(!again.contains("hunter2"), "under the mark: {again}");
}

/// **An unmark lists the section again** — a title is scrubbed as it is
/// listed and its words are not kept, so the value a title held comes back
/// only by listing it again (`ClaudeSection::scrubbed`).
#[test]
fn an_unmark_lists_the_titles_again() {
    let (projects, _) = projects_with(&[("cc-t", "about hunter2")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    let title = |app: &App| section(app).found[0].title.clone().unwrap_or_default();
    assert!(title(&app).contains("hunter2"), "premise: {}", title(&app));
    app.mark_secret("hunter2", true).expect("marked");
    assert!(!title(&app).contains("hunter2"), "marked: {}", title(&app));
    app.unmark_secret("hunter2").expect("unmarked");
    assert!(title(&app).contains("hunter2"), "unmarked: {}", title(&app));
}

/// **A rebuild lists nothing again while the leave and the secrets stand**
/// — a tree is rebuilt at every background event, and a listing reads each
/// file's tail: a conversation begun since is listed at the next `:graph`,
/// or when the secrets change, never by a refresh or a rule that is not
/// theirs (`App::listed_as_now`).
#[test]
fn a_refresh_lists_nothing_again_until_the_secrets_change() {
    let (projects, here) = projects_with(&[("cc-one", "One")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    std::fs::write(here.join("cc-two.jsonl"), conversation("p", "r", "Two")).unwrap();
    app.refresh_graph_views();
    assert_eq!(section(&app).found.len(), 1, "carried by a refresh");
    let _ = app.rebuild_rules();
    assert_eq!(section(&app).found.len(), 1, "carried by the rules' door");
    app.mark_secret("hunter2", true).expect("marked");
    assert_eq!(section(&app).found.len(), 2, "listed again under the mark");
}

/// **A secret marked while a conversation is being read: the reading under
/// way never reaches a view** — it was scrubbed before the mark: it is kept,
/// read again under the mark, and what asked for it goes on waiting; a view
/// that asks meanwhile is handed nothing from before the mark (review stop
/// 4, C HIGH 4). (What a view already shows from before the mark is let go at
/// the mark and asked for again: `a_secret_marked_reaches_the_tree_shown_
/// and_keeps_what_was_open`.) The result waits in
/// the channel until it is taken, so the mark always falls between the
/// asking and the taking here.
#[test]
fn a_secret_marked_while_a_conversation_is_read_waits_for_the_mark() {
    let (projects, here) = projects_with(&[("cc-race", "Raced")]);
    let file = here.join("cc-race.jsonl");
    let text = conversation("a question", "the password is hunter2", "Raced");
    std::fs::write(&file, &text).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    // Grown since the match read it, so `l` reads it again.
    let more = serde_json::json!({
        "type": "user", "uuid": "u2", "parentUuid": "a1", "isSidechain": false,
        "timestamp": "03", "cwd": "/work",
        "message": {"role": "user", "content": "and then?"}
    });
    std::fs::write(&file, format!("{text}{more}\n")).unwrap();
    app.open_graph_command();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    assert!(app.claude_reads.in_flight() > 0, "premise: being read");
    app.mark_secret("hunter2", true).expect("marked");
    // Another `:graph`, asking while the read is under way.
    app.open_graph_command();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    assert!(
        app.claude_reads.in_flight() > 0,
        "premise: still being read"
    );
    let meanwhile = format!("{:?}", app.graph_view().unwrap().claude().unwrap().read);
    assert!(
        !meanwhile.contains("hunter2"),
        "nothing from before the mark: {meanwhile}"
    );
    app.await_claude_reads();
    let held = format!("{:?}", app.graph_view().unwrap().claude().unwrap().read);
    assert!(
        held.contains("and then?"),
        "premise: the new reading: {held}"
    );
    assert!(!held.contains("hunter2"), "under the mark: {held}");
}

/// **A conversation that cannot be read is not asked for again** while its
/// file is as it was: the match for older imports goes on when every reading
/// it waits on has come, and goes on asking for what it has not read — a
/// file that failed, asked again, failed again, and the match waited for
/// good.
#[test]
fn a_conversation_that_cannot_be_read_is_not_asked_again() {
    let (projects, here) = projects_with(&[("cc-fine", "Fine")]);
    // Begun, and no entry written: no conversation to read.
    std::fs::write(
        here.join("cc-empty.jsonl"),
        "{\"type\":\"custom-title\",\"customTitle\":\"Empty\"}\n",
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    assert_eq!(
        app.claude_reads.kept().len(),
        1,
        "premise: the match read one, and could not the other"
    );
    // The match asked, not a view: nothing to say (review stop 4, C HIGH 3).
    assert!(
        app.notices
            .iter()
            .all(|(_, n)| !n.contains("could not be read")),
        "{:?}",
        app.notices
    );
    app.open_graph_command();
    assert_eq!(app.claude_reads.in_flight(), 0, "nothing asked again");
    // …until the file changes: the live session's begins as this one does
    // (review stop 4, A 4.2, C M1).
    let entry = serde_json::json!({
        "type": "user", "uuid": "e1", "parentUuid": null, "isSidechain": false,
        "timestamp": "01", "cwd": "/work",
        "message": {"role": "user", "content": "now it has one"}
    });
    std::fs::write(
        here.join("cc-empty.jsonl"),
        format!("{{\"type\":\"custom-title\",\"customTitle\":\"Empty\"}}\n{entry}\n"),
    )
    .unwrap();
    app.open_graph_command();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_row(&mut app, |e| matches!(e, Some(Entry::Conversation(_))));
    let view = app.graph_view_mut().unwrap();
    let id = view.selected_claude().unwrap().id.clone();
    if id != "cc-empty" {
        view.select_down();
    }
    type_keys(&mut app, "l");
    app.await_claude_reads();
    assert!(
        screen(&mut app, 120).contains("now it has one"),
        "read, changed"
    );
}

/// **A reading `l` asked for that cannot be had is said** — the only word
/// of it: the row stops saying `reading…`, and without this nothing says
/// why (review stop 4, A 4.4, C HIGH 3).
#[test]
fn a_reading_l_asked_for_that_fails_is_said() {
    let (projects, here) = projects_with(&[("cc-gone", "Gone wrong")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    assert_eq!(app.claude_reads.kept().len(), 1, "premise: kept");
    // Rewritten, larger, and no longer a conversation.
    let title = "{\"type\":\"custom-title\",\"customTitle\":\"Gone wrong\"}\n";
    std::fs::write(here.join("cc-gone.jsonl"), title.repeat(40)).unwrap();
    app.open_graph_command();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    app.notices.clear();
    type_keys(&mut app, "l");
    assert!(
        app.claude_reads.in_flight() > 0,
        "premise: being read again"
    );
    app.await_claude_reads();
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("could not be read")),
        "{:?}",
        app.notice()
    );
}

/// **A `[secrets]` change scrubs a reading again** — a prefix added (by a
/// `:reload` or a `:set`) changes the secrets and not the aliases, and a
/// reading kept from before stayed "as the file stands" under the old
/// patterns (review stop 4, C HIGH 1).
#[test]
fn a_secrets_change_scrubs_a_reading_again() {
    let (projects, here) = projects_with(&[("cc-tok", "Token")]);
    let value = "acme_tok_abcdefghijklmnop";
    std::fs::write(
        here.join("cc-tok.jsonl"),
        conversation("a question", &format!("the token is {value}"), "Token"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = graph_over(projects.path(), dir.path());
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let held = format!("{:?}", app.graph_view().unwrap().claude().unwrap().read);
    assert!(held.contains(value), "premise: no pattern knows it: {held}");

    std::sync::Arc::make_mut(&mut app.config)
        .secrets
        .prefixes
        .push("acme_tok_".into());
    app.open_graph_command();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    app.await_claude_reads();
    let held = format!("{:?}", app.graph_view().unwrap().claude().unwrap().read);
    assert!(held.contains("the token is"), "premise: read: {held}");
    assert!(!held.contains(value), "under the new prefix: {held}");
}

/// **The match for older imports goes on once, when the last reading it
/// waited on comes** — each early go re-listed the directory's tails on the
/// UI thread, reloaded the store and refreshed every view, N files N times
/// (review stop 4, A 4.3).
#[test]
fn the_match_goes_on_once_its_readings_are_in() {
    let (projects, _) = projects_with(&[("cc-one", "One"), ("cc-two", "Two")]);
    let dir = TempDir::new().unwrap();
    let app = graph_over(projects.path(), dir.path());
    assert_eq!(app.claude_reads.kept().len(), 2, "premise: both read");
    assert_eq!(app.claude_reads.matched, 1, "once");
}

/// **`:delete all` asks again about what goes on from Claude Code, by name**
/// (Steven, 10-16: a second gate, "leaving you where you left off in Claude
/// Code") — before a question already waiting, since it answers the one just
/// answered; `n` there deletes nothing, `y` deletes them too, and Claude
/// Code's file stays as it was.
#[test]
fn delete_all_asks_again_about_what_goes_on_from_claude_code() {
    let (projects, here) = projects_with(&[("cc-hello", "Test greeting")]);
    let file = here.join("cc-hello.jsonl");
    let before = std::fs::read(&file).unwrap();
    for second in ['n', 'y'] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 1);
        reading_claude(&mut app, projects.path());
        app.import_command("cc-hello");
        let imported = app.conversation().episode().id;
        let stored = app.store.list_episodes().unwrap().len();
        assert_eq!(stored, 2, "premise: one born here, one imported");
        app.dispatch_command_for_test("delete all");
        app.ask(ConfirmAction::DeleteAll); // another, waiting behind
        type_keys(&mut app, "y");
        let asked = app.confirm_prompt().expect("asked again").text();
        assert!(
            asked.contains("1 session going on from Claude Code")
                && asked.contains(&app.session_tag(imported)),
            "{asked}"
        );
        assert_eq!(app.store.list_episodes().unwrap().len(), stored, "not yet");
        type_keys(&mut app, &second.to_string());
        let now = app.store.list_episodes().unwrap().len();
        match second {
            'n' => assert_eq!(now, stored, "nothing deleted"),
            _ => assert_eq!(now, 0, "all of them"),
        }
    }
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before,
        "Claude Code's, as it was"
    );
}

/// **`:delete all shannon-only` keeps what goes on from Claude Code** — an
/// import, and an old import no match found ("possibly inherited", Steven,
/// 10-16) — named in its question and in what it says after, and deletes
/// every session born in Shannon. Words as words: spaces between them do not
/// make it a session's id.
#[test]
fn delete_all_shannon_only_keeps_what_goes_on_from_claude_code() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let born_here = app.conversation().episode().id;
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    let imported = app.conversation().episode().id;
    app.import_command("cc-hello");
    let old = app.conversation().episode().id;
    app.conversation_mut().set_inherited(None);
    let mut header = app.store.load_episode(&old).unwrap().episode;
    header.inherited = None;
    app.store.put_episode(&header).unwrap();

    app.dispatch_command_for_test("delete all   shannon-only");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains("born in Shannon")
            && asked.contains("keeping 2 sessions going on from Claude Code"),
        "{asked}"
    );
    type_keys(&mut app, "y");
    let left = app.store.list_episodes().unwrap();
    assert!(left.contains(&imported), "an import, kept");
    assert!(left.contains(&old), "possibly inherited, kept");
    assert!(!left.contains(&born_here), "born here, gone");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("kept 2 sessions going on from Claude Code"),
        "{said}"
    );
}

/// **With only what it keeps stored, it says so** — not "no stored sessions
/// to delete", which was false, and naming what it kept (review stop 4,
/// A2 7).
#[test]
fn delete_all_shannon_only_with_nothing_born_here_names_what_it_kept() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    let imported = app.conversation().episode().id;
    assert_eq!(
        app.store.list_episodes().unwrap(),
        vec![imported],
        "premise"
    );

    app.delete_all_sessions(&[imported]);

    assert_eq!(app.store.list_episodes().unwrap(), vec![imported]);
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(
            "no born-in-Shannon sessions to delete; kept 1 session going on from Claude Code"
        ),
        "{said}"
    );
}

/// **No question asked, no second one** — `confirm_delete = false` is the
/// user's choice to be asked nothing (`[[feedback_users_decide]]`).
#[test]
fn delete_all_unasked_asks_nothing_again() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    std::sync::Arc::make_mut(&mut app.config).tui.confirm_delete = false;
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    app.dispatch_command_for_test("delete all");
    assert!(app.confirm_prompt().is_none(), "nothing asked");
    assert!(app.store.list_episodes().unwrap().is_empty(), "all deleted");
}

/// An app over a session of `dir`, Claude Code's directory at `projects`
/// and nothing said yet about reading it — `:graph` open.
fn graph_unasked(projects: &std::path::Path, dir: &std::path::Path) -> App {
    let mut app = app_with_n_turns(dir, 1);
    app.set_claude_projects(Some(projects.to_path_buf()));
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app
}

/// The section `:graph` holds.
fn section(app: &App) -> crate::graph_view::ClaudeSection {
    app.graph_view()
        .and_then(GraphView::claude)
        .cloned()
        .expect("a section")
}

/// **Asked before reading** (Steven, 10-17: ASK before reading Claude
/// Code's conversations — "So they don't think we're doing anything sneaky
/// here"; 10-18, the shape: once a directory, `y`, `n` or always). The first
/// `:graph` here lists none and reads none — no title, no match for older
/// imports — and asks; `y` lists them in the view already open, and the
/// next `:graph` asks nothing.
#[test]
fn graph_asks_before_reading_claude_codes_conversations() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    let before = section(&app);
    assert!(!before.listed && before.found.is_empty(), "{before:?}");
    assert_eq!(app.claude_reads.in_flight(), 0, "nothing being read");
    assert!(!app.claude_sessions.checked("cc-one"), "nothing matched");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains("read Claude Code's conversations for this directory")
            && asked.contains(&tilde(&here))
            && asked.contains("(y/N, a always)"),
        "{asked}"
    );
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    assert_eq!(
        app.pending_confirm.len(),
        1,
        "one question, however often asked"
    );

    type_keys(&mut app, "y");
    app.await_claude_reads();
    let after = section(&app);
    assert!(after.listed, "{after:?}");
    let ids: Vec<&str> = after.found.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["cc-one"]);
    assert!(
        app.claude_sessions.checked("cc-one"),
        "and the match looked"
    );

    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    assert!(app.confirm_prompt().is_none(), "asked once a run");
    assert!(section(&app).listed);
}

/// **`n` reads nothing, and is not asked again unprompted** — the heading
/// says so and names the key, the next `:graph` asks nothing, and `l` on
/// the heading asks again: `y` then lists them.
#[test]
fn no_to_reading_holds_until_the_heading_asks_again() {
    let (projects, _) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    type_keys(&mut app, "n");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("`l` on their heading in `:graph` asks again"),
        "{said}"
    );

    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    assert!(app.confirm_prompt().is_none(), "not asked again unprompted");
    let shown = text_of(&mut app, 120, 30);
    assert!(
        shown.contains("── Claude Code · here: not read — l asks ──"),
        "{shown}"
    );

    app.workspace.focus_kind(PaneKind::Graph);
    type_keys(&mut app, "G"); // the heading, the last row
    type_keys(&mut app, "l");
    assert!(app.confirm_prompt().is_some(), "`l` on the heading asks");
    type_keys(&mut app, "y");
    app.await_claude_reads();
    assert_eq!(section(&app).found.len(), 1, "listed now");
}

/// **`a` is kept** — the next run reads here without asking, and says so
/// now; `y` would not be.
#[test]
fn always_to_reading_is_kept_for_the_next_run() {
    use crate::claude_sessions::{ClaudeSessions, Consent};
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let state = TempDir::new().unwrap();
    let record = state.path().join("claude_sessions");
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.claude_sessions = ClaudeSessions::load(Some(record.clone())).0;
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    type_keys(&mut app, "a");
    app.await_claude_reads();
    assert_eq!(section(&app).found.len(), 1);
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("will be read without asking"), "{said}");
    let (next_run, _) = ClaudeSessions::load(Some(record));
    assert_eq!(
        next_run.may_read(&here, Some(projects.path())),
        Consent::Given
    );
}

/// **`a` answers only the question that offers it** — on any other it is
/// a key like any other, and the question stays up (sticky).
#[test]
fn always_answers_no_other_question() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.ask(ConfirmAction::DeleteAll);
    type_keys(&mut app, "a");
    assert!(
        matches!(app.pending_confirm.front(), Some(ConfirmAction::DeleteAll)),
        "still asking"
    );
    assert!(
        !app.store.list_episodes().unwrap().is_empty(),
        "nothing deleted"
    );
}

/// **The two doors refuse where Shannon may not read** — the listing (each
/// file's tail, for its title) and a reading (the lineage): whoever asks,
/// nothing of a conversation there is read.
#[test]
fn neither_door_reads_where_shannon_may_not() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    let found = claude_import::list(&here).unwrap().remove(0);
    assert!(app.claude_list(&here).is_err(), "listed");
    assert!(
        matches!(
            app.claude_reading(&found),
            crate::app::claude_reads::Reading::Failed(_)
        ),
        "read"
    );
    assert_eq!(app.claude_reads.in_flight(), 0);
    app.claude_sessions.allow(&here, false).unwrap();
    assert_eq!(
        app.claude_list(&here).unwrap().len(),
        1,
        "premise: once allowed"
    );
}

/// **An import asks before reading, as the section does** (Steven, 10-18:
/// "I'd rather have a gate for reading from Claude Code sessions") — the
/// question says where and what for; `n` imports nothing and says so; asked
/// for again, it asks again, and `y` imports.
#[test]
fn an_import_asks_before_reading_and_imports_on_yes() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.import_command("cc-one");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        // ALL of them: leave is the directory's (E1 F3).
        asked.contains(&format!(
            "read all of Claude Code's conversations in {}, to import cc-one",
            tilde(&here)
        )) && asked.contains("(y/N, a always)"),
        "{asked}"
    );
    type_keys(&mut app, "n");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("not imported"), "{said}");
    assert_eq!(app.conversation().episode().inherited, None, "nothing in");

    app.import_command("cc-one");
    assert!(app.confirm_prompt().is_some(), "asked again: the user asks");
    type_keys(&mut app, "y");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("imported Claude Code conversation"), "{said}");
    assert!(app.conversation().episode().inherited.is_some(), "in");
}

/// **`:unread` takes leave back** (Steven, 10-18: "I think we should add a
/// command") — `always` forgotten for the next run, `n` for this one: the
/// open `:graph` says `not read`, what was read is let go, the next
/// `:graph` asks nothing, the next run asks again; once more, there is
/// nothing to take back.
#[test]
fn unread_takes_leave_back_and_lets_go_what_was_read() {
    use crate::claude_sessions::{ClaudeSessions, Consent};
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let state = TempDir::new().unwrap();
    let record = state.path().join("claude_sessions");
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.claude_sessions = ClaudeSessions::load(Some(record.clone())).0;
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    type_keys(&mut app, "a");
    app.await_claude_reads();
    assert!(section(&app).listed, "premise: read");
    assert!(
        !app.claude_reads.kept().is_empty(),
        "premise: a reading kept"
    );

    app.dispatch_command_for_test("unread");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no longer read"), "{said}");
    assert!(!section(&app).listed, "the open `:graph` says so");
    assert!(app.claude_reads.kept().is_empty(), "what was read, let go");
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    assert!(
        app.confirm_prompt().is_none(),
        "as `n`: not asked unprompted"
    );
    let (next_run, _) = ClaudeSessions::load(Some(record));
    assert_eq!(
        next_run.may_read(&here, Some(projects.path())),
        Consent::Unasked,
        "asked next run"
    );

    app.dispatch_command_for_test("unread");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("nothing taken back"), "{said}");
}

/// **Leave for every directory's still holds this one's** — `:unread` says
/// so and names `all`, which takes it back; anything else is refused.
#[test]
fn unread_says_what_still_holds_and_all_takes_it_back() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.claude_sessions.allow(projects.path(), false).unwrap();
    app.dispatch_command_for_test("unread");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("still read") && said.contains("`:unread all`"),
        "{said}"
    );
    assert!(app.may_read_claude(&here), "still");

    app.dispatch_command_for_test("unread all");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("taken back for every directory"), "{said}");
    assert!(!app.may_read_claude(&here));

    app.dispatch_command_for_test("unread everything");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("not `everything`"), "{said}");
}

/// **`:unread` takes back an `always` another Shannon recorded** (review
/// stop 5, E1 F1, probed): two running at once, the other says `a` after
/// this one loaded the record; `:unread` here took back only what this one
/// remembered, and the next launch read there unasked.
#[test]
fn unread_takes_back_another_shannons_always() {
    use crate::claude_sessions::{ClaudeSessions, Consent};
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let state = TempDir::new().unwrap();
    let record = state.path().join("claude_sessions");
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.claude_sessions = ClaudeSessions::load(Some(record.clone())).0;
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    let (mut other, _) = ClaudeSessions::load(Some(record.clone()));
    other.allow(&here, true).unwrap();
    app.dispatch_command_for_test("unread");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no longer read"), "{said}");
    let (next, _) = ClaudeSessions::load(Some(record));
    assert_eq!(
        next.may_read(&here, Some(projects.path())),
        Consent::Unasked,
        "taken back for the next launch"
    );
}

/// **Claude Code's directory is kept absolute** (review stop 5, E1 F2): a
/// relative `CLAUDE_CONFIG_DIR` was kept as given, leave recorded as a name
/// — another launch directory, another directory read — and asked about
/// again under its absolute spelling.
#[test]
fn claude_codes_directory_is_kept_absolute() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(std::path::PathBuf::from(
        "relative/projects/../projects",
    )));
    let kept = app.claude_projects.clone().unwrap();
    assert!(
        kept.is_absolute() && !kept.to_string_lossy().contains(".."),
        "{kept:?}"
    );
}

/// **`:unread` lets go of an inspector's page of a Claude Code turn** (review
/// stop 5, E1 F5): the page kept the turn's words, unshown, after leave to
/// read them was taken back.
#[test]
fn unread_lets_go_of_an_inspected_claude_turn() {
    let (projects, _) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    type_keys(&mut app, "y");
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "lj");
    let _ = panes_drawn(&mut app);
    let holds = |app: &App| {
        (app.workspace.windows())
            .filter_map(|(_, v)| v.inspector())
            .any(|i| i.painted.holds_claude())
    };
    assert!(holds(&app), "premise: the turn's page painted");
    app.dispatch_command_for_test("unread");
    assert!(!holds(&app), "let go");
}

/// **`:unread` reaches a `:graph` set aside under another view** — `:help`
/// opened in the graph's pane, `:unread`, `:q`: the tree came back showing
/// the conversations and the turns just taken back, and an inspector beside
/// it painting them again. `:unread` walked only the views on screen
/// (design review A of the Claude Code section, 10-21, finding 1).
#[test]
fn unread_reaches_a_graph_set_aside() {
    let (projects, _) = projects_with(&[("cc-one", "Kestrel notes")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    type_keys(&mut app, "y");
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    assert!(
        screen(&mut app, 120).contains("Kestrel notes"),
        "premise: listed and shown"
    );

    app.open_help_command(Some("unread"));
    assert!(app.graph_view().is_none(), "premise: the tree is set aside");
    app.dispatch_command_for_test("unread");
    app.close_pane_command(Asked::Plain);
    app.settle();
    assert!(app.graph_view().is_some(), "premise: the tree is back");
    assert!(!section(&app).listed, "it says `not read`");
    assert!(section(&app).read.is_empty(), "and holds no reading");
    let shown = screen(&mut app, 120);
    assert!(
        !shown.contains("Kestrel notes"),
        "nothing taken back is drawn:\n{shown}"
    );
}

/// **A reading lands in a `:graph` set aside while it was read** — `l`,
/// `:help` in the graph's pane before the reading came, `:q`: the tree came
/// back waiting on a reading that had landed while it was away, and no
/// press could finish it (the same door, design review A, 10-21, finding 1).
#[test]
fn a_reading_lands_in_a_graph_set_aside() {
    let (projects, _) = projects_with(&[("cc-one", "Kestrel notes")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    type_keys(&mut app, "y");
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(&mut app);
    type_keys(&mut app, "l");
    let id = section(&app).found[0].id.clone();
    assert!(section(&app).awaiting.contains(&id), "premise: being read");

    app.open_help_command(Some("graph"));
    assert!(app.graph_view().is_none(), "premise: set aside");
    app.await_claude_reads();
    app.close_pane_command(Asked::Plain);
    app.settle();
    let section = section(&app);
    assert!(!section.awaiting.contains(&id), "not still waiting");
    assert!(section.read.contains_key(&id), "the reading is in it");
}

/// **A reading that lands after leave was taken back is dropped** —
/// neither kept nor shown: the reading thread finished what it was asked
/// before `:unread`.
#[test]
fn a_reading_landing_after_unread_is_dropped() {
    let (projects, _) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    type_keys(&mut app, "y");
    assert!(app.claude_reads.in_flight() > 0, "premise: being read");
    app.dispatch_command_for_test("unread");
    let done = app.next_claude_landing();
    app.claude_read_done(done);
    assert!(app.claude_reads.kept().is_empty(), "not kept");
}

/// **`:unread` lets go of the match's wait too** — one left waiting for a
/// reading dropped, its file gone since, held every later match waiting
/// for good.
#[test]
fn unread_lets_go_of_the_matchs_wait() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let dir = TempDir::new().unwrap();
    let mut app = graph_unasked(projects.path(), dir.path());
    type_keys(&mut app, "y");
    assert!(app.claude_reads.in_flight() > 0, "premise: the match waits");
    app.dispatch_command_for_test("unread");
    let done = app.next_claude_landing();
    app.claude_read_done(done);
    std::fs::remove_file(here.join("cc-one.jsonl")).unwrap();
    std::fs::write(
        here.join("cc-two.jsonl"),
        conversation("two prompt", "two reply", "Two"),
    )
    .unwrap();

    let before = app.claude_reads.matched;
    app.claude_sessions.allow(&here, false).unwrap();
    app.match_old_imports();
    app.await_claude_reads();
    assert_eq!(app.claude_reads.matched, before + 1, "the match went on");
}

/// **A take-back the record could not keep is said, and last** — this run
/// reads no more there, the next would: the one thing not to miss.
#[cfg(unix)]
#[test]
fn unread_says_when_the_next_run_would_still_read() {
    use crate::claude_sessions::ClaudeSessions;
    use std::os::unix::fs::PermissionsExt as _;
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let state = TempDir::new().unwrap();
    let record = state.path().join("claude_sessions");
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.claude_sessions = ClaudeSessions::load(Some(record.clone())).0;
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.claude_sessions.allow(&here, true).unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o400)).unwrap();
    app.dispatch_command_for_test("unread");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("was not taken back for the next"), "{said}");
    assert!(!app.may_read_claude(&here), "this run: not read");
}

/// **A reading landing does not move the cursor to another conversation**
/// (review stop 5, A 3, probed): `:graph` after an import lands on its
/// first turn — the conversation's, once its reading lands, and so no
/// longer drawn under the session — and the cursor went to the row after
/// it, another conversation, whose `<CR>` imported THAT one. It lands on
/// the session.
#[test]
fn a_reading_landing_keeps_the_cursor_on_the_session() {
    let (projects, _) = projects_with(&[("cc-one", "One"), ("cc-two", "Two")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.import_command("cc-one");
    let imported = app.conversation().episode().id;
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    let on = |app: &mut App| app.graph_view_mut().unwrap().selected_entry();
    let before = on(&mut app);
    assert!(
        matches!(before, Some(Entry::Node(Node::Turn { episode, .. })) if episode == imported),
        "premise: on its turn before the reading: {before:?}"
    );
    app.await_claude_reads();
    assert_eq!(on(&mut app), Some(Entry::Node(Node::Episode(imported))));
}

/// **A corrupt origin cycle neither hangs nor names a session** (review
/// stop 5, B 1, C 4: the walk's own bound was pinned by nothing — as `loop`
/// every test passed, and `:delete all` froze). Two clones whose origins
/// name each other: the walk is the core's cycle-guarded chain, in a thread
/// here so a regression fails rather than hangs the suite.
#[test]
fn an_origin_cycle_neither_hangs_nor_names_a_session() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    app.dispatch_command_for_test("clone");
    let one = app.conversation().episode().id;
    app.dispatch_command_for_test("w");
    app.dispatch_command_for_test("clone");
    let two = app.conversation().episode().id;
    let mut graph = app.current_graph();
    let mut first = graph.episode(&one).unwrap().clone();
    let back = graph
        .episode(&two)
        .unwrap()
        .origin
        .expect("premise: a clone");
    first.origin = Some(shannon_core::Origin {
        episode: two,
        ..back
    });
    graph.insert_episode(first);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(format!("{:?}", crate::app::from_claude(&graph, &two)));
    });
    let answer = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("it returned");
    assert!(answer.contains("through: None"), "{answer}");
}

/// **An import from elsewhere reads that one file** (Steven, 10-19:
/// `:import ~/test.jsonl` and leave for `~` read what else was there — "Let's
/// only import named imports when they are outside the working
/// directory"; review stop 5, A 1: `a` to a directory above Claude Code's
/// held every project). Outside this directory's Claude Code folder the
/// question is about the file alone — no `always` — `y` imports it, and no
/// leave is kept: not for its directory, not for here.
#[test]
fn an_import_from_elsewhere_reads_that_file_alone() {
    let outer = TempDir::new().unwrap();
    let projects = outer.path().join("projects");
    let here = projects_in(&projects, &[("cc-one", "Mine")]);
    let stray = outer.path().join("stray.jsonl");
    std::fs::write(&stray, conversation("a stray prompt", "a reply", "Stray")).unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.clone()));
    app.import_command(&stray.display().to_string());
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains(&format!("read {} to import it?", tilde(&stray)))
            && !asked.contains("a always"),
        "{asked}"
    );
    type_keys(&mut app, "a");
    assert!(app.confirm_prompt().is_some(), "no `always` to answer it");
    type_keys(&mut app, "y");
    assert!(app.conversation().episode().inherited.is_some(), "imported");
    assert_eq!(app.claude_reads.in_flight(), 0, "nothing else read");
    assert!(
        !app.may_read_claude(outer.path()),
        "no leave for its directory"
    );
    assert!(!app.may_read_claude(&here), "nor for here");
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    assert!(app.confirm_prompt().is_some(), "`:graph` asks here");
}

/// **`..` is folded before leave is asked of** (review stop 5, A 2, B 6,
/// C 1, probed): leave for this directory let `<here>/../-there/<file>` be
/// read unasked. The question is about the file in `-there` now, by its own
/// name.
#[test]
fn a_dot_dot_import_is_asked_about_where_it_lands() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let there = projects.path().join("-there");
    std::fs::create_dir(&there).unwrap();
    std::fs::write(
        there.join("cc-far.jsonl"),
        conversation("far prompt", "far reply", "Far"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.claude_sessions.allow(&here, false).unwrap();
    let round_about = here.join("..").join("-there").join("cc-far.jsonl");
    app.import_command(&round_about.display().to_string());
    let asked = app.confirm_prompt().expect("asked, not read").text();
    assert!(
        asked.contains(&tilde(&there.join("cc-far.jsonl"))) && !asked.contains(".."),
        "{asked}"
    );
    assert_eq!(app.conversation().episode().inherited, None, "nothing read");
}

/// **An id is its whole name, or its first 8 characters or more** (Steven,
/// 11-09: "Since we have tab completion let's require the 8"; review stop
/// 17, E2 LOW 5) — fewer is refused, saying what an id is and naming no
/// conversation; 8 takes the one it abbreviates, here or in another
/// directory, as a path would; Tab lists this directory's by their short id,
/// as Shannon's own complete, and never another's.
#[test]
fn an_id_is_eight_or_more_and_tab_lists_this_directorys() {
    const MINE: &str = "a7601a75-eb0d-44cf-9737-df2df26d8862";
    const ALSO: &str = "a7601a76-1111-44cf-9737-df2df26d8862";
    const FAR: &str = "a7609999-2222-44cf-9737-df2df26d8862";
    let (projects, _here) = projects_with(&[(MINE, "Mine"), (ALSO, "Also mine")]);
    let there = projects.path().join("-there");
    std::fs::create_dir(&there).unwrap();
    std::fs::write(
        there.join(format!("{FAR}.jsonl")),
        conversation("far prompt", "far reply", "Far"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));

    app.import_command("a760");
    assert!(app.confirm_prompt().is_none(), "nothing asked");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("first 8 characters or more"), "{said}");
    assert!(
        !said.contains("a7601a75") && !said.contains("a7609999"),
        "{said}"
    );

    app.import_command("a7601a75");
    let asked = app.confirm_prompt().expect("eight: asked").text();
    assert!(
        asked.contains("a7601a75") && !asked.contains("a7601a76"),
        "{asked}"
    );
    type_keys(&mut app, "<Esc>");
    app.import_command("a7609999");
    let asked = app
        .confirm_prompt()
        .expect("another directory's: asked")
        .text();
    assert!(asked.contains("to import it?"), "that file alone: {asked}");
    type_keys(&mut app, "<Esc>");

    type_keys(&mut app, ":import a760<Tab>");
    assert_eq!(app.cmdline.line(), Some("import a7601a75"), "the first");
    type_keys(&mut app, "<Tab>");
    assert_eq!(app.cmdline.line(), Some("import a7601a76"), "then the next");
    type_keys(&mut app, "<Esc>");
    type_keys(&mut app, ":import a7609<Tab>");
    assert_eq!(app.cmdline.line(), Some("import a7609"), "never another's");
    type_keys(&mut app, "<Esc>");
    type_keys(&mut app, ":import a7601a75-e<Tab>");
    assert_eq!(
        app.cmdline.line(),
        Some("import a7601a75-e"),
        "as typed, longer"
    );
}

/// **A Claude Code rename is no id** (Steven, 11-09: renames collide): a
/// conversation renamed in Claude Code keeps its file, named by its id, and
/// `:import` reads the id alone — two titles that begin alike name neither,
/// by any start of them.
#[test]
fn a_claude_code_title_is_never_an_imports_abbreviation() {
    let (projects, _here) = projects_with(&[
        ("cc-one", "working-on-some-project"),
        ("cc-two", "working-on-some-other-project"),
    ]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    for start in ["working-on-some", "working-on-some-p"] {
        app.import_command(start);
        assert!(app.confirm_prompt().is_none(), "{start}: nothing asked");
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("no Claude Code conversation is named"),
            "{start}: {said}"
        );
    }
    app.import_command("cc-one");
    assert!(app.confirm_prompt().is_some(), "premise: its id finds it");
}

/// **An ambiguous id names this directory's conversations, and counts
/// another project's** — never names them, as Tab offers this directory's
/// alone (stop 17, V2a LOW 4; E2 LOW 5's rule). The refusal listed every
/// file it matched, another project's ids among them.
#[test]
fn an_ambiguous_id_names_only_this_directorys() {
    let (projects, _here) = projects_with(&[("c7609999-aaaa", "One"), ("c7609999-bbbb", "Two")]);
    let there = projects.path().join("-there");
    std::fs::create_dir(&there).unwrap();
    for id in ["d4401111-cccc", "d4401111-dddd"] {
        let path = there.join(format!("{id}.jsonl"));
        std::fs::write(path, conversation("far prompt", "far reply", "Far")).unwrap();
    }
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));

    app.import_command("c7609999");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("names 2 conversations") && said.contains("c7609999-aaaa"),
        "this directory's, named: {said}"
    );

    app.import_command("d4401111");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("2 conversations in other projects"),
        "premise: found there: {said}"
    );
    assert!(
        !said.contains("cccc") && !said.contains("dddd"),
        "never named: {said}"
    );
}

/// **Leave is the file's directory's, not this one's** (review stop 5, C 1:
/// keyed on this directory instead, every test passed) — `:import <id>`
/// finds a conversation in any project, and leave here must not read one
/// elsewhere; nor may a reading for a view.
#[test]
fn leave_here_reads_no_other_projects_conversation() {
    let (projects, here) = projects_with(&[("cc-one", "Mine")]);
    let there = projects.path().join("-there");
    std::fs::create_dir(&there).unwrap();
    std::fs::write(
        there.join("cc-far.jsonl"),
        conversation("far prompt", "far reply", "Far"),
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_claude_projects(Some(projects.path().to_path_buf()));
    app.claude_sessions.allow(&here, false).unwrap();
    app.import_command("cc-far");
    let asked = app.confirm_prompt().expect("asked").text();
    assert!(
        asked.contains("cc-far.jsonl") && asked.contains("to import it?"),
        "the one file: {asked}"
    );
    assert_eq!(app.conversation().episode().inherited, None, "nothing read");
    let far = claude_import::list(&there).unwrap().remove(0);
    assert!(
        matches!(
            app.claude_reading(&far),
            crate::app::claude_reads::Reading::Failed(_)
        ),
        "nor read for a view"
    );
}

/// The inspector's `Claude Code` field on session `id`'s page, if it has one.
fn claude_field(app: &mut App, id: EpisodeId) -> Option<String> {
    let mut view = app.build_graph_view();
    assert!(view.select_node(Node::Episode(id)), "premise: its row");
    crate::view::inspector_body(&view, true, "unix")
        .iter()
        .map(crate::paint::PageLine::text)
        .find(|line| line.contains("Claude Code"))
        .map(|line| line.trim().to_string())
}

/// **The inspector says what a session has of Claude Code** (Steven, 10-18,
/// asked whether a clone of an import is born in Shannon: "Can we at least
/// record the information into the inspector? This is what the inspector is
/// for") — an import, the conversation and how much came in; a clone of it,
/// born here, what `shannon-only` does with it (and does: the same answer),
/// the turns that came in and through which session — and, that one
/// deleted, that it is gone. A session with nothing of Claude Code's says
/// nothing.
#[test]
fn the_inspector_says_what_a_session_has_of_claude_code() {
    let (projects, _) = projects_with(&[("cc-hello", "Test greeting")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let born_here = app.conversation().episode().id;
    reading_claude(&mut app, projects.path());
    app.import_command("cc-hello");
    let imported = app.conversation().episode().id;
    app.dispatch_command_for_test("clone");
    let clone = app.conversation().episode().id;
    assert_ne!(clone, imported, "premise: a clone");

    assert_eq!(claude_field(&mut app, born_here), None);
    let of_import = claude_field(&mut app, imported).expect("the import's");
    assert!(
        of_import.contains("continues cc-hello — 1 turn came in"),
        "{of_import}"
    );
    let of_clone = claude_field(&mut app, clone).expect("the clone's");
    assert!(
        of_clone
            .contains("born here (`shannon-only` deletes it), holding 1 turn that came in through")
            && of_clone.contains(&imported.short_hex()),
        "{of_clone}"
    );
    assert_eq!(app.continued_sessions(), vec![imported], "as the page says");
    // A clone of the clone (written first: a clone's source must be on
    // disk) — through the import still, two origins up.
    app.dispatch_command_for_test("w");
    app.dispatch_command_for_test("clone");
    let twice = app.conversation().episode().id;
    assert!(
        twice != clone && twice != imported,
        "premise: a clone of the clone"
    );
    let of_twice = claude_field(&mut app, twice).expect("the second clone's");
    assert!(of_twice.contains(&imported.short_hex()), "{of_twice}");

    app.dispatch_command_for_test(&format!("delete {}", imported.to_hex()));
    type_keys(&mut app, "y");
    assert!(
        !app.store.list_episodes().unwrap().contains(&imported),
        "premise: gone"
    );
    let of_clone = claude_field(&mut app, clone).expect("still the clone's");
    assert!(
        of_clone.contains("through a session now gone"),
        "{of_clone}"
    );
}

/// **Claude Code's home, resolved once at launch, reaches every reader**
/// (review stop 5, L1, HIGH): the gates — the API session's and the one a
/// Claude Code session is judged by — name its credentials, under both its
/// spellings (a temporary directory's real path differs on macOS), and its
/// conversations are listed from it. Asked through `credential_path`,
/// which only a guard answers: outside where Shannon started, a read asks
/// with no guard at all.
#[tokio::test]
async fn claude_codes_home_reaches_both_gates_and_the_listing() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("cc");
    std::fs::create_dir_all(home.join("projects")).unwrap();
    let mut app = test_app();
    let creds = home.join(".credentials.json").display().to_string();
    assert_eq!(app.rules.credential_path(&creds), None, "premise: unset");

    app.set_claude_home(Some(home.clone()));

    for path in crate::app::with_real_path(&home.join(".credentials.json")) {
        let path = path.display().to_string();
        assert!(app.rules.credential_path(&path).is_some(), "{path}");
        assert!(app.claude_rules.credential_path(&path).is_some(), "{path}");
    }
    assert_eq!(
        app.claude_projects,
        Some(crate::claude_sessions::normal(&home.join("projects")))
    );
    // And the knowledge roots' `{claude-home}`, where its CLAUDE.md and
    // skills are read from — from a start of the test's own: the default
    // named the real checkout (review stop 12, T4 8).
    let start = dir.path().join("start");
    std::fs::create_dir_all(&start).unwrap();
    app.start = Some(start);
    app.launch_knowledge(None, None);
    assert_eq!(
        app.knowledge_roots
            .as_ref()
            .and_then(|r| r.claude_home.clone()),
        Some(crate::claude_sessions::normal(&home)),
        "premise: the start is known, so the roots were set"
    );
}

/// Two conversations, the tree's cursor on the first one's first turn, read
/// with `l` — the conversation's id, and the row's entry.
fn on_a_claude_turn(app: &mut App, projects: &std::path::Path) -> (String, Option<Entry>) {
    reading_claude(app, projects);
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.await_claude_reads();
    app.workspace.focus_kind(PaneKind::Graph);
    onto_first_conversation(app);
    type_keys(app, "l");
    app.await_claude_reads();
    type_keys(app, "j");
    let view = app.graph_view().unwrap();
    assert!(
        matches!(view.selected_entry(), Some(Entry::ClaudeTurn { .. })),
        "premise: on a Claude Code turn"
    );
    let id = view.selected_claude().expect("its conversation").id.clone();
    (id, view.selected_entry())
}

/// **A split of a tree on a Claude Code turn is on that turn** (V4 verify,
/// M1) — the copy carries the section it was split from, with what was read
/// of it. Over a fresh section, which reads none, the turn had no row and the
/// copy stood on the NEXT conversation, whose `<CR>` imports that one.
#[test]
fn a_split_on_a_claude_code_turn_stays_on_it() {
    let (projects, _) = projects_with(&[("cc-a", "First"), ("cc-b", "Second")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let (id, entry) = on_a_claude_turn(&mut app, projects.path());
    let from = app.workspace.focused();
    app.split_command("", SplitDir::Horizontal);
    assert_ne!(app.workspace.focused(), from, "premise: split");
    let copy = app.graph_view().unwrap();
    assert_eq!(copy.selected_entry(), entry, "the same turn");
    assert_eq!(
        copy.selected_claude().map(|f| f.id.clone()),
        Some(id),
        "of the same conversation"
    );
}

/// **`:b N` back to a Claude Code turn this tree has not read lands on its
/// conversation** — the row that reads it again — not on your session (V4
/// verify, M1's neighbour: `go_to` found no row and moved nothing).
#[test]
fn b_n_to_an_unread_claude_turn_lands_on_its_conversation() {
    let (projects, _) = projects_with(&[("cc-a", "First"), ("cc-b", "Second")]);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let (id, _) = on_a_claude_turn(&mut app, projects.path());
    app.settle();
    app.dispatch_command_for_test("q");
    let n = app.buffers.trees().next().unwrap().id().number();
    app.dispatch_command_for_test(&format!("b{n}"));
    let view = app.graph_view().expect("a tree");
    assert!(
        matches!(view.selected_entry(), Some(Entry::Conversation(_))),
        "its conversation's row: {:?}",
        view.selected_entry()
    );
    assert_eq!(view.selected_claude().map(|f| f.id.clone()), Some(id));
}

/// **A Claude Code conversation comes in as a session** — by id from this
/// directory's conversations, built as a live turn is (prompt, tool round,
/// reply), its tool output SCRUBBED on the way in, each turn sealed as what
/// produced it, and the session still on your model to continue (Steven,
/// 10-04: continued "as a Shannon session").
#[tokio::test]
async fn a_claude_code_conversation_is_imported_as_a_session() {
    let value = "Zk3pQ9vX2mL7tR4wB8nY";
    let secret = format!("OPENAI_API_KEY={value}");
    let (projects, _) = claude_projects_with_one(&secret);
    let mut app = test_app();
    assert!(!app.scrub(&secret).contains(value), "premise: caught");
    let model = app.conversation().episode().config.model.clone();
    app.dispatch_command_for_test("effort max");
    reading_claude(&mut app, projects.path());
    app.import_command("abc12345");

    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("imported Claude Code conversation abc12345"),
        "{said}"
    );
    assert!(said.contains("1 turn, 1 tool call"), "{said}");
    let messages = app.conversation().messages().to_vec();
    let roles: Vec<_> = messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::User, Role::Assistant],
        "{messages:#?}"
    );
    assert_eq!(messages[0].content, "what is in .env?");
    assert_eq!(messages[1].content, "Let me look.");
    assert_eq!(messages[3].content, "It holds one key.");
    let stored = serde_json::to_string(&messages).unwrap();
    assert!(!stored.contains(value), "scrubbed on the way in: {stored}");
    let turns = app.conversation().own_turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].provenance.config.model,
        "claude-code/claude-opus-5-5"
    );
    // Shannon sent it no effort and the transcript records none: the turn
    // claims none, whatever the session it lands in is set to.
    assert_eq!(turns[0].provenance.config.effort, None);
    assert_eq!(turns[0].provenance.config.max_tokens, None);
    assert_eq!(
        app.conversation().episode().config.model,
        model,
        "still yours"
    );
    assert_eq!(
        app.conversation().episode().config.effort,
        Some(shannon_core::Effort::Max),
        "your effort too"
    );
}

/// **Every field an import brings in is scrubbed** — the prompt, a round's
/// text, a call's input (a key in it, and a value only its key names), its
/// result, the reply — each its own credential, none in the conversation nor
/// in what the store holds. One pinned field said nothing of its siblings:
/// four of the five scrubs could each be deleted with the suite green, and a
/// field replayed raw goes to the store at once and to the provider on the
/// next turn (review stop 4, E 1).
#[tokio::test]
async fn every_field_an_import_brings_in_is_scrubbed() {
    use serde_json::json;
    let entry = claude_entry;
    let planted = [
        ("prompt", "sk-ant-api03-promptAAAAAAAAAAAAAAAA"),
        ("round text", "sk-ant-api03-roundtextBBBBBBBBBBBBB"),
        ("call input", "sk-ant-api03-callinputCCCCCCCCCCCCC"),
        ("call key", "sk-ant-api03-callkeyDDDDDDDDDDDDDDDD"),
        ("named value", "correct horse call staple"),
        ("result", "sk-ant-api03-resultEEEEEEEEEEEEEEEE"),
        ("reply", "sk-ant-api03-replyFFFFFFFFFFFFFFFFF"),
    ];
    let (projects, _) = claude_projects_holding(&[
        entry(
            "user",
            "1",
            None,
            json!({"role": "user", "content": format!("deploy with {}", planted[0].1)}),
        ),
        entry(
            "assistant",
            "2",
            Some("1"),
            json!({"id": "M1", "model": "claude-opus-5-5", "role": "assistant", "content": [
                {"type": "text", "text": format!("using {}", planted[1].1)},
                {"type": "tool_use", "id": "T1", "name": "Bash", "input": {
                    "command": format!("export K={}", planted[2].1),
                    planted[3].1: true,
                    "password": planted[4].1,
                }}
            ]}),
        ),
        entry(
            "user",
            "3",
            Some("2"),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "T1", "content": planted[5].1}
            ]}),
        ),
        entry(
            "assistant",
            "4",
            Some("3"),
            json!({"id": "M2", "model": "claude-opus-5-5", "role": "assistant", "content": [
                {"type": "text", "text": format!("done, {}", planted[6].1)}
            ]}),
        ),
    ]);
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    app.import_command("abc12345");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("1 turn, 1 tool call"), "premise: {said}");

    let held = serde_json::to_string(app.conversation().messages()).unwrap();
    let id = app.conversation().episode().id;
    let loaded = app.store.load_episode(&id).unwrap();
    let stored = serde_json::to_string(&loaded.turns).unwrap();
    assert!(
        stored.contains("deploy with"),
        "premise: the store holds the turn"
    );
    for (field, value) in planted {
        assert!(!held.contains(value), "{field} in the conversation: {held}");
        assert!(!stored.contains(value), "{field} in the store: {stored}");
    }
}

/// **A long result is scrubbed whole, then cut** — cut first, a marked
/// value across the cut kept its start in the store and on the wire, which
/// no matcher of the whole value sees (review stop 4, E 4). And it is cut.
#[tokio::test]
async fn a_long_result_is_scrubbed_before_it_is_cut() {
    use serde_json::json;
    use shannon_core::claude_import::RESULT_CAP;
    let entry = claude_entry;
    let result = format!(
        "{}hunter2seekrit{}",
        "x".repeat(RESULT_CAP - 5),
        "y".repeat(100)
    );
    let (projects, _) = claude_projects_holding(&[
        entry("user", "1", None, json!({"role": "user", "content": "go"})),
        entry(
            "assistant",
            "2",
            Some("1"),
            json!({"id": "M1", "model": "claude-opus-5-5", "role": "assistant", "content": [
                {"type": "tool_use", "id": "T1", "name": "Bash", "input": {"command": "cat"}}
            ]}),
        ),
        entry(
            "user",
            "3",
            Some("2"),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "T1", "content": result}
            ]}),
        ),
    ]);
    let mut app = test_app();
    app.dispatch_command_for_test("secret hunter2seekrit");
    reading_claude(&mut app, projects.path());
    app.import_command("abc12345");
    let content = app
        .conversation()
        .messages()
        .iter()
        .flat_map(|m| &m.blocks)
        .find_map(|b| match b {
            shannon_core::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("premise: imported with its result");
    assert!(!content.contains("hunte"), "its start, past the cut");
    assert!(content.len() < RESULT_CAP + 100, "cut: {}", content.len());
    assert!(content.ends_with("cut on import]"));
}

/// **An import is written at once** (Steven, 10-16, decision (a)): its turns
/// are new to Shannon's store, and it stayed lazy — written on its first
/// continue, lost to a quit before one. Its source stays in Claude Code's
/// directory, untouched; what is written is Shannon's copy.
#[tokio::test]
async fn an_import_is_written_at_once() {
    let (projects, _) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    app.import_command("abc12345");
    let id = app.conversation().episode().id;
    assert_eq!(app.conversation().own_turns().len(), 1, "premise: imported");
    assert!(
        matches!(app.store.episode_len(&id), Ok(Some(1))),
        "on disk before any turn: {:?}",
        app.store.episode_len(&id)
    );
    assert!(app.unwritten().owed.is_empty(), "so nothing holds the exit");
    // And with where it came from, in the header written (10-16): the
    // Claude Code session, how many of its turns came in, and which was the
    // last — the entry `:graph` hangs it under.
    let stored = app.store.load_episode(&id).unwrap().episode;
    assert_eq!(
        stored.inherited,
        Some(shannon_core::Inherited {
            session: "abc12345-0000-0000-0000-000000000000".into(),
            turns: 1,
            at: Some("1".into()),
            opened: vec![Some("1".into())],
        }),
        "what it continues, on disk"
    );
}

/// **An import from before imports recorded their source is found once, by
/// its turns** (Steven, 10-16: "Match once by content"): on opening
/// `:graph`, its record — the conversation, the turns that came in, the
/// entry it parted at — written to its header and its buffer, and the
/// session checked, never read for again.
#[tokio::test]
async fn an_old_import_is_found_once_by_its_turns() {
    let (projects, _) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    app.import_command("abc12345");
    let id = app.conversation().episode().id;
    // As an import from before the record: none, on disk or in the buffer.
    app.conversation_mut().set_inherited(None);
    let mut stored = app.store.load_episode(&id).unwrap().episode;
    stored.inherited = None;
    app.store.put_episode(&stored).unwrap();
    assert_eq!(
        app.store.load_episode(&id).unwrap().episode.inherited,
        None,
        "premise: an old import"
    );

    app.notices.clear();
    app.open_graph_command();
    app.await_claude_reads();
    let want = Some(shannon_core::Inherited {
        session: "abc12345-0000-0000-0000-000000000000".into(),
        turns: 1,
        at: Some("1".into()),
        opened: vec![Some("1".into())],
    });
    assert_eq!(
        app.store.load_episode(&id).unwrap().episode.inherited,
        want,
        "on disk"
    );
    assert_eq!(
        app.conversation().episode().inherited,
        want,
        "in the buffer"
    );
    let said = app
        .notices
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    // By name: which session, and which conversation it continues.
    assert_eq!(
        said,
        format!(
            "older import {} continues Claude Code's abc12345",
            id.short_hex()
        )
    );
    assert!(app.claude_sessions.checked(&id.to_hex()), "checked, once");
    // Found once its readings came, after `:graph` opened: the tree shows
    // it there, under the conversation it continues.
    let view = app.graph_view().expect("the graph");
    let rows = view.tree_rows(0, view.visible_len());
    let at = |entry: crate::graph_view::Entry| rows.iter().position(|r| r.entry == entry);
    let heading = at(crate::graph_view::Entry::ClaudeHeading).expect("the section");
    let session = at(crate::graph_view::Entry::Node(Node::Episode(id))).expect("in sight");
    assert!(heading < session, "under its conversation, not a root");
}

/// **An older import not found is checked, and never read for again** —
/// though its conversation turns up later (review stop 3, A F2: the guard
/// could be deleted with every test green, and every `:graph` would read
/// every transcript again for it).
#[tokio::test]
async fn an_older_import_not_found_is_not_looked_for_again() {
    let (projects, file) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    let id = an_older_import(&mut app, None);
    let away = file.with_extension("away");
    std::fs::rename(&file, &away).unwrap();
    app.open_graph_command();
    app.await_claude_reads();
    assert!(
        app.claude_sessions.checked(&id.to_hex()),
        "premise: checked"
    );
    assert_eq!(
        app.conversation().episode().inherited,
        None,
        "premise: unfound"
    );

    std::fs::rename(&away, &file).unwrap();
    app.open_graph_command();
    app.await_claude_reads();
    assert_eq!(
        app.store.load_episode(&id).unwrap().episode.inherited,
        None,
        "not looked for again"
    );
}

/// **Where Shannon may not read, an older import is not looked for — and
/// not checked** (10-18): the listing refused read as "none there", and the
/// first `:graph`, its question not yet answered, checked every older
/// import unfound, for good. Leave given, it is found.
#[tokio::test]
async fn an_older_import_is_not_checked_where_shannon_may_not_read() {
    let (projects, _) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    let id = an_older_import(&mut app, None);
    app.claude_sessions.take_back(projects.path()).unwrap();
    app.open_graph_command();
    app.await_claude_reads();
    assert!(
        !app.claude_sessions.checked(&id.to_hex()),
        "not checked unread"
    );

    app.claude_sessions.allow(projects.path(), false).unwrap();
    app.open_graph_command();
    app.await_claude_reads();
    assert!(
        app.store
            .load_episode(&id)
            .unwrap()
            .episode
            .inherited
            .is_some(),
        "found, once read"
    );
}

/// **`y` to every directory's conversations looks for older imports too**
/// (review stop 5, A 6): that leave holds this directory's, and the match
/// ran only for this directory's own `y` — an older import stayed among the
/// roots until the next `:graph`. Through `let_read_claude`, the door `y`
/// takes.
#[tokio::test]
async fn leave_for_every_directory_looks_for_older_imports() {
    let (projects, _) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    let id = an_older_import(&mut app, None);
    app.claude_sessions.take_back(projects.path()).unwrap();
    app.let_read_claude(projects.path(), false, None);
    app.await_claude_reads();
    assert!(
        app.store
            .load_episode(&id)
            .unwrap()
            .episode
            .inherited
            .is_some(),
        "found"
    );
}

/// **The match takes a reading from before the file last grew** — an old
/// import is in what was read then; waiting for the re-read instead, the
/// match would chase the live session's file, which grows as it is read
/// (review stop 4, C M2: waiting passed every test).
#[tokio::test]
async fn the_match_takes_a_reading_from_before_the_file_grew() {
    let (projects, file) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    app.open_graph_command();
    app.await_claude_reads();
    assert_eq!(app.claude_reads.kept().len(), 1, "premise: read, kept");
    let id = an_older_import(&mut app, None);
    let grown = std::fs::read_to_string(&file).unwrap()
        + &claude_entry(
            "user",
            "9",
            Some("4"),
            serde_json::json!({"role": "user", "content": "later"}),
        );
    std::fs::write(&file, grown).unwrap();
    app.open_graph_command();
    assert!(
        app.store
            .load_episode(&id)
            .unwrap()
            .episode
            .inherited
            .is_some(),
        "matched at once, from what was kept"
    );
    app.await_claude_reads();
}

/// **A record from before `at` is completed** — one that names its
/// conversation and not the turn it parted at (written between the two
/// records) is looked for, there alone: a newer copy holding the same turns
/// does not take its place (review stop 3, C 4c: the candidate guard passed
/// every test, and the search ranged over every conversation).
#[tokio::test]
async fn a_record_from_before_at_is_completed_in_its_own_conversation() {
    let (projects, file) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    let named = "abc12345-0000-0000-0000-000000000000";
    let id = an_older_import(
        &mut app,
        Some(shannon_core::Inherited {
            session: named.into(),
            turns: 1,
            at: None,
            opened: Vec::new(),
        }),
    );
    a_newer_copy(&file);
    app.open_graph_command();
    app.await_claude_reads();
    let record = app.store.load_episode(&id).unwrap().episode.inherited;
    assert_eq!(
        record,
        Some(shannon_core::Inherited {
            session: named.into(),
            turns: 1,
            at: Some("1".into()),
            opened: vec![Some("1".into())],
        }),
        "where it parted, in the conversation it names"
    );
}

/// **With no record, the newest of two conversations holding the same turns
/// wins** — the later run, as `find_run` takes on a tie; it was whichever
/// the listing gave last (review stop 3, B).
#[tokio::test]
async fn an_older_import_ties_to_the_newest_conversation() {
    let (projects, file) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    let id = an_older_import(&mut app, None);
    let newer = a_newer_copy(&file);
    app.open_graph_command();
    app.await_claude_reads();
    let record = app.store.load_episode(&id).unwrap().episode.inherited;
    assert_eq!(record.map(|r| r.session), Some(newer));
}

/// **A session Shannon ran through Claude Code is never "found" as an
/// import** — its first turn IS the transcript's, word for word, because
/// Shannon wrote that transcript; it recorded what the turn cost, and an
/// import records nothing. And that transcript, every turn of it one
/// Shannon ran and nothing continuing it, is Shannon's now: noted, so
/// `:graph` does not list it as Claude Code's (decision 4).
#[tokio::test]
async fn a_session_shannon_ran_is_not_an_import() {
    let dir = TempDir::new().unwrap();
    let (projects, _) = claude_projects_with_one("nothing secret");
    let (mut app, _provider) = provider_app();
    reading_claude(&mut app, projects.path());
    ran_through_claude_code(&mut app, dir.path());
    let id = app.conversation().episode().id;
    assert!(
        matches!(app.store.episode_len(&id), Ok(Some(1))),
        "premise: on disk"
    );

    app.notices.clear();
    app.open_graph_command();
    app.await_claude_reads();
    assert_eq!(
        app.store.load_episode(&id).unwrap().episode.inherited,
        None,
        "not an import"
    );
    assert!(
        app.claude_sessions
            .started("abc12345-0000-0000-0000-000000000000"),
        "its transcript is Shannon's"
    );
    // Said by name, as what happened to it — and without a count of no
    // imports beside it.
    assert_eq!(
        app.notice().as_deref(),
        Some("abc12345 left the Claude Code list: Shannon wrote it")
    );
}

/// **…but one a session continues stays listed, however it was written** —
/// it is that session's root (Steven's `5f6e7d8c`, 10-16: a transcript
/// Shannon ran, then imported and continued). Hidden, the session would hang
/// under nothing.
#[tokio::test]
async fn a_conversation_a_session_continues_stays_listed() {
    let dir = TempDir::new().unwrap();
    let (projects, _) = claude_projects_with_one("nothing secret");
    let (mut app, _provider) = provider_app();
    reading_claude(&mut app, projects.path());
    ran_through_claude_code(&mut app, dir.path());
    app.import_command("abc12345");
    assert!(
        app.conversation().episode().inherited.is_some(),
        "premise: continued"
    );

    app.open_graph_command();
    app.await_claude_reads();
    assert!(
        !app.claude_sessions
            .started("abc12345-0000-0000-0000-000000000000"),
        "still Claude Code's, under which the import hangs"
    );
}

/// **A distillation Shannon asked for is Shannon's too** — `S` or `:compact`
/// on the Claude Code backend runs in a session whose last turn is the squash
/// instruction and the draft it got back, and the summary sealed from that
/// draft is not those words, so it was no turn Shannon "ran" (Steven's
/// `6a7b8c9d`, 10-16, stayed listed). Both shapes: resumed after a turn
/// Shannon ran, and fresh after Shannon's handoff line.
#[tokio::test]
async fn a_distillation_shannon_asked_for_is_its_own() {
    use serde_json::json;
    use shannon_core::claude_import::{Scope, read};
    let dir = TempDir::new().unwrap();
    let (projects, file) = claude_projects_with_one("nothing secret");
    let (mut app, _provider) = provider_app();
    reading_claude(&mut app, projects.path());
    ran_through_claude_code(&mut app, dir.path());
    // As `S` with words of its own asks it: the instruction, then those.
    let asked = format!(
        "{}\n\nAdditional instructions: keep the paths",
        app.config.merge.squash_instruction
    );
    let reply = |id: &str, text: &str| {
        json!({"id": id, "model": "claude-sonnet-5-5", "role": "assistant",
               "content": [{"type": "text", "text": text}]})
    };
    let question = json!({"role": "user", "content": asked});
    let resumed = std::fs::read_to_string(&file).unwrap()
        + &claude_entry("user", "5", Some("4"), question.clone())
        + &claude_entry("assistant", "6", Some("5"), reply("M3", "I read .env."));
    std::fs::write(&file, resumed).unwrap();
    let handoff = "(Earlier in this conversation, served through another backend and \
                   recorded by Shannon:)\n\nUser: what is in .env?";
    let fresh = claude_entry("assistant", "h1", None, reply("H1", handoff))
        + &claude_entry("user", "h2", Some("h1"), question)
        + &claude_entry("assistant", "h3", Some("h2"), reply("H2", "I read .env."));
    let fresh_file = file.with_file_name("def45678-0000-0000-0000-000000000000.jsonl");
    std::fs::write(&fresh_file, fresh).unwrap();
    let premise = read(&fresh_file, Scope::Lineage).unwrap();
    assert_eq!(
        (premise.turns.len(), premise.skipped.handoffs),
        (1, 1),
        "premise: a handoff, then the distillation alone"
    );

    app.open_graph_command();
    app.await_claude_reads();
    for id in [
        "abc12345-0000-0000-0000-000000000000",
        "def45678-0000-0000-0000-000000000000",
    ] {
        assert!(app.claude_sessions.started(id), "{id} is Shannon's");
    }
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("abc12345")
            && said.contains("def45678")
            && said.ends_with("left the Claude Code list: Shannon wrote them"),
        "both, by name: {said}"
    );
}

/// **…and the instruction's words alone are Claude Code's** — typed there,
/// with no history Shannon gave before them. Known by its words only, a
/// distillation would take every conversation asking to "Summarize" under a
/// short instruction of the user's own. Nor does an EMPTY instruction of the
/// user's match every prompt: a session Shannon seeded and the user went on
/// with in Claude Code stays Claude Code's.
#[tokio::test]
async fn the_instructions_words_alone_stay_claude_codes() {
    use serde_json::json;
    let (projects, file) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    std::sync::Arc::make_mut(&mut app.config)
        .merge
        .squash_instruction = String::new();
    let user = |text: &str| json!({"role": "user", "content": text});
    let reply = json!({"id": "M1", "model": "claude-sonnet-5-5", "role": "assistant",
                       "content": [{"type": "text", "text": "Here is a summary."}]});
    let typed = claude_entry(
        "user",
        "1",
        None,
        user(shannon_core::DEFAULT_SQUASH_INSTRUCTION),
    ) + &claude_entry("assistant", "2", Some("1"), reply.clone());
    std::fs::write(&file, typed).unwrap();
    let handoff = json!({"id": "H1", "model": "claude-sonnet-5-5", "role": "assistant",
        "content": [{"type": "text", "text": "(Earlier in this conversation, served through \
                     another backend and recorded by Shannon:)\n\nUser: hi"}]});
    let went_on = claude_entry("assistant", "h1", None, handoff)
        + &claude_entry("user", "h2", Some("h1"), user("and now in Claude Code"))
        + &claude_entry("assistant", "h3", Some("h2"), reply);
    std::fs::write(
        file.with_file_name("def45678-0000-0000-0000-000000000000.jsonl"),
        went_on,
    )
    .unwrap();

    app.open_graph_command();
    app.await_claude_reads();
    for id in [
        "abc12345-0000-0000-0000-000000000000",
        "def45678-0000-0000-0000-000000000000",
    ] {
        assert!(
            !app.claude_sessions.started(id),
            "{id}: a conversation of Claude Code's own"
        );
    }
}

/// **A turn delete keeps the session's name and import record** (review
/// stop 2, A F1): the rewrite rebuilt the session from its shape alone, so
/// its sentence named no session, `:open <name>` found nothing, and `:graph`
/// lost the `⧓` on the conversation it continues. The imported turn deleted,
/// it parts from Claude Code at 0 — and the turn it parted at is no longer
/// said: its imported turns are no longer the conversation's run, and placed
/// by them it hung at a turn it no longer holds (review stop 3, A F7). It
/// still continues the conversation.
#[tokio::test]
async fn a_turn_delete_keeps_the_sessions_name_and_import_record() {
    let (projects, _) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    reading_claude(&mut app, projects.path());
    app.import_command("abc12345");
    app.dispatch_command_for_test("rename went-on");
    app.output_mut().begin_turn("and then");
    app.conversation_mut().begin_turn("and then");
    app.feed_stream(StreamMsg::Chunk("on".into()));
    app.feed_stream(StreamMsg::Complete);
    let id = app.conversation().episode().id;
    assert_eq!(app.conversation().own_turns().len(), 2, "premise: grown");

    app.new_moment();
    app.run_confirmed(first_turn_delete(&app, id));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains(" went-on "), "named: {said}");
    let kept = shannon_core::Inherited {
        session: "abc12345-0000-0000-0000-000000000000".into(),
        turns: 0,
        at: None,
        opened: Vec::new(),
    };
    let held = app.conversation().episode();
    assert_eq!(held.label.name.as_deref(), Some("went-on"), "in the buffer");
    assert_eq!(held.inherited.as_ref(), Some(&kept), "in the buffer");
    let stored = app.store.load_episode(&id).unwrap().episode;
    assert_eq!(stored.label.name.as_deref(), Some("went-on"), "on disk");
    assert_eq!(stored.inherited, Some(kept), "on disk");
}

/// **By file, too, and every refusal says why** — no target, a name that
/// matches nothing, and no Claude Code directory to look in. A file is
/// read where the user lets Shannon read, as an id is: asked — of the file
/// alone, Claude Code's directory unknown here — and `y` imports.
#[tokio::test]
async fn import_takes_a_file_and_refuses_visibly() {
    let (projects, file) = claude_projects_with_one("nothing secret");
    let mut app = test_app();
    app.import_command(&file.display().to_string());
    let asked = app.confirm_prompt().expect("asked first").text();
    assert!(asked.contains("to import it?"), "{asked}");
    type_keys(&mut app, "y");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("imported Claude Code conversation")),
        "{:?}",
        app.notice()
    );

    app.import_command("");
    assert!(app.notice().is_some_and(|n| n.contains("`:import <id>`")));
    app.import_command("zzz");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("directory is unknown")),
        "no projects directory set: {:?}",
        app.notice()
    );
    reading_claude(&mut app, projects.path());
    app.import_command("zzz");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("first 8 characters or more")),
        "too short for an id: {:?}",
        app.notice()
    );
    app.import_command("zzzzzzzz");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no Claude Code conversation is named"))
    );
}
