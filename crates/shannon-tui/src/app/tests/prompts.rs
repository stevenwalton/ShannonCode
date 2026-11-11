//! **Each prompt window its own draft and its own aim** (Steven, 10-26:
//! "each prompt pane has its own draft and its own aim … make sure that the
//! reaim is on the last prompt accessed"; the bug: two prompt windows shared
//! one aim — "Both change when I change my aim where only the last should").
//! Each prompt window owns a `PromptView`; the one you write in is the
//! workspace's `prompt_pane` — the one you stand in, else the one you stood
//! in last; the unnamed register is every draft's.

use super::*;
use crate::wm::PaneId;

const SIZE: (u16, u16) = (80, 30);

/// The app, (A's transcript, B's transcript), (A, B), and (the first
/// prompt, the second) — [`two_prompts`]'s.
type TwoPrompts = (
    App,
    (PaneId, PaneId),
    (BufferId, BufferId),
    (PaneId, PaneId),
);

/// Two transcripts on two sessions and two prompt windows, the second split
/// from the first while standing in B's transcript had aimed it at B — the
/// second standing, the one used last.
fn two_prompts() -> TwoPrompts {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let at_a = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    let at_b = app.workspace.focused();
    app.new_session_command();
    let (a, b) = (app.pane_session(at_a).id, app.pane_session(at_b).id);
    assert_ne!(a, b, "premise: two sessions");
    app.focus_pane(at_b);
    app.set_focus(PaneKind::Prompt);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    // The event's end: a split moves focus without saying so, and
    // `settle_focus` is where the prompt it lands in is noted.
    app.settle();
    let second = app.workspace.focused();
    assert_ne!(
        first, second,
        "premise: a second prompt window, standing in it"
    );
    assert_eq!(app.prompt_of(first).map(|p| p.aim), Some(b), "premise");
    assert_eq!(
        app.prompt_of(second).map(|p| p.aim),
        Some(b),
        "born where it was split from"
    );
    (app, (at_a, at_b), (a, b), (first, second))
}

fn aim_of(app: &App, prompt: PaneId) -> BufferId {
    app.prompt_of(prompt).expect("a prompt window").aim
}

fn draft_of(app: &App, prompt: PaneId) -> String {
    app.prompt_of(prompt).expect("a prompt window").draft.text()
}

/// **Standing in a transcript re-aims the prompt you used last, alone** —
/// the bug as Steven met it: both prompts followed.
#[test]
fn standing_in_a_transcript_reaims_only_the_prompt_used_last() {
    let (mut app, (at_a, _), (a, b), (first, second)) = two_prompts();
    app.focus_pane(at_a);
    assert_eq!(aim_of(&app, second), a, "the prompt used last follows");
    assert_eq!(aim_of(&app, first), b, "the other keeps its own");
    assert_eq!(app.prompt_target(), a, "what a session command acts on");

    // The first used, then a transcript stood in: the first follows now.
    app.focus_pane(first);
    assert_eq!(app.prompt_target(), b, "writing in the first: its own aim");
    let at_b = (app.session_panes(Over::ThisTab))
        .find(|&p| p != at_a)
        .unwrap();
    app.focus_pane(at_a);
    app.focus_pane(at_b);
    assert_eq!(aim_of(&app, first), b);
    app.focus_pane(at_a);
    assert_eq!(aim_of(&app, first), a, "the first followed");
    assert_eq!(aim_of(&app, second), a, "the second kept where it went");
}

/// **The pane two prompts go to closing re-aims them both** — each its own
/// aim, each said (review stop 11, A L3 / F 3): only the prompt you write in
/// moved, the other left aimed at a session no pane showed.
#[test]
fn a_closed_pane_re_aims_every_prompt_that_went_to_it() {
    let (mut app, (_, at_b), (a, b), (first, second)) = two_prompts();
    assert_eq!(
        (aim_of(&app, first), aim_of(&app, second)),
        (b, b),
        "premise"
    );
    app.focus_pane(at_b);
    app.notices.clear();
    app.dispatch_command_for_test("q");
    assert_eq!(aim_of(&app, first), a, "the first re-aimed");
    assert_eq!(aim_of(&app, second), a, "and the second");
    let said = (app.notices.iter())
        .filter(|(_, s)| s.contains("now aiming at"))
        .count();
    assert_eq!(said, 2, "each said: {:?}", app.notices);
}

/// **Each title names its own aim** — with two prompts there is always a
/// question of which goes where (`aim_needs_saying`'s third clause), and
/// each answers for itself. **And the one you write in wears `%`**, focus
/// in a transcript: which takes `<Enter>`, `:Send`, `:aim` and the next
/// re-aim was said nowhere (review stop 11, F 2).
#[test]
fn each_prompt_title_names_its_own_aim() {
    let (mut app, (at_a, _), (a, b), (first, second)) = two_prompts();
    app.focus_pane(at_a);
    let handle =
        |app: &App, b: BufferId| app.session_of(b).unwrap().conversation.episode().handle();
    assert_eq!(
        app.prompt_title(second),
        format!(" % prompt → {} ", handle(&app, a)),
        "the one you write in, marked"
    );
    assert_eq!(
        app.prompt_title(first),
        format!(" prompt → {} ", handle(&app, b))
    );
}

/// **Each prompt its own draft, and `<Enter>` sends the one you stand in to
/// where it goes** — typed in one, the other is untouched; sent from the
/// first, B gets it and the second still holds its draft.
#[tokio::test]
async fn each_prompt_has_its_own_draft_and_sends_it_where_it_goes() {
    let (mut app, (at_a, _), (a, b), (first, second)) = two_prompts();
    app.prompt_mut().insert_pasted("to a");
    app.focus_pane(at_a);
    assert_eq!(aim_of(&app, second), a, "premise");
    app.focus_pane(first);
    app.prompt_mut().insert_pasted("to b");
    assert_eq!(draft_of(&app, first), "to b");
    assert_eq!(draft_of(&app, second), "to a", "the other's own");

    app.submit_prompt();
    assert_eq!(draft_of(&app, first), "", "sent");
    assert_eq!(draft_of(&app, second), "to a", "still the second's");
    let sent_to = |app: &App, s: BufferId| {
        (app.session_of(s).unwrap().output.lines().iter()).any(|l| l.contains("to b"))
    };
    assert!(sent_to(&app, b), "B has it");
    assert!(!sent_to(&app, a), "A does not");
}

/// **One register for every draft** — vim's unnamed register is the
/// editor's: `yy` in one prompt, `p` in the other.
#[test]
fn a_yank_in_one_prompt_pastes_in_another() {
    let (mut app, _, _, (first, second)) = two_prompts();
    app.prompt_mut().insert_pasted("shared words");
    type_keys_sized(&mut app, SIZE, "<Esc>yy");
    app.focus_pane(first);
    type_keys_sized(&mut app, SIZE, "p");
    assert!(
        draft_of(&app, first).contains("shared words"),
        "pasted: {:?}",
        draft_of(&app, first)
    );
    assert_eq!(
        draft_of(&app, second),
        "shared words",
        "the yank's own kept"
    );
}

/// **A draft per tab** (Steven, 10-26: "A draft per tab"): a tab's prompt
/// is its own, so what is typed in one is not in another's, and is there
/// when its tab comes back.
#[test]
fn a_draft_is_the_tabs_it_was_typed_in() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().insert_pasted("tab one's");
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.prompt().text(), "", "a new tab's prompt is empty");
    app.prompt_mut().insert_pasted("tab two's");
    app.dispatch_command_for_test("tabnext 1");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.prompt().text(), "tab one's", "back with its tab");
    app.dispatch_command_for_test("tabnext 2");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.prompt().text(), "tab two's");
}

/// **`:only` keeps the prompt you write in, its draft with it** — the
/// prompt was closed with the rest and put back by the reconcile, harmless
/// while the draft was the app's; each window owns its draft now
/// (`Workspace::only`).
#[test]
fn only_keeps_the_prompt_you_write_in_and_its_draft() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let prompt = app.workspace.focused();
    app.prompt_mut().insert_pasted("half written");
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("only");
    assert_eq!(
        app.session_panes(Over::ThisTab).count(),
        1,
        "the other closed — not refused over the draft kept: {:?}",
        app.notice()
    );
    assert!(
        app.workspace.pane_ids().contains(&prompt),
        "the same prompt window"
    );
    assert_eq!(app.prompt().text(), "half written", "its draft kept");
}

/// **The prompt you stand in is the one you write in, noted or not** —
/// `Workspace::prompt_pane` asks the focused pane first. A floor, not a
/// path a key takes today: a split moves focus unsaid and `settle_focus`
/// notes it at the event's end, before the next key (review stop 11, F 8),
/// so the state is made here by the workspace's own calls — the note on
/// the first, the split unsettled — for a door tomorrow that writes in
/// the same event it moves focus.
#[test]
fn the_prompt_you_stand_in_takes_the_keys_before_it_is_noted() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let first = app.workspace.focused();
    // The first noted as the one stood in last, as its focus move noted it:
    // without, the fallback (the first prompt in the ring) answered alike.
    app.workspace.note_prompt(first);
    app.split_command("", SplitDir::Vertical);
    let second = app.workspace.focused();
    assert_ne!(first, second, "premise: standing in the new one, not noted");
    app.prompt_mut().insert_pasted("here");
    assert_eq!(draft_of(&app, second), "here");
    assert_eq!(draft_of(&app, first), "", "not the one noted last");
}

/// **An insert begun in a prompt you leave is not replayed into the next
/// insert there** (review stop 11, A 1): `3iab` in the second, a click into
/// the first (Insert kept, as a click keeps it), `xy<Esc>` there, back, and
/// `iz<Esc>` — the count's repeat was armed on the second's draft and fired
/// on its next `<Esc>`: `abzabzabz`. The count is dropped as focus leaves
/// (`App::end_what_focus_left`), as vim drops it when the cursor moves.
#[test]
fn an_insert_begun_in_a_prompt_left_is_not_replayed() {
    let (mut app, _, _, (first, second)) = two_prompts();
    type_keys_sized(&mut app, SIZE, "<Esc>3iab");
    app.focus_pane(first);
    type_keys_sized(&mut app, SIZE, "xy<Esc>");
    assert_eq!(draft_of(&app, first), "xy", "premise: typed there, once");
    app.focus_pane(second);
    type_keys_sized(&mut app, SIZE, "iz<Esc>");
    let text = draft_of(&app, second);
    assert_eq!(
        (text.matches('a').count(), text.matches('z').count()),
        (1, 1),
        "nothing replayed: {text:?}"
    );
}

/// **A selection in a prompt you leave is not left drawn** (review stop
/// 11, A L1): `v` and a motion in the second, a click into the first — the
/// anchor cleared was the prompt you arrived in, so the one left kept its
/// highlight in Normal mode.
#[test]
fn a_selection_in_a_prompt_left_is_not_left_drawn() {
    let (mut app, _, _, (first, second)) = two_prompts();
    app.prompt_mut().insert_pasted("hello there");
    type_keys_sized(&mut app, SIZE, "<Esc>0vll");
    let selected = |app: &App| app.prompt_of(second).unwrap().draft.selected();
    assert!(selected(&app).is_some(), "premise: selecting in the second");
    app.focus_pane(first);
    assert_eq!(selected(&app), None, "the one left holds no selection");
}

/// **A prompt window is about its own aim** (`App::pane_session`) — what
/// the row beside it describes, and what a session command run there acts
/// on; it was the prompt you write in's, for every prompt window.
#[test]
fn a_prompt_window_is_about_its_own_aim() {
    let (mut app, (at_a, _), (a, b), (first, second)) = two_prompts();
    app.focus_pane(at_a);
    assert_eq!(app.pane_session(second).id, a);
    assert_eq!(app.pane_session(first).id, b, "its own, not the other's");
}

/// **The wheel over a prompt moves that prompt's cursor** — the one under
/// the pointer, not the one you write in (`wheel_scroll_at`).
#[test]
fn the_wheel_moves_the_prompt_under_the_pointer() {
    let (mut app, _, _, (first, second)) = two_prompts();
    app.focus_pane(first);
    app.prompt_mut().insert_pasted("one\ntwo\nthree");
    app.focus_pane(second);
    app.prompt_mut().insert_pasted("x\ny\nz");
    let cursor = |app: &App, p| app.prompt_of(p).unwrap().draft.cursor();
    let (before_first, before_second) = (cursor(&app, first), cursor(&app, second));
    app.wheel_scroll_at(first, true, SIZE);
    assert_ne!(
        cursor(&app, first),
        before_first,
        "the one under the pointer moved"
    );
    assert_eq!(
        cursor(&app, second),
        before_second,
        "the one written in did not"
    );
}

/// **Each prompt pane draws its own draft** — rendered: both drafts on the
/// screen, each in its own pane (`view::draw`'s prompt arm).
#[test]
fn each_prompt_pane_draws_its_own_draft() {
    let (mut app, _, _, (first, _)) = two_prompts();
    app.prompt_mut().insert_pasted("written in the second");
    app.focus_pane(first);
    app.prompt_mut().insert_pasted("written in the first");
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    draw_frame(&mut app, &mut terminal);
    let screen = rendered_text(terminal.backend().buffer());
    assert!(screen.contains("written in the first"), "{screen}");
    assert!(screen.contains("written in the second"), "{screen}");
}

/// **"At the prompt" is the prompt you used last** — a door that hands you
/// back to the prompt (`set_focus`, `settle_focus`'s floor) lands in the one
/// holding your draft, not the first in the ring (`Workspace::focus_kind`).
#[test]
fn back_at_the_prompt_is_the_prompt_used_last() {
    let (mut app, (at_a, _), _, (first, second)) = two_prompts();
    app.focus_pane(at_a);
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.workspace.focused(), second, "the one used last");
    app.focus_pane(first);
    app.focus_pane(at_a);
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.workspace.focused(), first);
}

// ---- a draft holds its window (vim's E37 for a modified buffer) ----------

/// **A prompt does not close over its draft** — `:q`, `:close`, `<C-w>c`,
/// `<C-w>q` and `:hide` refused, the draft kept (`:hide` keeps what it
/// closes, and a draft has nowhere to wait); `:q!` closes it, the draft
/// dropped, said. An empty prompt closes at once.
#[test]
fn a_prompt_does_not_close_over_its_draft() {
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    for via in [":q", ":close", ":hide", "<C-w>c", "<C-w>q"] {
        let (mut app, _, _, (_, second)) = two_prompts();
        app.prompt_mut().insert_pasted("not sent yet");
        app.key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), SIZE);
        match via.strip_prefix(':') {
            Some(command) => {
                app.dispatch_command_for_test(command);
            }
            None => {
                app.key_for_test(ctrl_w, SIZE);
                let c = via.chars().last().unwrap();
                app.key_for_test(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), SIZE);
            }
        }
        app.settle();
        assert!(
            app.workspace.pane_ids().contains(&second),
            "{via}: not closed"
        );
        assert_eq!(draft_of(&app, second), "not sent yet", "{via}: kept");
        assert_eq!(
            app.notice().as_deref(),
            Some(crate::app::held::DRAFT_NOT_CLOSED),
            "{via}: said"
        );
    }
    let (mut app, _, _, (first, second)) = two_prompts();
    app.prompt_mut().insert_pasted("not sent yet");
    app.dispatch_command_for_test("q!");
    assert!(
        !app.workspace.pane_ids().contains(&second),
        "`:q!` closes it"
    );
    assert!(
        (app.notices.iter()).any(|(_, said)| said.contains("its draft dropped")),
        "said: {:?}",
        app.notices
    );
    // An empty one closes at once — blanks are no draft (`has_draft`).
    app.focus_pane(first);
    app.split_command("", SplitDir::Vertical);
    app.settle();
    let empty = app.workspace.focused();
    app.prompt_mut().insert_pasted("  \n ");
    app.dispatch_command_for_test("q");
    assert!(
        !app.workspace.pane_ids().contains(&empty),
        "nothing to keep"
    );
    // The only prompt, a draft in it: never closed, for its own reason —
    // the draft is not why.
    app.focus_pane(first);
    app.prompt_mut().insert_pasted("the last prompt's");
    app.dispatch_command_for_test("q");
    assert!(
        app.notice()
            .is_some_and(|said| said.contains("this is the only prompt")),
        "said: {:?}",
        app.notice()
    );
}

/// **`:only` does not close another prompt over its draft** — refused,
/// naming it; `:only!` closes it, the draft dropped, said. Another prompt
/// holding none closes with the rest.
#[test]
fn only_does_not_close_another_prompt_over_its_draft() {
    let (mut app, _, _, (first, _)) = two_prompts();
    app.dispatch_command_for_test("only");
    assert!(
        !app.workspace.pane_ids().contains(&first),
        "an empty one closes"
    );

    let (mut app, _, _, (first, second)) = two_prompts();
    app.focus_pane(first);
    app.prompt_mut().insert_pasted("the first's");
    app.focus_pane(second);
    app.dispatch_command_for_test("only");
    assert!(app.workspace.pane_ids().contains(&first), "refused");
    // The remedy first, then whose: the pane, by its corner (F 4, F 11).
    let corner = app.corner_of(first).expect("its corner");
    assert_eq!(
        app.notice().unwrap_or_default(),
        format!(
            "`:only`: not closed — `:only!` closes anyway (it is dropped); pane {corner} holds \
             a draft"
        )
    );
    // `<C-w>o`, the key, is `:only` — refused alike, never forced. No test
    // pressed it, and making it force passed (review stop 12, T5 F2).
    app.notices.clear();
    let key = |code, mods| KeyEvent::new(code, mods);
    app.key_for_test(key(KeyCode::Esc, KeyModifiers::NONE), (120, 40));
    app.key_for_test(key(KeyCode::Char('w'), KeyModifiers::CONTROL), (120, 40));
    app.key_for_test(key(KeyCode::Char('o'), KeyModifiers::NONE), (120, 40));
    assert!(
        app.workspace.pane_ids().contains(&first),
        "`<C-w>o` refused too"
    );
    let said = app.notice().map(|n| n.to_string()).unwrap_or_default();
    assert!(said.contains("not closed"), "and said why: {said}");
    app.dispatch_command_for_test("only!");
    assert!(!app.workspace.pane_ids().contains(&first), "forced: closed");
    assert!(
        app.notice()
            .is_some_and(|said| said.ends_with("— a draft dropped")),
        "said: {:?}",
        app.notice()
    );
}

/// **A tab does not close over a draft in it** — `:tabclose N` refused,
/// naming the tab; `:tabclose! N` closes it, the draft dropped, said. And
/// a tab's last pane, its prompt: `:q` refused over its draft, `:q!`
/// closing the tab.
#[test]
fn a_tab_does_not_close_over_a_draft_in_it() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().insert_pasted("tab two's");
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabclose 2");
    assert_eq!(app.tabs.count(), 2, "refused");
    assert_eq!(
        app.notice().as_deref(),
        Some(
            "`:tabclose`: not closed — `:tabclose!` closes anyway (it is dropped); in tab 2 \
             (`2gt`), pane 2 holds a draft"
        )
    );
    app.dispatch_command_for_test("tabonly");
    assert_eq!(app.tabs.count(), 2, "`:tabonly` refused too");
    app.dispatch_command_for_test("tabclose! 2");
    assert_eq!(app.tabs.count(), 1, "forced: closed");
    assert!(
        app.notice()
            .is_some_and(|said| said.ends_with("; a draft dropped")),
        "said: {:?}",
        app.notice()
    );

    // A tab whose last pane is its prompt, a draft in it.
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.workspace.pane_count(), 1, "premise: the prompt alone");
    app.prompt_mut().insert_pasted("last words");
    app.dispatch_command_for_test("q");
    assert_eq!(app.tabs.count(), 2, "`:q` refused over the draft");
    assert_eq!(
        app.notice().as_deref(),
        Some(crate::app::held::DRAFT_NOT_CLOSED)
    );
    app.dispatch_command_for_test("q!");
    assert_eq!(app.tabs.count(), 1, "`:q!` closes the tab");
    assert!(
        app.notice()
            .is_some_and(|said| said.ends_with("— a draft dropped")),
        "said: {:?}",
        app.notice()
    );
}

/// **Every close of a tab's last pane, its prompt holding a draft, is
/// refused but `:q!`** — `:q`, `<C-w>q` (`quit_window`) and `:close`,
/// `:hide`, `<C-w>c` (`close_pane_command`) through the one door,
/// `close_tab_of_its_last_window`. Only `:q` was tested; passing `!`'s force
/// to the others passed every test (review stop 11, A H(a), F M-C).
#[test]
fn every_close_of_a_tabs_last_prompt_holds_for_its_draft() {
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    for via in [":q", ":close", ":hide", "<C-w>c", "<C-w>q"] {
        let mut app = test_app();
        app.dispatch_command_for_test("tabnew");
        app.set_focus(PaneKind::Output);
        app.dispatch_command_for_test("close");
        app.set_focus(PaneKind::Prompt);
        assert_eq!(app.workspace.pane_count(), 1, "premise: the prompt alone");
        app.prompt_mut().insert_pasted("last words");
        app.key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), SIZE);
        app.notices.clear();
        match via.strip_prefix(':') {
            Some(command) => {
                app.dispatch_command_for_test(command);
            }
            None => {
                app.key_for_test(ctrl_w, SIZE);
                let c = via.chars().last().unwrap();
                app.key_for_test(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), SIZE);
            }
        }
        assert_eq!(app.tabs.count(), 2, "{via}: the tab kept");
        assert_eq!(app.prompt().text(), "last words", "{via}: and its draft");
        assert_eq!(
            app.notice().as_deref(),
            Some(crate::app::held::DRAFT_NOT_CLOSED),
            "{via}"
        );
    }
}

/// **A forced tab close says the drafts it dropped** — `:tabclose!` of the
/// tab you are in ("closed tab N — a draft dropped") and `:tabonly!` over
/// another's ("; a draft dropped"). Each passed every test silenced
/// (review stop 11, A H(b)(c), F M-A/M-B).
#[test]
fn a_forced_tab_close_says_the_draft_it_dropped() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().insert_pasted("tab two's");
    app.dispatch_command_for_test("tabclose!");
    assert_eq!(app.tabs.count(), 1, "premise: closed");
    assert_eq!(
        app.notice().as_deref(),
        Some("closed tab 2 — a draft dropped")
    );

    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().insert_pasted("tab two's");
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabonly!");
    assert_eq!(app.tabs.count(), 1, "premise: closed");
    assert!(
        app.notice()
            .is_some_and(|said| said.ends_with("; a draft dropped")),
        "said: {:?}",
        app.notice()
    );
}

/// **A draft holds the app open, as running work does** (review stop 11,
/// F 1) — `:qa`, and `:q` and `<C-w>q` on the last window, refuse naming the
/// pane whose draft would go; `:qa!` quits over it. They quit in silence,
/// where every other close refused over a draft. The last window is the
/// transcript beside the bar, from either (`App::last_window`), and the bar
/// alone.
#[test]
fn a_draft_holds_the_app_open() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().insert_pasted("unsent words");
    assert!(!app.dispatch_command_for_test("qa"), "`:qa` refused");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("add `!` to quit anyway")
            && said.contains("the draft in pane 2, not sent"),
        "{said}"
    );
    for stand in [PaneKind::Output, PaneKind::Prompt] {
        app.set_focus(stand);
        assert!(
            !app.dispatch_command_for_test("q"),
            "{stand:?}: `:q` refused"
        );
        assert!(
            !app.window_command_for_test(crate::keymap::WindowCmd::Quit),
            "{stand:?}: `<C-w>q` refused"
        );
        assert_eq!(app.workspace.pane_count(), 2, "{stand:?}: nothing closed");
        assert!(
            app.notice()
                .is_some_and(|said| said.contains("the draft in pane 2, not sent")),
            "{stand:?}: {:?}",
            app.notice()
        );
    }
    // The prompt alone: `:close` keeps the bar.
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.workspace.pane_count(), 1, "premise: the prompt alone");
    assert!(!app.dispatch_command_for_test("q"), "`:q` refused");
    assert!(
        !app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "`<C-w>q` refused"
    );
    assert!(
        app.notice()
            .is_some_and(|said| said.contains("the draft in pane 1, not sent")),
        "{:?}",
        app.notice()
    );
    assert!(app.dispatch_command_for_test("qa!"), "`:qa!` quits over it");
}

// ---- `:aim N`, the manual fallback (Steven, 10-26) ------------------------

/// Session A in no pane, B in the only transcript, standing in the prompt:
/// the app, A and B.
fn a_in_no_pane() -> (App, BufferId, BufferId) {
    let mut app = test_app();
    // A turn in A, or `:new` reuses the blank session it is.
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    let a = app.current;
    app.set_focus(PaneKind::Output);
    app.new_session_command();
    let b = app.current;
    assert_ne!(a, b, "premise: two sessions");
    app.set_focus(PaneKind::Prompt);
    assert!(
        app.panes_on(a, Over::ThisTab).next().is_none(),
        "premise: A in no pane"
    );
    (app, a, b)
}

/// **`:aim N` aims the prompt at session N, shown or not** — nothing moves
/// on screen; the title names the aim no pane here shows, and the notice
/// says where it goes. Bare, it says so again.
#[test]
fn aim_points_the_prompt_at_a_session_no_pane_shows() {
    let (mut app, a, _) = a_in_no_pane();
    let panes = app.workspace.pane_ids();
    app.dispatch_command_for_test(&format!("aim {}", a.number()));
    assert_eq!(app.prompt_target(), a, "aimed");
    assert_eq!(app.workspace.pane_ids(), panes, "nothing moved on screen");
    let handle = app.session_of(a).unwrap().conversation.episode().handle();
    assert_eq!(prompt_title(&app), format!(" prompt → {handle} "));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("the prompt now goes to") && said.ends_with("— in no pane here"),
        "{said}"
    );
    app.dispatch_command_for_test("aim");
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with("the prompt goes to"), "bare: {said}");
}

/// **At a session shown here, `:aim` says so** — the notice's "shown
/// here" was unpinned, so it could say "in no pane" and pass (review stop
/// 11, F M-G).
#[test]
fn aim_at_a_session_shown_here_says_so() {
    let (mut app, _, b) = a_in_no_pane();
    app.dispatch_command_for_test(&format!("aim {}", b.number()));
    let said = app.notice().unwrap_or_default();
    assert!(said.ends_with("— shown here"), "{said}");
}

/// **`:aim` Tab-completes the sessions it takes** (Steven, 10-26: "Make sure
/// we can tab complete too") — each by the handle its border shows, as `:b`
/// completes, and the line `<Tab>` fills aims there.
#[test]
fn aim_tab_completes_the_sessions_it_takes() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let (mut app, a, b) = a_in_no_pane();
    let handle =
        |app: &App, s: BufferId| app.session_of(s).unwrap().conversation.episode().handle();
    let (ha, hb) = (handle(&app, a), handle(&app, b));
    type_command(&mut app, "aim ");
    app.command_input(k(KeyCode::Tab));
    let offered = (app.completion_menu()).map_or(Vec::new(), |(c, _)| c.to_vec());
    assert!(
        offered.contains(&ha) && offered.contains(&hb),
        "both offered: {offered:?}"
    );
    let to_a = format!("aim {ha}");
    for _ in 0..offered.len() {
        if app.cmdline.line() == Some(to_a.as_str()) {
            break;
        }
        app.command_input(k(KeyCode::Tab));
    }
    let line = app.command_input(k(KeyCode::Enter));
    assert_eq!(line.as_deref(), Some(to_a.as_str()), "Tab filled A's");
    app.dispatch_command_for_test(&to_a);
    assert_eq!(app.prompt_target(), a, "and the line aims there");
}

/// **With two prompts, `:aim` and `:Send` say which pane's** — the one you
/// write in took the act, and nothing said which (review stop 11, F 2); and
/// `:aim` at a session only another tab shows says that tab (F 12).
#[tokio::test]
async fn aim_and_send_name_the_pane_while_two_could_be_it() {
    let (mut app, (at_a, _), (a, _), (_, second)) = two_prompts();
    let n = app.corner_of(second).expect("its corner");
    app.dispatch_command_for_test(&format!("aim {}", a.number()));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("the prompt in pane {n} now goes to")),
        "{said}"
    );
    app.prompt_mut().insert_pasted("hello");
    app.focus_pane(at_a);
    app.dispatch_command_for_test(&format!("Send b{}", a.number()));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("pane {n}'s draft sent to")),
        "{said}"
    );

    let (mut app, a, _) = a_in_no_pane();
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test(&format!("aim {}", a.number()));
    let said = app.notice().unwrap_or_default();
    assert!(said.ends_with("in no pane here — tab 2 shows it"), "{said}");
}

/// **It aims the prompt you write in, alone** — the other prompt window
/// keeps its own.
#[test]
fn aim_moves_only_the_prompt_you_write_in() {
    let (mut app, _, (a, b), (first, second)) = two_prompts();
    app.focus_pane(first);
    app.dispatch_command_for_test(&format!("aim {}", a.number()));
    assert_eq!(aim_of(&app, first), a);
    assert_eq!(aim_of(&app, second), b, "the other's own");
}

/// **An aim set at a session no pane shows stays through a close of
/// something else** — the reconcile re-aims only when a pane here that held
/// the aim closed (`reconcile_pane_state`); it took the `:aim` back at the
/// next close of anything — here another tab's, from this tab's prompt — to
/// this tab's transcript, announced as if it helped. The pane that DID show
/// it closing still re-aims (the tests of `reaim_prompts_whose_pane_went`).
/// Standing in a transcript is a re-aim of its own, so the prompt stays
/// where it is.
///
/// A closed tab's windows go to `Tabs::dropped`, not the reconcile's closed
/// list, so this never reaches `reaim_prompts_whose_pane_went`: it holds the
/// old bug only with both its halves together (review stop 12, T5 F7).
#[test]
fn an_aim_at_a_session_in_no_pane_stays_through_another_close() {
    let (mut app, a, _) = a_in_no_pane();
    app.dispatch_command_for_test(&format!("aim {}", a.number()));
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.prompt_target(), a, "premise: still aimed at A");
    app.notices.clear();
    app.dispatch_command_for_test("tabclose 2");
    assert_eq!(app.tabs.count(), 1, "premise: closed");
    assert_eq!(app.prompt_target(), a, "kept");
    assert!(
        !(app.notices.iter()).any(|(_, said)| said.contains("now aiming at")),
        "no re-aim announced: {:?}",
        app.notices
    );
}

/// **And through a close HERE of another session's pane** — the re-aim asks
/// whether the pane that closed was the aim's (`reaim_prompts_whose_pane_
/// went`'s `gone`), not only whether something closed: aimed at A (in no
/// pane) while standing in C's window, `:q` there closes C, and the prompt
/// was re-aimed at B, "now aiming at …". The focus then lands in B's
/// window and aims there, as standing in a transcript does — silently.
/// Named for that: it was "…stays through a close here", and the aim does
/// not stay (review stop 12, T5 F7).
#[test]
fn a_close_here_of_another_sessions_pane_says_no_reaim() {
    let (mut app, a, b) = a_in_no_pane();
    // A turn in B, or `:new` beside it reuses the blank session it is.
    app.conversation_mut().begin_turn("second");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    app.settle();
    app.dispatch_command_for_test(&format!("aim {}", a.number()));
    assert_eq!(
        app.prompt_target(),
        a,
        "premise: aimed at A from C's window"
    );
    app.notices.clear();
    app.dispatch_command_for_test("q");
    assert!(
        !(app.notices.iter()).any(|(_, said)| said.contains("now aiming at")),
        "no re-aim announced: {:?}",
        app.notices
    );
    assert_eq!(app.prompt_target(), b, "aimed where you stand, B's window");
}

/// **A name that is no session is refused, naming it** — a number nothing
/// has, a page `:ls!` numbers; the aim unchanged.
#[test]
fn aim_refuses_what_is_no_session() {
    let (mut app, _, b) = a_in_no_pane();
    app.dispatch_command_for_test("aim 999");
    assert!(
        app.notice()
            .is_some_and(|said| said.starts_with("`:aim`: ")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("help");
    let manual = (app.buffers.pages())
        .next()
        .map(|page| page.id())
        .expect("the manual's page");
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test(&format!("aim {}", manual.number()));
    assert!(
        app.notice()
            .is_some_and(|said| said.contains("not a session")),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.prompt_target(), b, "unchanged");
}
