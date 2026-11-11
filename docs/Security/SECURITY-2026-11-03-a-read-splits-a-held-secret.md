# SECURITY — a file-tool Read hands the model a held secret: numbered lines split it, the cut leaves a prefix

**Status: FIXED 2026-11-04, in review stop 16's E commit (the fix below,
as E proposed; Steven, 11-04: "The fix seems correct to me"). Opened
2026-11-03: the gap was on `origin/main` (c80c282) since the file tools
landed (F1, 77f31c2) — `Read` numbered and cut its lines before the
redaction that masks held secrets ran. `origin` is private (Steven,
11-04), so the gap was never public.**

**Found:** 2026-11-03, review stop 16, Reviewer E (white-hat), finding E1 —
measured at `exec::run_file` with probe tests, not by reasoning alone
(`docs/Reviews/2026-11-03-stop16/E-security.md`). Traced again by Claude in
the main tree: `files::run::read` builds the numbered text
(`text::window`) and returns it in `Done::did`; `Done::scrub`, the
redaction, runs only afterwards, in `exec::run_file`.

## HIGH — a value you marked secret reaches the API, the store and the transcript

A secret you mark (`:secret`) is held by Shannon and shown to the model as
a handle (`{{name.xxxx}}`). Every output the model receives passes the
redaction, which replaces a held value with its handle — by exact
substring. A file-tool Read transforms the file's text BEFORE that
redaction:

- each line becomes `N\t<line>`, so a held value that spans lines,
  `line one\nline two`, appears as `1\tline one\n2\tline two` — no longer
  a substring, so nothing matches, and the value goes out whole;
- each line is cut at 2,000 characters, so a single-line value that
  straddles the cut appears as a prefix of itself — again no match.

Measured (E): a value `hunter2probeLineOne\nhunter2probeLineTwo`, written
into a file by its handle (S21-Q27's flow — a Write that spends a handle),
then Read: both lines came back in clear, `redacted = []`, and the
outbound mask (`mask_message_secrets`) caught nothing either. A held value
at column 1,990 came back as its first ten characters. A single-line value
not at the cut is masked, as promised (the control).

The model's natural path reaches it: the file tools are what it is steered
to for files (`file-tools.md` §3.7), and the docs promise "a later Read
comes back as the handle" (`file-tools.md:367-370`) — false for any held
value with a newline (PEM armour aside: its redaction matches by
substring and survives the numbering), or past the cut. `cat` of the same
file masks the value (the shared `redact`, inferred, not run): the file
tool is weaker than the shell here.

**What is lost:** the value is sent to the API, written to the session
store and the transcript, and at `[exec] audit = "full"` to the audit log.

## Scope

- Every file-tool Read, on Shannon's own backend and as a record under
  Claude Code's.
- Every held value that contains a newline (not PEM), or that a line's cut
  at `line_chars` (2,000) falls inside.
- Not reached by `bash` output (no numbering, no cut before the redaction).

## Validate

A test at `exec::run_file` (or through the app's tool path):

1. hold a secret whose value has two lines;
2. Write `{{handle}}\n` into a file (the handle spent at the run);
3. Read the file, and assert that NEITHER line of the value appears in
   the result, whole or alone;
4. and a single-line held value placed across column 2,000: assert that no
   prefix of it of more than a few characters appears.

## Fixed (2026-11-04)

The Read scrubs the file's decoded text **whole, its lines kept**, and only
then windows, numbers and cuts it (`files::run::shown`):

- `secrets::redact_lines` — the redaction as ever, but every replacement
  that runs over newlines keeps them (`Lines::Keep`, one door,
  `Lines::instead`): the handle (or `[redacted: …]`) on the line the value
  starts on, `[… continued]` on each later line it has a character on. The
  four passes that can cross a newline go through it: the held-value mask,
  a key block, a configured literal, a URL's userinfo. The token pass
  never crosses one. So the k-th line out is the file's k-th: offsets and
  read-first's stamps (taken on the raw bytes) stay true.
- Scrubbed whole, because a value can start above the window and end in
  it — **where one could run over a line** (`secrets::crosses_lines`: a
  value held or configured with a newline, a key block's armour, a URL's
  userinfo broken by one). Anywhere else every replacement falls within a
  line, so each line shown is scrubbed alone, whole, before it is cut:
  the same text, at the cost of what is shown. Scrubbed whole, a 16 MiB
  file's Read took 2.1 s (release) where its window took 11 ms; measured,
  and held by `a_read_of_a_big_file_costs_what_it_shows`.
- The redactions reported are those in the lines shown
  (`secrets::added_redactions`): a token on line 500 is not news to a Read
  of lines 1–100, and a placeholder the file itself holds is its text.
- The door's scrub still runs over every call's text (`Done::scrub`);
  redaction is idempotent.

Held by (each mutation-checked, red when its guard is deleted):

- `files::run::tests::a_held_value_never_reaches_a_read` — the proptest:
  a value of capitals over 1–4 lines, in lowercase noise, at any column,
  under any `line_chars`/`max_bytes`/offset/limit: no capital reaches the
  model, and the window's total is the file's.
- `exec::tests::a_held_value_read_back_is_its_handle_whatever_its_lines` —
  E's probe: two lines written by their handle, Read back as
  `1\tPASS={{db.…}}\n2\t[… continued]\n3\tnext`; a value at column 1,990
  leaves no prefix.
- `secrets::redact::props::a_scrub_by_lines_keeps_every_line` — the four
  passes, planted in output-shaped noise: as many newlines and lines out
  as in.
- `secrets::redact::tests::a_value_over_lines_keeps_them_only_when_read_by_number`,
  `…::only_the_redactions_a_scrub_put_in_are_counted`,
  `…::what_can_run_over_a_line_is_known_first`,
  `files::run::tests::a_read_counts_only_what_it_took_from_its_lines`,
  `…::a_value_over_lines_of_any_kind_stays_out_of_a_read` (each of the
  four makes the Read scrub whole, held at the Read),
  `files::text::tests::a_line_is_scrubbed_whole_then_cut`.

## Fix (as proposed by E)

Scrub before the transform, by range: find held values (and pattern
redactions) in the decoded body as byte ranges; window, number and cut as
now; redact each emitted line's covered part — the handle on the range's
first line, a continuation marker on its later ones — keeping the line
count, so offsets and the read-first stamp (computed on the raw bytes)
stay true. A proptest: a held value with newlines, at any column, across
the cut, never appears in a Read's result, neither whole nor as any of its
lines. The class is known (`[[feedback_scrub_before_cut]]`: a truncation
before the matcher leaves a prefix): every transform of a model-facing text
must come after the scrub, or carry the scrub's ranges through it.
