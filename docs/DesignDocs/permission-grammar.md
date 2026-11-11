# The permission grammar — a design study

Started 2026-10-09. The contract for the redesign of what a user writes to
say which commands run unasked, which ask, and which are refused. Living:
sections are filled as the research lands, and the decisions log at the end
is the record. Examples throughout are benign on purpose.

Companion pieces:

- `crates/shannon-core/src/permission/acceptance.toml` — the ACCEPTANCE
  SUITE: the verdicts the goals call for, as cases, run against the engine as
  it stands (`acceptance.rs`). Baseline 2026-10-09: 127 cases — 82 hold, 41
  gaps, 4 unspellable (82 cases from the goals, then 45 from the six
  programs' report).
- The research reports (prior art; conflict semantics; argv conventions and
  spec formats; the six example programs) — distilled into §7 when they
  arrive.
- What the engine does today, in full: the session summary of 2026-10-08
  (`TODO.md` carries the open items it found).

## 1. Goals

Steven, 2026-10-08. Harnesses fail users in two directions: an auto mode
that trusts the harness to decide (and makes the `find` mistake), or a prompt
on everything, which trains the user to press accept. Both end with the
prompt meaning nothing. The way out is a grammar expressive enough to write
the rule you mean, so that only the dangerous call asks.

- **G1** fewer false prompts, without more false allows.
- **G2** rules at two levels: global (`~/.config/shannon/`) and project
  (`.shannon/`).
- **G3** users can model any program themselves; the models we ship use the
  same format; models can be shared (a tool's README carries one).
- **G4** allow, ask or deny on a program, a subcommand (`gh pr view`), a flag
  (`find -delete`), or a flag's value (`gh api --method POST`).
- **G5** three categories under one grammar: programs, paths, web.
- **G6** paths are recursive, and the more specific rule wins: deny `/mnt/`
  but allow `/mnt/foo/`.
- **G7** a program with allowed arguments still cannot act on a denied path.
- **G8** rules compound: `allow find -name`, `allow find -iname`, `deny find
  -delete` together decide `find . -name '*.md' -iname x` (allow) and the
  same with `-delete` (deny).
- **G9** one grammar ideally; a simple tier plus an advanced tier is
  acceptable.

Steven's answers to the first round of questions (2026-10-08), which the
design takes as given until a better proposal displaces them:

- **Q1** specificity within a hierarchy, strictness across components:
  acceptable; paths will likely need treating differently from programs.
- **Q2** a program "blanket allowed" takes any flag; where only specific
  forms are allowed, an unknown flag asks.
- **Q3** a project file should not loosen the global policy — leaning no,
  worried about consequences; and an enterprise layer above the user's is
  foreseen, whose rules the user cannot override.
- **Q4** this is one layer of defence, not a sandbox. `$PATH` and `env`
  control, then namespaces, then containers, come later and are not this
  setting's job.
- **Q5** a path rule needs an operation.
- **Q6** both: we ship models for a few programs, and any user can model a
  program and share the model.
- **Q7** "glob-based" was about how the simple tier LOOKS; reading it
  structurally is the right approach.
- **Q8** grounded examples: `find`, `gh`, `git`, `sed`, `awk`, `uv` — common,
  complex, not a commitment.
- Also raised: `/bin/ls` may differ from `/usr/bin/ls` and
  `~/.local/bin/ls`; what does `allow cat` mean under `alias cat=bat`?
- Method: first understand how the shell parses a command and what forms are
  allowed; find the patterns; build the system around them.

## 2. How a command line is built — the layers

A rule is a claim about what a line will DO. Between the text the model
writes and what runs there are five layers, and each is a place a rule can
be right about the text and wrong about the act. The grammar has to be built
on the layers, not on the text.

### Layer 0 — the shell's grammar: lists, pipelines, commands

POSIX (XCU §2.10, the shell grammar; §2.9, shell commands):

```text
line        := and_or (';' | '&' | newline) and_or ...
and_or      := pipeline ('&&' | '||') pipeline ...
pipeline    := ['!'] command ('|' command)*
command     := simple_command | compound_command [redirects]
                | function_definition
compound    := '(' list ')' | '{' list '}' | for | while | until | if | case
simple      := [ASSIGNMENT=word ...] [name] [word ...] [redirects]
```

What this gives a rule:

- **The unit of judgement is the simple command**: one program, its words,
  its redirections — and the STATE the commands before it left behind: the
  working directory (`cd`), the environment (`export`, `X=`), and what names
  mean (`alias`, functions, `PATH`). `cd / && rm -rf *` and `cd build && rm
  -rf *` are the same two simple commands in different states.
- **Lists and pipelines are composition**, not commands: the line is as
  strict as its worst member. A pipe additionally passes data, which matters
  for exactly one program shape — one that turns its input into arguments
  (`xargs`) or into commands (`sh`).
- **Compound commands are containers.** `for`, `while`, `until`, `if` hold
  simple commands in known positions; `( )` and `{ }` hold a list in a
  subshell or the current shell; `case` and `function` define things.
- **A redirection is an act of its own** on a file: `>` writes, `>>`
  appends, `<` reads, whatever the program does.

Today: `shell::parse_line` does this. Refused whole, and so asking with no
rule able to answer: `( )`, `{ }`, `&`, `<<<`, `$(( ))`, `case`, `select`,
`function`, `coproc`, `[[ ]]`, `$'…'`, an expanding heredoc, a substitution
inside double quotes or nested past 32.

### Layer 1 — word expansion: the text is not the argv

POSIX (XCU §2.6, verified 2026-10-09), in order: tilde expansion;
parameter expansion (`$X`, `${X:-d}`), command substitution (`$(…)`) and
arithmetic (`$((…))`), together, beginning to end; field splitting (`IFS`);
pathname expansion (`*`, `?`, `[…]`); and "quote removal … shall always be
performed last".
Bash adds brace expansion (`{a,b}`) before all of these, and `**` under
`globstar`.

The program never sees the text; it sees the argv the expansions produced.
A rule matched against the text is matched against something the program
will never see. So:

- **A word that expands is unknown to a rule** unless its expansion is
  BOUNDED: a glob lands under its literal prefix (`src/*.rs` names files in
  `src`, and nothing above it unless a segment can match `..`); `~/x` is one
  file; `$X` and `$(…)` are anything, including several words (field
  splitting) or a flag (`-delete`).
- **Quoting removes expansion**: `'*.md'` is the five characters `*.md`, and
  to `find -name` that is its value, not a set of files. The lexer sees the
  quotes; the argv does not. A rule's reading must keep the distinction.
- **A word can become a flag** after expansion, so where a word expands, the
  flag set is not final. (`Command::hidden_flag` today.)

### Layer 2 — the name: what actually runs

POSIX (XCU §2.9.1.4 in the 2024 edition, "Command Search and Execution";
§2.9.1.1 in the 2018 one — verified 2026-10-09). Given a command name:

1. if it holds a `/`, it is a path — "the shell shall execute a non-built-in
   utility", no search;
2. otherwise, the first that applies: a special built-in; a function; an
   intrinsic utility; then the `PATH` search — the first matching executable
   file, in order — failing with 127 if none;
3. aliases are substituted before any of this, from the text (§2.3.1, under
   token recognition). The spec sets no interactive condition; bash does
   (`expand_aliases` is off in a non-interactive shell), zsh does not.

Then the kernel: a `#!` line hands the file to an interpreter; the dynamic
loader honours `LD_PRELOAD`/`DYLD_*`; the program reads its own
configuration and environment (`GIT_*`, `RIPGREP_CONFIG_PATH`).

What this gives a rule:

- **A name is a lookup in an environment.** `allow ls` is sound only if the
  environment the rule was written for is the one the command runs in. Three
  things change the lookup: a `PATH` that puts another directory first
  (`~/.local/bin/ls`), a function or alias of the same name, and a `/` in
  the name (`./ls`, `/bin/ls`), which skips the lookup altogether.
- **Where Shannon stands today** (verified in `exec.rs`): every command runs
  as `bash -c <line>` with the environment cleared and a small list kept
  (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `TMPDIR`, `LANG`,
  `LC_*`, …), plus `[exec] env`. A non-interactive `bash -c` reads no rc file
  and expands no aliases, so a function or alias in the user's own shell —
  Steven's zsh defines `find` and `uv` as functions — does NOT reach the
  agent's commands; `BASH_ENV`, which would, is an env hazard the gate asks
  about. But `PATH` is inherited from Shannon's own, i.e. the user's, so a
  same-named file earlier on it does shadow the system one, and the gate
  cannot see which file ran. A name with a `/` always asks today, `/usr/bin/ls`
  included; a `PATH` set on the line, or an `alias` on the line, disturbs
  the name and asks.
- **Probed 2026-10-09** (this machine, macOS): under `bash -c` with a
  `BASH_ENV` file defining both, a FUNCTION is defined and runs, an ALIAS is
  not found (`expand_aliases` is off in a non-interactive bash). Under the
  user's `PATH`, `find` is `/usr/bin/find` — the zsh function does not
  exist there — and `uv` is `~/.local/bin/uv`: a file in HOME. Under `zsh
  -c`, `.zshenv` IS sourced, and a function and an alias defined there both
  apply to the command. So `[exec] shell = "zsh"` lets the user's own rc
  reach the agent's commands where `bash` does not; and "a file in home is
  an execution of a file" must not read `~/.local/bin`, `~/.cargo/bin`,
  `/opt/homebrew/bin` as untrusted, or every user-installed tool asks
  forever (§9, question 4).
- **The honest reading**: a rule names a program BY NAME, and "the program"
  means the file the executor's `PATH` resolves it to. The gate can resolve
  the name itself at decision time, against the same `PATH` the executor
  uses, and judge the FILE: a system directory keeps the name's trust; a
  file in the project or in home is an EXECUTION OF A FILE, which is a path
  question with an exec operation (`./scripts/build.sh`, `.venv/bin/python`,
  `~/.local/bin/ls`). Pinning the executor's `PATH` is a later setting
  (Steven, Q4); until then the resolution is reported, not assumed.

### Layer 3 — argv conventions: no universal grammar

This is the layer the simple-tier proposal (`[PROGRAM] [ARGS] [PATH]`) is
about, and the layer where the proposal's concession ("no flags after the
path") turns out to be the smallest of the problems. There is no grammar of
argv; there are FAMILIES, and a program follows one or mixes several. The
split of a word list into options, option-arguments, subcommands and
operands depends on the family, and so does whether an option takes the
next word as its value.

| family | examples | what the matcher must know |
|---|---|---|
| POSIX getopt (XBD §12.2) | `ls -la`, `rm -rf x` | letters bundle (`-rf` = `-r -f`); an option-argument may be attached (`-ofile`) or next (`-o file`); `--` ends options; `-` is an operand |
| GNU long options | `grep --color=auto`, `tar --file x` | `--long=v` and `--long v`; abbreviations (`--recurs`); options PERMUTE — they may follow operands unless `POSIXLY_CORRECT`. Probed 2026-10-09: BSD `ls . -d` refuses (`-d: No such file or directory`), GNU `gls . -d` and clap's `bat /dev/null --plain` accept — whether a flag may follow the path is a fact of the program, not of the shell |
| subcommand trees | `git -C d status -s`, `gh pr view 1`, `cargo test --workspace`, `uv run pytest`, `docker run`, `kubectl get pods -n x` | options are SCOPED to a level (`git -C` before the verb, `status -s` after); the verb is a POSITION; `--` and `-` per level |
| expression languages | `find . -name x -o -type d -delete`, `test -f x`, `[ … ]` | single-dash WORDS that never bundle; order carries meaning (`-delete` acts on what the tests before it matched); paths come BEFORE the expression (`find`: options, then starting points, then the first `-`/`(`/`!` begins the expression). Probed 2026-10-09, BSD find: `find /dev/null -L` is "`-L`: unknown primary or operator" — after the path, `-L` is read as expression, not option; `find -- /dev/null -name null` still runs the primary, so `--` ends nothing here |
| key=value operands | `dd if=a of=b`, `make CC=cc all` | a word with `=` names a file or a variable, no dash |
| old-style / dashless bundles | `tar xvf a.tar`, `ps aux` | the first operand is a letter bundle; `f` takes the NEXT word |
| single-dash long options | `find -name`, `java -jar`, `go build -o x` | `-name` is one option, not `-n -a -m -e`; the bundling rule is per program |
| interpreters and scripts | `sh -c '…'`, `python -c`, `perl -e`, `awk 'prog' f`, `sed 's/a/b/' f`, `bash x.sh` | an operand or an option's value is CODE: another language, with its own writes and executions inside it |
| `+` options and `-NUM` | `date +%s`, `sort +1` (old), `head -20`, `grep -2` | a `+`-led word is an option to some programs and an operand to others; `-NUM` is `-n NUM` to a few |
| `@file` argument files | `curl -d @f`, `java @args`, `rustc @args` | the word names a file the program READS (and curl SENDS) |
| wrappers | `sudo -u x cmd`, `env A=1 cmd`, `nice cmd`, `timeout 5 cmd`, `xargs cmd`, `time cmd`, `flock f cmd` | the argv contains ANOTHER argv, after the wrapper's own options (and sometimes one operand); some wrappers change the environment or the privilege the inner one runs with |
| assignment prefixes | `FOO=x cmd` | an environment for this command only; some names change what runs (`PATH`, `LD_PRELOAD`, `GIT_CONFIG_*`) |

Two facts follow, and they are the centre of the design:

- **Without a model of the program, the option/operand split is a guess in
  both directions.** In `prog -o x y`, `x` is `-o`'s value or the first
  operand; `-abc` is one option or three; `-name` is one option or four.
  For a DENY the safe guess is the wider reading (every word may be any of
  the things it could be; a false include costs a prompt). For an ALLOW
  there is no safe guess: a word read as a flag's value is a word not
  checked as a path, and a word read as an operand is a flag not checked.
  Today's engine reads an unmodelled program's dashed words as operands, so
  `git -C /tmp/repo status` has the operands `/tmp/repo` and `status` — and
  `A` proposes a rule over them (TODO: HIGH, 10-07).
- **The simple tier can LOOK like the command and still be read
  structurally.** `allow bat --plain` parsed by bat's model is "program bat,
  flag plain", and matches `bat -p notes.md` and `bat notes.md --plain`
  alike; the same text read without a model is a prefix of words, which is
  the concession Steven named — and the right one for an unmodelled
  program, because a prefix is the one reading that needs no arity: the
  words up to the first one the rule does not name must be exactly these.
  (Claude Code's `Bash(gh pr view:*)` is this reading; its weakness is that
  it never grows past text.)

### Layer 4 — what the words mean: roles

An argv is a list of words; a program gives each a ROLE, and the role is
what a rule is about:

| role | examples | the rule it serves |
|---|---|---|
| file read | `cat F`, `cp SRC …`, `grep -f F`, `< F` | a path rule, read |
| file written | `cp … DST`, `tee F`, `sort -o F`, `> F`, `tar -cf F`, `sed -i F` | a path rule, write |
| file removed | `rm F`, `find -delete`, `git clean` | a path rule, delete |
| directory | `cd D`, `ls D`, `find D`, `git -C D`, `--target-dir D` | a path rule, at the program's operation, recursively |
| file executed | `bash F`, `python F`, `source F`, `./F`, `#!` | a path rule, exec — and a program rule for what it is |
| program name | `--pager P`, `find -exec P`, `sort --compress-program P`, `xargs P` | a program rule for P; the flag runs code |
| code | `sh -c S`, `awk S`, `sed S`, `python -c S`, `--jq S` | a language inside the argv: `sed 'w f'` writes `f`, `awk 'system()'` runs |
| pattern / data | `grep PAT`, `find -name PAT`, `echo W`, `git commit -m M` | not a file: no path rule applies, and an allow need not cover it |
| URL / host | `curl URL`, `git clone URL`, `pip -i URL`, `gh api` (implicit `api.github.com`) | a web rule, by host, and by direction (fetch vs send) |
| ref / id / number | `git checkout REF`, `gh pr view 123`, `head -n 20` | data |
| variable name | `export X`, `read X`, `unset X` | the bindings axis (`PATH`, `IFS`, `HOME` change later words) |

Three consequences:

- **A path's operation comes from the program**, never from the path. The
  same word is read by `cat`, written by `tee`, removed by `rm`, entered by
  `cd`, executed by `bash`. So "a path rule needs an operation" (Steven, Q5)
  is right, and the operation is supplied by the model: it is the ROLE of
  the position or flag the path sits in. An unmodelled program gives every
  word the widest role — a file it may read, write, remove or execute — and
  that is where its false prompts come from.
- **"Everything is a file" is a second set of paths the line never shows**:
  the working directory (`make` reads `Makefile`, `git` reads `.git`,
  `cargo` reads `Cargo.toml`), configuration (`~/.gitconfig`, `.ripgreprc`
  via `RIPGREP_CONFIG_PATH`), devices (`/dev/tcp/host/port` is a network
  connection written as a path), `/proc`, sockets, and whatever a symlink
  inside an allowed directory points at. A static gate reads names; only an
  OS layer sees opens. This is the limit Steven's Q4 accepts, and it bounds
  G7: a denied path is refused BY NAME.
- **Web is the host role** as path is the file role. The third category is
  not a different grammar; it is the operand kind `URL` with its own
  hierarchy (host, then path) and its own operations (fetch, send). What
  makes it hard is that hosts are mostly IMPLICIT: `gh api`, `cargo
  publish`, `uv pip install`, `npm install` reach hosts no argument names —
  a model fact, again.

### Layer 5 — effect: what the act is

The lattice today: `ReadOnly < Mutating < Destructive < Executor < Unknown`,
the join over the program's base and its flags, raised by a write redirect.
Steven's Q5 and Layer 4 refine it from one axis to an OPERATION PER TARGET:
read, write, delete, exec on each path; fetch, send on each host; and
"runs a program" as a program rule on the program it runs. The command's
effect is then the join over its targets, which is what the "strictest
across components" rule computes.

## 3. The patterns

What the layers give us, stated as the invariants the grammar is built on:

- **P1 — the unit is the simple command, in its state.** Lists, pipes and
  compound commands compose verdicts; `cd`, assignments and name changes
  carry forward. (Held today.)
- **P2 — every command is `NAME {OPTION | SUBCOMMAND}* OPERAND*` under some
  family, and the family decides the split.** A model names the family
  (arity per option, bundling, permutation, where the verb sits). Without
  one, a deny reads wide and an allow reads only a literal prefix.
- **P3 — a subcommand is a POSITION, a flag is a SET MEMBER, an operand has a
  ROLE.** `gh pr view` is three positions; `-name` is present or absent,
  wherever it is; `123` is a ref. Today's grammar makes all three "a word
  that must be present somewhere", which is the root of the 10-07 findings
  (`allow gh pr view *` allowing `gh pr close 12 --comment view`).
- **P4 — a path's operation is the program's, read off the role.** Paths
  are judged at an operation, in a hierarchy, recursively (G6, Q5).
- **P5 — a name is a lookup.** A program rule names the program the
  executor's environment resolves the name to; a name with a `/` is a
  file executed, judged as a path with the exec operation.
- **P6 — what expands is unknown; what is quoted is literal.** A rule's
  verdict is only as certain as the words it read; a word that may become
  a flag, a path or several words asks, naming what may hold.
- **P7 — program, path and web are three TARGET KINDS of one act**, each
  with its own hierarchy (the PATH-search name and its subcommand tree; the
  file tree; host then URL path) and its own operations. One grammar can
  name any of them; the rule forms differ by target, not by syntax.
- **P8 — the model is the bottleneck, not the grammar.** Every soundness
  question above (the split, the roles, the implicit hosts and paths, the
  flags that run code, the default method of `gh api`) is answered by a
  model or not at all. The grammar's job is to be the model's QUERY
  language, simple enough to write by hand against an unmodelled program
  and exact against a modelled one.

## 4. What a line cannot show

The limits of any static gate, so that the design promises nothing past
them:

- what a program OPENS that no argument names (Layer 4, "everything is a
  file"); symlinks; what an installed script, a `Makefile`, a `build.rs`, a
  `package.json` script, or a git hook runs;
- what an interpreter's code does, unless that language is parsed too
  (`sed`, `awk`, `jq`, `python -c`): today `sh -c` is an executor and asks;
  `sed -n Np` is allowed by a regex; the rest asks;
- what the network returns (`curl … | sh`): the pipe to `sh` is visible,
  the content is not;
- which file a name resolves to at run time (Layer 2) without resolving
  it ourselves, and whether that file changed between decision and run;
- a program the model has never seen: its options' arity, its roles, its
  effects. The conservative reading asks;
- the program's CONFIGURATION, which is a second argv the line never
  shows (§7.4): `git diff` runs `diff.external` by default, `gh x` may
  be a shell alias, `uv` reads its index from `pyproject.toml`. A
  harmless read is harmless under a clean config; the gate reads the
  line.

The first three are what Steven's Q4 defers to `PATH`/`env` control,
namespaces and containers. The fourth is addressable now (resolve, report,
and later pin). The fifth is G3: the user, or the tool's author, writes the
model.

## 5. Where today's engine stands

The 2026-10-08 summary is the full account; the acceptance suite is the
measurement. Of the 82 cases written from the goals, 57 hold; the 21 gaps
and 4 unspellable cases fall into these classes (the 45 cases from the six
programs' report add 20 gaps, all of class 5 — every one needs a model of
`gh`, `git`, `sed`, `awk` or `uv` — and 14 that hold only because an
unmodelled program asks about everything, marked `note`):

1. **selectors are a set, not positions** (9 cases: every `gh`, `cargo
   build test`, `git push … status`, the `uv run` family);
2. **a trailing `*` makes path words decorative** (2 cases; decided 10-07:
   presence);
3. **no specificity within the path tree** — deny always wins (1 case:
   `deny /mnt` + `allow /mnt/foo`), and **a directory named in a rule is
   the file of that name**, not what it holds (2 cases: `deny * /etc`;
   `find ~/private` under `deny * ~/private/**` only MAY hold);
4. **an allow naming a flag covers every other flag** (1 case; Steven's Q2
   says an unknown flag asks where only forms are allowed);
5. **no model format a user can write** (6 cases: `gh` spellings and the
   implicit POST, `bat`, `mkdir` under a write level, `curl`);
6. **no web category** (4 cases), **no exec operation on paths** (1 case),
   **a privilege wrapper cannot be named** (1 case).

Classes 1–4 are grammar; 5–6 are new surfaces. Nothing in the suite
regresses what holds: the credential override, privilege, the name
disturbed, reads outside the project, a deny beating a blanket allow on a
file inside the denied tree.

## 6. Where the goals pull against each other

Stated 2026-10-08 and answered in part (§1). Kept here with what the
answers leave open.

1. **Most-specific vs deny-wins.** Q1 accepts specificity WITHIN a hierarchy
   and strictness ACROSS components. Open: the definition of "more
   specific" for globs (a literal vs `*`, depth, anchors) and the tie-break;
   whether a project-level rule may be the more specific one (Q3 says a
   project file should not loosen: specificity must not beat trust).
2. **Flags a rule does not mention.** Q2 answers: blanket → any flag; forms
   only → unknown asks. Open: with a MODEL, does a benign modelled flag
   (`find -type`) ride on `allow find -name`, or ask? Rule-centric reading
   (only named flags are covered) matches Q2 literally and costs prompts;
   model-centric (benign flags ride, escalating ones must be named) is what
   the shipped carvings do today. The acceptance suite encodes Q2 literally
   (`compound-unmentioned-flag-asks`); the question is flagged for Steven.
3. **Project rules and trust.** Q3: no loosening; an enterprise layer above
   the user foreseen. So the layers are ordered by trust and a lower layer
   only narrows — the Landlock / IAM-boundary shape. Open: what a project
   file is FOR, then (its own denies and asks; allows only within what the
   layer above allows?), and whether the agent, which can write
   `.shannon/`, must always ask to write there.
4. **What the line cannot show** (§4). Accepted as the limit (Q4).
5. **Parents of a denied path.** `rg x ~` under `deny ~/private`: whether
   the program recurses is a model fact; without it, a parent of a denied
   path asks (today's `HoldsWhatItNames`). With it, `ls ~` does not reach
   `~/private` and `rg x ~` does.
6. **Arity without a model** (Layer 3). The prefix reading for allows; the
   wide reading for denies.

## 7. Prior art

Distilled from the research reports (full text, with every citation, under
the job's `tmp/research/`). Claims marked by the researchers as read at the
source unless noted.

### 7.1 Agent harnesses (report: `harnesses.md`, 2026-10-09)

Surveyed: Claude Code 2.1.295, OpenAI Codex CLI (current `prefix_rule`
engine and the typed `define_program` engine it removed on 2026-07-10),
Gemini CLI's policy engine, GitHub Copilot CLI, Cursor CLI, OpenCode, Amp.

**The finding that matters most**: no surveyed system meets G4 at the
flag-value level, G7, or G8 structurally. The one that came closest —
Codex's legacy engine, with `opt("-n", ARG_POS_INT)`, `args = [ARG_RFILES,
ARG_WFILE]` and a `match` result meaning "the program is allowed; the
caller decides whether the files it writes are" — was dropped for a prefix
engine, and its ergonomics problem (every program needs a model) is exactly
what G3 answers. G6 (narrower wins) exists in one place: Claude Code's
SANDBOX filesystem rules (`denyRead ~/` + `allowRead ~/projects`), while
its permission rules refuse it ("an allow rule can't carve an exception out
of a deny rule").

**How each matches a command:**

- Claude Code: the command TEXT, after normalisation — compound splitting
  on `&& || ; | & newline` (an allow must match every part; a deny any
  part, subshells and loops included), a built-in wrapper list, a built-in
  safe-env list, a built-in read-only set, redirect and `tee` targets
  checked against `Edit`/`Read` rules. `*` spans spaces since 2.1.0
  (2026-01); a security patch six days later for compound commands, and a
  startup warning since 2.1.246 for `Bash(git * main)`, which also matches
  `git -c core.fsmonitor=<script> diff main`. Its docs now say a deny "isn't
  a security boundary around the program". An LLM classifier sits in the
  path (2.1.47 fixed it "hallucinating match descriptions").
- Codex: ARGV TOKENS, an exact positional prefix (`["gh","pr","view"]`
  matches `gh pr view 7888`, not `gh pr --repo x view`); strictest of all
  matches wins; `bash -c` split only when it is a linear chain of plain
  words — anything with redirection, substitution, variables, globs or
  control flow is checked whole. Fail-closed by construction: no CVE; the
  cost is prompts. `host_executable(name = "git", paths = ["/usr/bin/git"])`
  pins a name to the files it may be — Steven's `/bin/ls` question, answered
  per program. Rules carry `match` / `not_match` examples checked at load.
- Gemini: TOML `[[rule]]`s with `commandPrefix` (a word-bounded regex over
  the JSON-serialised args), `decision`, `priority`; tier + priority, first
  match wins; each sub-command re-checked by a tree-sitter parse; a
  redirect downgrades an allow to ask unless `allowRedirection`. The
  project tier has been disabled by an open bug since 2026-02.
- The rest: first-token or first-subcommand granularity (Cursor
  `Shell(git)`, Copilot `shell(git push)`); OpenCode: a glob per parsed
  command node, LAST matching rule wins, a separate `external_directory`
  permission raised from path arguments.

**Layering and trust**: Claude Code merges the rule LISTS across scopes
(user, project, local, managed), deny-first everywhere; a project file's
`deny`/`ask` apply at once, its `allow` only after a trust dialog that
lists them; an untracked `settings.local.json` is yours unless git tracks
it or `.claude` is a symlink; managed settings can make themselves the
only rule source. Codex: a project `.codex/` layer loads only when trusted;
an admin `requirements.toml` can enforce rules; the higher layer wins even
when a lower one is more specific. Both run a sandbox beside the rules,
and both keep the two grammars apart (a path in a rule and a path in the
sandbox config are spelled differently) — the look-alike languages are a
named hazard.

**To take**: typed program models with typed option values and
read/write path roles (G3, G4, G7 in one mechanism); the `match` category
— "program allowed, paths still to judge" — as a result TYPE; inline
examples validated when the rule is read; matching argv tokens, never
text; deny-first with lists merged across scopes, and TRUST GATING ALLOWS
(a project file may narrow at once and widen only once the user has seen
what it widens); narrower-path-wins, as the sandbox already does; one host
rule feeding both the fetch tool and the proxy; fail-closed on anything
not fully parsed; redirect and `tee` targets as path acts; a deny message
returned to the model; a startup lint of the rules; the per-program pin of
which files a name may be.

**To avoid**: `*` spanning spaces in command text (every Claude Code
bypass in the report is a spelling the text matcher had not foreseen:
`git -C . push`, `git 'push'`, `/usr/bin/git`, an env prefix, a variable
expanded into the name); two path syntaxes; deny-beats-allow with no
specificity and no way to say otherwise; regex over a JSON serialisation;
built-in, non-configurable safety lists (CVE-2025-55284 was "an overly
broad allowlist of safe commands" — an invisible program model); an LLM
inside the deterministic matcher; a project tier that is documented and
inert; applying path denies to raw arguments WITHOUT a model of which are
paths — Claude Code tried in 2.1.259 and reverted in 2.1.260 because
`Read(./**/build/**)` denied `npm run build`. That revert is P8 observed in
the field.

### 7.2 Conflict resolution (report: `conflicts.md`, 2026-10-09)

Surveyed at the source: XACML's combining algorithms (from the OASIS PDF),
AWS IAM, Cedar, Windows DACLs, gitignore, AppArmor (including the
`priority=` it added), Landlock, sudoers, iptables/pf/nftables, IP routing,
nginx and Apache, OPA, Kubernetes RBAC and NetworkPolicy, polkit, CSS,
EditorConfig, and Claude Code's layering.

**Three regimes, and what each cost.** *Strictest wins* (XACML
deny-overrides, IAM's explicit deny, Cedar's forbid, AppArmor at equal
priority, Claude Code): no silent pick, but a broad deny makes every
narrower allow dead with no diagnostic — Claude Code documents the
resulting confusion ("you chose don't-ask-again and are still asked").
*Order wins* (gitignore, sudoers, pf, iptables, polkit, nginx's regexes):
sudoers' own page says the last match "is not necessarily the most
specific"; gitignore cannot re-include under an excluded directory; a rule
appended after a catch-all is dead. *Most specific wins* (IP routing's
longest prefix, nginx's prefixes, Apache's directories, Windows'
explicit-over-inherited, CSS): sound where "more specific" means a SUBSET —
a /24 inside its /16, a child object inside its parent — and a footgun
where it does not: CSS ranks incomparable selectors by a count and gets
specificity wars; nginx and Apache let a regex beat a longer literal by
category; Windows lets an explicit allow on a child defeat a deny
inherited from the parent, documented as a feature and a surprise in the
same page. **No surveyed security system derives precedence from pattern
specificity alone, and none lets specificity beat trust** — Windows is the
one that does, and it is the named footgun; IAM boundaries, Landlock
layers, nftables chains and Claude Code's managed settings all intersect
instead, and CSS sorts origin before specificity.

**The scheme is sound, with three conditions**, each with precedent:

1. **Specificity is strict inclusion of match sets** — A is more specific
   than B iff every path A matches, B matches, and not the reverse. That is
   routing's prefix order and Windows' subtree order, extended to globs
   segment by segment: a literal segment is inside a glob segment that
   matches it; anything is inside `**`; two different globs are
   incomparable. Incomparable rules (`deny **/secret/**` vs `allow
   /mnt/**`), or equal patterns with opposite verdicts, fall to the
   STRICTEST verdict — AppArmor's "cannot determine a winner → denied",
   XACML's deny-overrides, IAM. So the decision is total without the order
   being: take the minimal matching rules; one class decides; several, or
   a mixed class, the strictest among them. Worked: `deny /mnt/**` +
   `allow /mnt/foo/**` + `deny /mnt/foo/secret/**` on `cat
   /mnt/foo/secret/a` → the innermost, deny; `deny /mnt/*/secret` vs `allow
   /mnt/foo/secret` → the literal is a subset, allow — and a user who meant
   the glob as a guardrail needs a different KIND of rule (Cedar's forbid,
   IAM's boundary), not a more general pattern.
2. **The hierarchies are rooted at the program.** `allow find` + `deny
   find -delete` works only if the flag tree hangs under `find`, so the
   deny is an exception to the allow; and `deny find` + `allow find -name`
   likewise. If the program were a SIBLING component of its flags, the
   strictest-across-components rule would make `deny find` unconditional
   and no flag-level rule could ever carve it. Both shapes exist in the
   wild (AppArmor's priority carves across a whole rule; IAM never carves);
   we choose rooting, and say so.
3. **Specificity applies within a trust tier, never across one.** Across
   tiers the higher tier's verdict stands whatever the lower tier's
   specificity — otherwise a project file one segment more specific than
   your global deny wins, the Windows footgun exactly. And "allow is an
   intersection": with an unmatched component defaulting to ask, a grant
   is naturally a broad allow at a hierarchy's root with specific denies
   beneath, not a pile of specific allows.

**The sort key the report recommends** (CSS's cascade, adapted):

```text
1. tier       — hard rules of higher-trust layers over lower; then soft
2. session    — a live approval beats any soft pattern, never a hard rule
3. specificity — strict inclusion, within the component's hierarchy
4. strictness — deny > ask > allow, for ties and incomparables
5. order      — never decides; only names the DETERMINING rule (Cedar)
```

with IAM's lesson attached: a boundary must be an EXPLICIT hard deny, not
the absence of an allow, or another spelling, a symlink or a wrapper
routes around it (Claude Code applies a deny when either the written path
or the resolved one matches, an allow only when both do; sudo's
`-u#-1`). And three LINTS, since each surveyed footgun was a silent one:
equal patterns with opposite verdicts; a soft allow more specific than a
hard deny (it is dead); two incomparable rules both minimal for a sample
path.

**What this leaves for Steven (Q3 sharpened).** The CSS shape has a SOFT
tier where the less trusted layer wins (a project's normal rule beats the
user's normal rule, as a site's style beats a browser default) and a HARD
tier where the order reverses. That is the opposite of "a project may
not loosen" unless the user's denies are hard by default. The
alternatives are set out in §9.3.

### 7.3 Argv conventions and spec formats (report: `cli-specs.md`, 2026-10-09)

Sources read: POSIX XBD §12 (2018 and 2024), the glibc manual and
`getopt(3)`/`getopt_long(3)`, gitcli(7), Go's `flag`, the JDK `java`
page, Cobra's `command.go`, clap's API, the Fig, usage and carapace
repositories, Codex's legacy and current `execpolicy`, GTFOBins' JSON;
probes on this machine with GNU coreutils (`gls`, `ghead`, `gsort`).

**The split of an argv into options, values and operands is decidable
from the argv alone in NO family** — not even the subcommand word: Cobra
skips the word after any flag it does not know, so `gh --paginate pr
list` would read `pr` as `--paginate`'s value. The minimum a model must
say, per family (§2, Layer 3): arity per option (0, 1, n, optional and
attached only, or "until a terminator" for `find -exec … ;`); whether
shorts bundle; whether options may follow operands (GNU permutes, BSD
and wrappers stop, `POSIXLY_CORRECT` flips GNU — probed: `gls probe -d`
lists, `POSIXLY_CORRECT=1 gls probe -d` fails); whether long names
abbreviate (GNU and git: `--no-ver` is `--no-verify`, which skips
hooks); what `--` means at each level (terminator; git's revision/path
separator; "the inner argv starts here"); where the subcommand sits and
which options belong to which level; the ROLE of each position and
value; `+` words (`date +FMT` is a format, `sort +1` is an option on BSD
and a FILE on GNU — probed); `-NUM`; `@file` (a file curl reads; a
replacement of the whole command line for `java`/`rustc`, which no
matcher can see past); wrappers (`sudo -u x cmd` vs `sudo -E cmd` move
where the inner argv starts; `xargs` extends it from stdin).

**The rule for an unmodelled program, stated exactly** — an ALLOW is sound
only under the INTERSECTION of parses: every token must be acceptable
under every reading (`x` in `prog -o x y` is both `-o`'s value and the
first operand), so in practice only a whole-argv literal, or a literal
prefix, can be allowed — and a prefix is unsound against a later
override (`gh api --method GET … --method DELETE`) unless a model says
the flag is last-wins. A DENY or an ASK takes the UNION: it fires on any
reading — a letter inside a bundle, an attached value, an abbreviation,
a word after `--`, the inner argv of a wrapper, a substring of a code
string. The model collapses both to one parse, which is the whole reason
to want one. Two consequences for the design: a model carries an
`implementation` key (GNU `sed -i` and BSD `sed -i` parse the same argv
differently; `ps aux` vs `ps -aux`), and the harness never allows
through a token the model cannot classify — usage's "no `effect` is
unknown, not safe", Codex's `unverified`.

**Spec formats, and what each carries:**

| format | corpus | arity | roles / danger | fit |
|---|---|---|---|---|
| Fig (TypeScript, MIT) | ≈715 specs | yes, plus `flagsArePosixNoncompliant`, `optionsMustPrecedeArguments`, `optionArgSeparators`, `isCommand` | files/folders only; `isDangerous` is a UX flag | its `find` spec has no primaries ("TODO"); its `gh` spec's `api` has no `--method`, `-F` or `--input` |
| usage (KDL, MIT, Rust parser) | none | a formal argv grammar; `require_equals`, `value_optional`, `double_dash` modes, `sigil="+"` | `effect` read/write/destructive, command-level, "a coarse classification, not a permission model"; `complete type=command_args` for wrappers | the vocabulary to align with; no single-dash-long family, no `@file` |
| carapace (YAML + ≈1.6k Go completers, MIT) | large | `=`, `?`, `*` | none | `find -exec` takes ONE value; `sed -i` REQUIRES a value (wrong for GNU) |
| clap / Click / Cobra exports | per project | clap: full; Cobra `GenYaml`: NO arity | clap `ValueHint` has `FilePath`, `Url`, `CommandString`, `CommandWithArguments`; Click's `Path` has readable/writable bits | the authors' route: `clap_usage`, `to_info_dict`; Cobra needs a Go exporter |
| zsh `_arguments` | 966 functions | encodes arity, attachment, bundling, `:*pattern` ("up to a word matching") | none | importable in principle for the simple ones |
| Codex legacy `define_program` | ~10 programs | `flag()`, `opt(ARG_POS_INT)`, bundling switch | `ARG_RFILES`, `ARG_WFILE`, `forbidden` | the closest thing to ours; abandoned upstream |
| GTFOBins `api.json` | ≈478 executables | n/a | `shell`, `command`, `file-read`, `file-write`, `upload`, `library-load` | a seed list of the flags that run code or write files, per program |

Every corpus spec the researcher opened had an arity or coverage error
harmless for completion and fatal for matching. So: **imported models
are usable for DENY and ASK matching (the union reading needs only
candidates), never alone for ALLOW**, and the model carries a `trust`
field that says so.

**Recommendation taken into §8.3**: our own TOML schema, vocabulary
aligned with usage so a `clap_usage` or KDL import is near-mechanical,
importers from Fig (through a Node dump), carapace, usage, clap, Click;
and a hand-written VERIFIED set of about thirty programs — the wrappers,
the interpreters, `find`/`test`, `git`/`gh`/`cargo`/`uv`/`docker`/
`kubectl`/`npm`, `cp`/`mv`/`rm`/`tee`/`sort`/`sed`/`awk`/`grep`/`curl`/
`tar`/`dd` — seeded from Codex's policy and GTFOBins.

### 7.4 The six programs (report: `programs.md`, 2026-10-09)

Read from the local man pages and `--help` (BSD find/sed/awk at
`/usr/bin`, gh 2.102.0, git 2.56.0, uv 0.11.14) and the GNU pages, with
read-only probes. Six findings cut across all of them:

1. **The name is not what runs**: `find` here is a zsh function running
   Claude Code's bundled `bfs`, which accepts flags, paths and expression
   in ANY order and adds `-rm`; `uv` is a function that rewrites one
   exact argv into an `xargs` pipeline. (Not reached by the executor's
   `bash -c`, §2 — but the model must be keyed by what runs.)
2. **Configuration is a second argv.** `git` has some thirty config keys
   that name a PROGRAM (`core.pager`, `core.fsmonitor`, `core.sshCommand`,
   `diff.external`, `credential.helper`, `alias.x=!cmd`, `uploadpack.
   packObjectsHook`, …), settable on the line by `-c key=val` or
   `GIT_CONFIG_*`, and `git diff` runs `diff.external` BY DEFAULT, `git
   status` runs `core.fsmonitor` and writes the index; `gh` reads aliases
   (shell aliases run `sh`), `pager`, `editor`, `browser` from its config;
   `uv` reads indexes, sources and the interpreter from `pyproject.toml`.
   A "harmless read" is harmless under a clean config, and the line does
   not show the config (§4).
3. **`gh` is network on every call** — an update check and telemetry
   even on `--help` — and `gh api --hostname H` sends the request, with
   whatever token matches, to ANY host. The executor should set
   `GH_NO_UPDATE_NOTIFIER=1`, `GH_PROMPT_DISABLED=1`, `GH_PAGER=cat`,
   `GIT_PAGER=cat`, `GIT_OPTIONAL_LOCKS=0` (§9.11).
4. **Read-only programs with writing flags**: `find -fprint F` creates F
   at parse time even if nothing matches; `git log --output=F`; gawk
   `-d`/`-o`/`-p` write `awkvars.out`/`awkprof.out` in the cwd with no
   filename; sed `w F` creates F before the first line; `uv export -o`.
5. **A language inside the argv**: sed (`e`, `s///e`, `w`, `r`), awk
   (`system()`, `| cmd`, `cmd | getline`, `print > file`, `ARGV`,
   `ENVIRON`, gawk's `/inet/` sockets and `@load`), find's expression,
   `gh --jq`/`--template` (data-only). The protective flags are
   IMPLEMENTATION-SPECIFIC and silently ignored elsewhere: BWK awk prints
   "unknown option --sandbox ignored" and runs `system()` anyway (probed);
   `--sandbox` is GNU sed's and an illegal option to BSD sed.
6. **Secrets to stdout**: `gh auth token`, `git credential fill`, `git
   config -l` (URLs with tokens) — reads that must not run unasked when
   stdout reaches the model.

Per program, the facts a model must carry (the report has the full
tables, each cited):

- **find**: options `-H -L -P` (BSD also `-E -X -d -s -x -f`) before the
  paths only; paths until the first `-`/`(`/`!`; GNU defaults to `.`,
  BSD errors; `-exec`/`-execdir`/`-ok`/`-okdir` consume to a standalone
  `;` or `{} +` and the first word is a program (`sh -c` the usual
  escape); `-delete` acts where it stands (`find . -delete -name x`
  deletes everything) so the rule is on its presence; `-fprint*`/`-fls`
  write; `-newer F`/`-samefile F` stat a path outside the starting
  points; `-files0-from F` reads one. Verified: `gh api`'s rule is in its
  source — `method = POST` whenever a field or `--input` is given and
  `--method` is not.
- **gh**: Cobra — flags anywhere, `--` ends them, `-R` persistent on
  repo commands; the first word may be an ALIAS or an EXTENSION
  (`gh dash` runs a third-party binary; `gh alias set --shell` stores
  shell code); `gh api`: `-F k=@file`/`--input` read any file and send
  it; `--hostname` any host; `gh skill install --agent claude-code
  --scope project` writes into `.claude/skills` — a prompt-injection
  path into this harness; `gh pr checkout`, `repo clone`, `repo sync
  --force` run git and change the working tree.
- **git**: global options ONLY before the verb; an unknown verb is an
  external `git-<verb>` from `PATH`; subcommand options bundle, take
  `--long=v` or `--long v`, accept unique prefixes; `--` separates
  revisions from paths, `--end-of-options` separates options; the
  program-valued config keys above; `bisect run`, `rebase -x`,
  `submodule foreach`, `difftool -x`, `grep -O`, `--upload-pack`,
  `--receive-pack`, `clone --template`, remote helpers (`ext::` refused
  by default) run programs; `clean -fdx`, `reset --hard`, `checkout --
  .`, `branch -D`, `stash drop` are the data-loss set; `maintenance
  start` writes launchd/cron entries.
- **sed**: BSD `-i EXT` requires the argument (so `sed -i -e …` makes
  `-e` the suffix), GNU `-i[SUFFIX]` attached-only — the same argv parses
  differently; `-n -i` empties the file; `w F` is created at parse time;
  GNU `e` and `s///e` run the pattern space as shell; `r F` reads any
  file; `-f F` takes the program from a file.
- **awk**: options only before the program text (BWK, mawk); `-f F`
  program from a file (gawk searches `AWKPATH`); the language surfaces
  of finding 5; gawk `-l`/`@load` dlopens; `-E`/`-W exec` end option
  processing.
- **uv**: clap — global options before or after the verb; `uv run` is
  NEVER a read (creates `.venv`, writes `uv.lock`, downloads, builds
  sdists, then runs the command; a URL operand is downloaded and run;
  `-` runs stdin); `uv tree`/`export`/`audit` re-lock (network, builds)
  unless `--frozen`/`--locked`; `uv venv` deletes an existing venv; `uv
  init` runs `git init`; `uv tool install` and `uv python install
  --default` put executables on `PATH`, changing what later names run;
  `--directory`/`--project` relocate everything; index URLs and
  `--allow-insecure-host` redirect the supply chain.

The report's acceptance lists — ten harmless and ten should-ask lines
per program — are folded into the suite (§I–§O of
`acceptance.toml`).

### 7.5 Probes (2026-10-09; the probe directories under `$TMPDIR`, each with a `RESULTS.md`)

Steven: "before we do any building we should do a bit of testing/probing
until we are confident we've done the best to achieve our goals." Six
probes; four reported here, P3 in §8.6, P2 (the model format) pending.

**P1 — tree-sitter-bash against our analyzer** (`probe-treesitter/`).
286 distinct lines: the suite's 127, 129 string literals from the shell
and permission tests, 51 hand shapes. Where both parse, the command
counts agree on every line. Tree-sitter parses 27 lines ours refuses —
19 are the constructs we refuse on purpose (subshells, `{ }`, `&`, `<<<`,
heredocs with an expansion inside the body, `case`, `select`, function
definitions, `[[ ]]`, `|&`, `>|`, and `$(…)` inside double quotes) and
the tree names the commands inside them; 3 are MISREADS (`fi; rm x` is
two commands, the first named `fi`; `echo $"hello"` is two arguments;
`coproc ls` is a command named `coproc`); and on every line both refuse,
the recovered tree still holds `command` nodes, so a gate on it must
refuse the whole line on `has_error()` and never read the survivors. It
keeps quoting as structure (`raw_string '*.md'` vs `word *.md`;
`string(simple_expansion)` vs `simple_expansion`) — the one thing it does
better than our source re-scans — but gives no quote-removed values
(`'it'\''s'` stays three nodes), no flags (a flag is a `word` starting
with `-`), no wrapper peeling (`sudo`, `time`, `exec`, `env` are command
names), no brace lists (only `{N..M}` is a node), no cwd, no effects;
two heredocs on one command is an ERROR. 3.0 µs a line against our 1.1
µs, both negligible; a C toolchain and 1.3 MB. Grammars: python yes
(official); awk not on crates.io; a sed grammar three days old with 46
downloads. **Verdict: keep ours as the front end.** The cheaper way to
the one gain is to make `lex()` emit quoted/unquoted spans per word; a
"second opinion, refuse on disagreement" wrapper buys subshells and
`case` later, if wanted. Two incidental observations on ours, not
security findings: `ls;;` parses as one stage where bash rejects it;
two heredocs on one command parse as one stage, and whether both bodies
reach `Stage::data` was not measured.

**P5 — `HTTPS_PROXY` in the executor's environment** (`probe-proxy/`).
A CONNECT-logging proxy on loopback; eight spellings stripped, then each
mode set. Through the proxy, host visible (`CONNECT host:443`): curl,
wget (lowercase only), git over https, gh (no `ALL_PROXY`), uv, pip,
cargo, Python's urllib. Bypass: ssh, `nc`, `/dev/tcp`, any raw socket;
and `NO_PROXY='*'` or `unset https_proxy` on the line defeats it. The
minimum set that covered every https client: `https_proxy` and
`HTTPS_PROXY` both, plus lowercase `http_proxy` for plain http, plus
`ALL_PROXY`/`all_proxy`, plus `NO_PROXY` for loopback and RFC 1918.
git over ssh IS coverable through `GIT_SSH_COMMAND` with a `ProxyCommand`.
And the confound: Claude Code's sandbox already ships exactly this layer
— an authenticated local proxy in both spellings, `FTP_PROXY` as SOCKS5,
`GIT_SSH_COMMAND=ssh -o ProxyCommand='nc -X 5 -x localhost:PORT %h %p'`,
with the seatbelt's network deny underneath. **Verdict: a cheap,
real, cooperative layer for routing, audit and the host name; never a
boundary** — on an unsandboxed host it cannot see its own evasion.

**P6 — implementation identity** (`probe-identity/`). First finding: in
this harness session `find` and `grep` are shell FUNCTIONS from Claude
Code's shell snapshots (`bfs 4.1.1` and `ugrep 7.8.4` inside the `claude`
binary, dispatched on `argv[0]`), while `which find` says `/usr/bin/find`
— so resolution must mirror the spawn path, and ours (`bash -c`, no rc)
is the one that makes the file on `PATH` the truth. Apple's `/usr/bin`
is not uniformly BSD (GNU Make 3.81, BWK awk, BSD grep "GNU compatible",
bsdtar, Apple sort). `--version` is the ONE safe generic probe: BSD
tools reject it with usage on stderr and exit 1 (`find`, `sed`, `ls`,
`head`, `xargs`, `date`); GNU, uutils, bfs, mawk, busybox (on stderr,
from the rejected probe), toybox answer on a parseable first line.
`-version` and `-V` must never be sent: `ls -version` listed the cwd,
`rg -version` searched it recursively, `sort -V` and `grep -version`
read stdin (a hang on a tty); `bmake -V var` parses the Makefile and
runs its shell. Scheme: resolve on the executor's `PATH` (27–45 µs);
key a cache by (realpath, dev, ino, size, mtime), which collapses
Homebrew's 103 `gnubin` symlinks onto one binary; on a miss run `[path,
"--version"]` once with stdin `/dev/null`, both streams bounded, `LC_ALL=C`,
a 5 s timeout (a cold exec took 3 s once); classify by a pattern table
on stdout line 1, then stderr, then the path; skip builtin names
(`echo`, `test`, `true`) and, on a Mac without the developer tools, the
`/usr/bin` shims. Warm cost 1.6–4.5 ms, once per binary. `shellcheck`
0.11.0 is installed (Homebrew, GPL-3.0-or-later); `-f json` is one array
of `{file, line, column, level, code, message, fix}`; exit 0/1 by
findings. Docketed for `:analyze` (Steven, 10-09).

**P2 — the model format: usage KDL against our TOML** (`probe-model-format/`,
`find` and `gh api` written in both). `usage-lib` 7.0.0 REJECTS every
unknown key at every level (14 of 14 probes), so it cannot be extended
in place; a single-dash-long flag (`-name`) is a hard error; the root
program's `effect` cannot be declared and `effect` has three values; no
roles, no implied facts (`gh -f` ⇒ POST), no implementation, no trust;
`allow_hyphen_values` reaches only the first value, so `-exec rm -rf {}
\;` cannot be spanned; the argv grammar is fixed (always bundling,
always interspersed) and misreads BSD find three ways. gh parsed 12 of
13 invocations right; `gh -R o/r pr list` fails in usage while real gh
accepts it. Verified on the way: real `gh --paginate pr list` dies with
`unknown command "list"` (Cobra ate `pr`), and BSD `find -f -weird .`
takes the hyphenated word as `-f`'s value. The one surprise: KDL type
annotations (`(read_file)"--input <file>"`) are accepted and ignored by
usage and readable through its own `kdl` module — a working but hacky
side channel. **Verdict: our own TOML, with a usage importer** (§9.6
decided). The four files are in the probe directory for the schema work.

## 8. Candidate design (draft, 2026-10-09)

A draft, for Steven to pull apart. §8.1 rests on §7.1–7.2; §8.3 waits on
the spec-format report; the per-program cases on the programs report.
Everything here is checked against the acceptance suite on paper; nothing
is built.

### 8.1 The decision

**A command is a set of ACTS, each on a TARGET.** A simple command, read by
its program's model (or the unmodelled reading), yields:

- one PROGRAM act: the program, the subcommand chain, the flags present
  and their values — a set of touched NODES in the program's tree (`gh`,
  `gh/api`, `gh/api/--method=GET`, `gh/api/-H`);
- one PATH act per file role, at the OPERATION the role gives it: read,
  write, delete, exec (`cat F` reads F; `cp A B` reads A, writes B; `cd D`
  and `ls D` read D; `rm D` deletes D; `bash F` and `./F` execute F);
- one HOST act per URL role, at a DIRECTION: fetch or send (`curl -d @f
  URL` sends to URL's host; `gh api` fetches from `api.github.com`, which
  the model supplies);
- one PROGRAM act for each program a flag RUNS (`find -exec rm` is also an
  `rm`; `--pager less` is also a `less`), and an UNREAD-CODE act for code
  the model does not parse (`sh -c S`, `awk S`), which asks unless a rule
  names that flag.

A stage's verdict is the STRICTEST over its acts; a line's, the strictest
over its stages — P1, and the report's "allow is an intersection". The
overrides that ask whatever a rule allows (a credential path, privilege, a
secret handle, a name that may not run what it says) become HARD shipped
rules in this scheme (§8.4), not code beside it.

**A rule names ONE target, at one node of its hierarchy, with a verdict.**

| target | hierarchy | a rule names | example |
|---|---|---|---|
| program | program → subcommand chain → flag → flag value; and flag → flag value at each level | a form: the chain, and the flags and values that must be present | `allow find`, `deny find -delete`, `allow gh pr view`, `allow gh api --method GET`, `ask gh api -H` |
| path | the file tree; a directory holds what is below it | an operation and a tree: `:read`, `:write`, `:delete`, `:exec`, `:*` | `deny :* /etc`, `allow :read ~/notes`, `allow :write build`, `allow :exec ./scripts` |
| host | domain, then URL path | a direction and a host: `:fetch`, `:send`, `:web` | `allow :fetch api.github.com`, `deny :send *` |

**Per act, the most specific matching rules decide; strictest among
them.** Specificity is strict INCLUSION of what two rules cover (§7.2):
`gh api --method=GET` ⊂ `gh api` ⊂ `gh`; `/mnt/foo/secret` ⊂ `/mnt/foo` ⊂
`/mnt`; `api.github.com` ⊂ `*.github.com`. Two rules neither of which
includes the other are incomparable, and the strictest wins. A rule on an
ancestor covers the descendants that no more specific rule covers —
which is what makes `deny find -delete` an exception to `allow find`, and
`allow /mnt/foo` an exception to `deny /mnt` (G6, G8), and what the
rooting condition of §7.2 requires. Two bounds on that (10-13, §10): a
lower tier's rule takes a higher one's place only by being stricter as
well (a lower tier only narrows), and a rule a path act meets only from
above never takes the place of one naming it (it speaks of what lies
below, so it only adds strictness).

**An allow on a program is read with CONTAINMENT, as today's path allows
are.** When the most specific rules say allow, every touched node the
form did not name must still be covered: by a node on the form's own
path (a rule on `cargo` covers everything under it — Steven's "blanket
allowed takes any flag"), by another allow (`allow find -iname` beside
`allow find -name`), by the model (a flag the model marks benign rides on
the program's allow), or by a trailing `*` on the form, which says "and
anything under this node I have not modelled". A touched node covered by
none asks (Steven's "an unknown should ask"). A deny or an ask fires on
CONTACT: it needs only its own nodes present. So `*` keeps one meaning:
it widens an ALLOW to the unknown; it is never needed on a deny.

**What is not a rule's to grant.** A program rule never covers a path or a
host act: those are judged by path and host rules, with their own
defaults (G7). A path rule never grants a program. The three defaults,
for an act no rule reaches:

- program: a MODELLED program at a read-only node, allow; anything else,
  ask (today's rule);
- path: inside the project tree or the scratch directory, allow at EVERY
  operation — the place is approved, and whether `rm` or `mkdir` may run
  there is the program act's question; outside, ask (today's rule for
  reads; for writes OPEN, §9.7 — the alternative is that a write or a
  delete inside the project needs a path grant too);
- host: ask (as Claude Code's `WebFetch`; OPEN, §9).

**Unmodelled programs.** No model, so no roles and no arity: every word is
a possible flag, value, path and host at once. An ALLOW form is read as a
LITERAL PREFIX of the argv — the words up to the first one the form does
not name must be exactly these (`allow gh pr view` covers `gh pr view
123`, not `gh pr --repo x view 123`, nor `gh view pr`) — which is the one
sound reading that needs no arity (§2, Layer 3; Codex's `prefix_rule`).
A DENY or an ASK is read on contact anywhere, every spelling a word may
have (today's reading). A flag's value cannot be checked (`gh api
--method GET` under no model is the words `--method` then `GET`, in
order, as a prefix would read them). The model is what lifts the
reading: G3.

**Worked, from the acceptance suite** (rules in the draft syntax):

- `deny find -delete` · `find . -delete -name '*.md'` → program act
  touches `find`, `find/-delete`, `find/-name`; the deny matches on
  contact → deny. Without `-delete`: no rule matches; `find` is a
  modelled read → allow; path act read `.` inside → allow.
- `allow find -name`, `allow find -iname`, `deny find -delete` · `find .
  -name x -iname y -L`: the two allows match, incomparable, both allow;
  containment: `-L` is covered by neither form, so by the model (a
  benign test: allow) or, rule-centric, by nothing (ask). **§9.1.**
- `allow gh pr view` · `gh pr close 12 --comment view`: no form matches
  (`gh/pr/close` is the chain; `view` is a value) → default: unmodelled,
  ask. `gh pr view create`: the chain is `gh/pr/view`; `create` is a
  value, and `deny gh pr create` names a chain → allow.
- `allow gh api --method GET`, `deny gh api --method POST`, `ask gh api
  -H` · `gh api --method GET -H x repos/o/r`: the allow and the ask both
  match, incomparable → ask. `gh api repos/o/r -f title=x`: the model
  says `-f` makes the method POST; the deny matches → deny. Without a
  gh model: the allow's prefix `gh api --method GET` does not match
  `gh api repos/o/r -f title=x` → ask, and the deny cannot see the
  method — the honest limit, said in the pane.
- `deny :* /mnt`, `allow :* /mnt/foo`, `deny :* /mnt/foo/secret` · `cat
  /mnt/foo/secret/a` → the innermost rule → deny; `cat /mnt/foo/a` →
  allow; `cat /mnt/bar/a` → deny.
- `allow rm *`, `deny :* ~/important` · `rm ~/important/x`: program act
  allow; path act delete `~/important/x` → deny; strictest → deny (G7).
  `rm build/x`: path act delete inside the project → allow by the place's
  default; program act allow → allow, as today. (Under the alternative in
  §9.7 it would ask until `allow :delete build` is written.)
- `allow :read ~/notes` · `cat ~/notes/a > /srv/data/a`: path acts read
  `~/notes/a` (allow), write `/srv/data/a` (no rule, outside → ask) →
  ask.
- `allow ls *` · `/bin/ls -la`: the name has a `/`, so the program act is
  an EXEC of the file `/bin/ls` — a path act with no rule → ask; `allow
  :exec /bin` would cover it, and the Layer 2 resolution (§9.4) may
  trust system directories by default.

### 8.2 The rule line

One line, one target, one verdict — written to look like the thing it
names:

```text
<allow|ask|deny> <program> [subcommand ...] [-flag | --flag=VALUE | --flag VALUE ...] [*]
<allow|ask|deny> :<read|write|delete|exec|*> <path-or-glob>
<allow|ask|deny> :<fetch|send|web> <host-or-glob>[/path]
<allow|ask|deny> re:<regex>                     — the advanced tier, as today
```

What changes from today's grammar, and why: a bare word after the program
is a POSITION in the subcommand chain, not a selector that may sit
anywhere (the 10-07 findings); a path word in a program rule is gone —
paths are their own target with an operation, so a program rule cannot
make one decorative (the `*` finding); `*` widens an allow to the unknown
and nothing else; `**` is not needed to say "and below", since a
directory holds what is below it (G6) — `/mnt` and `/mnt/**` are one
rule, and a glob is for the name, not the depth; `:readonly` and the
effect classes go, replaced by the operations on paths and the model's
classes on programs; `prog:-flag` sugar and `-a,-b` lists can stay or
go (open; nothing in the goals needs them). The `[secrets] paths` list
becomes shipped HARD path rules (`ask :read ~/.ssh`, `deny` under
`path_policy = "deny"`), one grammar and one dialect instead of two —
with `\c` for the case-folding those entries have today.

### 8.3 The model

**Our own TOML schema**, one file per program, in the user's config
directory (`models/<name>.toml`) or a project's `.shannon/models/`, and
the same files shipped for the verified set (§7.3). Vocabulary aligned
with usage's (`cmd`, `flag`, `arg`, `var`, `require_equals`,
`value_optional`, `double_dash`, `effect`) so a `clap_usage` export or
a KDL spec imports near-mechanically; importers for Fig, carapace, Click.

What a model says (the field set from §7.3, by level):

- **program**: `family` switches — `bundling`, `single_dash_long`,
  `long_abbrev`, `interspersed` (with the `POSIXLY_CORRECT` override),
  `double_dash` (`terminator` | `rev_path_separator` | `inner_argv` |
  `ignored`), `plus_words`, `numeric_options`, `first_word_bundle`
  (tar, ps), `at_file` (`none` | `value_read` | `argv_expand`);
  `implementation` (gnu | bsd | bfs | bwk | gawk | …) and how it is
  recognised; `trust` (`completion` | `generated` | `author` |
  `verified`; `generated`, added 10-27, marks the shipped models carried
  over from the old registry and not yet checked by hand — it decides as
  `verified` and says what it is) and what it is verified for; `effect` of the bare program; implicit `hosts` and
  `paths` (config files read); `env` that selects a program; `examples`
  (`match` / `not_match`) checked when the model is read.
- **command** (a subcommand node, nested): its own flags and
  positionals, `effect`, aliases, `external` (an unknown word is a
  program: git, cargo, gh extensions), `argv_at` for a node that carries
  another command line (`uv run CMD`, `docker run IMAGE CMD`, `kubectl
  exec -- CMD`, `find -exec … ;`), `inner_is_code` for `sh -c`.
- **flag**: spellings; `arity` (0, 1, n, `optional_attached`, `until`
  a terminator); `global`; `last_wins` or `accumulate`; the VALUE's
  `role` (`read_file`, `write_file`, `delete`, `dir` (searched), `list`
  (listed: names only, 10-11), `exec_path`,
  `program`, `code`, `argv`, `url`, `host`, `enum`, `kv`, `pattern`,
  `data`); `effect` it raises the command to; `implies` (gh `-f` ⇒
  method POST); `switches` (sed `-e` moves the script out of the
  operands; `-jar` changes the next word's role; `python -c` ends
  options).
- **positional**: `index` | `rest` | `last`; `role`; `shifted_by` a flag;
  `kv` keys to roles (dd, make).

A sketch, the `gh api` node, so the shape can be judged (illustrative,
not the final syntax):

```toml
[program]
name = "gh"
family = { bundling = true, interspersed = true, double_dash = "terminator" }
trust = "verified"
effect = "read"                      # every call is network: fetch
hosts = ["api.github.com"]           # implicit; `--hostname` and GH_HOST retarget
external = "extension"               # an unknown first word runs gh-<word>

[command.api]
effect = "read"
positional = [{ role = "endpoint" }]
examples.match = ["gh api repos/o/r/pulls", "gh api -X GET search/issues -f q=x"]

[[command.api.flag]]
names = ["-X", "--method"]
arity = 1
role = "enum"
last_wins = true

[[command.api.flag]]
names = ["-f", "--raw-field", "-F", "--field"]
arity = 1
role = "kv"
value_read = "@"                     # `k=@file` reads and sends the file
implies = { method = "POST" }        # unless --method is given (verified in gh's source)

[[command.api.flag]]
names = ["--input"]
arity = 1
role = "read_file"
implies = { method = "POST" }

[[command.api.flag]]
names = ["--hostname"]
arity = 1
role = "host"
```

Under it, `deny gh api --method POST` fires on `gh api repos/o/r/issues
-f title=x` because the model derives the method; `allow gh api --method
GET` covers `gh api -X GET …` because `-X` is a spelling; and a rule the
model cannot read as a node of this tree is refused when written, as a
rule naming a flag `find` does not have is today.

**Trust.** A `verified` model (shipped, or marked so by the user) backs
allows and denies. An `author` model — one a tool's README ships —
backs both for the user who installs it: their choice, shown in
`:rules`. A `completion` model (imported from Fig or carapace) backs
only denies and asks: a candidate reading is enough to refuse, not to
grant. The model's trust is said beside every verdict it decided.

### 8.4 Layers, tiers, and the overrides

Rules come from: shipped, the user's global config, a project's config, a
`:source`d file, the session (`:allow`, `A`). Per §7.2: specificity acts
within a tier; across tiers the sort key decides (§9.3 is the choice of
shape). The four overrides that today sit in code after the rules
(`permission/mod.rs:957–1067`) are expressible as shipped HARD rules once
paths, exec and hosts are targets: a credential path (`ask :read` on
each entry); privilege (`ask sudo`, `ask doas`, …, hard — and `deny sudo`
becomes writable, which today's grammar refuses); a secret handle (stays
code: it is a value, not a target); a name that may not run what it says
(an env hazard, a disturbed binding — stays code, as a property of the
stage).

### 8.5 Against the goals

- G1: a modelled program asks only at the nodes a rule or its model marks;
  the pile of prompts moves to unmodelled programs, where it belongs,
  and G3 is the way out.
- G2: tiers by file; §9.3.
- G3: one model format, shipped and user-written; §8.3.
- G4: a rule at any node of the program tree, including a flag value.
- G5: three targets, one line shape, one glob dialect.
- G6: inclusion specificity on the path tree; a directory holds what is
  below it.
- G7: a program act never covers a path act.
- G8: rules at different nodes compose by strictest-across, and an
  ancestor's allow covers what no exception names.
- G9: the structured line, and `re:` beneath it.

What it costs: `allow rm *` no longer grants the files `rm` touches (§8.1,
G7 literally); every user-installed tool is unmodelled until someone
writes its model, so its allows are prefixes; the engine's path matching,
containment and contact are rewritten around acts rather than patched.

### 8.6 P3 — the design, run (`probe-acts/`, 2026-10-09)

A throwaway crate (~1,100 lines) over shannon-core's `parse_line` for
Layers 0–1, with its own Layer 3 reader, toy models (find, gh, git, sed,
awk, uv, curl, bat, the coreutils, sh), the rule reader (today's
spellings translated plus `:op PATH` and `:fetch HOST`), and §8.1's
decision: acts per stage; per act the most specific matching rules by
inclusion, strictest among them; tiers (user over shipped); shipped
asks of two kinds — CARVINGS a blanket lifts (`find -exec`) and GUARDS
only a rule at least as specific lifts (credential paths, `rm
--no-preserve-root`, writes into `.claude/`, a node that prints a
secret); containment for an allow; the defaults of §8.1 with Steven's
Q7 (inside the project: read and write allow, delete and exec ask).

**Result: 127 of 127 suite cases hold** (five expectations flipped by
the 10-09 answers, one by a finding below), **and 26 of 26 adversarial
lines**: an exception under a deny (`deny find` + `allow find -name`);
`find . -exec cat {} \;` allowed under `allow find *` while `-exec rm`
asks, with or without `allow rm *` (Q7); `git -c core.pager=less log`
asking under `allow git *` (today it runs); the credential guard
standing against `allow :read ~` and lifted by `allow :read ~/.ssh`
(Q9); incomparable path rules falling to the strictest; `--web` under
`allow gh pr view *` asking; a redirect or `-o` outside the project
asking under an allowed fetch; `make test CC=/tmp/cc` asking past the
prefix of `allow make test`; `gh x` asking under `allow gh *`; a write
into `.claude/` asking under `allow :write .` and allowed under `allow
:write .claude` (Q13); `allow rm build/*` as a scoped grant that covers
`rm build/a.o` and not `rm -r build`.

Three findings, folded into §8.1: a rule's flag spelling must be
resolved through the model (`-H` is `header`), which the reader does
and today's grammar does for modelled programs; a flag whose VALUE
names a program (`--pager less`) and a flag that runs a program named
by configuration with no value (`--web`, `-p`) are each an act of their
own, so a blanket never covers them; and `gh alias set --shell` under a
blanket is allowed, because the stored alias runs at `gh x`, an unknown
word, which asks then. The probe's rules translate every spelling the
suite uses, which is the migration a user's config will need.

### 8.7 P4 — the diary replayed: real prompts before and after (2026-10-09)

**The question.** G1 is a rate. The suite says whether a case decides as
intended; it cannot say how often a real session prompts, or which
prompts the draft removes and which it adds. The one real-usage record
is Shannon's diary (`~/.local/state/shannon/log/shannon.log.<date>`),
whose three lines per gate decision say what today's engine did: ran it
unprompted ("allowed without prompting"), prompted and the human
allowed ("allowed once"), prompted and the human denied. The audit
trail records `run` for both of the first two and the episode store
stores them alike, so neither can give the rate. Read with Steven's
explicit yes (10-09), restricted to those lines; the extraction and
both listings are in `$TMPDIR/probe-p4/`.

**The corpus.** 251 tool calls, 2026-07-25 to 10-09 (18 days with any),
four projects. 244 are shell lines; 7 are provider tools rendered as
commands (`Skill`, `WebFetch`, `WebSearch`, `ToolSearch`), which today's
gate asks on as unknown programs — not shell, not decided here. 73 of
the shell lines are the Claude Code backend's Read and Write, which
reach the gate as `cat 'path'` and `cat > 'path'`. One session of 144
calls dominates (a Claude Code session driven through the gate on
10-05); the rest are short dogfooding sessions, many of them Steven
testing refusals — so the 29 human denials are mostly tests, not
judgements. Starting directories come from the episode→project map;
for ten deleted episodes they are guessed from the date (marked in the
listing). Today's effective policy was the shipped defaults alone (no
`[permissions]` in the config).

**The replay.** `probe-acts --replay`: each line decided by §8.6's engine
under (A) the shipped rules alone, and (B) plus five user rules this
user would write — `allow cargo *`; read and write of the executor's
scratch directory (`/private/tmp/claude-<uid>`, the backend sandbox's
`TMPDIR`; §8.1's "scratch directory" grant); read and write of
`~/.config/shannon/projects`, a stand-in for the grant Shannon's own
memory tool will carry. The toy models grew from 10 to 30 programs for
it (rg, fd, sort, uniq, cut, tr, xargs, timeout, python3, cargo, date,
sleep, du, chmod, …): with the 10, (A) was 154 prompts. **The model is
the bottleneck, measured: twenty one-line models moved 49 prompts.**

| today \ draft    | A allow | A ask | B allow | B ask | lines |
|------------------|--------:|------:|--------:|------:|------:|
| ran unprompted   |      56 |    13 |      63 |     6 |    69 |
| asked, approved  |      77 |    68 |     104 |    41 |   145 |
| asked, denied    |       6 |    23 |       6 |    23 |    29 |
| sent to edit     |       0 |     1 |       0 |     1 |     1 |
| **prompts**      |         | **105** |       | **71** | **175** today |

No line is denied by the draft.

**Reading the table (Steven, 10-09).** The approvals are not judgements.
They were given under the condition this redesign exists to end —
alarm fatigue; most of the Claude Code work ran in auto mode because
the rules it needed could not be written — so "asked, approved → allow"
is an ALARM REMOVED, not a false alarm confirmed, and the denials are
mostly refusal tests. The data were generated under the policy being
replaced: a prompt rate measured on a fatigued approver's approvals
says nothing about which approvals were right (the off-policy trap;
selection on the outcome). What the table can say is which alarms the
draft removes and what the remaining ones are made of. **The measure
that matters is alarm precision**: of the alarms that remain, how many
are decisions — a delete, an exec of code, a secret, a read or write
outside, a line the front end cannot read — and how many are noise — a
`~` the gate could expand, an environment variable Shannon itself sets,
a flag the model lacks. Today: 175 alarms, ≈104 of them noise by that
reading (reads and writes inside the project; `cargo` under a blanket
the user would write). The draft under B: 71 alarms, ≈60 decisions and
≈11 noise (7 placeable expansions, 4 model gaps). Turning 100 alarms
into 10 where one in ten deserves a deny, instead of one in a hundred,
is a tenfold improvement even if the user still accepts most of them —
alarms are not to be removed but to be made worth reading. The
decision/noise split is ours, not ground truth; it is the lens the
suite's cases are written through, and the plan's measure.

Two of Steven's remarks belong here. `CLAUDE.md` instructions have no
teeth (the assistant keeps slipping — a `cd` in a build command this
very session); the narrower, mechanical subset of such instructions is
exactly what a rule can enforce, and the grammar is the teeth. And his
friend in security: "an LLM with access to arbitrary bash and any URL
is probably a bad idea" — there are layers, and the levers must be
pullable by the user to balance security against utility; the goal is
to hand users those levers (§1's posture).

**Alarms removed** (104 of 145 approvals, under B): reads
under `~/…` paths inside the project (`rg … ~/src/…/src` — today
asks because an unexpanded `~` is unplaceable; the draft expands it);
the backend's writes inside the project (23 `cat > 'file'`); the `cd …
&& cargo …` lines under the blanket (23); reads of the scratch
directory; pipelines of modelled reads (`find … | sed | sort | uniq -c |
sort -rn`); `git log`, `git status`, `sleep`, `grep -r … --include=`.

**What still asks** (71 under B), by cause: reads outside the project
(15: `/etc/hosts`, `~/src/a-project`, `~/.claude/skills`, the
backend's memory directory — Steven's 10-05 rule, and today asks too);
code from stdin (`python3 - <<'EOF'`, 5), `python3 -c` (2), and an exec
of a script inside the project (3; Q7); stream-fed argv (`… | xargs
wc`, 5); a secret handle in the line (4, hard); `while read file; do rm
…` loops (4 — `read` is unmodelled, and the loop deletes); an expansion
the gate cannot place (`$TMPDIR`, `$HOME`, `${XDG_STATE_HOME:-…}`, 7);
modelled writers with no rule (`git add`, `git commit`, `sed -i`, 4 —
§8.1's program default); deletes (5: a file inside, `/tmp/scratch`
outside; Q7); constructs the front end refuses (`&`, a subshell,
`$((…))`, `$(…)`, an unterminated quote; 6); `find -delete`/`-exec`
carvings (2); `gh push --force` (an unknown `gh` word, 1).

**New prompts** (6 under B, 13 under A): the backend's own memory files
under `~/.claude/projects/…/memory` (4 — the `.claude` write guard and
the outside-read default; a shipped grant for the backend's memory
directory, as for Shannon's own, makes it 0, and Shannon-native sessions
never see it); `find … | xargs wc -c` and `find … | xargs cat` (2) —
today reads the producer and allows a read program fed from an inside
`find`; the draft's "stream-fed argv asks" (§8.1) does not. OPEN: a
stream from a modelled read of inside paths feeding a modelled read
program — the `Stage::data` item on the docket.

**The human said no; the draft would run** (6): `gh api user` ×3 — a
GET, allowed under the implicit-host concession (§10, 10-09). Steven,
10-09: denied because he did not know what it did — `gh api --help`
never shows `user`; it is GitHub's REST `GET /user`, "get the
authenticated user", documented on GitHub's API pages and nowhere in
`gh`. So the question is less whether `gh api` should ask than what the
ask must SAY: the model knows the method (GET), the host
(`api.github.com`), the path, and can carry a one-line gloss per known
endpoint — "the authenticated user's profile" — so the person deciding
is not deciding blind. Whether a network read that returns account
identity asks until a host rule names `api.github.com` stays Steven's
call. The other three were refusal tests (`sleep 300`; `grep … | head`;
`pwd && echo "---" && ls -la | head`).

**Probe fixes made for the replay**, each a design fact folded here: a
secret handle anywhere in the line is a HARD shipped ask (§8.4 said so;
the probe lacked it and allowed `echo` of a handle); `echo`'s dashed
words are text (`echo -----`), a model property (`dash_data`); `cargo
build/test/clippy/doc/run` are execs of the project's code (build
scripts, proc macros, tests), asking until `allow cargo *`; `git add`
and `git commit` write `.git`, a placed write; `git commit -q`.

**Design items the replay surfaces** (for the plan):
1. **Known-environment substitution.** The executor's environment is
   Shannon's (§9.11: `env_clear` + a KEEP list), so `$HOME`, `$TMPDIR`,
   `$USER`, `$XDG_*` that Shannon itself passes can be substituted at the
   gate before placement — 7 asks here, every one approved.
2. **Stream-fed argv from an inside producer** (above; 2 regressions).
3. **The writer default is deliberate, and must be said.** `cat > x`
   allows inside; `tee x` and `sed -i x` ask (§8.1: the program act
   decides; the suite's `sed-in-place-asks-by-default`). Keep it, and
   have `:analyze` say "a writer with no rule" so the user writes one.
4. **Implicit hosts** (`gh api`) under the concession — Steven's call.
5. **Shipped grants for Shannon's own surfaces**: its memory directory
   per project, the backend's memory directory, the executor's scratch
   directory. Today these are approved places in code (`memory_write`,
   registered dirs); in the new grammar they are shipped `:read`/`:write`
   grants that `:rules` lists.
6. **The models are the work.** Every remaining "unknown words" ask is a
   model gap, and the toy models are unverified (BSD `date` with an
   operand would pass the toy; the real model carries today's regex's
   knowledge).

**Caveats.** One user, one machine, nine active days, one rule set; the
big session is Claude Code's own command style (long `cd … && …`
chains); starts guessed for ten lines. A sanity check of the design
against real lines, not a benchmark — but it is the first number G1 has.

## 9. Open questions for Steven

1. (§6.2) With a model, do benign modelled flags ride on a program allow, or
   does only a named flag count?
2. (§6.1) A definition of "more specific" for globs, or shall we take the
   one the research recommends (gitignore / nginx / Windows ACL precedent)?
3. (§6.3, §7.2) What a project file may do. Three shapes, from the prior
   art:
   - **Trust-ordered tiers, a lower tier only narrows** (IAM boundaries,
     Landlock, Claude Code): a project `deny`/`ask` applies at once; a
     project `allow` counts only where no higher tier speaks, and only
     after a trust step that shows what it would widen. Your literal
     answer; simplest to explain; the cost is that a project cannot carve
     an exception out of a SHIPPED default either, unless shipped is
     below project.
   - **Soft and hard** (CSS): every rule is soft unless marked hard; among
     soft rules the nearer layer wins (session > project > global >
     shipped), so a project may carve shipped and global soft rules;
     among hard rules trust wins (managed > global > project) and nothing
     soft touches them. Your Q3 holds if your global `deny` is hard by
     default. More expressive; a second concept to teach.
   - **Numeric priority** (Gemini, AppArmor): explicit, total, and the
     report's "what not to copy".
   Either way: must the agent's writes to `.shannon/` always ask (it
   can grant itself rules otherwise), and is an untracked local file
   yours (Claude Code: unless git tracks it)?
4. (Layer 2) A program resolved to a system directory keeps the name's
   trust; one resolved into the project or home is an exec of a file.
   Acceptable? Which directories are "system"? Note `uv` is
   `~/.local/bin/uv` on this machine, and Homebrew's tools are under
   `/opt/homebrew/bin`: the trusted set is probably "every directory on
   `PATH` when Shannon started, less relative entries and the project tree",
   with a setting to narrow it. And `[exec] shell = "zsh"` sources
   `.zshenv`: does Shannon say so at launch, or pin `bash`?
5. (Layer 4, web) The default for a host no rule names: ask, as Claude
   Code's `WebFetch` does? Then `allow curl *` reaches nowhere until a host
   is allowed — correct, or surprising?
6. (G3) Model format: our own TOML with importers, or adopt a format with an
   existing corpus (Fig / usage / carapace)? Waits on the research.
7. (§8.1) The path default inside the project: allow at every operation,
   the program act deciding whether `rm`/`mkdir` run (recommended — keeps
   `allow rm *` running `rm build/x`, as today), or writes and deletes
   needing a path grant even inside?
8. (§8.1, §8.2) `*` on an allow means "and the nodes under this one that
   nothing covers". So `allow cargo test` covers `cargo test` with flags
   the model knows (none, for an unmodelled cargo), and `allow cargo test
   *` covers any — the suite's `specific-form-unknown-flag-asks` and
   `gh-pr-view-allowed-with-any-flag` both hold under it. Agreed, or
   should a subcommand allow be blanket without the `*` (Claude Code's
   reading)?
9. (§8.2, §8.4) The `[secrets] paths` list as shipped hard path rules —
   one grammar and one glob dialect — with `paths_except` as the more
   specific allows beneath. Agreed? It removes the second dialect, and
   `:rules` would list them.
10. (§7.3, §7.4) **Implementation identity.** `find` is BSD here, GNU on
    Linux, `bfs` in your zsh; `awk` is BWK here and gawk there, and BWK
    ignores `--sandbox`. The model is keyed by implementation; the gate
    resolves the name on the executor's `PATH` at decision time (§9.4)
    and reads the implementation off the file (`--version` once, cached
    by path and mtime? the install directory? Homebrew's `g*` names?).
    Which, and what when it cannot tell — ask, or the BSD reading?
11. (§7.4) **The executor's environment.** Set, by default and
    overridable in `[exec] env`: `GIT_PAGER=cat`, `GH_PAGER=cat`,
    `PAGER=cat`, `GH_NO_UPDATE_NOTIFIER=1`, `GH_PROMPT_DISABLED=1`,
    `GIT_OPTIONAL_LOCKS=0`, `NO_COLOR=1`? It makes reads deterministic
    and keeps a pager — a program named by config — from running.
12. (§8.3) **Model trust.** Shipped models are verified by us; a model
    from a tool's README is `author` and backs allows for whoever
    installs it; an imported completion spec backs only denies and asks.
    Agreed? And where do a user's models live — `~/.config/shannon/
    models/`, `.shannon/models/` (trust-gated like rules)?
13. (§7.4) `gh skill install --agent claude-code --scope project` writes
    into `.claude/skills` of the repository, and `uv tool install` puts a
    new name on `PATH`: both change what the harness runs next. Should a
    write into the harness's own instruction surfaces (`.claude/`,
    `.shannon/`, `~/.config/shannon`, the tool bin dirs) be a shipped hard
    ask whatever program does it?

## 10. Decisions log

- 2026-10-07 — a trailing `*` on an allow is PRESENCE: `allow rm build/* *`
  must not allow `rm notes/a`, does allow `rm build/a notes/a`; a rule ends
  where a command does.
- 2026-10-08 — goals G1–G9 and answers Q1–Q8 (§1).
- 2026-10-09 — the acceptance suite is the measurement; the shell-structure
  study (§2–§4) is the ground the grammar is built on.
- 2026-10-09 (Steven, on §9; the research reports are in `docs/Research/`):
  - §9.2 specificity is strict inclusion, the strictest on incomparables
    — the research's recommendation, taken.
  - §9.3 trust-ordered tiers, a lower tier only narrowing — leaning;
    soft/hard not ruled out.
  - §9.4 program identity as proposed. That `[exec] shell = "zsh"`
    sources `.zshenv` is documented, not announced; letting the user
    choose which rc files the agent's shell sources is a later setting.
  - §9.5 the web default is ASK, and `allow curl *` asks on an unknown
    host — never a deny, which would disable `curl`.
  - §9.6 model format: leaning to an existing format; learn more first.
  - §9.7 NOT allow at every operation inside the project: `rm -r src/`
    destroys the project, and the cost is high — to be resolved
    (proposal in the exchange of 10-09).
  - §9.8 `*` covers what nothing else covers — the least strict thing
    in a rule; `allow find *` loses to `deny find -delete`.
  - §9.9 the credential list as shipped path rules — and shipped rules
    never impose: a user who wants the agent in `~/.ssh` allows it.
  - §9.10 implementation identity: my judgement; Linux and macOS both.
  - §9.11 the executor's environment: yes.
  - §9.12 model trust tiers: yes.
  - §9.13 writes into the harness's own surfaces: shipped SOFT asks.
  - Concessions: the web category may centre on the fetch tool rather
    than every program that reaches a host; `awk`, `sed`, `python -c`
    are languages — reasonable coverage, not completeness; a novel
    program may stay unmodelled, provided the format lets its author
    model it.
  - Method: probe before building; nothing is built until the probes
    say the goals are met.
- 2026-10-09 (the probes, §7.5 and §8.6): keep our shell front end,
  giving `lex()` quoting spans (P1); the proxy environment as a
  cooperative layer, both spellings of `https_proxy` and lowercase
  `http_proxy`, never a boundary (P5); `--version` once per binary,
  cached by (realpath, dev, ino, size, mtime), as the identity probe,
  never `-version` or `-V` (P6); the model is our own TOML with a usage
  importer — §9.6 decided (P2); the acts design decides every suite
  case and 26 adversarial lines over toy models (P3). Steven, 10-09:
  shellcheck as an advisor in `:analyze`; the harness's attribution
  trailer on commits is his to rule on (left off).
- 2026-10-09 (P4, §8.7) — with Steven's explicit yes, the gate lines of
  the diary (never the episode store, which cannot tell a rule-allow
  from an approval) replayed through the draft: 175 prompts today → 105
  under the shipped rules → 71 under five user rules; 6 new prompts (4
  the backend's memory directory, 2 `xargs` fed by an inside `find`); no
  denies; `gh api user` the one human-denied line the draft would run.
  Folded in: a secret handle in the line is a hard ask; `cargo` builds
  are execs; known-environment substitution, stream-fed argv from an
  inside producer, and shipped grants for Shannon's own surfaces go to
  the plan. Steven, 10-09: no `Claude-Session:` trailer on commits — "it
  is noise".
- 2026-10-09 (Steven, on reading P4) — approvals are not judgements:
  they were made under alarm fatigue, under the policy being replaced;
  the replay counts ALARMS REMOVED, and the measure going forward is
  ALARM PRECISION (of the alarms left, how many are decisions). The
  grammar is the teeth `CLAUDE.md` lacks: the mechanical subset of
  working instructions becomes rules. `gh api user` was denied as an
  unknown endpoint (not in `gh --help`; GitHub's `GET /user`): the ask
  must say what the model knows. Then: plan mode.
- 2026-10-09 — plan approved (phases P0–P7, `docs/Milestones/Milestone
  4.md`). Answered for it: implicit hosts stay NOT acts (the concession;
  the pane shows method, host, endpoint, gloss); `prog:-flag` and `-a,-b`
  translate forever, class rules (`:readonly`…) become a load note; the
  project `.shannon/config.toml` and `.shannon/models/` are in scope,
  gated by the trust store. P0 landed: section Q (the 26 adversarial
  lines) in the suite — `153 — 91 hold, 49 gaps, 13 unspellable` today;
  §9.11 built — the executor's environment is one table
  (`exec::child_env`: the allowlist, then `GIT_PAGER=cat`, `GH_PAGER=cat`,
  `PAGER=cat`, `NO_COLOR=1`, `GH_NO_UPDATE_NOTIFIER=1`,
  `GH_PROMPT_DISABLED=1`, `GIT_OPTIONAL_LOCKS=0`, then `[exec] env`, later
  wins), which the gate will read for known-environment substitution.
- 2026-10-10 (P4 shipped: P4a the rule line, P4b the decision, P4c the
  switch) — `RuleSet` decides over acts; the old engine, `glob.rs`, the
  `secrets/paths.rs` matching and the `Cause`/`Unaccounted` shapes are
  gone. `158 — 158 hold, 0 gaps, 0 unspellable` (the 26 adversarial
  lines, the 4 unspellable given rules, `specific-form-unknown-flag-asks`
  split in two, new `q-unmodelled-*` cases). Steven, 10-10: **the grammar
  over the models** — "we won't be able to model most programs, nor should
  we; the defense comes from the language"; an unmodelled program's allow
  is its bare words as a prefix with its flags anywhere, its deny or ask
  contact (a one-letter flag meets a short cluster holding it, a flag
  written with a value reads the next word as one); **held-out models**
  (`bat`, `mkdir`) live only in the acceptance harness, as the proof a
  user can model a program and the language then says the right things.
  Calls made in the switch, for Steven to confirm or reverse: carvings are
  not rules (a flag past a read is never benign, so `find -exec` asks
  beneath `allow find src` without a shipped line; `[permissions] remove`
  of one is a note naming the allow that lifts it); hosts are acts
  (`:fetch`, `:send`, `:web`; the web asks by default, implicit hosts stay
  the pane's gloss); `*` after a program is the blanket with paths judged
  where they are, `**` grants them anywhere; a directory holds what is
  below it AND itself (`build/**` ≡ `build`, so `allow rm build/**` covers
  `rm -rf build`; `build/*` does not); code the model does not read is a
  hard ask no rule covers; a privilege wrapper is a hard ask that a deny
  of yours turns into a refusal (`deny sudo` fires now); what `find -exec`
  or `rg --pre` runs is an act of its own under any blanket; a glob is
  placed whole and judged as the entries it picks (a grant of `/srv/out`
  covers `rm /srv/out/*.o`, a `re:` grant reads it as text), and for a
  deny or an ask it is met by CONTACT — the pattern and the glob could
  name one entry (`cat .en*` meets `.env*`, `cat ~/.ss?/*` meets `~/.ssh`,
  and a bare `*` meets `*.pem`: `rm build/*` asks — an alarm-precision
  cost to measure in the replay); a shipped credential guard is lifted by
  a rule naming the file, which is what `A` proposes; a flag's value in a
  program rule is text as written (`deny tar --file=/etc/*`), the path
  rule follows the `cd` (`deny tar /etc/*`); `find -name '*.pem'` reads
  no file, so no guard meets it; a hidden flag (`$X`, a bare glob) is a
  hard ask only where a flag could matter (`rm`, `find`, `grep`), to `cat`
  or `ls` the word is only unplaced; a start Shannon could not say places
  nothing a background job may run alone; `/dev/null` and the std streams
  are no file (a rule naming them meets nothing); `:read` and `:write`
  take paths (`deny :write` alone is refused), the class heads are
  refused with the rule to write instead; GNU long options abbreviate
  (`rm --no /` is `--no-preserve-root`; `long_abbrev` on the getopt_long
  models). Docketed, not built: glob-vs-regex contact (a `re:` deny reads
  the glob as text); the hidden-flag ask on `rm *.o`.
- 2026-10-10 (the replay over the switched engine, §8.7 again) — the
  probe's replay is now `crates/shannon-core/examples/replay.rs`, run
  over the same gate lines (scratch, never committed): 244 shell lines,
  175 prompts today. Shipped rules only: **112 asks, 0 denies** (the probe
  said 105); with the five user rules: **99 asks, 0 denies** (the probe
  said 71). The gap is the shipped models, not the grammar: the probe's
  toy `rg` had no guard, and the shipped one guards `--no-ignore-vcs`
  ("reads what .gitignore hides") — **25 of the 99** are that one flag on
  lines Steven approved; `allow rg --no-ignore-vcs` lifts it (his call:
  the alias makes every `rg` carry it). The rest: reads outside the start
  (18 — place rules), `./scripts` execs inside (7 — `allow :exec`), the
  xargs stream (5), hidden flags (5), `cat -v` unknown to the model (2),
  `git add`/`git commit` writers (3). The 6 new prompts are as predicted:
  4 the backend's memory directory under `~/.claude` (P6a's shipped
  grant) and 2 `xargs` fed by an inside `find` (docketed). The 6
  human-denied lines the engine runs: `gh api user` in three spellings
  (the model knows the endpoint — the concession stands), a `git log |
  sort | uniq | awk` pipeline, `pwd && ls -la | head`, and a July-format
  `{{secret:…}}` that is not a handle today. Alarm precision: of the 99,
  25 are one guard the user can lift, 18 are places to grant, and the
  remaining 56 are the engine's genuine questions.
- 2026-10-10 (the review of the switch: A correctness, B robustness, C
  tests, E security — E stopped by a classifier at its second lead, so
  leads 3–8 of its brief are unrun) — four CRITICALs in the switch, all
  reproduced against the engine and fixed with regression tests and
  acceptance cases (`r-*`): (1) `writes_granted` was computed once per
  STAGE from every write path act, redirects included, so the shipped
  scratch grant ran `git reset --hard > /tmp/scratch/log` unasked while
  the same command without the redirect asked — grants are now per
  program, from the files that program itself names, never a redirect,
  and a writer that names no file is granted nothing; (2) a bracket class
  is the shell's (`.[.]/x` is `../x`, `.[]e]nv` is `.env`) and `placed`
  read `[` as a letter, so `cat .[.]/.ssh/config` was Allow and safe
  unattended — a dot-led segment with a class is placed nowhere, and
  `globs_meet` reads classes as bash does; (3) `${NAME:-default}` was
  read as the default for any NAME not in the executor's table, but the
  shell sets `_`, `OLDPWD`, `PWD` (after a `cd`) and `for` variables —
  `echo ~/.ssh/config; cat ${_:-README.md}` was Allow and safe
  unattended, `cd ~/.ssh && cat $PWD/config` likewise — a name the gate
  does not know is never read as unset, and a shell-owned name is never
  read from the gate's table; (4) the sed reader split on `;` before
  reading `s`, so `s/a;b/c/w FILE` and `s/x/y;z/e` read as prints — the
  script is now read as sed reads it, with unknown letters failing closed
  as code. HIGHs: `cluster_holds` sliced every word at byte one (a panic
  on `frob éé`) and read a bare word as a cluster (`deny make -k` denied
  `make check`); the privilege hard ask was skipped for inner argvs
  (`find . -exec sudo ls \;` ran under allows on both, and deleting the
  guard passed every test); an exec grant leaked to every program act in
  the stage; unbounded recursion on `git `×8000 or a 256k glob aborted the
  process (a word over 4 KiB or a line over 64 KiB is unsound before
  reading, inner argvs stop at depth 8, `glob_match` and `globs_meet` are
  iterative); the rule-side leading-dot check in `globs_meet` refused
  `cat .x*` against `*.pem` (unsound: a pattern's `*` matches a leading
  dot); a deny on an inner program was cut down to the outer (`deny uv run
  rm` denied `uv run pytest`); `host_of` missed a trailing dot and IPv6.
  **Steven's three decisions (10-10):** shipped denies stay LIFTABLE by a
  more specific allow (no `!` grammar needed yet); contact ASKS and never
  denies — a deny met only by a glob's contact or from above is an ask,
  an exact match stays a deny (`cat /etc/*` under `deny cat /etc/*`); and
  `DIR/**` is what is BELOW the directory for a delete — `allow rm
  build/**` does not cover `rm -rf build`, `allow rm build` does, `allow
  rm ./**` never covers the project — while a read of the directory is a
  read of what is below it and is covered (the `dir/**` ≡ `dir` call of
  the morning is reversed). Also from the round: the operation ladder and
  the `above` softening now have core tests (they were held only by the
  manual's table); `bash /etc/x.sh` is an exec of the script, so `deny
  :write /etc` stops it; `if=~/x` and `KEY=~/x` arguments expand their
  tilde; `<(…)`'s `$()` placeholder is placed nowhere; `[exec] env` names
  are unknown to the gate (`RuleSet::with_exec_env`); `acts::normalize`
  is `pipeline::normalize_path`; a rule of over 64 alternation readings is
  refused. Docketed from the round: 105 of 137 deciding tests and the
  whole manual table run on the no-start stand-in production never has
  (give `set()` and the harness a start — P5); a `[secrets]` entry with
  `[`/`{`/`**` becomes a regex whose contact ignores globs (`cat
  vault/?.txt` under `vault/[ab].txt`); awk scanning and the proposal's
  `reads_as_a_grant` are quadratic; `policy()` is rebuilt per decision;
  `{}` is placed as the start, not `<dir>/*`; the pane says "asking by
  default" for an act allowed by a grant; E's leads 3–8 (inner argvs and
  wrappers under allows, guards with `except`, `credential_path` vs
  `decide_line`, every `By::Default` arm of `safe_unattended`, proposal
  width, the pane's explanations, secret-handle coverage) — to rerun in
  narrower scopes.
- 2026-10-10 (the second round: E's leads 3–8 rerun as two CONSISTENCY
  reviews — invariant framing, no adversary vocabulary, benign operands —
  and neither was stopped; the permutation is the way to run security
  rounds from here) — two CRITICALs: (1) a handle in a `for` list, after
  a `{` that quote removal glued to it, or in a comment was spent by the
  executor and never seen by the gate, which scanned the parser's fields
  and not the text (`for x in {{tok.X}}; do echo $x | tr a-z n-za-m; done`
  was Allow and safe unattended) — `decide_line` now scans the RAW line
  and the handle scanner steps one byte past a false opener; (2) an
  assignment-only stage returned before its redirections were read (`X=1 >
  ~/.bashrc` had no acts, Allow, safe unattended) — one `redirect_acts`
  reader for both shapes. HIGHs, all one class — a property checked at
  the stage's peel and missing for every inner argv (last round's
  privilege fix again): an inner `xargs` (`find -exec xargs rm`, `uv run
  xargs rm`) now unsound through a model attribute (`argv_from_stdin`); a
  `re:` rule reads an inner program as its own text (`deny re:^rm` meets
  `find -exec rm`); a program rule's path words fence the inner program
  its form applied to (`deny uv run rm /srv/keep`); the stage's command is
  known by identity (`ProgramAct.inner`), not by a wrapper's name. Also:
  switches apply once the whole argv is read and `on` the flag they name,
  so `tar -c` writes the archive `-f` names and not the members (`tar -cf
  .git/hooks/pre-commit x` meets the guard); a `chdir` flag (`git -C`, `uv
  --directory`/`--project`, `tar -C`) places the program's later relative
  words under its directory, and what it runs there is unsound; a
  redirection's target or a wrapper's argument (`PathAct::redirect`,
  `ActKind::Path::indirect`) is offered by `A` as `:<op> PATH`, never
  folded into the program's grants (`cat /srv/x` had taught `allow cat
  /srv/x`, which covered `cat notes.txt > /srv/x`) — and a program grant
  never covers a redirection at all (Steven, 10-10: "not allowing
  redirects in the language is a fine compromise for now"; the 07-27
  reading, under which `allow cat /tmp/*` contained `cat /tmp/a > /tmp/b`,
  is reversed: `allow :write /tmp` is the grant for the shell's write); an
  operand
  the model reads as DATA is carried as written even when it looks like a
  path, so `allow git push origin feature/x` names the branch;
  `credential_path` answers for any non-allow rule on the place (a user's
  `deny :* ~/.ssh` had made it answer None); the carve-out applies to the
  places not in an exception rather than skipping the guard when one is;
  `safe_unattended` no longer accepts a place at ANY operation (`flock
  build/lock …` creates the lock); the pane's host sentence names the
  direction's own verb and `explain_stage` says the hard ask before the
  rest; `[claude_code] web = "allow"` is consent for foreground work — a
  child's WebFetch holds unattended (`Judge::Inert` for `TodoWrite`,
  `Judge::Allow` for configured). Docketed: `uncovered` as an enum (the
  pane says "no allow lifts this" for `gh auth token` under `allow gh *`,
  which `allow gh auth token` lifts); a grant's op ceiling for an operand
  taught from a read (`tar -xf a` → `allow tar a` covers `tar -cf a`);
  `tar -x` writing member names the gate never sees; a quoted `'~/x'`
  expanded as home; `Unplaced` carrying its reason in the word; the stale
  `stage_paths` citations.
- 2026-10-10 (Steven, on the docketed `tar` case: "we didn't approve
  `-cf`, so that would be an unexpected outcome if approving `tar` with
  no flags approves it with flags") — §9.1, answered 10-09 in the suite
  (`compound-unmentioned-flag-rides-on-the-model`: model-centric for a
  modelled program) and missing from this log until now, stands: a flag
  the model marks benign rides on a form that does not name it. What was
  wrong was the mark. `benign` was read off the flag's own effect and
  role, never off the switches it triggers, so `tar -c` — which turns the
  archive `-f` names into a write — rode, and `tar -x` (members written
  where it runs) carried no effect at all. Now the loader raises a flag
  that switches a value into a write, delete or exec role to that role's
  effect (`Role::effect`, the one mapping the reader and the loader
  share), so every reader of a flag's effect sees it; `tar -x` is marked
  a write. And `A` named the flags that raised a call by looking up the
  command's raw words, so the cluster `-xf` named nothing: a no-rule ask
  now lists the flags no form carries unnamed, read by the acts reader,
  and `A` names those — `tar -xf /tmp/a.tar` proposes `allow tar -x
  /tmp/a.tar`, which does not cover `tar -cf /tmp/a.tar`. Measured before
  deciding: dropping riding altogether (only a named flag counts) turns 3
  acceptance cases and 11 tests red, most of them proposals that no
  longer cover their own call (`rm -rf build` → `allow rm build` asks on
  `-r`). Still open, the same principle on operands: a program rule's
  path word grants at any operation, so `allow cp /tmp/a /tmp/b` covers
  `cp /tmp/b /tmp/a`.
- 2026-10-10 (Steven, the same evening: "`allow rm` allowing `rm -rf` is
  a significant difference … the rules for programs are about
  arguments/flags while the rules about paths are about access") — the
  entry above kept riding, and `allow rm` covered `rm -rf`: "harmless"
  had been read as "raises nothing", and `rm` already deletes, so no flag
  of its raised anything. That was my reading of the 10-09 answer, made
  for `find -L`, never checked against a writer, and not said. Decided,
  with Steven (his proposal, refined in the exchange):
  1. **A program rule is the program, its subcommand chain in order, its
     flags and its data words** (`allow gh pr view` is not `gh pr
     create`; flags are position-invariant and known by every spelling a
     model gives). `*` approves any flag. Targets are the path rules'.
  2. **An allow covers only the flags it names**, except that on a call
     that only reads, a flag the model marks harmless rides — as it would
     run with no rule, so an allow never makes `ls -la` stricter than no
     rule does. `allow rm` covers `rm x`, not `rm -rf x`; `allow rm -r -f`
     or `allow rm *` does. `A` names every flag such a call used. BUILT
     in this entry's commit. (Still as before: a flag an allow names must
     be present for the rule to apply — `allow rm -r -f build` does not
     cover `rm build`; to settle with Steven.)
  3. **`-f=` declares that a flag takes a value** (`allow bat
     -l=|--language=`), any value; `-f=V` names one. A declared value is
     still judged as a path wherever it could be one, so a wrong
     declaration costs a prompt and never hides a target (Steven:
     "preferred"). Conflicts: any rule declaring a value makes the flag
     valued for reading; a rule naming the flag without `=` matches every
     use; `deny bat -l=*` denies every value. For a modelled program the
     model's facts win — spellings, arity, operand roles and operations,
     what a flag does, the argv family, inner commands — and a rule that
     disagrees gets a load note; a user's model of the same name replaces
     the shipped one. To build.
  4. **Path words leave program rules**: a target is judged by path rules
     only (`allow rm` + `allow :delete build`), which also closes the `cp`
     operand case above — a path rule carries its operation. An
     unmodelled program's targets carry none, so only a `:*` grant (or a
     model) covers them outside the start; inside, they run by default.
     Verified: `allow bat *` from a project asks for `bat
     ~/elsewhere/a`, `--foo=~/elsewhere/a` and `--foo ~/elsewhere/a`
     alike; `allow :* ~/elsewhere` lifts it, `allow :read` does not. To
     build. A bare path rule (no operation) stays refused unless Steven
     wants it to mean `:*`.
  Also in this commit: `tar`'s root is a read (`-t`, `-d`), its modes
  raise it, so a listing rides its harmless flags. Docketed: the user
  models directory's name — Steven finds `models` ambiguous next to AI
  models; `programs/` (mine), `command-definitions`, `modeled-programs`
  — and "the model" in user-facing text with it.
- 2026-10-10 (Steven, on whether a flag an allow names must be present:
  "each variant covering its own rule (today) is best … if you wanted a
  program run only with certain flags (i.e. not the bare program) then
  you wouldn't be able to do this") — PRESENT, as today: `allow rm -r -f`
  does not cover `rm x`. Built in this entry's commit, item 4 of the entry
  above, refined as it met the code:
  1. **An allow's path words are arguments**, carried as written, never a
     grant (`allow rm build/a.o` covers that call's form; the delete asks
     until `allow :delete build`). A deny's or an ask's path words stay
     FENCES on the program where its form is met — a refinement of the
     plan, which had them leave program rules too: an unmodelled program's
     argument may look like a place (`deny docker pull ubuntu:?*`), and a
     fence scoped by the form is the safe reading of it; and a deny never
     silently stops protecting its place. `**` on an allow is `*`; a
     `re:` place on an allow is refused with the path rule to write
     instead. `ProgForm.grants` is `fences`; `Applied` carries none.
  2. **A path rule grants a place and no flag.** With no program rule, a
     writer whose every write a path rule grants ran with any flag — so
     `allow :delete build` alone ran `rm -rf build`, which adding `allow
     rm` then made ask (an allow made it stricter). It now runs only with
     no flag a program rule would have to approve: `rm build` runs, `rm
     -rf build` asks until `allow rm -r -f`. Acceptance:
     `path-write-grant-does-not-approve-a-flag` (`mkdir -p` asks under
     `allow :write build/**` alone) — a G1 cost, Steven's to weigh.
  3. **`A` proposes a SET** (`proposed_allows`): the program's form when
     its act asks — chain, flags, data operands, never a place — then one
     `:<op> PATH` per place that asks, a redirection's target among them
     (the special case for redirects is gone, and with it the quadratic
     `reads_as_a_grant` re-parse). Offered only if the whole set answers
     the call; `A` opens the first, and the call re-proposes the rest
     once it is taught. The pane lists them all before the first press
     ("then A adds: …"), so one `A` is not taken for the whole answer.
     `rm -rf build` → `allow rm -r -f`, then `allow :delete build`; a
     credential read → `allow :read ~/.ssh/config`.
  This closes the `cp`/`uniq` operand case (a path rule carries its
  operation). Translated: the 07-27 / 10-09 scoped grant (`allow rm
  build/*` granting `build/*` to `rm`) is gone; an old config's allow
  with a path word now narrows to that argument, never widens.
- 2026-10-10 (Steven, on an unmodelled program's plain words: keep them
  arguments — a stat of each word was weighed and set aside, since an
  agent that may write inside the project decides which words exist;
  `bat` is served by a user model, P6b) — item 3 of the entry two above,
  `-f=`, BUILT:
  1. **`-f=` is `-f=*`**: the flag takes a value, any value. Its word in
     the form keeps the value attached (`-C=*`, `-C=sub`), so the form's
     next word is never read as the flag's.
  2. **Unmodelled, any rule on the program giving a flag a value declares
     it valued** (`decide.rs` `values_of`): the word after it, when the
     flag carries none attached and the word is no flag, is the flag's.
     It leaves the allow prefix (`allow mytool -C= build` covers `mytool
     build -C sub`) and is covered when an allow names its flag; a flag
     no allow names takes its value into `uncovered` (`flag -l=rust`), so
     `A` proposes this value, not any. A form's own words are read the
     same way, and a word it wrote after a declared flag is the value it
     names (`allow mytool -C sub build` beside `ask mytool -C=/` is
     `-C=sub`, not any `-C`) — a refinement met in the code: "a rule
     naming the flag without `=` matches every use" holds only where the
     rule wrote no word after it.
  3. **The value stays a place**: the reader already made every plain
     word of an unmodelled argv a path act at no operation, so nothing
     changed there; `allow mytool -l=` asks on `mytool -l /srv/x/a` until
     `allow :* /srv/x`.
  4. **Contact reads no declaration** (a deny's or an ask's words are met
     anywhere, values among them), and `deny mytool -l=*` meets `-l` with
     no value read too (`mytool -l` last, or before another flag): a
     missing value is the reader's guess, not the command's.
  5. **Modelled: the model's arity wins.** `-f=` on a flag that takes one
     is `-f=*`; a value on a flag the model gives none is refused at parse
     (`allow rm -r=x`), where it was a rule that never met a command.
     Not done: a modelled program's flag the model does NOT know (an
     unknown touch) still reads its next word as the model's reader did.
  Acceptance 197 (+4: `declared-value-is-the-flags`, `undeclared-value-
  is-an-argument`, `declared-value-is-still-a-place`, `deny-every-value-
  of-a-flag`); the manual shows `allow bat -l=|--language=` with tested
  rows.
- 2026-10-11 (Steven: "giving the user the ability to model programs …
  let's move forward"; a review after it) — P6b, the user's models,
  BUILT. My calls, each cheap to change:
  1. **Where**: `programs/<name>.toml` beside the config file, one
     constant (`model::USER_DIR`) — a stand-in for the name Steven has
     not settled (`models` reads as AI models; `:help shannon-models`
     already IS the AI models' tag, which bears him out). Read at launch
     and on every config change, before the one rule rebuild, as themes
     are; at most 256 files, 32 notes.
  2. **A file replaces the shipped model of its name whole** (no merge:
     a model is one author's statement), and must be named after the
     program it models — `cat.toml` modelling `bat` is refused, so a
     file cannot quietly stand in for another.
  3. **Every rule is read by the models in force**:
     `RuleSet::configured_by(…, models)` reads the config's rules, reads
     the session's again from their text, and re-reads the shipped ones
     (a shipped rule a model cannot read keeps the shipped reading —
     defensive, no shipped rule can fail today); `:allow`/`:un…` read by
     the same models, and so does `A`'s coverage check. What only the
     user's models refuse is said where they load, since
     `Config::review` reads by the shipped set (gap, as written then: a
     rule the shipped set refuses and the user's model would read is
     still said as refused at load, and is out of force — the review
     found it IN force while said refused; closed in C4 below).
  4. **`examples` are held to the reader** (`acts::example_misreads`):
     `match` reads in full, `not_match` leaves a word unknown. For a
     user's file, each misread is said and the model loads (a misread
     asks more, never less); for the shipped set, a test — which the
     model's doc had claimed and nothing did. Its first run found a
     reader bug: a non-bundling program's attached short value (`awk
     -F:`, `awk -f/x.awk`) read as an unknown flag. Fixed, getopt's
     reading, for every model with `bundling = false` and no single-dash
     longs (awk, perl, python, …).
  5. **`trust = "completion"` backs no allow**: with no rule its call
     asks (`NoRule::Completion`), no harmless flag rides on an allow,
     and its places are at no operation (only `:*` grants them, every
     fence meets them) — a completion spec's roles are a guess at what
     a call does. Its words still read by it, so denies meet every
     spelling. All 78 shipped models are `verified`: nothing shipped
     moved.
  6. **The directory is Shannon's own state** (`register_own_state`): a
     model loosens the gate as the config does, and the shipped `ask
     :write ~/.config/shannon` guards only the default place. So reading
     a model file asks too, as reading `config.toml` does.
  Steven's `bat` example, with a model: `bat` is a read, so `bat
  notes.txt -p` runs inside with no rule at all (`-p` harmless), and
  `bat -p /srv/notes/a.md` asks until `allow :read /srv/notes`. The
  manual shows the model; its test loads that very text as a user's file
  and decides the rules shown beside it.
- 2026-10-11 — the review of `0170ced..319f04b` (A, B, C, and E as two
  consistency reviews). Steven's answers to the calls it raised:
  1. **A program deny/ask fences a redirect**: `deny cat /srv/keep/**`
     meets `cat a > /srv/keep/x` (the `!redirect` exemption from a27f98d
     had come to exempt only fences, once allows had none).
  2. **Privilege, a special case** (Steven: "it may include commands after
     it that we have or haven't modeled. Granting sudo doesn't grant
     everything, only everything user defines such as `allow sudo ls *`
     … This does not change path rules"). The hard ask stays, except an
     allow of the user's naming the program the wrapper runs, and exactly
     the flags given the wrapper (`allow sudo -u=pg psql *` lifts `sudo -u
     pg psql -l`, not `sudo psql`, which runs it as root). `allow sudo` and
     `allow sudo *` name no program and lift nothing; `-s`/`-i` (a shell)
     are lifted by nothing; the argv wrappers `sudo`, `doas`, `pkexec`,
     `run0` alike, `su` (a shell line) never. Privilege is Shannon's fact
     (`acts::PRIVILEGE_WRAPPERS`, held equal to the models' marks): a
     user's `sudo.toml` reads sudo's words, never whether it raises.
  3. **A user's model adds caution, never takes Shannon's away**: what
     runs code (the node's `exec` effect, a code role), a file or a program
     it runs (inner argvs), and a credential it prints are the union of
     the user's reading and Shannon's (E2: `eval.toml` of a read ran `eval
     'rm -rf src'`).
  4. **An exec grant covers the exec act**: on a modelled program's path
     it no longer stands in for the model (`/usr/bin/gh auth token`,
     `/usr/bin/find . -delete` ran under `allow :exec /usr/bin`).
  5. **A `.claude` directory that contains the start is not guarded** for
     the paths inside the start (a project in `.claude/worktrees/x` asked
     on everything); one at or below the start, or elsewhere, still is.
  Built, commit by commit:
  - C1 (2–3): `Act::Privilege` carries the wrapper's own flags (the peel
    now keeps them per wrapper, `Command::wrapper_argc`) and what it runs;
    `decide::privilege_lift`. Found on the way, and fixed: a rule through
    an inner program gave the inner program EVERY flag it wrote
    (`ProgForm::flags_before` now splits them), so `allow uv --quiet run
    pytest *` covered nothing; the outer act's flags were cleared rather
    than required. `run0` is modelled (unmodelled, it was no privilege
    wrapper at all, and `allow run0 *` read its command as words —
    inferred, not run), `doas` gained its flags — both from the man pages as
    recalled, unverified here; a flag missing is unknown and lifts
    nothing.
  - C2 (4, and the completion holes): `decide_program` lets an exec grant
    stand in only for an unmodelled program. A completion model's words
    are placed as an unmodelled program's too, beside its own places
    (`acts::unmodelled_places`, now the one reading of both), so a role
    of `data` hides no file from a fence (E1: `bat /srv/keep/a` under
    `deny :* /srv/keep`, `bat .env`), and none of its subcommands rides
    on an allow. Left as designed: a completion-trust WRAPPER is still
    allowed with no rule (E2) — the peel that finds what it runs is
    Shannon's (`shell::program::wrapper`), never the model's, and what it
    runs is judged on its own acts.
  - C3 (1, and E1's union): a program's fence meets a redirection and a
    wrapper's argument (both are the stage touching a place for it). A
    dashed word's values are read once (`acts::cluster_values`): after
    `=`, and after each leading letter of a short cluster (`-xoout.txt`
    may be `-o out.txt`), up to `MAX_CLUSTER` (32) letters — past it the
    word is unplaced and asks, so the splits cost no more than a word.
    Every value is placed, `/` or not (E1: `--config=.env`, `-c.env`
    walked past the credential guard), and contact takes every reading:
    a cluster's letter runs to the first character no option letter is
    (`-o./out` is `-o`), takes what follows it as its value (`-lrust`
    meets `deny -l=rust`), and a deny's plain words meet values as words
    (`deny mytool --lang rust` meets `--lang=rust`). Allows are
    unchanged: E1 found no allow covering more than it names. The replay
    did not move: 112 asks shipped-only, 99 with the five rules, 0
    denies.
  - C4 (the doors, A and C): what the rules could not honour is said where
    the gate is built — `App::policy_for` (the one builder, replacing the
    eight-argument `rules_for`; construction goes through `rebuild_rules`
    too), which knows the user's models and the session's rules; once
    while it stands. `Config::review` no longer judges rules (by the
    shipped set it missed a rule only a user's model refuses and said
    refused one the user's model reads — that rule was in force all
    along, not out of it as P6b's gap said). A session rule a new model
    refuses leaves force SAID; `:reload` names a model file that reads
    differently (`Models::user_changes`, "changed: programs/bat.toml");
    `:rules`' unread section no longer calls a session rule the
    config's. What this session removed is read by the models in force
    (`:unallow` then a model change put the rule back), at `:allow`'s
    "back in force" too. One re-read policy (`reread_all`) for session,
    shipped and `with_models`; `shipped_after` (no callers) gone;
    `Rule::parse`/`read` and `RuleSet::configured` are test-only, so
    code must name the models it reads by; `extend_from_lines` reads by
    the policy's own. `A` for an unmodelled program reads the words as
    the decision does (`decide::unmodelled_words`: `-l=rust`, not the
    argument `rust`). A user's own model is named as theirs in a refusal
    ("your model of `bat` (programs/bat.toml)"). Every door the review
    found unheld now has a test; two mutants stay green as equivalences:
    `:rules`' as-written mark reads only whether a note exists (the
    models change its words, which the listing drops), and the text
    match once beside the re-read of what was removed was dead — deleted.
  - C5 (B: the directory is untrusted input, read on the event loop): a
    chain past `MOST_CHAIN_WORDS` (8) is refused before a tree is built
    (20,000 words overflowed the stack at every launch and left the
    terminal raw); at most `MOST_EXAMPLES` (64) examples of
    `MOST_EXAMPLE_BYTES` (1 KiB); the directory read up to
    `MOST_USER_BYTES` (1 MiB, all files together — a count of 256 files
    of 1 MiB was tens of seconds per config change) and listed up to
    `MOST_ENTRIES` (4,096), the rest said; entries that could not be
    listed are said; a user's text in a note is clipped to 80
    characters. Tests hold each bound by count, and both directions of
    `example_misreads`; the attached-short-value branch's arity filter
    is held by a fixture model (`fz -Ic` with `-I` zero-arity), so its
    premise cannot rot with a shipped one.
  - C6 (E2, own state): Shannon's own state is no `[secrets]` entry any
    more (`RuleSet::with_own_state`, `defaults::own_state_lines`): each
    registered path is a literal — `\c<path>`, or its escaped regex where
    it holds a space or a glob character — where read as the list's globs
    a config under `Application Support` or `cfg[1]` protected nothing,
    silently (and `cat …/api_key` ran). Under `path_policy = "allow"` a
    write into it still asks (`ask :write`): the knob says how a
    credential's read is treated, not that the gate may be loosened.
    Registered where it really is too (`with_real_path`): a config
    directory reached through a link was guarded only by the link's name.
    An empty, relative or root `HOME` is no home (`xdg_base`), as
    `secrets::home_dir` already held. Not done, docketed: the pane still
    explains an own-state guard as a credential's (`explain_act` reads
    the verdict alone, and a cited line does not say which it is), and
    `:rules` lists neither own state nor the user's models (P5).
  - C7 (5): a shipped guard found anywhere (`**/.claude`, `**/.shannon`,
    `**/.git/hooks`, a credential name) reads a path inside the start
    below the start, so a `.claude` that contains the start — a project
    in `.claude/worktrees/x` — no longer guards every path in it; one at
    or below the start, and every path outside it (the harness's own
    `settings.json`), are guarded as ever. Generalised from Steven's
    `.claude` answer to every shipped anywhere-guard, on the same reason
    (a directory above where you work is not something the project
    holds); a user's own `**/x` rule is theirs and unchanged. The four
    TUI tests red in a `.claude/worktrees` checkout pass there now. Not
    done: `test_app` still takes its start from the working directory
    (docketed, as the hook tests' load-sensitive 5 s deadline is).
  - C8 (older model bugs the review found): a call that SENDS — a body,
    or any method given or implied that is not GET/HEAD/OPTIONS — is a
    write, so `gh api -X PATCH`, `-f`, `--input` ask with no rule (the
    `api` node is a read and its host no act, by 10-09's decision); ANY
    method counts, not the first (`curl -X GET -X DELETE`); and an allow
    naming a valued flag holds only where every occurrence carries its
    value (`allow gh api -X=GET` covered `-X GET -X DELETE`), as an
    unmodelled flag's grant already did — so `last_wins`, a field nothing
    read, is gone. `git -c`: the list is of keys known to run nothing
    (`kv_safe`), every other key asks (`include.path`, `…uploadpack`,
    `…program` were missing from the list of dangerous ones). A flag
    whose value implementations take differently is read both ways
    (`or_attached`, on sed's `-i`): BSD's reading of `sed -i s/x/y/ f` took
    `f` for the script while GNU edits it; GNU's alone would miss BSD's
    script. Replay unchanged (112 / 99 / 0).
  Docketed from the review, not done here: ~~the pane explains an
  own-state guard as a credential's, and keeps its own copy of
  `is_credential_line`~~ (done 10-11, below: `Cited::guard`); `:rules`
  lists neither own state nor the user's models; `stage_tone` colours by the shipped
  registry, so colour and verdict can disagree for a user's model (A);
  `sed -e 1p` asks under `allow sed` and runs with no rule — two
  predicates where one belongs (E1 F6, P5); `>|` and `<>` are refused
  by the parser, so they ask and never deny; `tee` is unmodelled; a
  lower tier's declaration (`-f=`) can widen a higher tier's allow once
  project rules exist (P6d); `test_app`'s start from the working
  directory; the hook tests' 5 s deadline under load.
- 2026-10-11 (Steven's hands-on test, Claude Code backend, in an isolated
  `XDG_*` sandbox; the engine replayed each finding first). Most of what
  surprised was the plan's setup — Shannon started in a directory holding
  its own state, verdicts written for the API backend, `;` as the plan's
  notation for two rules — and these, found by it:
  - **A rule names one command.** `allow rm * ; allow :delete build` was
    added as ONE rule, its `;`, `allow`, `:delete` read as words of `rm`.
    A rule word that is a shell operator (`;`, `&&`, `|`, …, or a word
    ending in an unescaped `;`) is refused with the reason (`line::parse`).
  - **The pane says what lifts a held call.** The no-`A` reason waited
    behind a press of `A`; a credential deny under Claude Code named only
    the knob and read as having no way out. Now an ask with no `A` says
    why on the pane ("no A: …"), a deny says what lifts it ("to lift it:
    `:allow :read .env` lifts it, or `[claude_code] path_policy` decides
    these" — and the named grant does: a test types it and the call is
    answered), and a deny no longer offers "`a` runs it once".
  - **A guard is said as it was met.** `Cited` carries `guard` (a
    `[secrets]` path or Shannon's own state, from where the line came, not
    its spelling — both are `deny :* \c…`) and `reached` (met from above,
    `decide_path`'s `soft`). `.` above Shannon's state "holds Shannon's own
    state, which this may reach", where it said "`.` matches a pattern
    that commonly holds credentials" — false twice. The two string-prefix
    copies (`is_credential_line`, `is_credential_guard`) are gone.
  - `core.pager` and its kind (`Hard::Code`) said "code no rule covers",
    which reads as "write the rule"; none answers it: "code Shannon does
    not read; this asks whatever a rule allows", as its siblings say.
  - **`Esc` answers no decision** (Steven: a vim user presses it by reflex,
    before `<C-w>`, and it denied the call they meant to allow; "keep `q`
    as the quit here"). Silent, as vim's `Esc` in Normal mode: a count
    dropped. `d`/`n`/`q` deny. The y/n confirmations keep `Esc` = no.
  - **An exact rule stays exact** (Steven): `allow lsd -a` covers `lsd
    -a`, not `lsd -a .` — an unknown word may be a subcommand. The ask
    names the form that covers the rest: "`allow lsd -a *` covers any
    other word or flag".
  - **Docketed (Steven: "wording now, depth docketed"): a read has no
    depth.** A directory act reaches what is below it whatever the
    program (`path_cands`: `above`), so started in `~` — Shannon's state
    in `~/.local/state` — every `ls`, `git status` and unmodelled program
    asks there, and no allow lifts it. `ls` without `-R` reads names, not
    contents: a model property (a shallow read of a directory) would let
    only recursive reads, deletes and unmodelled words reach below.
  - Not reproduced: copying from `:rules` by mouse drag (a click, Visual,
    drag and release copy there in a test, the selection drawn reversed;
    `tui.copy_on_select` is off by default, so a drag alone copies on no
    pane). Asked Steven for the gesture's details.
- 2026-10-11, second run (Steven, a Claude Code session, started
  in `proj/`; read from its audit trail and log). One bug explained every
  surprise in A, B, C and F — `git status`, `find . -name x`, `lsd -a`,
  `bat --plain notes.txt` under `allow bat *`, `lsd -A` with no `A`:
  **launch registered the cwd dev key `.test_key.api` as Shannon's own
  state, relative, whether or not it existed** (`keys::key_files_for` →
  `register_key_file`). Every read of `.` — any modelled reader of the
  start, any unmodelled program (which touches its working directory) —
  reached it from above and asked with no lift, in every project, under
  BOTH backends: the guard is a deny under Claude Code and an ask under
  the API, and either one met from above asks (probed after 5c69036,
  whose message wrongly says Claude Code only). The
  previous round's probe passed its own protected list, not launch's, so
  it never saw the line (`[[feedback_wiring_is_untested]]`). Fixed
  (`key_file_to_protect`): every key file is guarded at its absolute path,
  and the cwd default only when it exists; a named or saved key is
  guarded absent too (the user chose it; setup writes it).
  - Still open, and now realistic, not only for a start of `~`: where the
    dev key DOES exist in a project (Shannon's own repo), the literal
    guard still makes every read of the start ask under Claude Code. The
    docketed depth item (a read of names — `ls`, `find` without a code
    action, `git status` — is not a read of contents) is what answers it;
    `*.api` as a `[secrets]` pattern already guards that file without
    the from-above ask, so the two spellings of one guard disagree today.
  - `Hard::Code` said "code the model does not read — code Shannon does
    not read": the act's own words already name the code; the sentence now
    adds only "this asks whatever a rule allows".
  - Confirmed as designed: `\cat` reads as `cat` (the model wrote it from
    Steven's global CLAUDE.md, which Claude Code loads); `cat .env` ran in E
    because A's `:allow :read .env` was still in force; `allow lsd -a|--all`
    is "already in force" once lsd is modelled with `-a|--all`; `allow lsd
    -1` with `deny lsd --oneline` denies `lsd -1` (one flag, equals,
    strictest). Steven's `lsd.toml` reads as he meant it.
  - Docketed (Steven): a model file's load error is said through
    `:messages`; small changes to how, his list to come.
  - Then removed outright (Steven: "cruft left over from when we
    started"): launch no longer reads a key from `./.test_key.api` in the
    start at all — a name nobody would guess, read from whatever directory
    Shannon ran in, so a repository could supply the key. Keys come from
    `ANTHROPIC_API_KEY`, `SHANNON_API_KEY_FILE` or the key `:setup` saved;
    the `*.api` `[secrets]` pattern stays.
- 2026-10-11, **a read has a depth: a listing is not above what it lists**
  (the docketed item; Steven: "names are fine", `role = "list"`). A read
  of a place's NAMES — `ls`, `find` and `fd` with no action, `git status`,
  `git ls-files` — no longer meets a guard below it from above; a SEARCH
  (`rg`, `git diff`, `git grep`), a delete, a run, an unmodelled word, a
  completion model's word all still do, and the guarded file named
  outright is guarded as ever (`ls keys/.k.api` denies under Claude Code).
  So a project holding the dev key, or a start of `~` with Shannon's state
  below, no longer asks of every `ls` and `git status`.
  - Model: `Role::List` beside `Dir` (now "searched"); `reads_here` is
    `true`/`"dir"` (searched) or `"list"`, per node, a subcommand taking
    its parent's (`git`: searched; `status`, `ls-files`: listed). `ls`,
    `find` (its starting points and `-f`), `fd` (its trees,
    `--base-directory`, `--search-path`) list. `git -C dir status` lists
    `dir`: the directory moved to is read as the node reads where it runs.
  - **A listing reaches names only while the call stays a read**: under
    `allow find *`, `find . -delete` would otherwise delete Shannon's state
    with nothing met from above (tested).
  - Accepted (Steven): a recursive listing of an ancestor prints the NAMES
    inside a guarded directory (`find .` lists `secret/a` under `deny :read
    secret`); `ls secret`, `find secret`, `cat secret/a` are judged as
    before.
  - For P6d: `list` is LESS caution than `dir`, so a project model must not
    turn a shipped `dir` into a `list` ("project models only add caution").
  - Tests: a_listing_meets_no_guard_below_it_and_a_search_still_does,
    a_users_model_says_it_lists_and_a_listing_reaches_no_guard_below.
    Mutations, each red: the from-above skip dropped; "stays a read"
    forced true; a completion model's listing kept. `names_a_file` read
    its own hand list of file roles and missed `list` (a test caught it):
    it asks `Role::is_file` now. Replay unchanged (112 / 0 denies): it
    registers no own state under the start. Acceptance 197 hold.
- 2026-10-11, **P5: the pane marks what each act stands on** (Steven:
  "highlight less … focus the highlighting on the parts we should bring
  attention to"; of three mocks he chose "verb tinted + block on cause").
  - 5.1: the colour is read from the ACTS. It was read from the flat
    effect table, which knew no tree and no user model, so an allowed `git
    status` and a user's `lsd` painted as unknown, and a finding said `git
    · unknown`. A word still unresolved stays the shell's question (`echo
    "$FOO"` makes no claim — Steven's earlier ruling).
  - 5.2: each act says where it is written (`Written`: the verb, and the
    cause — byte ranges carried from the parser's word spans through the
    reader's argv indices). The cause of a program act: a flag the model
    says does more than its node (`find . -delete`: the `-delete`;
    `-exec … \;` through its terminator), a word no model knows, the flags
    a deciding deny or ask names (`deny rm --no-preserve-root`), and the
    verb itself when its node is past a read (`rm`, `git push`) or no model
    reads it. A path act's cause is its word; a redirection's, its operator
    and target; a privilege wrapper's, its word; an unsound stage, the
    whole stage.
  - The pane: the stage's tone TINTS its verbs' text; a BLOCK, in the
    louder of what the act does and what was decided (an ask at least
    yellow, a deny red), goes on each cause. Safe gets no block. A path
    the program names wears none while allowed — its writer shows it — but
    a redirection has no program to carry it. An act that asks with no
    word of its own (the `.` a listing reads where it runs) blocks the
    verb.
  - Seen in building: under Q7 a delete inside asks, so `rm x` blocked
    both `rm` and `x`. Steven, 10-12: "expecting to deny an `rm` but also
    denying `x`?" — a path a program names is blocked only when it is
    decided on its own account (a rule; outside the start: `rm /tmp/a`,
    `cat /etc/hosts`), never when only its operation decided it, which the
    verb or flag already shows. A denied read (`ls delete_this_dir`)
    blocks the path red and tints `ls` green: the tint is the program's
    danger, the block and the headline the verdict (Steven, 10-12:
    "exactly how I want it").
  - Tests: an_act_says_which_words_it_stands_on (core),
    a_word_keeps_where_it_is_written (parser),
    the_block_goes_on_the_cause_and_the_verb_is_tinted (rendered cells).
    Mutations, each red: raising flags unmarked; the node clause dropped;
    `-exec` narrowed to its word; a wrapper's place, a redirection's, a
    deciding rule's flags dropped; the substitution shift and the
    written-index offset dropped; the allowed-path exclusion, the decision's
    raise, the verb fallback, the mark order, the bridge. (The first
    allowed-path test used `cp`, which is unmodelled — its words are at no
    operation and could not be blocked either way; a probe showed it, and
    the test uses `uniq`, with the premise asserted.) A layout property went
    red on a span carried into `Redirect`'s equality: `same_command` now
    compares what a redirection does, not where it is written.
- 2026-10-12, **P5: a finding says what its verdict rests on.**
  - A verdict no rule made rests on a model, so the finding names it —
    its file and trust, dim on the head line (`model: shipped, verified`;
    `model: programs/lsd.toml, author`): a user's model allowing `lsd` is
    their statement, not ours. A rule's verdict names the rule instead.
    The act carries a `ModelRef { name, trust, source }`.
  - A call to an API whose host is implicit (`gh api user`) says the
    call: every method given or implied (none is a fetch, as the reader
    already decides), the host (`--hostname`, else the model's), the
    endpoint, the model's gloss — `GET api.github.com/user — the
    authenticated user's profile`. Still no host act (the concession).
  - A program act is labelled as called (`git push` beside an allowed
    `git status`; the `rm` a `find -exec` runs), not by its stage.
  - `:analyze` explains its allows (each program act allowed, after what
    asks); the prompt pane lists only what it asks.
  - **`A` says every rule it will offer.** `proposed_allows` ran for the
    first stage that asked: under `git push && cat /etc/hosts` the pane
    offered `allow git push`, and only after it was taught did the
    `cat` stage ask. It now continues, each later stage as it will be
    offered once those before are taught (`stage_proposals` per stage,
    each answer verified to allow its stage, one round per stage), and
    ends at a stage no rule answers — which the prompt says past the
    last `then A adds:` (`then: \`sudo ls\` still asks — …`), in the
    sentence the app says after a rule is taught (`still_asks`, moved to
    the view: one sentence, two callers). `:analyze` counts them (`+ 1
    more`).
  - Tests: a_finding_says_what_its_verdict_rests_on,
    every_a_is_said_before_the_first (rendered),
    a_proposal_runs_through_every_stage_that_asks (core), the gh reader
    test asserting the whole call. Mutations, each red: the trust span
    gone; shown on a rule's verdict; allows listed on the prompt; the call
    line gone; asks not first; the stage's label; no default method;
    `--hostname` ignored; one stage only; the still-asks line gone;
    `more` ignored.
- 2026-10-12, **P5: `:rules` lists what the gate reads.** It listed
  `RuleSet::rules()`, while the decision also reads the credential guards
  `[secrets]` asks for, Shannon's own state, and the scratch grant
  (`policy()`): "the policy in force — what the gate reads" was missing
  the guard that holds `cat ~/.ssh/config`. `guard_lines`, `grant_lines`
  and `guard_exceptions` are what `policy()` reads, and what the page
  lists. Sections: yours first (config, session, `[memory]`/`[skills]`),
  then what ships by kind — guards, credential paths, own state, allows,
  grants — then what was removed. The plan's "carvings" and "hard"
  sections have nothing to hold: the carvings are the models' now
  (`defaults.rs` module doc), and a hard ask is no rule (the page's
  opening names them). Test: rules_lists_the_guards_and_grants_the_gate_
  reads; mutations red: no guard lines, no grant lines (also red in the
  scratch decision test — one source), the allow/guard split.
- 2026-10-13, **a narrower rule never loosens a deny; a place's own
  account is said first.** Steven read `:analyze rm -r /` say "`/` holds
  Shannon's own state, which this may reach" as its reason, and asked
  whether it should not be that `/` is outside the directory. Tracing it
  found two defects in the resolution behind it (`decide.rs`):
  - **Across tiers**, the deciding set was the pool's most specific
    rules, so a narrower shipped ask took the place of a broader user
    deny: under `deny :* ~`, `cat ~/.ssh/config` ASKED (the `~/.ssh`
    guard). A lower tier's rule now displaces a higher one only by being
    narrower AND stricter (`resolve`).
  - **Within a tier**, a rule met from above is always narrower than one
    naming the act, so the user's own `ask :* /srv/keep` took the place
    of their `deny :delete /srv` for `rm -r /srv`. A contact never takes
    the place of a rule naming the act (`ruled`'s `within`); a contact's
    deny asks, as before, now as its `action()` rather than afterwards.
  - **The reason.** A contact is `by` only where it raised the decision
    (`rg foo .` above Shannon's state, which would run unasked). Where
    the place asks on its own account — outside, a delete inside, a rule
    naming it — that account is `by` and the contacts are the verdict's
    `reaches`, said after it: "`/` is outside where Shannon started —
    asking; it also holds paths that commonly hold credentials, which
    this may reach (ask :* \c~/.ssh and 9 more)". The cited rule is the
    strictest, named before met-from-above, own state before credentials
    before other rules — no longer the last of equals. Only a read (or a
    word at no operation) adds "command output is sent to the model".
  - Tests: a_narrower_rule_never_loosens_a_deny,
    a_contact_is_the_reason_only_where_it_raised_the_decision,
    a_place_is_explained_by_its_own_account_first (rendered); `rg foo /`
    now pins `Default(Outside)` with its `reaches`. Mutations, each red:
    the cross-tier condition; the contact `within`; the own-account
    branch; a contact's deny kept a deny; the read-only clause; the
    citation order.
  - Measured, not changed: `gh api user -X` (a valued flag with nothing
    after it) is allowed as a GET — gh's parser refuses it and nothing
    is sent; `echo POST | xargs gh api user -X`, where the value would
    arrive, is a hard ask (stream-fed argv).
- 2026-10-13, **P6a: Shannon's surfaces, measured before built.** The
  replay's new prompts were 13 (5 Shannon's memory, 5 Claude Code's, 2
  `xargs`, 2 reads outside). The scratch grant was already shipped
  (`grant_lines`) and both memory directories were already `[memory]`'s
  grants; the replay has no `[memory]` rules. With them as `--rule`s,
  13 → 4 — an upper bound, since the app grants Claude Code's only in its
  sessions. The plan's "shipped grants" became "keep them `[memory]`'s":
  they follow the setting, which a shipped grant would not, and `:rules`
  names it. What was missing was a test on the real paths (the fixtures'
  config was `<tmp>/config`, Claude Code's memory `<tmp>/claude/…`, so
  `ask :write ~/.config/shannon` and `ask :write **/.claude` were never
  met): shannons_memory_is_granted_past_its_config_dirs_ask,
  claude_codes_memory_under_a_dot_claude_is_written_unasked, each with
  its premise asserted; mutation (no write grant) red in both. The rules
  now read `allow :read <place>` and `allow :write <dir>` — `DIR/**` is
  "below DIR" only for a delete grant (`Resolved::grants_delete`), so the
  meaning is unchanged; a disabled place is one `deny :read <place>`.
- 2026-10-13, **a walk that deletes needs its place granted.** Checking
  the manual against the engine for P7: under `allow find *`, `find .
  -delete` ran unasked, while under `allow rm *`, `rm -rf build` asked
  until `allow :delete build` — Steven's 10-10 ruling (a program rule is
  about arguments and flags; places are the path rules') held for `rm`
  and not for `find`. A `list` role is a walk's root, and `-delete`
  raised only the program's effect: the root stayed a read act. Now a
  walk whose node or a given flag deletes makes its roots — and where it
  runs, given none — delete acts (`acts.rs`, `walk_deletes`), keyed on
  the flags given, since a run (`-exec`) outranks a delete as an effect.
  Only `find` has a listing root and a deleting flag today (`fd`, `ls`
  none; `git`'s `-C` is a `dir`, which moves, not walks). This reverses
  the carving era's "an allow that speaks to it sets it aside" for the
  place: `allow find *` still lifts the flag, and `find build -delete`
  runs beside `allow :delete build`. Tests changed with it:
  a_config_adds_rules_and_removes_only_what_it_names,
  a_taught_allow_answers_a_shipped_ask_never_the_deny, the manual's row
  for `allow find:*`; the `remove` note for a former carving now says
  the places are the path rules'. Test:
  a_walk_that_deletes_needs_its_place_granted; mutations red: no
  `walk_deletes`; keyed on the effect (the `-exec … -delete` line); the
  no-root walk kept a read.
- 2026-10-13, **a program from a file is run, not read.** Also found
  checking the manual: `awk -f prog.awk notes.txt` and `sed -f edit.sed
  notes.txt` ran unasked. Their models gave `-f` `role = "read_file"`,
  so the program file was a read inside the project — where a write also
  runs unasked — and its `system()` or `w` was never seen, while `bash
  x.sh` (`exec_file`) asks. Both are `exec_file` now. A sweep of every
  `read_file` flag (35, all models) found no other file whose contents
  run: patterns, ignore lists, timestamps, bodies and manifests are data
  or already asked of (`curl -K`, `tar -T`, `find -files0-from`; `xargs
  -a` is a hard ask as all xargs is). One lead outside this class:
  `git clone --template DIR` copies DIR's hooks into the clone's
  `.git/hooks`, past the shipped `ask :write **/.git/hooks` (docketed).
  And `A` offered `:exec <file>` alone for a file a modelled program
  runs, which never answers it (the program act is the model's): where
  the grant alone does not answer, the program's form joins it — `bash`
  + `:exec x.sh`, `awk -f` + `:exec prog.awk` — each pair verified to
  answer. Test: a_program_from_a_file_is_run_not_read; mutations red:
  awk's `-f` a read; sed's; the early return in `stage_proposals`.
- 2026-10-13, **the review of `c3cdeda..e10b6d5`** (A correctness, B
  robustness and the gate's consistency, C tests). B compared both
  commits over 197,248 decisions (39 rules × two tiers, singly and in
  pairs, 32 benign lines, both `path_policy`s): 0 looser, 4,921 stricter,
  each the intended Ask → Deny. All three found what the gate did not:
  - **P6a's respelling loosened a disabled place** (HIGH, all three): a
    disabled place became one `deny :read X`, and `DIR/**` was strictly
    within `DIR`, so a user's `allow :read X/**` — the spelling `:help`
    teaches — took its place in the same tier, and a disabled skill was
    read unasked. Fixed for the class: `DIR/**` is narrower than `DIR`
    only for an act that may be a delete, as it matches less only for a
    delete grant (`Resolved::within_as`); so a user's own `allow :read
    d/**` no longer beats their `deny :read d` either.
  - **The cross-tier displacement decided nothing** (A, C): a narrower
    rule left standing is from a lower group, so taking out a broader one
    beside it changed no decision, and deleting either half passed the
    suite. `resolve` now returns what stands; the decision is the
    strictest standing rule and its reason the rules at that strictness
    (`at`), for path, program and host acts alike.
  - **The reason named a bystander** (A, B, C): `by` cited a user's own
    `allow :read ~` as "places a rule guards" that `~` holds, and, by
    list order, a contact's ask as the way out of a deny. Now a reason is
    the place's own account or what it is met by softly, never both;
    `lifted_instead_by` takes `first_cited`; what a guarded place's output
    costs is said once; `rm -r .` above a guarded file is marked again
    (`block_tone` read the account alone).
  - Smaller: a contact is within a named rule only at an equal pattern
    (both stand); a brace in a disabled place is said, not dropped; the
    `KEPT` table is an exhaustive `match`; `credential_held` asks the
    citation, not the rule's text; the replay marks a guarded place below.
    Docketed: a bound on rules × path words (B: debug 200 × 50 took 3.5 s,
    quadratic in rules before this change).
  - Tests: a_trailing_double_star_is_narrower_only_for_a_delete,
    a_verdict_cites_what_decided_it, the_reason_names_what_decided_it
    (rendered: the lever in both rule orders, no bystander, said once,
    the mark), what_is_disabled_is_not_told_and_refused (your `DIR/**`
    grant), the brace, `credential_path("~")`, reaches under a deny.
    Mutations, each red: `below` always counted; never counted; citations
    at every strictness; `met` without `soft`; without its strictness;
    `block_tone` without `reaches`; the account's own tail; no `reaches`
    in `credential_path`; braces spellable.
- 2026-10-13, **Steven on 6c and 6d.**
  - 6c (identity by `--version`) DEFERRED: probing runs the binary,
    nothing reads a model's `implementation` yet, and both readings are
    taken where implementations differ. Revisit when a second
    implementation's model ships (GNU sed).
  - 6d, the project's `.shannon/config.toml` (loaded only for a trusted
    start):
    1. A project file's rules are the user's own rules, as the global
       file's are, and lift a shipped guard as those do ("there does need
       to be some way to bypass the shipped guards").
    2. A project file may turn off shipped asks (`[permissions] remove`).
    3. A project's models (`.shannon/models/`) load at `trust =
       "completion"`: they say what words are, never allow by themselves.
    4. Load order: system (shipped), global (`$XDG_CONFIG_HOME/shannon/
       config.toml`), then project — for SETTINGS, the later file wins,
       as git's local config over its global.
    Read together (Claude's reading, to confirm): project and global
    rules are one tier — this replaces 10-09's "shipped < project <
    global < session" — so between the two files specificity decides, the
    stricter on a tie, never the order: a project carves a narrower
    exception under a global deny and does not override an equally
    specific one. `[memory]`'s write place, the project's name and root,
    `paths_except` and `exec.env` stay yours only (`YOURS_ONLY`, why
    stated there); a project file that changes a safety setting says so
    at load, as a sourced one does.
- 2026-10-13, **6d built: the project's file follows its trust, and stands
  right above yours.**
  - What trust is, said (Steven's question: `:untrust .` left `ls .`
    allowed). Trust decides whether a directory's own files may configure
    what runs — Claude Code's hooks and MCP servers, Shannon's `.shannon/`
    config and models — never what a command may touch: that is the rules'
    and the inside default's, trusted or not (`ls .` is a modelled read
    where Shannon started). Said in `trust.rs`, the trust messages, the
    trust screen and the manual, which all spoke of Claude Code only.
  - **A project's layer stands right above the latest read of your files,
    however it arrives** (`ConfigStack::lay`): launch, the trust screen,
    `:trust`, `:reload`. On top, the screen's yes after launch had laid
    `CLAUDE_CODE_EFFORT_LEVEL` (a `:set`) let the project's `[turn] effort`
    beat the variable, which a trusted launch keeps. Lifted out before the
    rest is laid, so a reload of yours merges into your layer rather than
    stacking a copy over the project per reload.
  - `:reload` reads yours and then the project's, as one all-or-nothing
    change (`ConfigStack::source_all`): an edit to either is taken. A
    mutation found the re-read untested — the placement alone kept the
    project on top — so the test edits the project's file. `:source` of
    the loaded project file reads it as the project's: as `Sourced` it
    stood beside its own layer, a second copy.
  - Trust followed mid-session (`App::follow_trust`, the end of
    `trust_dir` and `:untrust`): the screen's yes or `:trust` reads the
    file, `:untrust` takes it off, the models follow either way. Before,
    only the trust at launch counted.
  - A start not trusted says what it holds unread
    (`Trusted::unread_project`, at launch; Steven agreed): the trust screen
    asks only for Claude Code, so on the API a project's file was silently
    not applied.
  - ~~Claude's addition, to confirm (3d4c235): a project's model adds a
    program Shannon does not ship and never replaces one.~~ REVERTED by
    Steven's answer 2 below ("No"). A flat `gh.toml` under the user's
    `allow gh *` let `gh auth token` run.
  - ~~Open: `.shannon/skills` is a default `[skills]` location, read
    whether or not the start is trusted.~~ CLOSED by Steven's answer 3
    below: trust gates all of `.shannon/*`.
  - Steven's dogfooding, same day: the note never showed. The trust screen
    covers the notice row, and the key answering it wiped every launch note
    unseen — a config that did not parse among them — so keys on a
    full-frame screen no longer start a new moment (`begin_moment`). The
    note is said where it can be seen (`App::launch_trust`: without the
    screen, and after its no); a trust says what it read in its one
    sentence (as a second notice, the row's cut hid it). `:scriptnames`
    is a live pane, as `:ls` and `:rules` are.
  - Steven again: still no note — on Claude Code he met the trust screen,
    which did not say what the folder held. It does now
    (`trust::project_held`). The safety note says `:untrust`, the project's
    verb, not `:unsource`.
  - **`:config save` (Steven, 10-13: "saves the local settings created in
    that session … unsurprising to any users"; asked: sets and rules;
    refused where not trusted; "`:config! save` to write even if we
    haven't trusted").** Writes into `.shannon/config.toml`, making it:
    each key a `:set` named at its value in force
    (`ConfigStack::typed_for_project`), the session's rules, a shipped
    rule `:un…`'d as `remove`, a project rule `:un…`'d taken out — edited
    in place with `toml_edit`, comments kept — then reads it back where
    trusted, the rules from it now (session copies dropped, listed once).
    Claude's calls: the environment's effort is its own origin
    (`Origin::Environment`) so it is not saved as typed; a list files add
    to saves only what the `:set` added (a `:set` holds the whole list in
    force, your entries with it); a `-=` there, a `YOURS_ONLY` key and
    your config's rule removed are said, not written; a `.shannon` that is
    a link is refused (the repository holds it: a link to
    `~/.config/shannon` made the save write your own config); the file is
    replaced, never written through. `:reload` reads a project file made
    since launch in a trusted start.
  - Tests: `config_save_writes_what_the_session_set`,
    `the_sets_a_projects_file_would_hold`,
    `a_reload_reads_a_projects_file_made_since`, the screen and verb in
    the trust tests. Mutations, each red: the environment counted as
    typed; no `YOURS_ONLY` filter; the screen without what it holds; the
    `:unsource` verb for a project; no trust check; no link check (it
    wrote through); session rules kept; `remove` not written; `:reload`
    blind to a file made since.
  - **Steven on the three left open (10-13):**
    1. "More specific rules win no matter the config" — the one-tier
       reading above, confirmed.
    2. A project's model barred from replacing a shipped one: "No".
       3d4c235 reverted: a trusted project's model replaces Shannon's as
       yours does, at completion trust, with Shannon's facts standing
       under it (the union in `acts_of_argv`, `Source::Project` back in —
       mutation: without it a flat `find.toml` ran `find . -exec rm {} \;`
       under `allow find *`). The `gh auth token` case returns under a
       trusted project's flat `gh.toml`, as under your own; trust guards
       it.
    3. "Trust should gate `.shannon/*`": every file under a `.shannon/` is
       read only where the directory holding it is trusted
       (`Trusted::untrusted_shannon`), skills and memory locations
       included (the one knowledge judge, `App::never_read`, now says its
       whole reason: a credential path, or a `.shannon/` not trusted with
       the `:trust` that reads it). The screen and the note list all of a
       start's `.shannon/` (bounded: 64 read, 3 named, then a count — or
       "more" past the bound).
  - Tests: `a_reload_reads_the_projects_file_after_yours`,
    `a_projects_file_arriving_late_stands_where_launch_puts_it`,
    `the_projects_file_follows_trust_and_reload`,
    `an_untrusted_start_says_what_it_does_not_read`. Mutations, each red:
    the project laid on top; not lifted out for yours; no `follow_trust`
    after a trust; none after an untrust; `:reload` reading yours alone;
    `:source` of it as `Sourced`; the note given a trusted start.
- 2026-10-13, **the review of `0e862ca~1..2921362` (6d)** (A correctness,
  B robustness/security, C quality/tests; each in its own worktree at
  2921362). Fixed in 725427b and five `review 6d (n/5)` commits:
  - **Case** (B, probed on this host): the instruction-surface write
    guards were case-sensitive on a case-insensitive filesystem —
    `.SHANNON/config.toml` wrote the real file unasked. `\c` now; a rule
    is the same whatever its folding (`Body::same`), so an old `remove`
    still names it.
  - **`:config save`** (A): a list was written whole (the team's entries
    wiped) — lists change in place (`Typed { values, lists, left }`); a
    linked `config.toml` was read through and its target copied into the
    repository — refused; an inline table lost nested keys; `exec.env.X`
    slipped the YOURS_ONLY check (A, B, C); commented project rules
    mislabelled and not taken out; setup's provider choice saved as if
    typed (laid as your file's now).
  - **Trust** (A, B, C): the models gate was held by nothing — one door
    (`Trusted::project_models`), tested; project models reported; the
    trust sentence leads with what moved (the row's cut hid it); an
    unparsable project file is one refusal; a trust that cannot be saved
    is not made; `project_held` named honestly.
  - **`:reload`** (A, C): a missing file refused everything — now left
    out, and a gone one taken off and said; all-or-nothing measured.
  - **Notices** (A): only a whole-frame screen keeps the row; a float's
    keys no longer pile up. A launch that opened setup says what is
    unread.
  - **`:verbose set`** names `CLAUDE_CODE_EFFORT_LEVEL` (A, C).
  - **Tests** (C): the `:scriptnames` pane read as drawn and shown live;
    premises asserted; stack's dotted-key helpers folded into `set`'s.
  - Docketed (B, older than 6d): under a user's `allow git *`,
    `git checkout other -- .` and `tar -x` write files without meeting the
    path guards, and `git config core.fsmonitor <cmd>` arms a command a
    later `git status` runs — a broad rule is the user's call, but 6d made
    `.shannon/` a config surface. `TODO.md`.
  - Declined: spelling `PROJECT_DIR` in user-facing messages and the
    default `skills.paths` (C) — they are text the user reads, and the
    constant would only move the literal.
- 2026-10-27 (Steven, review stop 12 Q2: "Unintended. Order of arguments
  matter") — through a program that runs another, an allow names the
  inner program, then its words, in their order. Cut down to the outer
  act (`uv run`), it covers it only where the same rule, derived, meets
  every inner act of it (`decide::names_its_inner`): `allow uv run pytest
  *` no longer allows `uv run ls pytest` — uv's sync, then a read that
  allowed on its own. One level: a rule written further out never covered
  the innermost act (its `via` names the middle program), so such a line
  asked already. Acceptance `prefix-uv-run-another-*`; the manual says it.
- 2026-10-27 (Steven, Q3: "Delete it, but make sure you're sure") — the
  shipped `allow re:^sed -n …p` is deleted: the sed model reads the script.
  Measured first over its whole domain (8943 generated lines, both
  credential policies, plus compound lines): no decision moved. What moved
  is `safe_unattended`, false → true on 2394 lines, since a rule never
  clears that bar and a modelled read does; each was already true for
  `cat` over the same words, and wherever sed and `cat` differ, sed is the
  stricter. Held by `sed_printing_lines_is_never_looser_than_cat`.
- 2026-10-27 (Steven, Q1: "your home. Other homes should be implicitly
  covered … because they are further back") — a `~/` credential entry is
  YOUR home, in every reading: the plain entry was already, and the glob
  one (`~/vault/**`, read as `glob_entry_regex`) now is too. Measured:
  started inside your home, another home is outside and every read there
  asks; started at or above the homes (`/`, `/Users`), its `~/`-rooted
  credentials are inside and read unattended — Q8. The old
  `secrets::sensitive_path`, an any-home second encoding nothing in the gate
  called, is deleted; its specification is `permission::credential_tests`,
  asked of the gate. Porting it found a carve-out lifting every guard
  (`SECURITY-2026-10-27-a-carve-out-lifted-every-guard.md`, fixed) and two
  readings stricter than the old one's — Q9.
