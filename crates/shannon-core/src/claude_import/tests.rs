use super::*;
use serde_json::json;

fn user(uuid: &str, parent: Option<&str>, content: Value, ts: &str) -> Value {
    json!({
        "type": "user", "uuid": uuid, "parentUuid": parent, "isSidechain": false,
        "timestamp": ts, "cwd": "/work/proj",
        "message": {"role": "user", "content": content}
    })
}

fn assistant(uuid: &str, parent: &str, id: &str, content: Value, ts: &str) -> Value {
    json!({
        "type": "assistant", "uuid": uuid, "parentUuid": parent, "isSidechain": false,
        "timestamp": ts, "cwd": "/work/proj",
        "message": {"id": id, "model": "claude-opus-5-5", "role": "assistant", "content": content}
    })
}

/// A conversation in every shape the probe found (10-04): a compaction, a
/// reply written block by block with its thinking, two parallel calls whose
/// first result a single-parent walk drops, a result with an image, Claude
/// Code's own meta message and an attachment, a call never answered, a
/// subagent's entry written last, and a line that is not JSON.
fn fixture(dir: &Path) -> PathBuf {
    let mut boundary = json!({
        "type": "system", "subtype": "compact_boundary", "uuid": "boundary",
        "parentUuid": null, "logicalParentUuid": "a1", "timestamp": "03", "cwd": "/work/proj"
    });
    boundary["isSidechain"] = json!(false);
    let mut summary = user(
        "summary",
        Some("boundary"),
        json!("Summary of before."),
        "04",
    );
    summary["isCompactSummary"] = json!(true);
    let mut meta = user("meta", Some("r2"), json!("Caveat: generated locally"), "11");
    meta["isMeta"] = json!(true);
    let mut side = assistant(
        "side",
        "a4",
        "M9",
        json!([{"type": "text", "text": "a subagent"}]),
        "15",
    );
    side["isSidechain"] = json!(true);
    let lines = [
        user("u1", None, json!("old question"), "01"),
        assistant(
            "a1",
            "u1",
            "M1",
            json!([{"type": "text", "text": "old answer"}]),
            "02",
        ),
        boundary,
        summary,
        user("u2", Some("summary"), json!("now do two things"), "05"),
        assistant(
            "a2t",
            "u2",
            "M2",
            json!([{"type": "thinking", "thinking": "hm"}]),
            "06",
        ),
        assistant(
            "a2a",
            "a2t",
            "M2",
            json!([{"type": "tool_use", "id": "T1", "name": "Bash", "input": {"command": "ls"}}]),
            "07",
        ),
        assistant(
            "a2b",
            "a2a",
            "M2",
            json!([{"type": "tool_use", "id": "T2", "name": "Read", "input": {"file_path": "/x"}}]),
            "08",
        ),
        // Parented on its OWN call's entry, off the walk from the leaf.
        user(
            "r1",
            Some("a2a"),
            json!([{"type": "tool_result", "tool_use_id": "T1", "content": "listing"}]),
            "09",
        ),
        user(
            "r2",
            Some("a2b"),
            json!([{"type": "tool_result", "tool_use_id": "T2",
                    "content": [{"type": "text", "text": "file"}, {"type": "image", "source": {}}]}]),
            "10",
        ),
        meta,
        json!({"type": "attachment", "uuid": "att", "parentUuid": "meta", "isSidechain": false,
               "timestamp": "12", "attachment": {"type": "todo"}}),
        assistant(
            "a3",
            "att",
            "M3",
            json!([{"type": "text", "text": "both done"}]),
            "13",
        ),
        assistant(
            "a4",
            "a3",
            "M4",
            json!([{"type": "tool_use", "id": "T3", "name": "Bash", "input": {"command": "pwd"}}]),
            "14",
        ),
        side,
        json!({"type": "ai-title", "aiTitle": "Two things", "sessionId": "s"}),
    ];
    let mut text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    text.push_str("not json\n");
    let path = dir.join("2b3c4d5e.jsonl");
    std::fs::write(&path, text).unwrap();
    path
}

fn ids(blocks: &[Block]) -> Vec<&str> {
    blocks
        .iter()
        .map(|b| match b {
            Block::ToolUse { id, .. } => id.as_str(),
            Block::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
            Block::Text { .. } => "text",
            Block::Context { .. } => "context",
            Block::Thinking { .. } => "thinking",
            Block::RedactedThinking { .. } => "redacted_thinking",
        })
        .collect()
}

/// **What `claude --resume` holds**: from the latest compaction's summary,
/// the summary and the next prompt one prompt, a reply's blocks one message,
/// both parallel calls answered in order — the one a single-parent walk
/// drops recovered — a call with no result answered as such, and what was
/// not brought in counted.
#[test]
fn the_resume_path_reads_as_turns() {
    let dir = tempfile::tempdir().unwrap();
    let read = read(&fixture(dir.path()), Scope::Resume).unwrap();
    assert_eq!(read.id, "2b3c4d5e");
    assert_eq!(read.cwd.as_deref(), Some("/work/proj"));
    assert_eq!(
        read.earlier,
        Some(Earlier::Compacted),
        "it goes back past a compaction"
    );
    assert_eq!(read.turns.len(), 1, "{:#?}", read.turns);
    let turn = &read.turns[0];
    assert_eq!(turn.prompt, "Summary of before.\n\nnow do two things");
    assert_eq!(turn.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(turn.rounds.len(), 2, "{:#?}", turn.rounds);
    let (first, second) = (&turn.rounds[0], &turn.rounds[1]);
    assert_eq!(ids(&first.calls), ["T1", "T2"], "one message, both calls");
    assert_eq!(ids(&first.results), ["T1", "T2"], "T1 recovered, in order");
    assert!(matches!(
        &first.results[1],
        Block::ToolResult { content, .. } if content == &format!("file\n{IMAGE}")
    ));
    assert_eq!(second.text, "both done");
    assert_eq!(ids(&second.calls), ["T3"]);
    assert!(matches!(
        &second.results[..],
        [Block::ToolResult { content, is_error: true, .. }] if content == UNANSWERED
    ));
    assert_eq!(turn.reply, "");
    assert_eq!(
        read.skipped,
        Skipped {
            thinking: 1,
            images: 1,
            meta: 1,
            attachments: 1,
            system: 1,
            unanswered: 1,
            unparsable: 1,
            ..Skipped::default()
        }
    );
}

/// **A session Shannon seeded opens on its own handoff**, which is left out
/// and counted: it is a copy, as one line of text, of turns Shannon holds as
/// turns — imported, it was a wall of `[Assistant ran Edit: {json}]` under a
/// prompt saying "earlier" (Steven, 10-05). Only there: the same words later
/// in a conversation are the model's, and stay.
#[test]
fn shannons_own_handoff_is_left_out() {
    use crate::backend::claude_code::{HANDOFF, turn_input};
    let dir = tempfile::tempdir().unwrap();
    // The line Shannon really sends, not a copy of it (review, Reviewer C):
    // a switch with earlier turns and no session to resume.
    let sent = turn_input(
        &[
            crate::Message::user("hi"),
            crate::Message::assistant("done"),
            crate::Message::user("carry on"),
        ],
        false,
    )
    .expect("a turn to send");
    let first: Value = serde_json::from_str(sent.lines().next().expect("a line")).unwrap();
    assert_eq!(first["type"], "assistant", "premise: the handoff line");
    let mut seeded = assistant("h", "", "H0", first["message"]["content"].clone(), "01");
    seeded["parentUuid"] = Value::Null;
    let lines = [
        seeded,
        user("u1", Some("h"), json!("carry on"), "02"),
        assistant(
            "a1",
            "u1",
            "M1",
            json!([{"type": "text", "text": format!("{HANDOFF} is how it began")}]),
            "03",
        ),
    ];
    let path = dir.path().join("seeded.jsonl");
    std::fs::write(
        &path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    let read = read(&path, Scope::Everything).unwrap();
    assert_eq!(read.turns.len(), 1, "{:#?}", read.turns);
    assert_eq!(
        read.turns[0].prompt, "carry on",
        "no turn under \"earlier\""
    );
    assert_eq!(read.turns[0].reply, format!("{HANDOFF} is how it began"));
    assert_eq!(
        read.skipped,
        Skipped {
            handoffs: 1,
            ..Skipped::default()
        }
    );
}

/// **Everything**: across the compaction to the first prompt, the summary
/// left out — it is a second copy of what was read.
#[test]
fn everything_reads_across_compactions() {
    let dir = tempfile::tempdir().unwrap();
    let read = read(&fixture(dir.path()), Scope::Everything).unwrap();
    assert_eq!(read.earlier, None);
    assert_eq!(read.turns.len(), 2, "{:#?}", read.turns);
    assert_eq!(read.turns[0].prompt, "old question");
    assert_eq!(read.turns[0].reply, "old answer");
    assert_eq!(read.turns[1].prompt, "now do two things");
    assert_eq!(read.skipped.summaries, 1);
}

/// **Lineage**: across the compaction to the first prompt, as `Everything`,
/// the summary KEPT as the turn it opened — the conversation as `:graph`
/// shows it — and every turn named by the entry that opened it. So what an
/// import read as its first turn is found whole in it, by that entry, after
/// Claude Code has compacted again and `Resume` starts somewhere else.
#[test]
fn lineage_keeps_each_compaction_where_it_happened() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let lineage = read(&path, Scope::Lineage).unwrap();
    let marks: Vec<(&str, Option<&str>, bool)> = lineage
        .turns
        .iter()
        .map(|t| (t.prompt.as_str(), t.uuid.as_deref(), t.summary))
        .collect();
    assert_eq!(
        marks,
        [
            ("old question", Some("u1"), false),
            (
                "Summary of before.\n\nnow do two things",
                Some("summary"),
                true
            ),
        ]
    );
    assert_eq!(lineage.skipped.summaries, 0, "kept, not skipped");
    assert_eq!(lineage.earlier, None);
    let resume = read(&path, Scope::Resume).unwrap();
    assert_eq!(
        resume.turns[0], lineage.turns[1],
        "the import's first turn, whole, in the lineage"
    );
}

/// A compaction as newer Claude Code files have it (Steven's files,
/// 10-17): the messages it keeps (`preservedSegment`) stay where they were,
/// before the boundary, and the boundary's `logicalParentUuid` names the
/// entry AFTER the summary — `tail`, which the walk reaches first. With
/// `resumed`, the file begins at the boundary, as one `claude --resume`
/// wrote: what came before is another file's.
fn kept_messages_fixture(dir: &Path, resumed: bool) -> PathBuf {
    // As measured on Steven's files (10-17, structure only): the first
    // message kept is an assistant's, the tail an entry after the summary,
    // which the boundary's `logicalParentUuid` names.
    let boundary = json!({
        "type": "system", "subtype": "compact_boundary", "uuid": "boundary",
        "parentUuid": null, "logicalParentUuid": "tail", "isSidechain": false,
        "timestamp": "05", "cwd": "/work/proj",
        "compactMetadata": {
            "preservedSegment": {"headUuid": "kept-a", "anchorUuid": "summary", "tailUuid": "tail"},
            "preservedMessages": {"anchorUuid": "summary", "uuids": ["kept-a"],
                                  "allUuids": ["kept-a", "tail"]}
        }
    });
    let mut summary = user(
        "summary",
        Some("boundary"),
        json!("Summary of before."),
        "06",
    );
    summary["isCompactSummary"] = json!(true);
    let kept = |parent: &str| {
        assistant(
            "kept-a",
            parent,
            "M2",
            json!([{"type": "text", "text": "a kept answer"}]),
            "04",
        )
    };
    // The tail follows the summary; in a file a resume wrote, the kept
    // messages come between, on the chain.
    let tail = |parent: &str| {
        json!({"type": "attachment", "uuid": "tail", "parentUuid": parent,
               "isSidechain": false, "timestamp": "07", "attachment": {"type": "todo"}})
    };
    let before = [
        user("u1", None, json!("old question"), "01"),
        assistant(
            "a1",
            "u1",
            "M1",
            json!([{"type": "text", "text": "old answer"}]),
            "02",
        ),
        user("kept-q", Some("a1"), json!("a kept question"), "03"),
        kept("kept-q"),
    ];
    let went_on = [
        user("u2", Some("tail"), json!("now go on"), "08"),
        assistant(
            "a2",
            "u2",
            "M3",
            json!([{"type": "text", "text": "going"}]),
            "09",
        ),
    ];
    let lines: Vec<Value> = match resumed {
        true => [boundary, summary, kept("summary"), tail("kept-a")]
            .into_iter()
            .chain(went_on)
            .collect(),
        false => before
            .into_iter()
            .chain([boundary, summary, tail("summary")])
            .chain(went_on)
            .collect(),
    };
    let path = dir.join("1a2b3c4d.jsonl");
    std::fs::write(
        &path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    path
}

/// **A compaction that kept its last messages is crossed** — where the file
/// stood when Claude Code compacted. Followed, its `logicalParentUuid` loops
/// back onto the walk: one of Steven's files read as one compaction of 29,
/// another as one of 47 (10-17), and no import's first turn could be found
/// in them.
#[test]
fn a_compaction_that_kept_messages_is_crossed() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let lineage = read(&path, Scope::Lineage).unwrap();
    let prompts: Vec<(&str, bool)> = lineage
        .turns
        .iter()
        .map(|t| (t.prompt.as_str(), t.summary))
        .collect();
    assert_eq!(
        prompts,
        [
            ("old question", false),
            ("a kept question", false),
            ("Summary of before.\n\nnow go on", true),
        ]
    );
    assert_eq!(lineage.earlier, None, "back to the first prompt");
    let everything = read(&path, Scope::Everything).unwrap();
    assert_eq!(everything.turns.len(), 3, "{:#?}", everything.turns);
    assert_eq!(everything.turns[0].prompt, "old question");
    assert_eq!(everything.skipped.summaries, 1);
    let resume = read(&path, Scope::Resume).unwrap();
    assert_eq!(resume.earlier, Some(Earlier::Compacted));
    assert_eq!(resume.turns.len(), 2, "{:#?}", resume.turns);

    // A boundary naming no entry at all is still a compaction, crossed so.
    let text = std::fs::read_to_string(&path).unwrap();
    let unnamed = text.replace(r#""logicalParentUuid":"tail","#, "");
    assert!(!unnamed.contains("logicalParentUuid"), "premise: unnamed");
    std::fs::write(&path, unnamed).unwrap();
    assert_eq!(read(&path, Scope::Lineage).unwrap().turns, lineage.turns);
}

/// **A conversation `claude --resume` made says its history is elsewhere** —
/// it begins at a compaction copied from the file it was resumed from, with
/// nothing before it here; `--all` cannot bring what the file does not hold,
/// so a reading that reaches it says so rather than "everything".
#[test]
fn a_resumed_conversation_says_its_history_is_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), true);
    for scope in [Scope::Lineage, Scope::Everything] {
        let read = read(&path, scope).unwrap();
        assert_eq!(read.earlier, Some(Earlier::Elsewhere), "{scope:?}");
    }
    let lineage = read(&path, Scope::Lineage).unwrap();
    assert_eq!(lineage.turns.len(), 2, "{:#?}", lineage.turns);
    assert!(lineage.turns[0].summary);
    assert_eq!(
        read(&path, Scope::Resume).unwrap().earlier,
        Some(Earlier::Compacted),
        "a resume stops at the compaction wherever its history is"
    );
}

/// **A resume reading holds what the compaction kept, after its summary** —
/// what `claude --resume` hands the model, and what the file a resume writes
/// holds (Steven, 10-17: splice them, "The cost is worth it"). Shannon's copy
/// had the summary and what followed only: the 14 messages the measured
/// file's latest compaction kept were not in it. Read from the original file or
/// from the one the resume wrote, the same turns; and the question after
/// the summary, a turn of its own here, is found in the lineage's summary
/// turn, which took it in.
#[test]
fn a_resume_reading_holds_what_the_compaction_kept() {
    let dir = tempfile::tempdir().unwrap();
    let original = read(&kept_messages_fixture(dir.path(), false), Scope::Resume).unwrap();
    let parts: Vec<(&str, &str, bool)> = original
        .turns
        .iter()
        .map(|t| (t.prompt.as_str(), t.reply.as_str(), t.summary))
        .collect();
    assert_eq!(
        parts,
        [
            ("Summary of before.", "a kept answer", true),
            ("now go on", "going", false),
        ]
    );
    let other = tempfile::tempdir().unwrap();
    let resumed = read(&kept_messages_fixture(other.path(), true), Scope::Resume).unwrap();
    assert_eq!(resumed.turns, original.turns, "as the resume's own file");

    let lineage = read(&kept_messages_fixture(dir.path(), false), Scope::Lineage).unwrap();
    let last = original.turns.last().and_then(|t| t.uuid.clone()).unwrap();
    assert_eq!(last, "u2", "premise: a turn of its own here");
    assert!(
        lineage
            .turns
            .iter()
            .all(|t| t.uuid.as_deref() != Some("u2")),
        "premise: no turn of its own there"
    );
    let holding: Vec<bool> = lineage.turns.iter().map(|t| t.holds(&last)).collect();
    assert_eq!(
        holding,
        [false, false, true],
        "the summary's turn took it in"
    );
}

/// **A reading ends before the turn named** — the conversation as it stood
/// at the turn before (Steven, 10-17: `<CR>` on a turn "should import up to
/// that turn"): `Resume` from the compaction latest THEN, not now; a turn a
/// summary opens ends the reading at its compaction's boundary, the
/// compaction going with it; and what was not brought in is counted of what
/// came in, not of the whole file.
#[test]
fn a_reading_ends_before_the_turn_named() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let mut text = std::fs::read_to_string(&path).unwrap();
    for line in [
        user("u3", Some("a2"), json!("and then more"), "10"),
        assistant(
            "a3",
            "u3",
            "M4",
            json!([{"type": "text", "text": "more"}]),
            "11",
        ),
    ] {
        text.push_str(&format!("{line}\n"));
    }
    std::fs::write(&path, text).unwrap();
    let lineage = read(&path, Scope::Lineage).unwrap();
    let opened: Vec<Option<&str>> = lineage.turns.iter().map(|t| t.uuid.as_deref()).collect();
    assert_eq!(
        opened,
        [Some("u1"), Some("kept-q"), Some("summary"), Some("u3")],
        "premise: four turns, the third a compaction's"
    );
    let prompts =
        |read: &Imported| -> Vec<String> { read.turns.iter().map(|t| t.prompt.clone()).collect() };

    let resume = read_until(&path, Scope::Resume, Some(&Until::Before("u3".into()))).unwrap();
    assert_eq!(prompts(&resume), ["Summary of before.", "now go on"]);
    assert_eq!(resume.earlier, Some(Earlier::Compacted));
    let last = resume.turns.last().and_then(|t| t.uuid.clone()).unwrap();
    assert!(
        lineage.turns[2].holds(&last),
        "ends on the turn before, found as the lineage names it: {last}"
    );
    let lineage_cut = read_until(&path, Scope::Lineage, Some(&Until::Before("u3".into()))).unwrap();
    assert_eq!(lineage_cut.turns, lineage.turns[..3]);

    let before_compaction =
        read_until(&path, Scope::Resume, Some(&Until::Before("summary".into()))).unwrap();
    assert_eq!(
        prompts(&before_compaction),
        ["old question", "a kept question"]
    );
    assert_eq!(before_compaction.earlier, None, "no compaction before it");
    assert_eq!(
        before_compaction.skipped.system, 0,
        "the boundary went with the turn it opens"
    );
    let everything_cut = read_until(
        &path,
        Scope::Everything,
        Some(&Until::Before("summary".into())),
    )
    .unwrap();
    assert_eq!(
        everything_cut.skipped.summaries, 0,
        "counted of what came in"
    );

    let first = read_until(&path, Scope::Resume, Some(&Until::Before("kept-q".into()))).unwrap();
    assert_eq!(prompts(&first), ["old question"]);
    let gone =
        read_until(&path, Scope::Resume, Some(&Until::Before("rewound".into()))).unwrap_err();
    assert!(gone.contains("no longer holds that turn"), "{gone}");

    // **After a reading's leaf**: what that reading held, the file grown
    // since — `<CR>` on the last turn shown copied every turn after it
    // (review stop 4, A 3).
    assert_eq!(
        lineage.leaf.as_deref(),
        Some("a3"),
        "the newest on its path"
    );
    let leaf = Until::After(lineage.leaf.clone().unwrap());
    append(
        &path,
        &user("u4", Some("a3"), json!("later"), "12").to_string(),
    );
    assert_eq!(
        read(&path, Scope::Lineage).unwrap().turns.len(),
        5,
        "premise: grown"
    );
    let as_shown = read_until(&path, Scope::Lineage, Some(&leaf)).unwrap();
    assert_eq!(
        as_shown.turns, lineage.turns,
        "what was shown, not what came since"
    );
    let resume_shown = read_until(&path, Scope::Resume, Some(&leaf)).unwrap();
    assert_eq!(
        prompts(&resume_shown),
        ["Summary of before.", "now go on", "and then more"]
    );
}

/// **An old import is found by what it holds** — its first turn (a summary
/// of the compaction latest then) and the turns after it, as a run of the
/// conversation's lineage. A session whose next turn is its own (Shannon's)
/// ends the run there; one matching nothing is `None`; a repeat is the
/// longer run's.
#[test]
fn an_old_import_is_found_by_its_turns() {
    let turn = |prompt: &str, reply: &str| ImportedTurn {
        prompt: prompt.into(),
        reply: reply.into(),
        ..ImportedTurn::default()
    };
    let lineage = [
        turn("first", "one"),
        turn("Summary.\n\ngo on", "going"),
        turn("and then", "done"),
        turn("later in Claude Code", "yes"),
    ];
    let said = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(p, r)| (p.to_string(), r.to_string()))
            .collect()
    };
    let same = |a: &str, b: &str| a == b;
    let imported = said(&[
        ("Summary.\n\ngo on", "going"),
        ("and then", "done"),
        ("a Shannon turn", "after"),
    ]);
    assert_eq!(
        find_run(&lineage, &imported, same),
        Some((1, 2)),
        "two came in, from the summary; the third is Shannon's"
    );
    assert_eq!(find_run(&lineage, &said(&[("unknown", "x")]), same), None);
    assert_eq!(find_run(&lineage, &[], same), None);
    let repeated = [turn("go", "ok"), turn("go", "ok"), turn("more", "fine")];
    assert_eq!(
        find_run(&repeated, &said(&[("go", "ok"), ("more", "fine")]), same),
        Some((1, 2)),
        "the longer run, where the turns after it match too"
    );
}

/// **A result too large is cut on a character boundary, and counted** — a
/// result of two-byte characters cut at a byte limit must still be text.
#[test]
fn a_large_result_is_cut_on_a_character() {
    let dir = tempfile::tempdir().unwrap();
    // One ASCII byte first, so the cap falls INSIDE a two-byte character
    // (all-`é`, every even offset is a boundary and the cut never searches).
    let big = format!("x{}", "é".repeat(RESULT_CAP));
    let lines = [
        user("u", None, json!("go"), "1"),
        assistant(
            "a",
            "u",
            "M",
            json!([{"type": "tool_use", "id": "T", "name": "Bash", "input": {}}]),
            "2",
        ),
        user(
            "r",
            Some("a"),
            json!([{"type": "tool_result", "tool_use_id": "T", "content": big}]),
            "3",
        ),
    ];
    let path = dir.path().join("s.jsonl");
    std::fs::write(
        &path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    let read = read(&path, Scope::Resume).unwrap();
    let Block::ToolResult { content, .. } = &read.turns[0].rounds[0].results[0] else {
        panic!("a result");
    };
    // Held whole, for its reader to cut once scrubbed (review stop 4, E 4),
    // and counted.
    assert_eq!(content, &big);
    assert_eq!(read.skipped.cut, 1);
    let cut = cut_result(content.clone());
    assert!(cut.len() < RESULT_CAP + 100, "{}", cut.len());
    assert!(cut.ends_with("cut on import]"));
    assert_eq!(cut_result("short".into()), "short");
}

/// **Text beside a result is about the result** — Claude Code writes
/// "[Request interrupted by user for tool use]" there — so it stays with it
/// rather than opening a new turn in the middle of a round.
#[test]
fn text_beside_a_result_stays_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let lines = [
        user("u", None, json!("go"), "1"),
        assistant(
            "a",
            "u",
            "M",
            json!([{"type": "tool_use", "id": "T", "name": "Bash", "input": {}}]),
            "2",
        ),
        user(
            "r",
            Some("a"),
            json!([
                {"type": "tool_result", "tool_use_id": "T", "content": "partial", "is_error": true},
                {"type": "text", "text": "[Request interrupted by user for tool use]"}
            ]),
            "3",
        ),
    ];
    let path = dir.path().join("s.jsonl");
    std::fs::write(
        &path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    let read = read(&path, Scope::Resume).unwrap();
    assert_eq!(read.turns.len(), 1, "{:#?}", read.turns);
    assert!(matches!(
        &read.turns[0].rounds[0].results[..],
        [Block::ToolResult { content, is_error: true, .. }]
            if content == "partial\n\n[Request interrupted by user for tool use]"
    ));
}

/// **A listing names each conversation as Claude Code does** — a title the
/// user set over one it generated over the last prompt — newest first; a
/// directory that is not there is no conversations.
#[test]
fn a_listing_names_and_orders_conversations() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.jsonl");
    std::fs::write(
        &old,
        "{\"type\":\"last-prompt\",\"lastPrompt\":\"do   it\\nnow\"}\n",
    )
    .unwrap();
    let newer = dir.path().join("new.jsonl");
    std::fs::write(
        &newer,
        [
            json!({"type": "ai-title", "aiTitle": "Generated"}),
            json!({"type": "custom-title", "customTitle": "Mine"}),
            json!({"type": "ai-title", "aiTitle": "Generated later"}),
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect::<String>(),
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a conversation").unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(past)
        .unwrap();
    let found = list(dir.path()).unwrap();
    let named: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|f| (f.id.as_str(), f.title.as_deref()))
        .collect();
    assert_eq!(named, [("new", Some("Mine")), ("old", Some("do it now"))]);
    assert_eq!(list(&dir.path().join("absent")), Ok(Vec::new()));
}

/// **A listing names the resumes a conversation began** — each
/// `continued-in` record Claude Code leaves in the file a `claude --resume`
/// moved on from, once each, in their order (Steven, 10-17: CC's own resumes
/// as lineage) — from the same tail as the title, so one written further
/// back than it is not seen.
#[test]
fn a_listing_names_the_resumes_a_conversation_began() {
    let dir = tempfile::tempdir().unwrap();
    let resumed =
        |id: &str| json!({"type": "continued-in", "continuedInSessionId": id, "sessionId": "src"});
    let src = dir.path().join("src.jsonl");
    std::fs::write(
        &src,
        [
            json!({"type": "custom-title", "customTitle": "Source"}),
            resumed("n1"),
            resumed("n2"),
            resumed("n1"),
            json!({"type": "continued-in"}),
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect::<String>(),
    )
    .unwrap();
    // A resume recorded, then the file grown past the tail by a long line.
    let buried = dir.path().join("buried.jsonl");
    let filler = json!({"type": "user", "message": {"content": "x".repeat(TITLE_TAIL as usize)}});
    std::fs::write(&buried, format!("{}\n{filler}\n", resumed("lost"))).unwrap();
    let found = list(dir.path()).unwrap();
    let of = |id: &str| -> Vec<String> {
        found
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.resumed_in.clone())
            .unwrap()
    };
    assert_eq!(of("src"), ["n1", "n2"]);
    assert!(of("buried").is_empty(), "past the tail: unseen");

    // A tail naming more than a real one would: the first `RESUMES_KEPT`
    // (review stop 5, B 4 — every listing parsed them all, O(k²)).
    let many: String = (0..RESUMES_KEPT + 10)
        .map(|n| format!("{}\n", resumed(&format!("r{n}"))))
        .collect();
    std::fs::write(dir.path().join("many.jsonl"), many).unwrap();
    let found = list(dir.path()).unwrap();
    let many = found.iter().find(|f| f.id == "many").unwrap();
    assert_eq!(many.resumed_in.len(), RESUMES_KEPT);
    assert_eq!(many.resumed_in[0], "r0", "the first, in order");
}

/// **Claude Code's directory for a path**: every character other than a
/// letter or digit a `-` (its `.claude` directory reference) — and none for
/// a path that reference does not settle, though a directory a guess would
/// have named is there.
#[test]
fn the_project_directory_is_named_as_claude_code_names_it() {
    let projects = tempfile::tempdir().unwrap();
    let plain = projects.path().join("-Users-me-my-proj");
    std::fs::create_dir(&plain).unwrap();
    assert_eq!(
        project_dir(projects.path(), Path::new("/Users/me/my.proj")),
        Some(plain)
    );
    assert_eq!(project_dir(projects.path(), Path::new("/nowhere")), None);
    for guess in ["-Users-me-x-", "-Users-me-x--", "-Users-me-caf-"] {
        std::fs::create_dir(projects.path().join(guess)).unwrap();
    }
    for unsettled in ["/Users/me/x\u{1F600}", "/Users/me/caf\u{e9}"] {
        assert_eq!(
            project_dir(projects.path(), Path::new(unsettled)),
            None,
            "{unsettled}: not ASCII, so not named — a guess would find another's"
        );
    }
    // At the documented cut a name is still whole; one past it is cut and
    // hashed as the docs do not give.
    let at = format!("/{}", "a".repeat(CUT_PAST - 1));
    let whole = projects
        .path()
        .join(format!("-{}", "a".repeat(CUT_PAST - 1)));
    std::fs::create_dir(&whole).unwrap();
    assert_eq!(project_dir(projects.path(), Path::new(&at)), Some(whole));
    let past = format!("/{}", "a".repeat(CUT_PAST));
    std::fs::create_dir(projects.path().join(format!("-{}", "a".repeat(CUT_PAST)))).unwrap();
    assert_eq!(
        project_dir(projects.path(), Path::new(&past)),
        None,
        "past the cut: the name is not that directory, and the hash is not known"
    );
}

/// **The memory of a project Claude Code's name is not known for is
/// nowhere**, and `{project}` falls back to Shannon's own name — the same
/// answer as `project_dir`, since it decides what is read and written unasked.
#[test]
fn an_unsettled_name_has_no_claude_code_memory() {
    let home = tempfile::tempdir().unwrap();
    for start in [format!("/{}", "b".repeat(CUT_PAST)), "/w/caf\u{e9}".into()] {
        let start = Path::new(&start);
        assert_eq!(
            auto_memory_dir(home.path(), start),
            None,
            "{}",
            start.display()
        );
        assert_eq!(project_key(start), crate::store::project_name(start));
    }
    assert_eq!(
        project_key(Path::new("/w/plain")),
        "-w-plain",
        "a settled name is Claude Code's"
    );
}

/// **Claude Code's memory directory**: its repository's,
/// so a worktree made by `git worktree add` shares the main one's — and a
/// `.git` file a repository wrote itself, pointing at another's worktree
/// entry, borrows nothing (the back-link must name it).
#[test]
fn claude_codes_memory_is_its_repositorys() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let home = base.join("claude");
    let memory = |root: &Path| {
        let name = directory_name(root).expect("a temporary directory's path is ASCII");
        home.join("projects").join(name).join("memory")
    };

    let main = base.join("main");
    std::fs::create_dir_all(main.join(".git/worktrees/w")).unwrap();
    std::fs::create_dir_all(main.join("src")).unwrap();
    assert_eq!(
        auto_memory_dir(&home, &main.join("src")),
        Some(memory(&main))
    );

    // `git worktree add ../w`: a `.git` file there, and the entry's files.
    let worktree = base.join("w");
    std::fs::create_dir_all(&worktree).unwrap();
    let entry = main.join(".git/worktrees/w");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", entry.display()),
    )
    .unwrap();
    std::fs::write(entry.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        entry.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .unwrap();
    assert_eq!(auto_memory_dir(&home, &worktree), Some(memory(&main)));

    // A repository that points its own `.git` at that entry: its own.
    let forged = base.join("forged");
    std::fs::create_dir_all(&forged).unwrap();
    std::fs::write(
        forged.join(".git"),
        format!("gitdir: {}\n", entry.display()),
    )
    .unwrap();
    assert_eq!(auto_memory_dir(&home, &forged), Some(memory(&forged)));

    // One that forges a whole entry of its own — its back-link true, its
    // `commondir` naming the main repository's: not under that one's
    // `worktrees/`, so its own.
    let crafted = base.join("crafted");
    let fake = crafted.join("fake");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::write(
        crafted.join(".git"),
        format!("gitdir: {}\n", fake.display()),
    )
    .unwrap();
    std::fs::write(
        fake.join("commondir"),
        format!("{}\n", main.join(".git").display()),
    )
    .unwrap();
    std::fs::write(
        fake.join("gitdir"),
        format!("{}\n", crafted.join(".git").display()),
    )
    .unwrap();
    assert_eq!(auto_memory_dir(&home, &crafted), Some(memory(&crafted)));

    // No repository: where it started.
    let loose = base.join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    assert_eq!(auto_memory_dir(&home, &loose), Some(memory(&loose)));

    // A bare repository's worktree (`git clone --bare r.git`, then
    // `git -C r.git worktree add ../bw`): the common directory is the bare
    // repository itself, not a `.git` inside one — so it, not its parent.
    let bare = base.join("r.git");
    let entry = bare.join("worktrees/bw");
    let worktree = base.join("bw");
    std::fs::create_dir_all(&entry).unwrap();
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", entry.display()),
    )
    .unwrap();
    std::fs::write(entry.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        entry.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .unwrap();
    assert_eq!(
        auto_memory_dir(&home, &worktree.join("src")),
        Some(memory(&bare))
    );
}

/// **A `.git` anyone can place never holds the launch** — a FIFO, a link to
/// `/dev/zero`, a file larger than any path: each is no worktree, and the
/// answer comes at once (Reviewer B, round 28: a FIFO `/tmp/.git` hung
/// Shannon on a raw terminal). Asked on a thread with a deadline, so a
/// regression fails here instead of hanging the suite.
#[test]
fn a_placed_git_file_never_holds_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let home = base.join("claude");
    let fifo = base.join("fifo");
    std::fs::create_dir_all(&fifo).unwrap();
    let name = std::ffi::CString::new(fifo.join(".git").display().to_string()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo touches nothing else.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let zero = base.join("zero");
    std::fs::create_dir_all(&zero).unwrap();
    std::os::unix::fs::symlink("/dev/zero", zero.join(".git")).unwrap();
    let huge = base.join("huge");
    std::fs::create_dir_all(&huge).unwrap();
    std::fs::write(
        huge.join(".git"),
        format!("gitdir: {}\n", "a/".repeat(GITFILE_CAP)),
    )
    .unwrap();
    for start in [fifo, zero, huge] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (theirs, at) = (home.clone(), start.clone());
        std::thread::spawn(move || tx.send(auto_memory_dir(&theirs, &at)));
        let found = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{} held the launch", start.display()));
        assert_eq!(
            found,
            Some(
                home.join("projects")
                    .join(directory_name(&start).unwrap())
                    .join("memory")
            ),
            "{}: no worktree, its own",
            start.display()
        );
    }
}

/// `text` written to the end of `path`, as Claude Code writes a transcript:
/// nothing before it touched.
fn append(path: &Path, text: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

/// **A transcript read on is the one read afresh, at every line** — both
/// fixtures written a line at a time (Steven, 10-17: "Once we've imported
/// conversations we shouldn't need to re-read"). A build goes on from where
/// the last turn opened, so a line of any shape — a reply's next block, a
/// parallel call's result, a compaction and its summary, a prompt joining
/// the one before, a subagent's entry, a line that is not JSON — must leave
/// it as a whole reading of the same file leaves it.
#[test]
fn a_transcript_read_on_is_the_one_read_afresh() {
    let (dir, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // A prompt that closes a turn with a call unanswered, and brings an
    // image: what it counts and what closing the turn before counts, both,
    // before the build that goes on from it.
    let opener = dir.path().join("opener.jsonl");
    for line in [
        user("o1", None, json!("first"), "01"),
        assistant(
            "o1a",
            "o1",
            "N1",
            json!([{"type": "tool_use", "id": "T9", "name": "Bash", "input": {"command": "ls"}}]),
            "02",
        ),
        user(
            "o2",
            Some("o1a"),
            json!([{"type": "text", "text": "look"}, {"type": "image", "source": {}}]),
            "03",
        ),
        assistant(
            "o2a",
            "o2",
            "N2",
            json!([{"type": "text", "text": "seen"}]),
            "04",
        ),
    ] {
        append(&opener, &format!("{line}\n"));
    }
    let sources = [
        fixture(dir.path()),
        kept_messages_fixture(other.path(), false),
        opener,
    ];
    for (n, source) in sources.iter().enumerate() {
        let text = std::fs::read_to_string(source).unwrap();
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        // A line at a time.
        let grown = dir.path().join(format!("grown{n}.jsonl"));
        let mut transcript = Transcript::new(&grown);
        let mut was = None;
        for line in &lines {
            append(&grown, line);
            holds_on(&mut transcript, &mut was, &format!("after {line}"));
        }
        // And in three appends, cut at every two places: several lines at
        // once — a compaction and its kept messages, a parallel result after
        // the next prompt — arrive as one read (review stop 4, C M5).
        for i in 0..=lines.len() {
            for j in i..=lines.len() {
                let grown = dir.path().join(format!("split{n}-{i}-{j}.jsonl"));
                let mut transcript = Transcript::new(&grown);
                let mut was = None;
                for part in [&lines[..i], &lines[i..j], &lines[j..]] {
                    if !part.is_empty() {
                        append(&grown, &part.concat());
                        holds_on(&mut transcript, &mut was, &format!("{n}: {i}, {j}"));
                    }
                }
            }
        }
    }
}

/// **Read `transcript` on, and hold it to a fresh reading of its file — and
/// to what it says it kept**: the first `kept` turns are the last reading's,
/// and on a file only appended to it kept all but that reading's last turn
/// at least — built on from where that opened, not built whole, which
/// would have the caller scrub every turn again (review stop 4, C HIGH 2: a
/// build that dropped where it could go on from passed everything else).
fn holds_on(transcript: &mut Transcript, was: &mut Option<Imported>, step: &str) {
    transcript.read_on().unwrap();
    let afresh = read(transcript.path(), Scope::Lineage);
    match (transcript.lineage(), afresh) {
        (Ok((on, kept)), Ok(afresh)) => {
            assert_eq!(on, &afresh, "{step}");
            if let Some(was) = was {
                assert_eq!(on.turns[..kept], was.turns[..kept], "kept as was: {step}");
                assert!(
                    kept + 1 >= was.turns.len(),
                    "built on, not whole: {step} — kept {kept} of {}",
                    was.turns.len()
                );
            }
            *was = Some(on.clone());
        }
        (Err(_), Err(_)) => {}
        (on, afresh) => panic!("{step}: read on {on:?}, afresh {afresh:?}"),
    }
}

/// **A file rewritten at the same length or longer is read afresh** — the
/// seal, the end of the last line read, is what tells it from one appended
/// to; a rewrite shorter than what was read is caught by its length alone,
/// and those were the only rewrites tested (review stop 4: A, B and C).
/// A transcript may be rewritten in place, its tail re-appended after an
/// entry is removed, and come out as long as it was.
#[test]
fn a_rewrite_as_long_is_read_afresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let mut transcript = Transcript::new(&path);
    transcript.read_on().unwrap();
    transcript.lineage().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let last = text.lines().last().unwrap();
    assert!(last.contains("\"going\""), "premise: the last line read");
    assert_eq!(text.matches("\"going\"").count(), 1, "premise: there alone");
    let more = user("u3", Some("a2"), json!("and then"), "10");
    let rewritten = text.replace("\"going\"", "\"gonna\"") + &format!("{more}\n");
    std::fs::write(&path, rewritten).unwrap();
    transcript.read_on().unwrap();
    let (now, kept) = transcript.lineage().unwrap();
    assert_eq!(now, &read(&path, Scope::Lineage).unwrap());
    assert_eq!(kept, 0, "built whole");
}

/// **A turn a summary opens is cut at its own boundary — and a summary with
/// none of its own begins its turn itself** (review stop 4, B 3). A file a
/// resume wrote can hold one; cutting at the nearest boundary further back
/// left `<CR>` on the turn before it a copy of nothing, and without a guard
/// in the splice, a panic on the UI thread.
#[test]
fn a_cut_before_a_summary_without_its_own_boundary_keeps_the_turn_before() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resumed.jsonl");
    let boundary = json!({
        "type": "system", "subtype": "compact_boundary", "uuid": "b0",
        "parentUuid": null, "isSidechain": false, "timestamp": "00", "cwd": "/work/proj"
    });
    let summary = |uuid: &str, parent: &str| {
        let mut s = user(uuid, Some(parent), json!("Summary of before."), "01");
        s["isCompactSummary"] = json!(true);
        s
    };
    let reply = |uuid: &str, parent: &str, text: &str| {
        assistant(
            uuid,
            parent,
            uuid,
            json!([{"type": "text", "text": text}]),
            "02",
        )
    };
    for line in [
        boundary,
        summary("s1", "b0"),
        user("u1", Some("s1"), json!("first"), "01"),
        reply("a1", "u1", "one"),
        summary("s2", "a1"),
        user("u2", Some("s2"), json!("second"), "03"),
        reply("a2", "u2", "two"),
    ] {
        append(&path, &format!("{line}\n"));
    }
    let lineage = read(&path, Scope::Lineage).unwrap();
    let opened: Vec<Option<&str>> = lineage.turns.iter().map(|t| t.uuid.as_deref()).collect();
    assert_eq!(opened, [Some("s1"), Some("s2")], "premise");
    let cut = read_until(&path, Scope::Resume, Some(&Until::Before("s2".into()))).unwrap();
    let prompts: Vec<&str> = cut.turns.iter().map(|t| t.prompt.as_str()).collect();
    assert_eq!(
        prompts,
        ["Summary of before.\n\nfirst"],
        "the turn before it"
    );
    // Ended before its very first entry: nothing — and no panic.
    let nothing = read_until(&path, Scope::Resume, Some(&Until::Before("b0".into()))).unwrap();
    assert!(nothing.turns.is_empty(), "{nothing:?}");
}

/// **An entry written again under its own uuid is read as written last** —
/// by a build on from before too, which kept the first (review stop 4, C).
#[test]
fn an_entry_written_again_is_read_as_written_last() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let mut transcript = Transcript::new(&path);
    transcript.read_on().unwrap();
    transcript.lineage().unwrap();
    // A long line read last, then: the seal keeps its end, not its room
    // (review stop 4, B 7).
    let long = user("long", Some("a2"), json!("x".repeat(1 << 20)), "08");
    append(&path, &format!("{long}\n"));
    transcript.read_on().unwrap();
    assert!(
        transcript.seal.capacity() <= 2 * SEAL,
        "{}",
        transcript.seal.capacity()
    );
    let again = assistant(
        "a2",
        "u2",
        "M3",
        json!([{"type": "text", "text": "going, again"}]),
        "09",
    );
    append(&path, &format!("{again}\n"));
    transcript.read_on().unwrap();
    let (now, _) = transcript.lineage().unwrap();
    assert_eq!(now, &read(&path, Scope::Lineage).unwrap());
    assert!(now.turns.last().unwrap().reply.contains("again"), "{now:?}");
}

/// **A one-shot reading takes a last line that is whole but unended** —
/// there is no next read to wait for (review stop 4, A 6) — and counts one
/// that is not JSON.
#[test]
fn a_one_shot_reading_takes_an_unended_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let more = user("u3", Some("a2"), json!("unended"), "10");
    append(&path, &more.to_string());
    let read_once = read(&path, Scope::Lineage).unwrap();
    assert_eq!(read_once.turns.last().unwrap().prompt, "unended");
    append(&path, "\n{\"type\": \"user\", \"uu");
    let cut_short = read(&path, Scope::Lineage).unwrap();
    assert_eq!(cut_short.skipped.unparsable, 1, "said, not lost");
}

/// **A FIFO where a transcript was is refused, not waited on** — opened
/// plainly, it held the thread that opened it, a reading thread for good or
/// the UI's for `<CR>` (review stop 4, B 6).
#[cfg(unix)]
#[test]
fn a_fifo_where_a_transcript_was_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fifo.jsonl");
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo touches nothing else.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let at = path.clone();
    std::thread::spawn(move || {
        let mut transcript = Transcript::new(&at);
        let _ = tx.send(transcript.read_on().map(|_| ()));
    });
    let read = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("held by the FIFO");
    assert!(read.is_err(), "refused: {read:?}");
}

/// **A line past the longest read is read over, not into memory** — its
/// span counted to its end, its tail kept to know the file by, the next line
/// read as ever; one with no end is read over too (review stop 4, E 6). A
/// buffer smaller than the line, so the reading-over goes chunk by chunk.
#[test]
fn a_line_past_the_longest_is_read_over() {
    let text = format!("short\n{}\nnext\n{}", "x".repeat(100), "y".repeat(50));
    let mut reader = BufReader::with_capacity(8, std::io::Cursor::new(text.into_bytes()));
    let mut line = Vec::new();
    let mut next = |line: &mut Vec<u8>| {
        let r = read_line(&mut reader, line, 16).unwrap();
        (r.spans, r.ended, r.whole)
    };
    assert_eq!(next(&mut line), (6, true, true));
    assert_eq!(line, b"short\n");
    assert_eq!(next(&mut line), (101, true, false), "read over to its end");
    assert!(
        line.ends_with(b"xx\n") && line.len() <= SEAL,
        "its tail kept"
    );
    assert_eq!(next(&mut line), (5, true, true));
    assert_eq!(line, b"next\n");
    assert_eq!(next(&mut line), (50, false, false), "unended, read over");
    assert_eq!(next(&mut line), (0, false, true), "the end");
}

/// **…and a transcript's is counted, the lines after it read** — one real
/// line past [`LINE_CAP`], through `read_on`. Its tail is kept to know the
/// file by, and is no line: this one's last [`SEAL`] bytes are a whole entry
/// of their own, and read as one, it would be a turn the file never had.
#[test]
fn a_transcript_line_past_the_longest_is_counted_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let planted = |pad: usize| user("evil", Some("a"), json!("p".repeat(pad)), "2").to_string();
    let short = planted(0).len();
    let tail = planted(SEAL - 1 - short);
    assert_eq!(tail.len() + 1, SEAL, "premise: the tail is the entry");
    let huge = format!("{}{tail}", "x".repeat(LINE_CAP));
    let lines = [
        user("u", None, json!("before"), "1").to_string(),
        assistant("a", "u", "M", json!([{"type": "text", "text": "hi"}]), "2").to_string(),
        huge,
        // Under the planted entry: read as one, it is on the chain walked.
        user("u2", Some("evil"), json!("after"), "3").to_string(),
    ];
    std::fs::write(&path, lines.map(|l| l + "\n").concat()).unwrap();
    let mut transcript = Transcript::new(&path);
    assert!(transcript.read_on().unwrap());
    let (read, _) = transcript.lineage().unwrap();
    assert_eq!(read.skipped.unparsable, 1, "said, not read");
    assert_eq!(read.turns.last().unwrap().prompt, "after");
    assert!(
        read.turns.iter().all(|t| !t.prompt.starts_with("ppp")),
        "no planted turn"
    );
    assert!(
        !transcript.read_on().unwrap(),
        "known by its tail: nothing new"
    );
}

/// **A round of many calls closes in linear time** — each call's result was
/// looked for in every result: 312 ms at 8k calls, quadratic (review stop 4,
/// E 6). Answered and not alike.
///
/// **By how it scales, not by a wall clock**: a 2 s ceiling failed on the
/// unmutated tree under load, two runs in three, while its doc said the
/// margin was measured (review stop 12, T9 9). Four times the calls take
/// about four times as long when linear and sixteen when quadratic; the
/// bound is eight, each size its fastest of three runs, so load that slows
/// both sizes alike moves nothing.
#[test]
fn a_round_of_many_calls_closes_in_linear_time() {
    let dir = tempfile::tempdir().unwrap();
    let file_of = |n: usize| {
        let path = dir.path().join(format!("s{n}.jsonl"));
        let calls: Vec<Value> = (0..n)
            .map(
                |i| json!({"type": "tool_use", "id": format!("T{i}"), "name": "Bash", "input": {}}),
            )
            .collect();
        let results: Vec<Value> = (0..n / 2)
            .map(
                |i| json!({"type": "tool_result", "tool_use_id": format!("T{i}"), "content": "ok"}),
            )
            .collect();
        let lines = [
            user("u", None, json!("go"), "1"),
            assistant("a", "u", "M", json!(calls), "2"),
            user("r", Some("a"), json!(results), "3"),
        ];
        std::fs::write(&path, lines.map(|l| format!("{l}\n")).concat()).unwrap();
        path
    };
    let fastest = |path: &std::path::Path| {
        (0..3)
            .map(|_| {
                let started = std::time::Instant::now();
                read(path, Scope::Resume).unwrap();
                started.elapsed()
            })
            .min()
            .expect("three runs")
    };
    let (n, big) = (10_000, 40_000);
    let (small_file, big_file) = (file_of(n), file_of(big));
    let read_big = read(&big_file, Scope::Resume).unwrap();
    assert_eq!(
        read_big.turns[0].rounds[0].results.len(),
        big,
        "every call answered"
    );
    assert_eq!(read_big.skipped.unanswered, big / 2);
    // Measured 10-27, idle: 76 ms and 315 ms, a ratio of 4.1.
    let (small_took, big_took) = (fastest(&small_file), fastest(&big_file));
    assert!(
        big_took < small_took * 8,
        "4x the calls took {big_took:?} against {small_took:?}: not linear"
    );
}

/// **A link where a transcript was is refused, and not listed** — Claude
/// Code's are its own files; through a link, another file was read as one
/// (review stop 4, E 6).
#[cfg(unix)]
#[test]
fn a_link_where_a_transcript_was_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = fixture(dir.path());
    let link = dir.path().join("link.jsonl");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let listed: Vec<String> = list(dir.path())
        .unwrap()
        .into_iter()
        .map(|f| f.id)
        .collect();
    assert!(listed.contains(&real.file_stem().unwrap().to_string_lossy().into_owned()));
    assert!(!listed.contains(&"link".to_string()), "{listed:?}");
    let refused = Transcript::new(&link).read_on();
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("symbolic link")),
        "{refused:?}"
    );
}

/// **A grown transcript keeps the turns before its last one** — the build
/// goes on from where the last turn opened, so the turns closed before it
/// come through unchanged, and a caller keeps what it made of them (the
/// scrubbed copy `:graph` shows); unread, every turn is kept.
#[test]
fn a_grown_transcript_keeps_the_turns_before_its_last() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let mut transcript = Transcript::new(&path);
    transcript.read_on().unwrap();
    let (first, kept) = transcript.lineage().unwrap();
    assert_eq!((first.turns.len(), kept), (3, 0), "premise: built whole");
    assert!(!transcript.read_on().unwrap(), "nothing new");
    assert_eq!(transcript.lineage().unwrap().1, 3, "all of it kept");
    // A subagent's entry, run elsewhere: off the chain, and the file's.
    let mut side = assistant(
        "side",
        "a2",
        "M9",
        json!([{"type": "text", "text": "a subagent"}]),
        "09",
    );
    side["isSidechain"] = json!(true);
    side["cwd"] = json!("/work/moved");
    append(&path, &format!("{side}\n"));
    assert!(transcript.read_on().unwrap());
    let (same, kept) = transcript.lineage().unwrap();
    assert_eq!(kept, 3, "the chain as it was");
    assert_eq!(
        same.cwd.as_deref(),
        Some("/work/moved"),
        "where it last ran"
    );
    assert_eq!(same, &read(&path, Scope::Lineage).unwrap());
    for line in [
        user("u3", Some("a2"), json!("and then more"), "10"),
        assistant(
            "a3",
            "u3",
            "M4",
            json!([{"type": "text", "text": "more"}]),
            "11",
        ),
    ] {
        append(&path, &format!("{line}\n"));
    }
    assert!(transcript.read_on().unwrap());
    let (grown, kept) = transcript.lineage().unwrap();
    assert_eq!(grown.turns.len(), 4);
    assert_eq!(kept, 2, "the turns before the summary's, which went on");
}

/// **A line still being written waits for the next read** — Claude Code
/// writes as it goes, and half a line is no entry: read, it would count as
/// not JSON and be gone, its entry never read.
#[test]
fn a_line_still_being_written_waits() {
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(kept_messages_fixture(dir.path(), false)).unwrap();
    let start = text[..text.len() - 1].rfind('\n').unwrap() + 1;
    let (before, last) = text.split_at(start);
    let grown = dir.path().join("grown.jsonl");
    append(&grown, before);
    let mut transcript = Transcript::new(&grown);
    assert!(transcript.read_on().unwrap(), "premise");
    let had = transcript.lineage().unwrap().0.clone();
    append(&grown, &last[..last.len() / 2]);
    assert!(!transcript.read_on().unwrap(), "half a line: nothing");
    assert_eq!(transcript.lineage().unwrap().0, &had);
    assert_eq!(had.skipped.unparsable, 0, "not counted as not JSON");
    append(&grown, &last[last.len() / 2..]);
    assert!(transcript.read_on().unwrap(), "now whole");
    assert_eq!(
        transcript.lineage().unwrap().0,
        &read(&grown, Scope::Lineage).unwrap()
    );
}

/// **A rewound or rewritten transcript is built afresh** — a prompt that
/// goes on from an earlier entry (a rewind in Claude Code) makes another
/// chain, so nothing built can be gone on from; a file rewritten, or cut
/// short, is not the one read, so it is read from its start.
#[test]
fn a_rewound_or_rewritten_transcript_is_built_afresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = kept_messages_fixture(dir.path(), false);
    let mut transcript = Transcript::new(&path);
    transcript.read_on().unwrap();
    transcript.lineage().unwrap();
    // Gone on from the first reply, and for longer than the chain it left:
    // the place its last turn opened at is on the new chain too, as another
    // entry.
    let mut parent = "a1".to_string();
    for n in 0..4 {
        let (q, a) = (format!("fork{n}"), format!("fork{n}-a"));
        append(
            &path,
            &format!(
                "{}\n",
                user(&q, Some(&parent), json!(format!("instead {n}")), "12")
            ),
        );
        let reply = json!([{"type": "text", "text": format!("then {n}")}]);
        append(
            &path,
            &format!("{}\n", assistant(&a, &q, &format!("F{n}"), reply, "13")),
        );
        parent = a;
    }
    assert!(transcript.read_on().unwrap());
    let (rewound, kept) = transcript.lineage().unwrap();
    assert_eq!(kept, 0, "another chain");
    assert_eq!(rewound, &read(&path, Scope::Lineage).unwrap());
    assert_eq!(rewound.turns.len(), 5, "{:#?}", rewound.turns);

    let elsewhere = tempfile::tempdir().unwrap();
    for text in [
        std::fs::read_to_string(fixture(elsewhere.path())).unwrap(),
        format!("{}\n", user("u1", None, json!("short"), "01")),
    ] {
        std::fs::write(&path, text).unwrap();
        transcript.read_on().unwrap();
        let (now, kept) = transcript.lineage().unwrap();
        assert_eq!(kept, 0);
        assert_eq!(now, &read(&path, Scope::Lineage).unwrap());
    }
}
