# What the model is told, and where it lives

Every piece of text Shannon puts in front of a model that we have found,
by where to edit it (re-derived 2026-10-31 for review stop 14, C 6 — the
list may still miss one; the recipe at the end finds candidates). By
**item name** or quoted phrase, never line number: search for it
(`rg 'SYSTEM_TEMPLATE'`, `rg 'allowed by Shannon'`). After an edit, `cargo
build --bin shannon` and try it. Few tests pin a prompt's wording: the
unattended `bash` description's "NOBODY IS WATCHING" is pinned
(`app/tests/jobs.rs`, `app/tests/marks.rs`), and the mark tool's result
(`app/tests/position_marks.rs`); the rest only by shape.

Two backends hear different things. **API** turns (Shannon runs the loop)
get Shannon's system prompt and tools. **Claude Code** turns (`claude -p`)
keep Claude Code's own prompt and tools; Shannon adds its notes
(`--append-system-prompt`), the context wrapper and the handoff below, and
answers each tool call's hook — every row marked "both" or "Claude Code".

## System prompt

| What | Item | File | Sent to |
|---|---|---|---|
| Who the model is, its `bash`/`skill`/`mark` use, the markdown pane | `SYSTEM_TEMPLATE` | `crates/shannon-core/src/backend/request.rs` | API |
| Held values (`{{secret.…}}` handles), where the manual is | `SHANNON_NOTES` | `crates/shannon-core/src/backend/request.rs` | API (appended) and Claude Code (`--append-system-prompt`, `claude_code.rs`) |
| The scratch directory sentence | `system_prompt` | `crates/shannon-core/src/backend/request.rs` | API, when a scratch dir is vouched for |

## Tools (description and input schema)

| Tool | Item | File | Sent to |
|---|---|---|---|
| `bash`, watched | `ToolDef::bash` | `crates/shannon-core/src/backend/request.rs` | API foreground |
| `bash`, unattended (`:child`, a `:Send` reply) | `ToolDef::unattended_with` | `crates/shannon-core/src/backend/request.rs` | API jobs |
| The secrets paragraph added to `bash` | `ToolDef::with_secrets` | `crates/shannon-core/src/backend/request.rs` | API, when the session holds secrets |
| `skill` | `ToolDef::skill` | `crates/shannon-core/src/backend/request.rs` | API, where a skill is found |
| `mark` | `ToolDef::mark` | `crates/shannon-core/src/backend/request.rs` | API, always |

## Context told with a prompt, and the history

| What | Item | File | Sent to |
|---|---|---|---|
| The wrapper around every memory, skill and unsent block: `<shannon-context kind=… source=…>`, its tags defused | `Block::context_text` (`defuse_tags`), sent by `Message::wire` | `crates/shannon-core/src/message.rs` | both |
| How to keep memories (`⚙ Shannon told the model: how to keep memories`) | `guidance` | `crates/shannon-core/src/knowledge/context.rs` | by session kind (`App::session_kind`; Claude Code's own prompt says how for its memory) |
| The skills list | `skills_list` | `crates/shannon-core/src/knowledge/context.rs` | by session kind |
| What stands in for files past the size budget | `UNSENT` | `crates/shannon-core/src/knowledge/context.rs` | by session kind |
| The earlier turns, as one assistant line, when a conversation moves to Claude Code mid-way ("[Assistant ran bash: …]", "[refused or failed: …]") | `HANDOFF`, `handoff` | `crates/shannon-core/src/backend/claude_code.rs` | Claude Code |

## Instructions for a task Shannon asks of the model

| What | Item | File | Config |
|---|---|---|---|
| Distil a conversation (`S`, `:compact`) | `DEFAULT_SQUASH_INSTRUCTION` | `crates/shannon-core/src/config.rs` | `[merge] squash_instruction` |
| …and what you add to it ("Additional instructions: …") | the phrase | `crates/shannon-tui/src/app.rs` | — |
| Continue a response cut off (`:continue`) | `DEFAULT_CONTINUE_INSTRUCTION` | `crates/shannon-core/src/config.rs` | `[continue] instruction` |

## What a tool call answers the model

Not instructions, but the model reads them and acts on them.

| What | Where |
|---|---|
| A command's result: its output, "(no output)", "[output truncated]", "[timed out and was killed…]", "[exit status N]", the redaction note, "could not run the command" | `exec::Outcome::report` — `crates/shannon-core/src/exec.rs` |
| A command you edited before it ran ("NOTE: the user did not run your command. They replaced it…"); Claude Code's twin ("…edited it before it ran") | the phrases — `crates/shannon-tui/src/app.rs` |
| A mark set ("mark `a` set here.", or "set at \"…\", in your reply 1 turn ago." — where a quote landed), and why one was not | `answer_mark_call`, `agent_mark_at`, `found_in` — `crates/shannon-tui/src/app/marks.rs` |
| A skill's text, or why it did not load | `answer_skill_call` — `crates/shannon-tui/src/app/knowledge_flow.rs` |
| A refused or denied command, API | the gate's reasons — `crates/shannon-tui/src/app.rs` (`refuse_or_hold`, and the reasons handed to `finish_tool_call`) |
| A refused or allowed call, Claude Code: "Shannon refused this tool call: {reason}" (on stderr, which the model reads), "allowed by Shannon" | `Outcome::refuse` — `crates/shannon-core/src/hook.rs`; the reasons — `crates/shannon-core/src/backend/claude_code.rs` (`UNNAMED_REASON`, …) and `app.rs` |
| Results Shannon writes for calls that never ran: "the turn ended before this ran", "cancelled part-way — it may have done some of its work", "unknown tool `…`", "no readable `command` argument" | the phrases — `crates/shannon-core/src/conversation.rs`, `crates/shannon-tui/src/app.rs` |

Adding a new text the model reads? Add its row here in the same commit.
Nothing checks that; to re-derive candidates, `rg 'finish_tool_call\(|Reply::Deny|ToolResult \{|description:'
crates` and read what each hands the model.
