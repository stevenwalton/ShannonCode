# Memory and skills — BUILT (2026-10-06)

Steven, 10-06: "I'd like to make sure we can support skills and memories.
Remember that we want to be able to define these areas and include multiple
locations."

Status: decided (§6, Steven's answers), built as §7 says; the user's
account is `:help shannon-memory`. The Claude Code
facts in §2 are its public documentation's (the memory and skills pages,
checked 2026-11-05); what is inferred is marked so. §1–§5 are the proposal
as it was put; where §6–§7 differ, they win.

## 1. What is already decided

- **Memories are plain text, and not locked to a project directory**
  (`vision.md` §Memories): "They go wherever you want! … Default to a
  `$XDG_CONFIG_HOME/shannon/project/` location. Load those or don't! Have a
  shared location that multiple agents can have a collective memory. Shard
  your memories if you want … use your config (your global or your
  project's) to modify." Plain text, because ~620 KB across every
  `~/.claude` memory dir makes packing worthless and costs `grep`/`$EDITOR`
  (TODO.md (40)).
- **Model-written memories are a setting**: `:set memory=off|auto|ask`,
  default `auto` (TODO.md (40), DECIDED).
- **Editing loads the altered version into the conversation**, and memories
  are read and edited in-app (`vision.md` §Memories, §Full Agent Access;
  Milestone 4 "Memories in-app").
- **They travel with a branch by default** (`design.md` §1.4: "Skills / a
  memory directory — carries by default, overridable per-branch").
- **Config can read other agents' locations, `~/.claude` among them, if the
  user wants** (`design.md` §0.4).

## 2. How Claude Code does it (its public documentation)

- **Three kinds, three location rules.** Instructions (`CLAUDE.md`: managed
  policy, `~/.claude`, the project's, `CLAUDE.local.md`, `.claude/rules/`;
  those in the directories above the working directory load at launch,
  those below it on demand); auto memory (`~/.claude/projects/<project>/
  memory/`, derived from the git repository, so its worktrees share one: an
  index `MEMORY.md`, whose first 200 lines or 25 KB load every session, and
  a file per memory); skills (`.claude/skills/<name>/SKILL.md` in the
  starting directory and every parent up to the repository root, beside
  the user's, managed and plugin ones). Several roots, a walk rule per kind.
- **Where they go.** `CLAUDE.md` content arrives as a user message after the
  system prompt, not inside it — under "These instructions OVERRIDE any
  default behavior", as the context a model is sent shows (observed).
- **Skills by progressive disclosure.** Names and descriptions are listed
  within a budget of 1% of the context window; the body loads when the
  model calls the `Skill` tool. A skill's `allowed-tools` run without asking
  during the turn that invokes it; the grant clears with the next message.
- **Writes.** The model writes memories with its ordinary Write/Edit tools.
  A repository's own settings may NOT move the memory directory: from a
  project's settings, `autoMemoryDirectory` is neither read from nor
  written to.

## 3. The proposal

**One mechanism: a path per kind, as vim's `runtimepath`.** Three kinds,
each a list of locations searched in order, each list STACKED across config
files (every file adds, as `[permissions] rules` does — `config/stack.rs`
`STACKED`), so a project's config adds a shared or sharded location to
yours rather than replacing the list:

```toml
[memory]
mode = "auto"                     # off | auto | ask — DECIDED
# Instructions: read whole, every session. First match per name per
# location; `{project}` is the project id the history store already uses
# (`store::project_store_root`), `{start}` where Shannon started.
instructions = ["~/.config/shannon/AGENTS.md", "{start}/AGENTS.md"]
# Memories: an index (MEMORY.md) and one file per memory, each location.
paths = ["~/.config/shannon/memory", "~/.config/shannon/projects/{project}/memory"]
# Where the model writes. One place, yours: a file that is not yours may
# not set it (§4 D4).
write = "~/.config/shannon/projects/{project}/memory"

[skills]
paths = ["~/.config/shannon/skills", "{start}/.shannon/skills"]
# Claude Code's are one line away, opt-in: "~/.claude/skills"
```

- **Claude Code's formats, read as they are.** `SKILL.md` with `name`/
  `description` frontmatter; `MEMORY.md` index plus frontmatter files. Then
  `~/.claude/skills` or a CC memory dir is just another location (design.md
  §0.4), and nothing new needs learning.
- **What the model sees is recorded in the episode.** Instructions and each
  memory index are loaded into a *context block* at the start of a session —
  a meta-turn in the graph, as `S`'s summaries are — never spliced into the
  system prompt. An edit (yours in `$EDITOR`, or the model's) lands as a
  further block holding the diff. So the transcript, `:graph` and a resumed
  session all say exactly what the model was told and when; a branch carries
  what was loaded before its fork point (design.md §1.4) while the files
  stay shared, as the filesystem is; and the prompt cache survives an edit.
- **Skills are a tool.** The list (name + description, under a budget) is
  in the context block; a `skill` tool returns a body, recorded as any tool
  call is. `/name args` in the prompt invokes one as `/child` does. A
  skill's `allowed-tools` is a RESTRICTION, never a grant: a skill from a
  project directory must not widen the gate (CC's grant is the hole a repo
  file escalates through).
- **The gate knows the places.** Memory and skill locations are approved
  places for READS (as the scratch directory is). `memory.write` is an
  approved place for writes under `mode = auto`, asks under `ask`, and is
  not approved under `off`. Vouched as the scratch directory is
  (`scratch::prepare`: a directory, yours, 0700) or said at launch and left
  out.
- **In-app.** `:memory` lists what is loaded, location by location, and
  opens one in a pane; `:memory edit <name>` is `$EDITOR`, and saving it
  lands the diff (above). `:skills` lists skills and where each came from.
- **Bounded.** An index and an instruction file are read to a cap (lines
  and bytes, said when cut); the skill list to a budget; a memory body only
  when asked for.

**A first slice**, decision-invariant enough to build after D1–D3: the
config keys and their stacking, location discovery with `{project}`/
`{start}`, loading instructions and indexes into a recorded context block,
and `:memory`/`:skills` to see them. Then the `skill` tool, then
model-written memories with the write place approved, then diffs on edit.

## 4. Decisions for Steven

- **D1. Three kinds or two?** Instructions (always loaded, user-written:
  AGENTS.md/SHANNON.md) apart from memories (indexed, model-written), as
  above — or one "memory" kind whose locations hold both. Recommended:
  three; they differ in who writes them and how much of each is sent.
- **D2. Which instruction file names?** `AGENTS.md` (the cross-agent
  convention), `SHANNON.md`, and `CLAUDE.md` opt-in? And from `{start}`
  only, or walking up to the git root as CC's skills do? Recommended:
  `AGENTS.md` + `SHANNON.md`, `{start}` up to the git root.
- **D3. Context block vs system prompt.** Recommended: the recorded block
  (above). The cost: a new graph node kind (or a flagged turn), which `S`,
  squash and `:continue` must carry.
- **D4. Who may name a location.** Recommended: any file may ADD a read
  location (said at load when it is not yours, as a safety setting is), but
  only yours may set `memory.write` — where the model writes unasked must
  not be a cloned project's choice — as Claude Code ignores
  `autoMemoryDirectory` from a project's settings (its settings
  reference). Project-supplied
  text is loaded as the project's, not under any "overrides everything"
  framing.
- **D5. Who writes under `auto`.** The model in its own turn (with the
  bash it has), or also a background pass after a turn?
  Recommended: the model in its turn only, for now.
- **D6. Defaults for `{project}`.** `~/.config/shannon/projects/{project}`
  per vision.md, with `{project}` the history store's id — or a readable
  name (the start dir's basename plus a short hash), since you will browse
  these by hand? Recommended: the readable form, one function for both.

## 6. Decided (Steven, 10-06)

- **Readable AND writable locations**, and either may name a directory or a
  specific file.
- **Panes `:memories` and `:skills`** list each; a row is selected and opened
  in `$EDITOR`. *(Superseded 10-06 evening: one setup screen with both
  lists, `Tab` between them — §7.)* **`:skill <name> [args]`** invokes a skill from the command
  line (`:skill shannon-review`).
- **D1** instructions apart from memories; `AGENTS.md` and `SHANNON.md`
  both loaded, the list configurable. `CLAUDE.md` is opt-in — and on by
  default in a Claude Code session (where `claude` reads it itself).
- **D2** a name is looked for from where Shannon started up to the
  repository root. **D3** the recorded context block. **D4** any config
  file may add memory and skill locations. **D5** the model writes memories
  in its own turn only. **D6** `{project}` is readable.

## 7. As built

**Config.** `[memory]` and `[skills]`:

```toml
[memory]
mode = "auto"           # off | auto | ask — whether the model writes memories
instructions = ["AGENTS.md", "SHANNON.md"]
claude = "auto"         # Claude Code's CLAUDE.md and .claude/skills: auto, on, off
paths = ["{config}/memory", "{config}/projects/{project}/memory"]
write = "{config}/projects/{project}/memory"
project = "hash"        # how {project} is named: hash | claude-code
disabled = []           # files and places NOT read

[skills]
paths = ["{config}/skills", ".shannon/skills"]
disabled = []           # skills NOT used: a name, or a place
```

- **One grammar for every location list.** `~`, `/`, or a placeholder
  (`{config}` — Shannon's config directory, `{project}`, `{start}`) leads →
  that one place. Anything else is RELATIVE → looked for in where Shannon
  started and each directory above it up to the project's root (the
  nearest holding one of `memory.project_root`'s markers — gutentags'
  version-control list by default, `.git` among them; Steven, 10-07), the
  root's first. `[]`, or no marker found: where Shannon started alone. The
  markers are yours only — they decide what "the repository" is, and so
  what a relative location may read. A place may be a file or a directory:
  instructions — the file, or every `*.md` in the directory; memories — the
  file read whole, or the directory's `MEMORY.md` read and its other files
  listed; skills — the file is one skill, or every `<name>/SKILL.md` in the
  directory (and its own `SKILL.md`).
- **Stacking.** `memory.instructions`, `memory.paths` and `skills.paths`
  STACK: your config's list replaces the default, and a file that is not
  yours ADDS to what is there, defaults included (one rule for every
  stacked list; `permissions.rules`' default is empty, so nothing changes
  for it). `memory.write` is yours only: a file that is not yours setting
  it is ignored, and said so. Both sections are safety settings.
- **`{project}`**, by `memory.project` (Steven, 10-06 night: "keep the hash
  as the default but allow a Claude Code like option"): `hash` — the start
  directory's name and the first 8 hex of the history store's project hash
  (`store::project_name`), each directory its own; `claude-code` — Claude
  Code's name for the repository (`claude_import::project_key`), one for a
  subdirectory and every worktree. One derivation, `knowledge::project_name`,
  at launch and when the setting changes; yours only, as `write` is. The
  history store's own path is unchanged either way.
- **Disabled** (Steven, 10-06 night: "block the file … we disable further
  reading … not going to automatically modify context history"):
  `memory.disabled` (files and places) and `skills.disabled` (names and
  places), both STACKED. `find` marks what they hold `State::Disabled`
  (`DisabledSet`, the one predicate — `why` names the entry): not told,
  not listed to the model, not a usable skill. Disabling is of files and
  places (Steven, 10-07): the index beside a disabled memory is told as it
  is — to silence a location, disable the location. The gate refuses a read of each (`deny
  :read`, built in `places::rules` from `find`'s marks and every place
  named, there or not yet, so screen and gate are one answer); every door
  that loads a skill asks `Found::skill_disabled`, which holds a name in
  any spelling Claude Code calls it (`/x`, a plugin's `p:x`, its
  frontmatter name — the skills docs, "How a skill gets its command
  name") and refuses by place only where no usable skill of
  that name is left. What the model has stays; a told file's next turn
  carries its `unloaded` block. The screen's `x` is a session `:set`
  (`App::toggle_shelf_file`), typed, one entry; it takes back only an
  entry naming its row, and asks the predicate after, so an entry a
  config file holds is said — Shannon never writes your config. Claude
  Code's own `CLAUDE.md` in its session is claude's to read: `x` says
  `claude = "off"` is what stops it.
- **`claude`**: `auto` — a Claude Code session reads its own files (claude
  does; Shannon never sends them again), an API session does not; `on` —
  an API session loads `CLAUDE.md` (and `~/.claude/CLAUDE.md`) and
  `.claude/skills` (and `~/.claude/skills`) too; `off` — neither, and a
  Claude Code session runs with `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` (its
  skills it still reads). The variable is Claude Code's hard off (its
  environment-variable reference), carried per request
  (`SendRequest::own_instructions`, set at the one launch door from the
  live config), so `:set memory.claude=off` holds from the next request.
- **Claude Code's memories** (Steven, 10-06: "When using a Claude Code
  session we should be able to access the Claude Code memories"; image 82
  had `cat` of its `MEMORY.md` DENIED, and the model wrote its memory into
  Shannon's place instead, asking which of two it should use). Its
  directory is `{claude-memory}` (`claude_import::auto_memory_dir`:
  `<home>/projects/<the repository's root, named>/memory` — per repository,
  shared across worktrees (Claude Code's memory docs), worktrees resolved
  with its own validation; a name the `.claude` directory docs do not settle,
  a path that is not ASCII or past the 200-character cut the Agent SDK's
  sessions docs describe, whose hash they do not give, has none). Where `claude` reads its
  files this is the session's WRITE place (`knowledge::Write::ClaudeCode`):
  Shannon sends no memory guidance there (claude's prompt has its own), and
  approves reads and, under `auto`, writes there — in a Claude Code session
  only (`App::claude_rules`, built beside `rules`). Under `on` an API
  session reads it as any memory location. `off`, or `memory = off`, sets
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` too (`SendRequest::own_memory`).
  Vouched by `scratch::prepare_owned` (yours, no foreign WRITE: observed
  `0755`, 11-08), and made only where Claude Code keeps `projects`.
  Not followed: Claude Code's `autoMemoryDirectory` setting.
- **`~/.claude` is a credential path** in the shipped `[secrets]` seed — but
  for `[secrets] paths_except` (default: `~/.claude/projects/*/memory`,
  `~/.claude/CLAUDE.md`, `~/.claude/skills`), which carves a place out of a
  pattern holding the whole of it, never out of one naming a file in it
  (`*.pem` still holds there). Yours only. Carved out is not approved: a
  read there asks unless a location approves it. `~/.claude.json` (an API
  key, `mcpServers`) joined the seed with it. A project's `.claude` is not
  under it.

**The context block** is `Block::Context { kind, source, text }` on the
user message of the turn it is first sent with: kinds `instructions`,
`memory`, `skills` (the list), `skill` (a body), `unloaded`. What is loaded
is derived from the history — the latest block per source — so a turn
attaches only what is new or changed, a source no longer found gets an
`unloaded` block, and a composed turn (`S`, `:return`, a prefill
`:continue`) that drops the blocks simply has them sent again next turn.
Hashed into the turn's id; masked as text is; on the wire a text block,
before the prompt (`Message::wire`, the API backend; `claude -p`'s input).
The transcript shows one line for it, live and on replay, each file named
by the fewest trailing path components that tell it apart from the others
in that line (`tool_record::context_line`) — both default memory locations
end `memory/MEMORY.md`, and a repository walk finds two `AGENTS.md`.

**Files are read bounded** (an instruction or index file to 40 KB, all of
them together to 128 KB — past it a file is named and not read — the skills
list to 8 KB, a skill's text to 64 KB after its arguments are placed), as
text, and never from a credential path (`[secrets]`) **as named or as it
resolves** (`knowledge::credential`; review 27: a repository's `AGENTS.md`
linked to a key was judged by its name). What is told, and a skill's text,
pass `App::outbound` — redacted as a command's output is, held values as
their handles — and the source path is masked too.

**Precedence is order.** The first location wins a skill's name; a stacked
list puts your entries first, then what other files added (`stack::fold`).

**The gate.** Every memory and skill location that exists is an approved
place for reads (instruction files are told whole and need none). Under
`mode = auto` the write location is too, and an allow is in force for writes
there (`allow :write <dir>` — a directory holds what is below it — listed
in `:rules` as from `memory = auto`);
under `ask` writes there ask; under `off` no write is approved, and what
`paths` names is still read. The write location is made if missing and
vouched as the scratch directory is (a directory, yours, 0700); where the
vouch fails, writes ask, and the model and `:memories` say so
(`App::write_mode` — the fact, not the setting).

**Skills.** A `skill` tool (API sessions; beside `bash`, which stays first)
returns a skill's text — the frontmatter dropped, `$ARGUMENTS` filled —
recorded as any tool call is. It is offered only where a skill is found
(`App::tool_request`, the one place tools are chosen): a tool that can only
fail is noise in every request. Answered at once in the foreground and in a
job alike — it runs nothing, so an unwatched `:child` is never held on it.
`:skill <name> [args]` sends a turn whose user message carries the body as
a `skill` context block; refused with a reason when no name is given, the
name is no skill (the names there are, listed), or the session is busy;
`<Tab>` completes names. A skill's name is its directory's (a lone file's
stem); a name found twice is the first location's, the other listed as
shadowed. In a Claude Code session the list names each SKILL.md for its
Read tool, and `:skill` on Claude Code's own skill sends the prompt alone —
`claude` loads it. Frontmatter is read by hand (`key: value` lines; no YAML
crate). `allowed-tools` is not read.

**A screen, not panes** (Steven, 10-06: "an interface similar to
`:models`"; the panes were text pages, `~` past their end, and their
`<Enter>` answered the event loop QUIT). `crate::shelf::Screen`, a screen
of `crate::setup` as `:models`' picker is: two lists, `Tab` between them,
a file a row under the place it was found, its badge (sent · index ·
listed · claude's · not read · disabled) in its colour; a place cut in the
middle to keep its note ("the model writes here"); notes wrapped, never
cut; an empty list saying where a file goes. `<Enter>` is
`Effect::Edit(path)`: the event loop passes `setup_event` the editor that
holds its terminal (`App::edit_shelf_file`; a test passes its own), and the
screen stays open, read again (`App::refresh_shelves`: an edit, a turn's
end — the screen is modal, so nothing else can change it). `x` is
`Effect::Toggle`: disabled, or enabled again, for the session. `d` is
`Effect::Remove`, asked ON the screen (a question behind a full screen is
one nobody sees): `knowledge::removal` says what goes — a memory and its
index lines, a skill's directory; never an instruction file, an index, a
configured location or a file not read — worked out again when `y`
answers; any other key keeps. Deleted, not hidden; links unlinked, never
followed. `n` takes a name on the screen and is `Effect::Create`: a
template (`knowledge::draft_memory` in the session's write place,
`draft_skill` under `knowledge::skill_home`; never a skill name found
already, never `memory`) written `0600`, `$EDITOR`, then `Draft::settle`,
which says what it came to — left as written or deleted, it is taken away
(save-to-apply), as it is when the editor fails (`Draft::discard`); saved,
a memory with a name and a description gets its line in its index (read
through `remove::read_index`). `N` asks where it would go as `n` does
(`App::memory_place`, `App::skill_place`), and is `Effect::AskModel`: the screen
closes and a request is drafted in an EMPTY prompt, never sent (Steven,
10-06: "the point is to be able to work *with* the agent"; "Good ideas on
things like prompting the model"). A third list, MCP (`Shelf::Servers`):
Claude Code's servers as its `init` names them (`StreamMsg::ProviderServers`
→ `App::claude_servers`; nothing read of Claude Code's config), each
connected or down; `x` toggles `[mcp] disabled`, and `App::disabled_call`
refuses anything of a disabled server's — its tools (`McpServer::owns`),
its prompts through `Skill` (named as its tools are), its resources by
`server` (`McpServer::named_by`) — the name as Claude Code normalises it
(`McpServer::normal`, the one answer the screen and the gate both ask).
Enter, `d`, `n` and `N` say a server is Claude
Code's (`claude mcp`). `Tab` turns forward through the three lists,
`BackTab`/`h`/`Left` back. What a key came to is said at the top of the
screen and in `:messages` (`App::shelf_said`). The smoke drive presses
`<Enter>` on the skills list and the memories one (reached by `BackTab`,
past the MCP list).
