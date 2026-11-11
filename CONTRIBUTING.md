# Contributing

Issues are the most useful thing right now: a bug, a vim motion you miss, a
permission rule that cannot say what you mean, a screen that misled you. Pull
requests are welcomed too. 
A security problem goes through `SECURITY.md`, not an issue.

By submitting a contribution you agree that it is licensed under the MIT
license (`LICENSE`), as the rest of Shannon is.

## Building and checking a change

The bar for a change is four commands, run separately, so that a failure
says which:

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo doc --no-deps --workspace
```

and, for anything you can see or type, the smoke drive, which runs the real
binary through a terminal (no API key, no network):

```sh
cargo build --bin shannon
ANTHROPIC_API_KEY= python3 tools/drive/smoke.py
```

`AGENTS.md` holds the conventions: how code, tests and docs are written here.
It is also what an AI agent working in this repository is told.

## Reading the history in the comments

Comments here say why, and often whose decision it was and what found the
problem:

- "Steven, 10-06: …" quotes the maintainer's decision, dated.
- "review stop 12", "Reviewer D", "round 17" name the review pass that found
  a problem.
- A `[[name]]` names a note in the maintainer's working notes.

Shannon is developed in a separate repository and published here, a review
stop at a time. The review notes, the docket (`docs/Milestones/`) and the
ideas these comments cite stay there, as do the commits whose ids the docs
quote: those names are history, not links.
