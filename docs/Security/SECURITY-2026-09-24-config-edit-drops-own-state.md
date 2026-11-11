# SECURITY — `:config edit` silently un-protected Shannon's own state and key file

**Status: FIXED** (found and fixed 2026-09-24/27, verified by probe and by a
mutation-checked regression test). One end is untestable by construction; see
"What is not covered".
**Severity: HIGH** — a documented protection was silently removed for the rest
of the session. Partially mitigated by output redaction (see Scope).

## The issue

At startup `main.rs` added Shannon's own sensitive paths to the config it
handed the app — by **mutating `config.secrets.paths`**:

- `register_own_state`: the config file, the log directory, the episode store;
- `register_key_file`: the resolved `SHANNON_API_KEY_FILE`.

The permission gate is built from `[secrets]`, so a `cat` of any of those
paths was `Ask` with a `SensitivePath` cause — one that can never be
remembered as a standing grant.

`:config edit` → `save_and_apply_config` replaced `self.config` with the
**freshly parsed file** and rebuilt the gate from it (`rebuild_rules`). The
file never contained the startup registrations, so the rebuilt gate no longer
had them.

## Verified (probe, 2026-09-24)

An app whose config held `/opt/creds/shannon-token` (standing in for a
registered path; outside every shipped pattern), then `save_and_apply_config`
with an unrelated edit (`[tui] mouse = true`):

    before edit: rules.decide_line("cat /opt/creds/shannon-token") -> Ask
    after edit:                                                    -> Allow
    app.config.secrets.paths after edit: []

## Scope

- **Trigger:** any successful `:config edit`; persisted until relaunch.
- **Readable without asking afterwards** — foreground read-only commands run
  without a prompt, and unattended jobs could too:
  - the episode store: conversations from **every project**, which then go to
    the provider as tool output;
  - the log directory: every command every session ran (already scrubbed);
  - `config.toml` itself;
  - a `SHANNON_API_KEY_FILE` **outside** the `*.api` seed pattern. The default
    `.test_key.api` stayed covered by `*.api`.
- **What still held (Layer B):** command output is redacted, so an `sk-ant-…`
  key read from the key file is scrubbed by its issuer shape, startup
  `[[alias]]` values stay masked, and `secrets.extra` literals are scrubbed.
  The WALL (the gate) was gone; the NET remained. Anything the net does not
  recognise — above all other projects' conversation content — went out.

## How it got in

`rebuild_rules` was itself a fix (Reviewer E): the gate used to stay STALE
after `:config edit`, so `path_policy = "deny"` was reported as applied while
the gate still said `Ask`. The fix made the gate follow the config — and the
config it follows is the file, which does not carry the startup registrations.
A fix inside the neighbourhood of the previous one (`feedback_fix_neighbourhood`),
and AGENTS.md's rule exactly: *a value derived at startup is re-derived by one
function that both construction and mutation call.* The registrations were
applied at construction only.

A second door made it worse to spot: `rebuild_rules`' doc said construction
called it, and construction built its own inline copy of the rules.

Nothing caught it: `the_running_config_refuses_its_own_state_directories`
tested the registration at startup; no test drove an edit after it.

## The fix

**Registrations are not the user's config, and no longer live in it.** Steven:
people point an LLM at that file to help them edit it, so it is the wrong home
for anything a user should not be editing — and the file-as-parsed is what
every edit replaces.

- `main.rs::app_config` returns the config **exactly as on disk**, plus a
  separate `protected: Vec<String>`; `register_own_state` and
  `register_key_file` push into that list (the only two mutation sites —
  `rg 'config.secrets.paths.push'` found exactly lines 285 and 307).
- `App::protect_paths(protected)` stores them in `App.protected_paths`.
- `rules_for(secrets, protected, aliases)` is now the ONE place a `RuleSet` is
  built — construction and `rebuild_rules` both call it — and it adds the
  protected paths to the gate's copy of `[secrets]` every time.

The registrations are computed each launch from the same resolvers that create
the files; they are never written to disk, so there is nothing to edit or go
stale.

## Regression test

`a_config_edit_keeps_shannons_own_paths_protected` (app/tests.rs): a path
outside every shipped pattern is `Allow` → `protect_paths` → `Ask` → a real
`save_and_apply_config` → still `Ask`, and not in `config.secrets.paths`.
Mutation-checked, each RED: `rules_for` dropping the protected paths;
`rebuild_rules` passing none; `protect_paths` not rebuilding.

## What is not covered

`run()` in `main.rs` calling `app.protect_paths(protected)` is reachable by no
test — `App::run` takes a live terminal and nothing imports a binary's `run`
(`project_shannon_drive_harness`). The list it passes is tested
(`the_running_config_refuses_its_own_state_directories`), and the consumer is
tested; the one line joining them is not. The smoke harness could assert it
by asking the stub to read a registered path, if the stub could issue tool
calls — it cannot yet.

## Follow-up (design, Steven 2026-09-27)

Applying an edited config immediately is itself a hazard: it swaps live state
mid-session. The direction is vim's — editing the file changes nothing until
it is **sourced** (`:source`, reload = sourcing the current file). That
removes the path this bug travelled, but the fix above stands on its own: the
registrations must not live in the user's config whatever applies it.

## Follow-up: only this project's store was registered (2026-09-30)

**MEDIUM, pre-existing, FIXED.** Found during the rename to Shannon. The
registration this report restored named `store_root()`, the CURRENT
project's store. The episode store is per project, and the disclosure the
registration exists for is a session reading ANOTHER project's
conversations. So `cat ~/.local/state/shannon/projects/<other>/episodes/*`
was still an ordinary read: Allow, and safe unattended, under the default
policy. The test asserted only the current project's path, so it passed.

Fixed by registering the whole state directory (`Config::state_dir`), which
the store, the log and redacted records all now resolve through. Directories
left under the pre-rename name (`~/.config/shanon`, `~/.local/state/shanon`)
are protected too while they exist; they hold the same config and history
until they are moved. The test reads another project's episode file and an
old-name path, and deleting either registration turns it red.

## How to validate

1. Launch with `SHANNON_API_KEY_FILE=/some/path/outside/the/repo` (any file).
2. Ask the model to `cat` that path → a decision pane (Ask).
3. `:config edit`, change anything harmless, save.
4. Ask again → still a decision pane. (Before the fix: it ran without asking.)
