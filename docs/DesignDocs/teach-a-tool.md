*shannon-teach-a-tool.txt*   Teaching Shannon a tool it does not know

                SHANNON CODE — TOOL MODELS AS DATA

STATUS: PROPOSAL, for discussion with Steven (written 2026-09-30). Nothing
below is built. It follows "user-taught carve-outs: YES" (TODO, 2026-09-30)
and the slices already landed: rules in config (`a3fa1a9`), and an editable
`A` (`58431c9`).

==============================================================================
1. THE PROBLEM                                              *teach-problem*

Rules can now be written, but a rule about a tool Shannon does not model can
only name SPELLINGS. Measured on 2026-09-30 with `allow gh api *` plus
`deny gh api -X` / `-f` / `-F`:

    gh api -X POST …          Deny     the spelling the rule names
    gh api -XPOST …           Deny
    gh api --method POST …    ALLOW    the same flag, another spelling
    gh api … --field t=x      ALLOW

The engine cannot know `--method` is `-X` until something tells it. That is
the gap between "users can write rules" and "users can write rules that
hold". It is also why Claude Code's and Copilot's permissions push people to
blanket-allow `uv` or `find`: nothing between "this exact string" and "the
whole program" can be expressed, so alarm fatigue wins.

Shipping a complete safe list is impossible (custom tools, internal tools,
tools not yet written) and not wanted: it does not adapt to the user's
environment. The user must be able to TEACH a tool. And what is taught must
be readable and editable, as with rules.

==============================================================================
2. WHAT A MODEL IS TODAY                                       *teach-today*

Compiled Rust in `crates/shannon-core/src/shell/program.rs`. A `Program` is
`{ name, base: Effect, bundles_shorts, args }`, and an `Arg` is
`{ canonical, spellings, takes_value, elevates }`. Around it sit **eight
parallel tables keyed by program name**:

    FILE_FLAGS         a flag's value names a file (containment checks it)
    CODE_FLAGS         a flag's value is code (`bash -c`, `python -c`)
    WRAPPERS           runs the rest of the line (`timeout`, `env`, `sudo`)
    GUARDED_MODES      a mode left unmodelled because it is dangerous
    dash_number        `head -5` is `head -n 5`
    dashes_are_data    `echo "---"`'s dashes are data
    writes_operand     `uniq IN OUT` writes OUT
    names_a_variable   `read -a NAME`, `printf -v NAME`

One tool's knowledge is spread over nine places. That shape cannot become
user data as it stands. **Step zero is one record per tool**, with each table
becoming a field of it. This is the refactor the rest depends on, and it is
worth doing for its own sake: today adding a program means remembering up to
nine places (`[[feedback_rules_rot_by_retyping]]`).

==============================================================================
3. THE PROPOSAL                                             *teach-proposal*

A. A MODEL IS A RECORD, IN THE CONFIG'S OWN FORMAT.

    [[tool]]
    name = "gh api"                 # a program, or a subcommand path
    base = "read"                   # read | write | destroy | run | unknown
    flags = [
      { names = ["-X", "--method"], value = true,
        raises = { to = "write", unless = ["GET", "HEAD"] } },
      { names = ["-f", "--raw-field", "-F", "--field", "--input"],
        value = true, raises = "write" },
      { names = ["-H", "--header"], value = true },
      { names = ["-q", "--jq"], value = true },
      { names = ["--paginate"] },
    ]

    [[tool]]
    name = "uv run"
    wraps = { after_flags = true }  # the rest of the line is the command
    flags = [
      { names = ["--with", "--python", "--directory"], value = true },
      { names = ["--frozen", "--no-sync"] },
    ]

What is new against today's `Arg`:
- **Subcommand paths** (`gh api`, `gh pr view`). The record that matches
  most specifically wins: `gh pr view` over `gh`.
- **Elevation by value** (`unless = ["GET"]`). Today `elevates` is
  unconditional, so `--method` would be "write" even for `GET`.
- **Wrappers as data**, so `allow pytest *` covers `uv run pytest` and
  `python -m pytest`, and rules about ruff apply inside `uv run`.

B. LAYERED THE WAY NEOVIM LAYERS LSP CONFIGS.
`vim.lsp.config` deep-merges a user's table over the shipped one, and
nvim-lspconfig is a community collection of defaults that users override.
Same here:

    shipped seed  →  community pack (opt-in)  →  your config  →  project
    (.shannon/)

A later layer extends an earlier record with the same name, field by field.
`replace = true` starts that record fresh.

C. AN UNTRUSTED LAYER CAN ONLY ADD CAUTION.
A project's `.shannon/` is written by whoever wrote the repository. It may
say a flag is dangerous, add spellings, or name a wrapper. It may NOT lower a
`raises`, lower a `base`, or remove a flag. Lowering is what your own config
is for. Every project-layer model is named at load, like the safety keys.
Without this, `git clone` plus a project file would be a way to allow
anything.

D. WHERE SPELLINGS COME FROM.
Hand-writing the spellings of a large CLI is the tedious part. Shell
completion specs already describe flags, aliases, which flags take values,
and subcommands for hundreds of CLIs: fish's `complete`, carapace specs,
Fig's autocomplete specs. **They hold grammar, not danger.** A generator
could turn one into a DRAFT record: spellings and values filled in, every
`raises` left for a human. [My confidence: medium. I know these exist; I
have not checked their coverage of `gh` and `uv`, or their licences.]

E. TEACHING IN THE APP.
`:teach gh api` opens a draft record in `$EDITOR`: from a completion spec if
one is found, else empty with the fields commented. Saving writes it into
your config, and `:analyze` then shows which model decided each stage.
Running `gh api --help` to draft from would EXECUTE the tool, so that path
asks first, like any other command.

F. EVERY MODEL CARRIES EXAMPLES.
The shipped models are held by tests (`every_dangerous_escalation_is_carved_
out`). A taught record may carry examples, which `:teach` checks at load
and reports if one no longer holds:

    examples = [{ run = "gh api -X POST repos/o/r/issues", is = "write" }]

==============================================================================
4. WHAT IT DOES NOT SOLVE                                     *teach-limits*

- **Programs whose danger is in their PROGRAM TEXT** (sed and awk scripts,
  `python -c`, a `jq` filter calling `env`). A flag model cannot see inside
  a script. Those need a per-language reader. That is TODO's sed/awk slice,
  separate from this one.
- **A model can be wrong.** A taught `base = "read"` on a tool that writes
  is the user's statement, and is honoured. `:analyze` must make the source
  of every judgement visible ("gh api: your config"), so a wrong model is
  findable.

==============================================================================
5. STAGING                                                   *teach-stages*

0. One record per tool: fold the eight tables into `Program`. Pure refactor.
   The suite must not change, and a test asserts each old table's entries
   survive.
1. Subcommand paths and elevation by value in the compiled seed.
   `gh api` / `gh pr view` / `git push` are seeded to prove them.
2. `[[tool]]` in config, merged over the seed (user layer only).
3. `:analyze` names the model behind each stage.
4. `:teach` with an empty template; then from completion specs.
5. The project layer, add-caution-only, named at load.
6. Examples checked at load.

==============================================================================
6. DECISIONS FOR STEVEN                                  *teach-decisions*

1. The record format: `[[tool]]` tables in config.toml (proposed), or a
   directory of one file per tool (`~/.config/shannon/tools/gh.toml`, like
   `nvim/lsp/*.lua`)? One file per tool shares and diffs better.
2. A project layer that can only add caution (proposed): is "a repository
   can never make Shannon trust more" the rule you want?
3. Community pack: shipped in the binary but off by default, or fetched?
   Fetching is a supply-chain surface; I lean towards shipping it.
4. Order: step 0 is a pure refactor with no visible change. Do it first, or
   prototype steps 1 and 2 on top of the current tables to get feedback
   sooner?

 vim:tw=78:ts=8:ft=help:norl:
