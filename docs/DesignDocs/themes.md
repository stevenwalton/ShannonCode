# Themes and highlight roles

Steven, 2026-10-05, showing his ghostty palette and `bat` in his terminal:
"I'm showing you the theme not to replicate the exact colors, but to show you
the mappings. If we can inherit colors in a similar way that would be nice. We
talked about doing it with the LSP to do syntax highlighting. I eventually
would like a way to select different themes and for people to install them …
ensure that we use overlapping primitives as the LSP can be used for many
things (completion, syntax highlighting, warnings, etc)."

## The three primitives

1. **A role** — `style::Ink`. What a piece of text IS: a keyword, a string, a
   command's flag, a line a write adds. Never a colour. Named the way
   tree-sitter names its highlight captures (`function.call`,
   `variable.parameter`, `markup.raw`), which is the vocabulary neovim, helix
   and zed themes already share and the one an LSP's token types map onto. A
   macro declares each role with its dotted name, so `Ink::ALL` is complete by
   construction.
2. **A styled cell** — `style::CellStyle { ink, bold, italic, dim }`. What
   every highlighter produces, one per character, and what the output cache
   stores. It holds a role, so a new theme recolours the transcript without
   highlighting it again.
3. **A theme** — `theme::Theme`. Role → `Paint` (a colour and emphasis), per
   `Context`. `Theme::style(context, cell)` is the one place a role becomes a
   colour; `palette.rs`'s consultation test keeps colours out of the render
   sites.

Every highlighter speaks (1) and produces (2):

| Highlighter | Where | How it names roles |
|---|---|---|
| The gate's shell lexer | decision pane, transcript records | `permission_view::lexeme_syntax` |
| syntect | a model's fenced code, a Read's output, a write's diff | `highlight::ROLES`: TextMate scope selectors → role |
| markdown | the transcript | `markdown.rs`: heading, raw, fence |
| a language server | **not yet** — see below | `Ink::from_lsp` |

syntect is driven through a synthetic syntect `Theme` whose "colour" for each
selector carries the role's index. syntect's own selector scoring therefore
picks the role, which is exactly what selector scoring is for. No `meta.*`
selector is used: a meta scope spans a whole construct, so it would paint the
arguments of a call as the call.

## Contexts

The decision pane is the one surface where green, yellow and red are a
verdict. A string painted green there reads as "this runs". So a theme has two
maps: `Code` (everywhere) and `Decision` (keys prefixed `decision.`; a role
the pane does not name falls back to the code entry). The pane paints a
command's shell and, for a Write, a diff in its file's syntax. So
`Theme::load` checks **every** role (`Ink::ALL`), and warns when a theme puts
one on a verdict hue in either shade, or on the exact magenta of a withheld
value. The exceptions are `diff.*` and `diagnostic.*` (`theme::MARKS`), whose
colour is a severity on purpose. An earlier version checked a list of the
shell's six roles kept beside the lexer, while the shipped theme painted a
diff's types yellow (review of f50fda5). The shipped theme names
`decision.type`, `decision.tag` and `decision.markup.raw` for that reason.

**The verdict's own colours are a theme's too** (Steven, 10-14: "If there's a
color defined in our program, we should let someone be able to specify how
*they* want that drawn"). Until then they were `Look::Own` — "a theme
recolours the shell and never the answer". But the shipped verdict names the
terminal's red and green, whose values the terminal's own theme decides, so
that lock was never whole. What stands in its place is said at load, never
enforced: a shell role on the theme's own verdict colours (`taken_hues`, now
read against the theme's `ui.safe`/`ui.caution`/`ui.danger`), and a scale
with two steps alike (`verdicts_alike`: equal, one terminal hue in two
shades, or 24-bit colours within ≈40 of 441). Held by
`a_hostile_theme_is_told_what_it_did`.

**Emphasis is the theme's too.** A command's name is bold because the shipped
`function.call` is, not because the lexer's cell says so. A cell's own
emphasis is for what the text itself carries (markdown `**bold**`), and it
adds to the role's, so a weight baked into a role's cell could never be taken
off.

**Specificity beats inheritance.** A file's `string = "yellow"` does not
reach the pane's strings, because the shipped theme names `decision.string`
and the more specific key wins whether it is inherited or written in the file.
This is helix's rule too. The manual says so.

## Inheriting the terminal's palette

The shipped theme, `terminal`, names ANSI colours only (a test refuses
`Rgb` and indexed colours past 15). `keyword` is the terminal's magenta,
whatever the terminal's theme makes magenta, which is how `bat --theme=ansi`
inherits a palette. The mapping follows the convention most terminal themes
are drawn for: keywords magenta, strings green, functions blue, types yellow,
comments the dim grey, numbers and constants bright magenta.

## Theme files

`tui.theme = "<name>"` reads `themes/<name>.toml` beside the user's config:
helix's shape (`inherits`, a role per key, `[palette]`, a table for emphasis,
`modifiers = ["bold"]`). A role is one key however TOML spells it. Unquoted
`function.call` is TOML's nested table, and `[decision]` is a table too, so
both are flattened to dotted keys; a table with a colour's field is a paint.
Read through `config::read_file`, so a FIFO or a device is refused rather than
read on the draw thread. The name is a plain name, never a path. Inheritance
depth is bounded at 8, so a cycle is an error. These are each reported, not
skipped silently:

- an entry that names no role;
- a colour that is not one;
- a field a paint does not have;
- a bad palette entry;
- a non-string `inherits`.

A parse error goes through `Config::describe_parse_error`, by line and never
quoting the line. A load says at most `MOST_NOTES` things.

**Applying it.** `App::load_theme` runs at launch (`set_config_stack`, once the
directory is known) and on every config change (`adopt_config`). So `:reload`
also takes an edited theme file whose name did not change. **A load says a
thing once** (`App::theme_said`): notes when the theme or the notes are new,
and a refusal when it is a new one. A bad `tui.theme` used to repeat its
refusal on every unrelated `:set`. A failure keeps the current theme and says
why.
`App::set_theme` is the one door that writes the field, and all it does.
**Every page is painted at the draw** (2026-10-06, approved by Steven): a
`TextView` holds `paint::PageLine`s whose runs carry ROLES (`Look::Role`) or
the app's own style (`Look::Own`), and `view::painter` — one exhaustive match
over the pane's kind — gives each pane its map (`Context::Decision` for the
decision pane and `:analyze`, `Code` for the rest). So an open decision and
an open `:analyze` show a new theme on the next frame; `ToolPrompt::restyle`
and its `Arc<Theme>` are gone. The verdict's colours are roles too, since
2026-10-14 (above). `:analyze` stays a snapshot of the rules.

**Installing a theme** is dropping a file in `themes/`. A name is looked up
there first, then among the shipped themes — so a file of yours named for a
shipped one is yours, and `inherits = "nord"` starts one of yours from a
shipped one.

**Shipped** (2026-10-14): 21 files in `crates/shannon-tui/themes/`, read into
the binary (`theme::SHIPPED`, a macro over the names, held to the directory
by `every_theme_file_is_shipped`): Catppuccin ×4, Tokyo Night ×4, Gruvbox ×2,
Nord, Dracula, One Dark, Solarized ×2, Rosé Pine ×3, Kanagawa ×3. Each is its
upstream palette's sixteen terminal colours and ground — upstream, not a
terminal's copy, which drifts (`themes/NOTICE.md` says whose, and the
licenses) — inheriting `terminal`, with what that palette needs said:
dark text on the decision pane's fills and the bar (white on a pastel red
reads poorly); in a light theme, its text where the shipped theme paints
white; where a palette repeats its magenta in the bright slot (Nord, One
Dark, Rosé Pine), the pane's expansions moved off the withheld value's
colour; Solarized's grey brights moved off its strings and flags;
Kanagawa Lotus's olive yellow, too near its green, traded for its orange as
the caution. `every_shipped_theme_loads_and_says_nothing` holds each to the
load-time checks. Everforest is not shipped: its terminal mapping was our
derivation, not its authors'.

**Imported** (2026-10-14): `tui.theme_import` (default `["ghostty"]`) names
directories of terminal palettes — ghostty's and iTerm2's format, sixteen
colours, a background, a foreground, a selection — each a theme by its
file's name, after yours and the shipped ones (`theme::Places`,
`theme/ghostty.rs`). `ghostty` resolves to ghostty's own lookup:
`$XDG_CONFIG_HOME/ghostty/themes` (else `~/.config/ghostty/themes`), then
`$GHOSTTY_RESOURCES_DIR/themes` or the platform's bundle. A palette is made
a theme by the shipped template's rules (`ghostty::entries`): its ground and
text, dark text on the fills and the bar, a light ground's text where
`terminal` paints white, a repeated magenta moved off the decision pane.

Two boundaries. **A name is found, never joined**: an imported name must
equal a file name in a directory's listing (`ghostty::listed`), so `../x`
or `sub/x` names nothing (`an_imported_name_reaches_no_further_than_its_
listing`). **Only colours are read**: a ghostty theme file is a ghostty
config file and may set any option; every other key is skipped unread. And
no test app reads this machine's directories: the environment is handed
over at launch (`App::set_theme_env`), `None` until then.

**The picker** (2026-10-14): `:themes` opens two panes, as `:graph` and
its inspector do — the list (`PaneKind::Themes`, `theme_picker::ThemeList`)
and beside it the preview (`PaneKind::ThemePreview`,
`theme_picker::Preview`). The pairing is said once, `PaneKind::companion`,
and read by its four consumers: a host brings its companion as a viewer
(`App::open_viewer`) or a split (`App::split_command`), takes it when it
leaves (`App::release_companion`), and a companion with no host of its kind
goes (`App::reconcile_pane_state`). Each named the graph and the inspector
until the second pair arrived.

- **The list is a page**: every motion, and `/` `?` `n` `N` — a search
  MOVES to a theme and the list stays whole (Steven: "act like our standard
  searching", against ghostty's narrowing). `<CR>` is its one claimed key:
  it wears the theme under the cursor through `:set`'s door (`set_items`,
  the name one value — an imported one has spaces), so it is a session layer
  like any `:set`. Read once, at open (`Places::themes`, which lists
  ghostty's 463 files), grouped by place; the worn mark is re-derived each
  frame, from the theme WORN, not the name `tui.theme` holds (which may be
  one that could not be read).
- **The preview wears the candidate**: its page (`Look::Own` styles from the
  candidate theme), its ground (`view::reground` — where the candidate leaves
  `ui.background` unset it is the terminal's own, put back, not the worn
  theme's), its border and number (the pane's painter is built from the
  candidate). Loaded once per name; a new width re-lays it, keeping the
  reader's place; the cursor on a line naming no theme (the lead, a heading) previews the theme worn. It holds two
  transcripts side by side (a reply with markdown and code; a tool's record
  with a diff and diagnostics), the prompt with a secret (the one thing the
  prompt colours), the status row, a refusal and a notice, a confirmation, a
  decision with both fills, the terminal's sixteen as the theme says them,
  `:graph`'s lanes and marks, the bars, the manual's markup, and what loading
  said. A theme that cannot be read is previewed as the reason, in the worn
  theme.
- **A viewer typed in the list opens across the top**, as over `:graph`: the
  rule is now "over a host" (`companion().is_some()`), so `:help themes` —
  the pointer the list's own status row gives — keeps the pair.

## The language server (planned, not built)

The primitives above are the ones an LSP client would land on, so its arrival
adds producers and nothing parallel:

- **Semantic tokens** → `Ink::from_lsp(type, modifiers)` → `CellStyle`.
  Layered over syntect's pass for the same file: syntect first (fast, no
  project needed), then the server's tokens replace roles where they arrive.
  `from_lsp` exists, with no consumer yet. That is a deliberate seam under
  earned use, and the doc comment says so.
- **Diagnostics** → the `diagnostic.{error,warning,info,hint}` roles, which the
  theme already paints. They will need an underline channel on `CellStyle` (the
  decision pane already draws carrier underlines, through `Run`), not a second
  colour table.
- **Completion** → a completion item's kind (function, variable, keyword …)
  maps onto the same roles, so a completion menu is coloured by the theme
  without a theme of its own.

Where it applies: files (a Read's output, a write's diff, a file opened in the
prompt's editor). A model's code block has no project, so it stays syntect's.

**A security note before any of it is built:** a language server is a program
that runs project code (rust-analyzer runs build scripts and proc macros). So
starting one is an execution the gate and `:trust` must answer, like any
command. It is never a highlighting detail.

## Every colour is a role (2026-10-14)

The chrome joined the theme: every constant `palette.rs` named is a `ui.*`
role (`style::Ink`, appended after the highlighters' so syntect's indices did
not move) — the ground, text weights, borders, the status row, the verdict
and its two decision-pane fills, confirm and notice colours, the picker's
chosen line and bar, `:graph`'s tree, lanes, marks and shared rows, the
analyser's carriers, the manual's markup. `palette.rs` is now only the
shipped theme's values, read by `theme::TERMINAL`. **No other file names a
colour**: `only_the_theme_names_a_colour` walks every source file at test
time (a file list could not check that it lists everything) and refuses
`palette::` or `Color::` outside test modules. Render sites ask `Theme::ui`;
pages built ahead of the draw carry roles (`paint::Run::role`), including the
decision pane's verdict, its legend and a carrier's underline.

A paint has a **background** (`bg`). `ui.background` is the ground: drawn
under the whole frame before anything else (`view::ground`) and again after a
`Clear` (`setup::clear`), so unstyled text and every widget wear it. Unset —
as shipped — nothing is drawn and the terminal's own shows, which is how
helix treats an unset `ui.background`.

**A ground owns the cursor** (11-02). The prompt and a transcript placed the
terminal's cursor, whose colour is the terminal's config's, chosen for the
terminal's ground; a page drew its own, a reversed cell, which followed the
theme. Catppuccin latte on Steven's dark ghostty left the prompt's cursor a
faint yellow on cream (image 42) while the decision pane's was dark (image
50). `ui.cursor` is the cursor's colour — its `bg`, or a bare colour, as vim's
`Cursor` group's guibg is — told to the terminal as `#rrggbb` (OSC 12) after
a frame that changed it, and given back (OSC 112) for `$EDITOR` and at exit
(`terminal::paint_cursor`). Unnamed in a theme with a ground, it is that
ground's text: the block a page's reversed cell showed (`Theme::resolve`).
Unnamed with no ground, or `"default"`, the terminal keeps its own — Neovim's
default too (`'guicursor'`: no group, "host-terminal default cursor colors").
A ghostty palette's `cursor-color` is read as it. A colour a terminal cannot
be told — a terminal colour's name — is said at load.

**The cursor's shape says the mode** (11-02, S21-Q22: "the same 2-signal
process … an immediate response"). `tui.guicursor` is neovim's
`'guicursor'`, read by neovim's rules, each taken from its source rather
than from memory: the mode is `cursor_get_mode_idx`'s (`cursor_shape.c`) —
the `:` line by where its cursor is (`c` at the end, `ci` inside), then an
operator waiting (`o`), then the keymap's mode, with `r` waiting as Replace
(`nvim_get_mode()` says `R` there, in Visual too); the value's reading is
`parse_shape_opt`'s (every mode a steady block, each part overwriting what
it names, an error refusing the value — Shannon keeps the shapes it had and
says why, as a theme that cannot be read is kept); the escape is
`tui_set_mode`'s (DECSCUSR, blinking only when `blinkon` and `blinkoff` are
both above 0) and DECSCUSR 0 gives the terminal's own back. The default is
neovim's less its Terminal part. One source for both signals:
`App::cursor_mode` asks the keymap the status row's mode comes from
(`guicursor.rs`; colour and shape written and given back together,
`terminal::CursorTaken`).

**One cursor, the terminal's, in every pane** (11-02, S21-Q23 — Steven: "If
it is elegant and saves us from more future errors then that's how I want
it"; Reviewer D, `scratchpad/review_r19D.md`). A page painted its own, a
reversed cell: no shape, no blink, `ui.cursor` only by a derivation's
coincidence, no anchor for an input method. And the two mechanisms
disagreed in four live defects: a focused page with `:` open drew two
cursors (F5); a secret's colouring erased a decision page's (F3); one on the
space a full row drops was drawn past the row and clipped (F4); and the
prompt's and a transcript's, over their reversed Visual range, were the
range's own colour once `ui.cursor` followed the ground (F1, a regression of
the paragraph above). This doc had said a page could not use the terminal's
cursor for F1's reason — while the transcript already did.

Now each draw returns where its cursor is, and `view::draw` is the one
door: the focused pane's, or the `:` line's while it is open (the predicate
the keys use, where draw order decided); `view::place` makes the one
`set_cursor_position` call and draws the cell under the cursor plain, so a
Visual range cannot swallow it, whichever builder reversed the cell. A
page's cell is measured by the transcript's map
(`TextView::cursor_cell` → `wrap::cursor_in_wrapped_word`). Shape, blink and
colour reach every pane. `:graph` keeps a list's look: its cursor is its
reversed row, and the terminal's is placed on the row's first cell only
inside a Visual range. Given up: several cursors on screen at once, which
vim does not show either. Not done: the shelf's name field still draws a
`▏` caret of its own (F8).

**A `[palette]` entry named for a terminal colour** (`black` …
`bright-white`) says what that colour is, for every role — the inherited
ones, which the shipped theme paints `magenta`, included (`Theme::resolve`
remaps after resolving; a number below 16 is the same slot). So a theme
drawn for a terminal's sixteen colours, as ghostty's and iTerm2's are, is a
whole theme here with sixteen entries and a `ui.background`. Colours written
outside a theme (`tui.ctx_colors`' `70:yellow`) go through the same remap
(`Theme::said`).

The two truecolor holdouts (`SHARED_ROW`, `MARK_BG`) are still the shipped
values, named by `the_shipped_theme_inherits_the_terminals_colours` as its
only exceptions.
