//! The `:help` manual — Shannon's user documentation, *comprehensive AND
//! terse*, judged as a whole document, not per feature.
//!
//! Docs work because a terse entry (`:h :x` is two sentences) sits
//! inside a file (`editing.txt`) that has an intro, sections, context, and
//! cross-references. This module reproduces that structure: one manual with a
//! table of contents, `*tags*` marking every section, `|links|` between them,
//! and a concepts chapter before any key list. `:help` opens the whole
//! manual; `:help <arg>` opens it **at** whatever `arg` names — a chapter, any
//! `*tag*` in the text, a `:command`, or an option as `:set` reads it (the
//! OPTIONS chapter, generated from the default config). One page, one anchor
//! (every target Tab-completes).
//!
//! Content documents only what exists: when a feature lands, its chapter is
//! edited in the same commit, exactly like code comments.

/// One chapter of the manual: the `:help <name>` argument, its `*tag*`, and
/// its body. Chapters are data so the composer, the topic lookup, and Tab
/// completion can never disagree about what exists.
pub struct Topic {
    /// The `:help <name>` argument (and the Tab-completion word).
    pub name: &'static str,
    /// Extra `:help <alias>` names that resolve to this same chapter — vim
    /// overloads a term by giving one location several `*tags*` (e.g. a reader
    /// hunting for `security` should land on the `decisions` chapter). Aliases
    /// Tab-complete too, and the chapter carries a matching `*tag*`.
    pub aliases: &'static [&'static str],
    /// One-line description shown in the table of contents.
    pub summary: &'static str,
    /// The chapter body, starting with its `====` rule and tagged heading.
    pub body: &'static str,
}

/// Every chapter, in manual order. The COMMANDS chapter is generated from the
/// command table and the OPTIONS chapter from the default config, at compose
/// time (`chapter_body`); their `Topic`s here carry empty bodies — one list
/// still drives the TOC, the lookup, and completion.
pub const TOPICS: &[Topic] = &[
    Topic {
        name: "intro",
        aliases: &[],
        summary: "what Shannon is, and the ideas everything rests on",
        body: INTRO,
    },
    Topic {
        name: "editing",
        aliases: &[],
        summary: "the modal editor in the prompt (and the chat pane)",
        body: EDITING,
    },
    Topic {
        name: "commands",
        aliases: &[],
        summary: "the : command reference",
        body: "", // generated from the command table at compose time
    },
    Topic {
        name: "windows",
        aliases: &["panes"],
        summary: "panes, splits, focus, and the <C-w> commands",
        body: WINDOWS,
    },
    Topic {
        name: "buffers",
        aliases: &[],
        summary: "sessions out of sight, :ls, and tab pages",
        body: BUFFERS,
    },
    Topic {
        name: "graph",
        aliases: &[],
        summary: "the :graph viewer and every operation on history",
        body: GRAPH,
    },
    Topic {
        name: "sessions",
        aliases: &[],
        summary: "models, branches, children, and how a turn ends",
        body: SESSIONS,
    },
    Topic {
        name: "decisions",
        // The words people arrive with. Deliberately NOT "tools" — that
        // reads as the `:tool` command, which is a different thing.
        aliases: &["permissions", "security"],
        summary: "the decision pane, :analyze, what bounds a command",
        body: DECISIONS,
    },
    Topic {
        name: "rules",
        aliases: &[],
        summary: "what answers a call before you are asked",
        body: RULES,
    },
    Topic {
        name: "secrets",
        aliases: &[],
        summary: "what never reaches the model, and how it is still used",
        body: SECRETS,
    },
    Topic {
        name: "config",
        aliases: &[],
        summary: "the config file and every section in it",
        body: CONFIG,
    },
    Topic {
        name: "memory",
        aliases: &[],
        summary: "instruction files, memories, and skills",
        body: MEMORY,
    },
    Topic {
        name: "options",
        aliases: &["settings"],
        summary: "every setting, at its default",
        body: "", // generated from the default config (`options_chapter`)
    },
];

/// The topic names, for the `:help <topic>` refusal message (the valid names).
pub fn topic_names() -> Vec<&'static str> {
    TOPICS.iter().map(|t| t.name).collect()
}

/// Every chapter name followed by its aliases (`panes` for `windows`).
///
/// A list of these WAS the `:help` completion list, and that was the bug: it
/// offered the chapters while `:help` accepted every tag and command.
/// Completion is [`targets`] now, which includes these.
fn topic_words() -> impl Iterator<Item = &'static str> {
    (TOPICS.iter()).flat_map(|t| std::iter::once(t.name).chain(t.aliases.iter().copied()))
}

/// The words `page` marks as `want` — a [`Mark::Tag`] or a [`Mark::Link`], as
/// `marks` reads them: a name with no space, standing apart, so prose using
/// a pipe or an asterisk is neither. It was a namespace (every tag began
/// `shannon-`), dropped as verbose (Steven, 11-08) once the reading itself
/// could tell prose from a tag.
///
/// Lifted out of `every_cross_reference_resolves_to_a_tag`, which had the only
/// copy of this rule, when completion needed it too: one definition of "a
/// tag", consulted by the link check and by `targets`.
pub(crate) fn marked(page: &str, want: Mark) -> Vec<&str> {
    let mut out = Vec::new();
    for line in page.lines() {
        for (at, mark) in marks(line, Dialect::Manual) {
            if mark == want {
                out.push(&line[at.start + 1..at.end - 1]);
            }
        }
    }
    out
}

/// **What a stretch of the manual's text is marked as** — vim's help
/// syntax, read by [`marks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// `*tag*` — a definition: where `:help` lands, what a link names.
    Tag,
    /// `|tag|` — a reference to one.
    Link,
    /// `` `text` `` — what you type, quoted.
    Literal,
    /// **A key in vim's notation** — `CTRL-W`, `<Esc>`, `<C-w>`, and a
    /// placeholder for what you type (`<id>`, `{count}`): vim's
    /// `helpSpecial`. Unquoted, as vim writes keys.
    Key,
    /// `*words*` or `_words_` (`strong`: `**words**`, `__words__`) in the
    /// tutor's text, as markdown writes emphasis — the tutor defines no tags.
    Emphasis { strong: bool },
}

/// **Whose text a line is**: the manual's, whose `*…*` defines a tag, or the
/// tutor's (`crate::tutor`), written in the manual's marks but with
/// markdown's emphasis, since it defines none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    Manual,
    Tutor,
}

/// **The marks of one line**, in order, each its byte range (delimiters
/// included) — the ONE reading of the manual's marks: the painter
/// (`help_view::style_marked`) and the tags and links the manual holds
/// ([`marked`]) both ask it. They were two: the painter painted any pair of
/// `*` as a tag, spaces and all, so `allow * /tmp/x/**` drew `* /tmp/x/*`
/// as one, while the tag list held only `shannon-` words.
///
/// As vim reads help (`syntax/help.vim`): a `*tag*` and a `|link|` hold
/// a name — no spaces (`tag_name`) — and a tag stands between spaces or
/// a line's ends; a `` `literal` `` is read first, so what it quotes is
/// not marked again.
///
/// The tutor reads a paragraph at a time (`tutor::lines`), its lines joined
/// by `'\n'`, since markdown's emphasis crosses them; nothing else does.
pub(crate) fn marks(line: &str, dialect: Dialect) -> Vec<(std::ops::Range<usize>, Mark)> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut unclosed = Unclosed::default();
    while let Some(c) = line[at..].chars().next() {
        match mark_at(line, at, dialect, &mut unclosed) {
            Some((len, mark)) => {
                out.push((at..at + len, mark));
                at += len;
            }
            None => at += c.len_utf8(),
        }
    }
    out
}

/// The mark that opens at byte `at` of `line`, if one does: its length and
/// what it is.
fn mark_at(
    line: &str,
    at: usize,
    dialect: Dialect,
    unclosed: &mut Unclosed,
) -> Option<(usize, Mark)> {
    let rest = &line[at..];
    let before = line[..at].chars().next_back();
    let open = |c: char| before.is_none_or(|b| !b.is_alphanumeric()) && rest.starts_with(c);
    // What lies between `delim` and the next one, if `rest` opens with it.
    let between = |delim: &str| {
        let inner = rest.strip_prefix(delim)?;
        let end = inner.find(delim)?;
        Some((&inner[..end], end + 2 * delim.len()))
    };
    match rest.chars().next()? {
        '`' => between("`")
            .filter(|(inner, _)| !inner.is_empty() && !inner.contains('\n'))
            .map(|(_, len)| (len, Mark::Literal)),
        // A link stands apart from a word on either side: `{dump|edit|save}`
        // and `[N|+N|-N|$]` are a synopsis's alternatives, not links.
        '|' => (between("|").filter(|(inner, len)| {
            let after = rest[*len..].chars().next();
            tag_name(inner) && open('|') && after.is_none_or(|a| !a.is_alphanumeric())
        }))
        .map(|(_, len)| (len, Mark::Link)),
        '*' | '_' if dialect == Dialect::Tutor => emphasis(rest, before, unclosed),
        '*' => {
            let (inner, len) = between("*")?;
            let after = rest[len..].chars().next();
            let apart =
                before.is_none_or(char::is_whitespace) && after.is_none_or(char::is_whitespace);
            (tag_name(inner) && apart).then_some((len, Mark::Tag))
        }
        // `${VAR}` is the shell's, `$name` a status template's.
        '<' | '{' if before != Some('$') => key_name(rest).map(|len| (len, Mark::Key)),
        'C' if open('C') => {
            let key = rest.strip_prefix("CTRL-")?;
            let len = key_name(key).or_else(|| {
                key.chars()
                    .next()
                    .filter(|c| !c.is_whitespace())
                    .map(char::len_utf8)
            })?;
            Some(("CTRL-".len() + len, Mark::Key))
        }
        _ => None,
    }
}

/// **The emphasis `rest` opens, if it does** (its length, and whether it is
/// strong): `*x*`, `_x_`, `**x**`, `__x__`, opening at a word's start and
/// closing at a word's end, so `snake_case` opens nothing and `_a_b_` is one
/// emphasis. An escaped delimiter opens nothing either, the shrug's
/// `¯\_(ツ)_/¯`; a delimiter with a space after it, a table's `*` key.
fn emphasis(rest: &str, before: Option<char>, unclosed: &mut Unclosed) -> Option<(usize, Mark)> {
    if before.is_some_and(|b| b.is_alphanumeric() || b == '\\') {
        return None;
    }
    let strong = rest.starts_with("**") || rest.starts_with("__");
    let delim = &rest[..if strong { 2 } else { 1 }];
    let inner = &rest[delim.len()..];
    if inner.starts_with(char::is_whitespace) {
        return None;
    }
    let none_left = &mut unclosed[2 * usize::from(delim.starts_with('_')) + usize::from(strong)];
    if *none_left {
        return None;
    }
    let Some(end) = closing(inner, delim) else {
        *none_left = true;
        return None;
    };
    Some((end + 2 * delim.len(), Mark::Emphasis { strong }))
}

/// **Whether each delimiter (`*` `**` `_` `__`) has no closer left** in the
/// text [`marks`] is reading. Read a paragraph at a time, a pasted list of
/// `* item` lines scanned to the paragraph's end from every star: 54 s for
/// 100k lines, per keystroke (review stop 17, B F2). No closer after one
/// opener is no closer after any later one, so each scan runs once.
type Unclosed = [bool; 4];

/// Where `delim` closes a word in `inner`: at every position, not only those
/// a scan of whole delimiters lands on, so a later opener sees no closer an
/// earlier one did not ([`Unclosed`]).
fn closing(inner: &str, delim: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = inner[from..].find(delim) {
        let at = from + found;
        let word_ends = inner[..at].chars().next_back();
        let after = inner[at + delim.len()..].chars().next();
        if word_ends.is_some_and(|c| !c.is_whitespace())
            && after.is_none_or(|a| !a.is_alphanumeric())
        {
            return Some(at);
        }
        // A delimiter is ASCII: one on is a character's start.
        from = at + 1;
    }
    None
}

/// **A tag's name** — what a `*tag*` defines and a `|link|` names: letters,
/// digits, `-`, `.`, `_` (the options' tags, `tui.prompt.height`).
/// No space, so prose using a `*` or a `|` is never a tag.
fn tag_name(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// **The length of the key name `rest` opens with** — `<Esc>`, `<C-w>`,
/// `<S-Tab>`, `<id>`, or a placeholder in braces (`{count}`, `{a-z}`),
/// vim's notation; `None` for `<` or `{` that opens anything else (`a < b`,
/// a `{{secret}}` handle, `${VAR}`).
fn key_name(rest: &str) -> Option<usize> {
    let close = match rest.chars().next()? {
        '<' => '>',
        '{' => '}',
        _ => return None,
    };
    // To the first character no name holds, which must close it: to the
    // `close` anywhere ahead, a line of `<` was quadratic (stop 17, B F2).
    let in_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '^');
    let end = 1 + rest[1..].find(|c| !in_name(c))?;
    if !rest[end..].starts_with(close) {
        return None;
    }
    let name = &rest[1..end];
    let named = name.starts_with(|c: char| c.is_ascii_alphanumeric());
    // `{{a}}`'s inner braces are a handle's, not a placeholder.
    let handle = close == '}' && rest[end + 1..].starts_with('}');
    (named && !handle).then_some(end + 1)
}

/// **Is `line` a section heading** — an ALL-CAPS run at the start of the
/// line, before any `*tag*`? Detected by shape rather than by a list of known
/// headings, so a section added later is a heading without anyone
/// remembering to come back. Asked by the pane's styling
/// (`help_view::style_line`) and by each chapter's contents
/// ([`with_contents`]) — one answer to "what is a section".
pub(crate) fn is_heading(line: &str) -> bool {
    let head: String = line.chars().take_while(|c| *c != '*').collect();
    let trimmed = head.trim_end();
    !trimmed.is_empty()
        && !line.starts_with(' ')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_uppercase() || !c.is_alphabetic())
        && trimmed.chars().any(|c| c.is_ascii_uppercase())
}

/// **A chapter, opening with a contents of its own sections** (Steven,
/// 10-07: "each section have a table of contents for its section. This will
/// also help with searching") — each tagged heading below the chapter's
/// own, as `|tag|  TITLE`, between the chapter's heading and its first
/// line of prose. Generated, as the manual's CONTENTS is from [`TOPICS`],
/// so a section added is listed without anyone remembering to. A chapter
/// with no tagged section (the generated commands chapter) is unchanged.
fn with_contents(chapter: &str) -> String {
    // The rule, the chapter's own heading, and the blank line under it.
    const HEAD: usize = 3;
    let sections: Vec<(&str, &str)> = chapter
        .lines()
        .skip(HEAD)
        .filter(|line| is_heading(line))
        .filter_map(|line| {
            let tag = marked(line, Mark::Tag).into_iter().next()?;
            Some((line[..line.find('*')?].trim_end(), tag))
        })
        .collect();
    if sections.is_empty() {
        return chapter.to_string();
    }
    // `|` + tag + `|`, padded to the longest in this chapter.
    let link_w = sections
        .iter()
        .map(|(_, tag)| tag.len() + 2)
        .max()
        .unwrap_or(0);
    let mut lines = chapter.split_inclusive('\n');
    let mut out: String = lines.by_ref().take(HEAD).collect();
    for (title, tag) in &sections {
        out.push_str(&format!("  {:<link_w$}  {title}\n", format!("|{tag}|")));
    }
    out.push('\n');
    out.extend(lines);
    out
}

/// **What `:help` offers to `<Tab>`** — computed from the composed
/// manual, the same text [`anchor_line`] resolves against: each chapter name
/// and alias, every `*tag*` (`:help keymap` finds `*keymap*`), every
/// `:command`, and every key a key table lists (`keys_defined`).
///
/// Completion used to offer only the chapter names while
/// `:help` accepted every tag, so `:help sh<Tab>` did nothing on a page that
/// opened with dozens of tags and `:help keymap` worked typed out (Steven,
/// 2026-09-27). What completes and what is accepted were two lists;
/// `every_help_target_resolves` holds this one to `anchor_line`.
///
/// Built once — the manual is static text plus the command table.
pub fn targets() -> Vec<String> {
    static TARGETS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        let page = compose(&crate::command::commands_chapter());
        let mut out: Vec<String> = topic_words().map(str::to_string).collect();
        out.extend(marked(&page, Mark::Tag).into_iter().map(str::to_string));
        out.extend(crate::command::TABLE.iter().map(|s| format!(":{}", s.name)));
        // Every key a key table defines, as `:help` reads it (`key_line`).
        out.extend(
            keys_defined_in(&page)
                .into_iter()
                .flat_map(|(_, keys)| keys),
        );
        out.sort();
        out.dedup();
        out
    });
    TARGETS.clone()
}

/// Compose the full manual: header, table of contents, then every chapter —
/// with `commands` (the generated reference the caller renders from the
/// command table) injected in its TOC position.
pub fn compose(commands_chapter: &str) -> String {
    let mut out = String::with_capacity(24 * 1024);
    out.push_str(HEADER);
    out.push_str(
        "\n==============================================================================\n",
    );
    out.push_str(
        "CONTENTS                                                             *contents*\n\n",
    );
    // Each entry is `N. |tag|  summary`, with both columns sized from
    // the data. They used to be hand-tuned to whatever the table happened to
    // hold ("|lifecycle|", 18) — so adding a chapter with a longer name
    // overflowed the pad, and a tenth chapter widened the number, leaving the
    // contents ragged in two different ways at once. A constant that has to
    // track a table is a constant that will stop tracking it.
    let num_w = TOPICS.len().to_string().len();
    // `|` + name + `|`.
    let tag_w = TOPICS
        .iter()
        .map(|t| t.name.len() + 2)
        .max()
        .unwrap_or_default();
    for (i, t) in TOPICS.iter().enumerate() {
        out.push_str(&format!(
            "  {:>num_w$}. {:<tag_w$} {}\n",
            i + 1,
            format!("|{}|", t.name),
            t.summary
        ));
    }
    for t in TOPICS {
        out.push('\n');
        out.push_str(&with_contents(chapter_body(t, commands_chapter)));
    }
    out
}

/// **A chapter's text** — written, or generated from data: COMMANDS from
/// the command table (handed in), OPTIONS from the default config
/// (`options_chapter`). The one place a generated chapter is told apart, for
/// the manual, a chapter alone (tests) and the numbering check.
fn chapter_body<'a>(t: &Topic, commands_chapter: &'a str) -> &'a str {
    match t.name {
        "commands" => commands_chapter,
        "options" => crate::options_chapter::options_chapter(),
        _ => t.body,
    }
}

/// Which **source line** of `page` the argument to `:help` names — vim's
/// `:help {tag}`, and the ONE anchor rule.
///
/// A chapter is not the only thing worth asking for. The manual is threaded
/// with `*tags*` — that is what makes `|links|` navigable — and every one of
/// them is a place a reader might want to land, but only the thirteen chapter
/// names resolved: `:help delete` said "no such topic" about something the
/// manual documents in three places. A tag namespace nobody could query was a
/// tag namespace that existed only for the author.
///
/// Matching, in order of specificity, so a shorter name never shadows an exact
/// one:
///
/// 1. a CHAPTER name or alias — `config`, or `security` for the `decisions`
///    chapter — resolved to the `*<name>*` tag the chapter carries
/// 2. a tag — `window-session`, `focus`, or `shannon.txt`
/// 3. a KEY, asked bare (`key_line`) — `zM`, `[[`, `CTRL-^` however a
///    control key is spelled (`<C-^>`, `^^`, `ctrl-^`) — Normal mode's
///    (`KEY_CHAPTERS`), on the key table row that lists it, else at its
///    tag (`*CTRL-C*`); as vim's, `:help w` is the motion, `:help :w` the
///    command
/// 4. a COMMAND, with or without its colon — `delete` and `:delete` both find
///    the `:delete` row of the generated commands chapter, which is where a
///    reader asking that question means to end up. **A colon goes there
///    first**, ahead of every tier here, so each form of a command lands on
///    its one row (`:help :w` and `:help :write`). **One keystroke, asked
///    bare, is a key or nothing**: `:help q` is no `:q`, `:help :q` is
///    (Steven, 11-08)
/// 5. an OPTION, read as `:set` reads it (`option_line`) — `nomouse`,
///    `tui.scrolloff=8`, `nosecrets` — or by its last word alone
///
/// **Every tag was `shannon-` something** until 11-08 (Steven: "unnecessarily
/// verbose" — vim's own help has no prefix; a plugin's does). The old
/// spelling still reads, its prefix dropped, so a citation written in it —
/// the design docs, the reviews — still lands.
///
/// **A chapter is a tag, not a different kind of thing.** It used to be: a
/// chapter composed its own page (`HEADER` + that one body) while a tag
/// composed the manual and scrolled to a line. Two mechanisms, and only one of
/// them anchored — so `:help config` opened on ten lines of the manual's
/// general blurb with the chapter starting below the fold. Steven, finding it:
/// *"lands in the wrong place. Near, but not quite right. Which makes me think
/// anchors are being done wrong."* They were: there were two, and the one that
/// looked like it needed no anchor needed one most.
pub fn anchor_line(page: &str, name: &str) -> Option<usize> {
    let asked = name.trim();
    let name = asked.trim_start_matches(':');
    // The old spelling (`shannon-rules`), read as the tag it was.
    let name = name.strip_prefix("shannon-").unwrap_or(name);
    if name.is_empty() {
        return None;
    }
    // **A key, asked bare** (S21-Q14, Steven: "They will often know the keys
    // but not the name of things"): vim's `:help w` is the motion and
    // `:help :w` the command (measured, Neovim 0.12.6), so a key's row comes
    // before a command's, and a colon asks for the command.
    let key = (!asked.starts_with(':') && name.len() <= KEY_LONGEST)
        .then(|| key_spelling(name))
        .filter(|key| is_key_shaped(key));
    // A chapter name or alias IS a tag — the one it declares. Resolved first so
    // `:help config` cannot be captured by an unrelated `:config` command row.
    let chapter = TOPICS
        .iter()
        .find(|t| t.name == name || t.aliases.contains(&name))
        .map(|t| t.name);
    // **A command's row opens with its synopsis — asked of the function that
    // wrote it**, never parsed back out of the text. A parser of the rendered
    // forms (`:q[!], :quit[!]`) was a second statement of the renderer, and
    // broke the day the renderer learned vim's `:b[uffer]`: `:help :hide`
    // opened nothing. Any form that resolves now finds its row — and only the
    // row: prose that happens to open with `:vs` no longer captures it.
    let synopsis = match crate::command::resolve(name) {
        crate::command::Resolved::Command {
            cmd,
            args: "",
            count: None,
            ..
        } => crate::command::synopsis_of(cmd),
        _ => None,
    };
    let opens_row = |line: &str| {
        synopsis.as_deref().is_some_and(|s| {
            line.trim_start()
                .strip_prefix(s)
                .is_some_and(|after| after.is_empty() || after.starts_with(char::is_whitespace))
        })
    };
    // **A colon asks for the command, before anything sharing its name**:
    // `:help :write` opened the SAVING section by its `*write*` tag and
    // `:help :w` the row, so the long form and the short went to two places
    // (Steven, 11-09: as vim, "the short versions of the commands should go
    // to the same place as the long versions").
    if asked.starts_with(':')
        && let Some(row) = page.lines().position(opens_row)
    {
        return Some(row);
    }
    let one_keystroke = key.as_deref().is_some_and(|key| keystrokes(key) == 1);
    // In the order the list above gives. A key no table lists is defined
    // where its tag is, as vim's are (`*CTRL-C*`) — found however the key is
    // spelled (`<C-c>`, `^c`).
    (chapter.and_then(|chapter| tag_line(page, chapter)))
        .or_else(|| tag_line(page, name))
        .or_else(|| {
            let key = key?;
            key_line(page, &key).or_else(|| tag_line(page, &key))
        })
        .or_else(|| {
            (!one_keystroke)
                .then(|| page.lines().position(opens_row))
                .flatten()
        })
        .or_else(|| option_line(page, name))
}

/// **The `|link|` a follow from char `column` of `line` takes** — its name:
/// the one under the cursor, else the next on the line, as vim's `CTRL-]`
/// takes the keyword under or after the cursor (a list's `- |motions|` is
/// followed from its first column).
pub(crate) fn link_at(line: &str, column: usize, dialect: Dialect) -> Option<&str> {
    let at = (line.char_indices().nth(column)).map_or(line.len(), |(byte, _)| byte);
    (marks(line, dialect).into_iter())
        .find(|(range, mark)| *mark == Mark::Link && range.end > at)
        .map(|(range, _)| &line[range.start + 1..range.end - 1])
}

/// **Does `line` define the tag `name`**, as [`marks`] reads one — the same
/// characters are no tag in `**/*.pem` (`*/*`) or `***both***` (`***`), and
/// `:help /` and `:help *` opened those lines.
fn defines_tag(line: &str, name: &str) -> bool {
    marked(line, Mark::Tag).contains(&name)
}

/// The line of `page` that defines the tag `name` ([`defines_tag`]).
fn tag_line(page: &str, name: &str) -> Option<usize> {
    let tag = format!("*{name}*");
    (page.lines()).position(|line| line.contains(&tag) && defines_tag(line, name))
}

/// **A key as the manual spells it** — vim's `CTRL-X` however a control
/// key is written: `<C-x>`, `ctrl-x`, or `^x` at the start (Neovim's
/// `:help` reads `ctrl-w` and `^w` as `CTRL-W` and `^^` as `CTRL-^`,
/// measured, 0.12.6; `<C-w>` it does not, and this manual writes both).
/// Every other key as written; `^` alone is a key of its own. **A chord is
/// one key**: the space or `_` after a control key is vim's notation for
/// "then" (`CTRL-W q`, and vim's tag for it, `CTRL-W_q`), so both spell
/// `<C-w>q`'s `CTRL-Wq`.
fn key_spelling(key: &str) -> String {
    /// `s` past `prefix`, matched without case.
    fn after<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
        let head = s.get(..prefix.len())?;
        head.eq_ignore_ascii_case(prefix)
            .then(|| &s[prefix.len()..])
    }
    /// Past a chord's `then` — one space or `_` with a key after it (a `_`
    /// alone is the key: vim's `CTRL-W _`).
    fn then(rest: &str) -> &str {
        match rest.strip_prefix([' ', '_']) {
            Some(next) if !next.is_empty() => next,
            _ => rest,
        }
    }
    let ctrl = |name: &str| match name.chars().count() {
        1 => format!("CTRL-{}", name.to_uppercase()),
        _ => format!("CTRL-{name}"),
    };
    let mut out = String::new();
    let mut rest = key;
    if let Some(after) = rest.strip_prefix('^')
        && let Some(c) = after.chars().next()
    {
        out.push_str(&ctrl(&after[..c.len_utf8()]));
        rest = then(&after[c.len_utf8()..]);
    }
    while let Some(c) = rest.chars().next() {
        if let Some(inner) = after(rest, "<c-")
            && let Some(end) = inner.find('>').filter(|&end| end > 0)
        {
            out.push_str(&ctrl(&inner[..end]));
            rest = then(&inner[end + 1..]);
        } else if let Some(inner) = after(rest, "ctrl-")
            && let Some(c) = inner.chars().next()
        {
            out.push_str(&ctrl(&inner[..c.len_utf8()]));
            rest = then(&inner[c.len_utf8()..]);
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// **How many keystrokes `word` is**, as [`key_spelling`] writes keys: a
/// control key (`CTRL-^`), a named key (`<CR>`, `<S-Tab>`) and a
/// placeholder for one (`<c>`, `{char}`, `{a-z}`: vim's `f{char}`, the
/// tutor's `m<a-z>`) each count one.
fn keystrokes(word: &str) -> usize {
    let mut count = 0;
    let mut rest = word;
    while let Some(c) = rest.chars().next() {
        count += 1;
        rest = match (rest.strip_prefix("CTRL-"), key_name(rest)) {
            (Some(ctrl), _) if !ctrl.is_empty() => {
                let key = ctrl.chars().next().map_or(0, char::len_utf8);
                &ctrl[key..]
            }
            (_, Some(len)) => &rest[len..],
            _ => &rest[c.len_utf8()..],
        };
    }
    count
}

/// **Is `word` shaped like a key** — three keystrokes at most
/// ([`keystrokes`]): `zR`, `[[`, `y'a`, `CTRL-Wq`. What a key table lists,
/// and what a `:help` argument must look like to be looked for there;
/// longer, it is a name (`delete`, `when`), never a key. A placeholder
/// alone (`<rule>`) is looked for and found in no key table, so `:help`
/// goes on to the commands and options.
fn is_key_shaped(word: &str) -> bool {
    keystrokes(word) <= 3
}

/// **A key table's row: where its keys are on `line`, and the keys** —
/// indented, its keys, three spaces or more, then what they do
/// (`    zo  zO          open the fold at the cursor`), each as
/// [`key_spelling`] writes it. A cell's entries stand two spaces apart (`gg
/// G` is one entry, `:b#  CTRL-^` two); within one, a control key and the
/// key after it are one chord (`CTRL-W q`, vim's notation), so two control
/// keys meant as alternatives stand two apart. An entry that is a
/// `:command` with its arguments is the commands' — a row of only those
/// defines no key and is still a row — and a closing `(…)` is a note.
/// `None` for prose, which has no such gap, and for a row any of whose
/// words is neither (`ask git push --force`, `thinking = "shut"`): a rule,
/// a setting.
///
/// One row read alone; [`key_cells`] reads it in its table.
fn key_cell(line: &str) -> Option<KeyCell> {
    let (_, cell) = row_cell(line)?;
    let mut keys = Vec::new();
    for entry in (cell.split("  ")).map(str::trim) {
        if entry.starts_with(':') {
            continue;
        }
        keys.extend(chords(entry));
    }
    let at = line.len() - line.trim_start().len();
    (keys.iter().all(|key| is_key_shaped(key))).then_some(KeyCell {
        at: at..at + cell.len(),
        keys,
    })
}

/// A key table's cell: where on its line, and the keys it defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyCell {
    pub(crate) at: std::ops::Range<usize>,
    pub(crate) keys: Vec<String>,
}

/// **A table's row, whatever its cell holds** — indented, a cell, three
/// spaces or more, then what it does: its indent and its cell, a closing
/// `(…)` note left out. A heading over a group (`Motions:`) and a line
/// continuing the row above it have no such gap, and are no row.
fn row_cell(line: &str) -> Option<(usize, &str)> {
    let body = line.trim_start();
    let gap = body.find("   ")?;
    if !line.starts_with("  ") || body[gap..].trim().is_empty() {
        return None;
    }
    let cell = &body[..gap];
    let cell = match cell.find(" (") {
        Some(note) if cell.ends_with(')') => &cell[..note],
        _ => cell,
    };
    Some((line.len() - body.len(), cell))
}

/// **Every key table's cells in `lines`**, one entry a line — the ONE
/// answer to "what is a key here": `:help`'s lookup ([`key_line`],
/// [`targets`]) and the manual's painter (`help_view::style_page`) both ask
/// it, so what a table defines and what it draws as keys cannot differ. The
/// tutor does not: a lesson quotes the keys it means ([`Dialect`]).
///
/// **A table's rows are all keys, or none are.** A word of three letters
/// is shaped like a key (`gg`, `zt`) and so is `rg`: no reading of one row
/// tells them apart, and `:help run` opened the squash table's "a run"
/// row. Read in its table, it cannot: a table is the rows at one indent,
/// up to a line that is no row at that indent or less — prose, a heading,
/// a group's title (`Motions:`). A blank line does not end one (a
/// definition list stands its entries apart), nor does a row's
/// continuation, deeper. A table any of whose rows is not keys — `grep`
/// beside `rg`, `green` beside `red`, "no marks" beside "a run" — is a
/// table of words.
pub(crate) fn key_cells<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<Option<KeyCell>> {
    type Cell = Option<KeyCell>;
    /// A table still open: its indent, its rows, and whether all are keys.
    struct Open {
        indent: usize,
        rows: Vec<usize>,
        keys: bool,
    }
    /// The tables at `from` or deeper end: a table of words keeps none.
    fn close(open: &mut Vec<Open>, from: usize, cells: &mut [Cell]) {
        for table in open.extract_if(.., |table| table.indent >= from) {
            if !table.keys {
                table.rows.iter().for_each(|&n| cells[n] = None);
            }
        }
    }
    let lines: Vec<&str> = lines.into_iter().collect();
    let mut cells: Vec<Cell> = lines.iter().map(|line| key_cell(line)).collect();
    let mut open: Vec<Open> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match row_cell(line) {
            Some((indent, _)) => {
                close(&mut open, indent + 1, &mut cells);
                let keys = cells[n].is_some();
                match open.iter_mut().find(|table| table.indent == indent) {
                    Some(table) => {
                        table.rows.push(n);
                        table.keys &= keys;
                    }
                    None => open.push(Open {
                        indent,
                        rows: vec![n],
                        keys,
                    }),
                }
            }
            None => close(&mut open, line.len() - line.trim_start().len(), &mut cells),
        }
    }
    close(&mut open, 0, &mut cells);
    cells
}

/// A row read alone, as [`key_cells`] would read it in a table of one —
/// for a single line's look (`help_view::style_line`, the tests').
#[cfg(test)]
pub(crate) fn key_cell_alone(line: &str) -> Option<std::ops::Range<usize>> {
    key_cells([line]).pop().flatten().map(|cell| cell.at)
}

/// **One entry's keys**, a control key and the key after it joined as
/// one chord — `CTRL-W q` is `CTRL-Wq`, where `gg G` is two keys — and
/// keys written `h/j/k/l` the alternatives they are: `CTRL-W h/j/k/l` is
/// four chords. A `/` alone is the search key, and `/<CR>` one key.
fn chords(entry: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut waiting: Option<String> = None;
    for word in entry.split_whitespace().map(key_spelling) {
        let pieces: Vec<&str> = word.split('/').collect();
        let alternatives: Vec<String> = match !pieces.contains(&"") {
            true => pieces.into_iter().map(String::from).collect(),
            false => vec![word.clone()],
        };
        let chained = waiting.is_some();
        let joined: Vec<String> = match waiting.take() {
            Some(first) => (alternatives.iter())
                .map(|alt| format!("{first}{alt}"))
                .collect(),
            None => alternatives,
        };
        match joined.as_slice() {
            [only] if !chained && is_control(only) => waiting = Some(only.clone()),
            _ => keys.extend(joined),
        }
    }
    keys.extend(waiting);
    keys
}

/// Does `key` end in a control key that waits for another (`CTRL-W`,
/// `{count}CTRL-W`) — the first half of a chord?
fn is_control(key: &str) -> bool {
    key.rfind("CTRL-")
        .is_some_and(|at| key[at + "CTRL-".len()..].chars().count() == 1)
}

/// **The chapters whose keys are Normal mode's** — all a bare `:help a`
/// answers from. A pane's own keys are the pane's: `a` in the decision pane
/// is "allow", and `:help a` opened that where vim's opens append; `q` there
/// is "deny", and Normal mode has no `q` at all (Steven, 11-08: `:help q`
/// should map to nothing). So vim's are kept: netrw's `d` is `netrw-d`,
/// never `d`. Those tables are found through their chapter (`:help
/// decision`, `:help graph-keys`).
const KEY_CHAPTERS: &[&str] = &["editing", "windows", "buffers"];

/// **Every Normal-mode key `page` defines**, line by line ([`key_cells`],
/// in [`KEY_CHAPTERS`]: a chapter opens with a rule, its tags on the line
/// below).
fn keys_defined_in(page: &str) -> Vec<(usize, Vec<String>)> {
    let mut under_a_rule = false;
    let mut normal = false;
    (page.lines().zip(key_cells(page.lines())).enumerate())
        .filter_map(|(at, (line, cell))| {
            if std::mem::replace(&mut under_a_rule, line.starts_with("====")) {
                normal = (KEY_CHAPTERS.iter()).any(|name| line.contains(&format!("*{name}*")));
            }
            cell.filter(|_| normal).map(|cell| (at, cell.keys))
        })
        .collect()
}

/// **The keys one line defines**, read as a table of one — the tests'.
#[cfg(test)]
fn keys_defined(line: &str) -> Vec<String> {
    (key_cells([line]).pop().flatten()).map_or_else(Vec::new, |cell| cell.keys)
}

/// **The longest thing a `:help` argument can be and still be asked as a
/// key**, in bytes — far past any key the manual lists. Past it, reading one
/// as keys was quadratic on a pasted `<c-<c-…` (stop 17, B F7).
const KEY_LONGEST: usize = 32;

/// **The key table row that defines `key`** — the first listing it as
/// written (a `{count}` ahead of it aside: `{count}CTRL-W +` is `CTRL-W +`),
/// else the first listing it with what it waits for (`m` is `m{a-z}`, `/` is
/// `/{pattern}<CR>`), else a chord it starts (`CTRL-W`, the chords' table),
/// else, past one keystroke, the first listing it in another case (Neovim's
/// `:help GG` opens `gg`, measured). One keystroke's case is the key: `U` is
/// not `u`.
fn key_line(page: &str, key: &str) -> Option<usize> {
    let rows = keys_defined_in(page);
    let first = |same: &dyn Fn(&str) -> bool| {
        (rows.iter().find(|(_, keys)| keys.iter().any(|k| same(k)))).map(|(at, _)| *at)
    };
    fn uncounted(key: &str) -> &str {
        key.strip_prefix("{count}").unwrap_or(key)
    }
    first(&|k| uncounted(k) == key)
        .or_else(|| first(&|k| k.split_once('{').is_some_and(|(head, _)| head == key)))
        .or_else(|| {
            (is_control(key))
                .then(|| first(&|k| uncounted(k).starts_with(key)))
                .flatten()
        })
        .or_else(|| {
            (keystrokes(key) > 1)
                .then(|| first(&|k| k.eq_ignore_ascii_case(key)))
                .flatten()
        })
}

/// **Where `page` documents the setting `name` names** — read as `:set`
/// reads a word (`set::option_of`: `nox`, `invx`, `x!`, `x&`, `x?`,
/// `x=v`, short names), so `:help` takes every form `:set` does (Steven,
/// 10-19: `:help nosecrets` found nothing, as no setting had a tag). Its own
/// tag (`*tui.scrolloff*`), else its nearest section's — the prose
/// chapter's (`*secrets*`) before the OPTIONS chapter's
/// (`*config.secrets*`), so `[keymap.normal]`'s keys open at KEY
/// REMAPPING. A word naming no key is the option it is the last word of,
/// where it is the last word of only one ([`options_ending`]).
fn option_line(page: &str, name: &str) -> Option<usize> {
    const OPTION_DEPTH: usize = 6;
    let key = shannon_core::config::set::option_of(name)?;
    let ending = || match options_ending(&key).as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    };
    let section = crate::options_chapter::SECTION_TAG;
    let at = |key: &str| {
        std::iter::successors(Some(key), |k| k.rsplit_once('.').map(|(up, _)| up))
            // Two page scans an ancestor, and a typed word can hold thousands
            // of dots (3 s for 4,000, review stop 6, B 5); no setting is
            // deeper than three, so six is room and a bound.
            .take(OPTION_DEPTH)
            .flat_map(|k| [k.to_string(), format!("{section}{k}")])
            .find_map(|tag| tag_line(page, &tag))
    };
    at(&key).or_else(|| at(&ending()?))
}

/// **The options whose last word is `word`** — `scrolloff` is
/// `tui.scrolloff` — or with `no`/`inv` before it (`nomouse`). Several is
/// no answer for `:help`, and what its refusal offers instead
/// ([`no_such_topic`]).
pub fn options_ending(word: &str) -> Vec<String> {
    let words = [
        Some(word),
        word.strip_prefix("no"),
        word.strip_prefix("inv"),
    ];
    shannon_core::config::set::option_names()
        .into_iter()
        .filter(|key| {
            let last = key.rsplit('.').next().unwrap_or(key);
            words.iter().flatten().any(|w| *w == last)
        })
        .collect()
}

/// The refusal for a `:help` argument that names nothing — the valid chapters,
/// plus the fact that tags work at all, since a namespace you cannot discover
/// is one you will not use. The last word of several options names them
/// instead: which one is the reader's to say.
pub fn no_such_topic(name: &str) -> String {
    let word = name.trim().trim_start_matches(':');
    let several = shannon_core::config::set::option_of(word)
        .map(|key| options_ending(&key))
        .unwrap_or_default();
    if several.len() > 1 {
        return format!("no help for {name:?} — which: {}?", several.join(", "));
    }
    // One keystroke asked bare is a key or nothing; the command is a colon away.
    if matches!(
        crate::command::resolve(word),
        crate::command::Resolved::Command { args: "", .. }
    ) {
        return format!("no help for {name:?} — for the command, `:help :{word}`");
    }
    format!(
        "no help for {name:?} — chapters: {}; or any *tag*, `:command` or option in the manual",
        topic_names().join(", ")
    )
}

/// One row of `*graph-keys*`: its keys (`mf`, `<Space>`), its
/// command if it has one (`:pick`), and what it does.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct GraphKeyRow {
    pub keys: Vec<&'static str>,
    pub command: Option<&'static str>,
    pub does: &'static str,
}

/// **The rows of the graph's key page, read from the page itself** — so the
/// tests that hold it to `App::graph_verb` (both ways) and to the manual's
/// `press` sentences read the one table a person reads, never a copy.
/// A row is in three columns: keys from 6, a command from 18, what it does
/// from 32; a group's title is indented 4 and is not a row.
#[cfg(test)]
pub(crate) fn graph_key_rows() -> Vec<GraphKeyRow> {
    let section = GRAPH
        .lines()
        .skip_while(|l| !l.contains("*graph-keys*"))
        .skip(1)
        .take_while(|l| !is_heading(l));
    section
        .filter(|l| l.starts_with("      ") && l.len() > 32)
        .map(|l| {
            let column = |r: std::ops::Range<usize>| l.get(r).unwrap_or("").trim();
            let command = column(18..32);
            GraphKeyRow {
                keys: column(6..18).split_whitespace().collect(),
                command: (!command.is_empty()).then_some(command),
                does: l.get(32..).unwrap_or("").trim(),
            }
        })
        .collect()
}

/// Compose one chapter on its own, for the tests that check a chapter in
/// isolation (the width contract, alias resolution).
///
/// **Not a `:help` path.** `:help <chapter>` opens the whole manual anchored at
/// the chapter's tag, like every other argument — see [`anchor_line`]. Keeping
/// a second composition alive as the *display* path is what gave chapters their
/// own un-anchored behaviour.
#[cfg(test)]
pub fn compose_topic(name: &str, commands_chapter: &str) -> Option<String> {
    let t = TOPICS
        .iter()
        .find(|t| t.name == name || t.aliases.contains(&name))?;
    Some(format!(
        "{HEADER}\n{}",
        with_contents(chapter_body(t, commands_chapter))
    ))
}

const HEADER: &str = r#"*help*   Shannon Code help                                         *shannon.txt*

Shannon Code is a terminal coding agent built as an ENVIRONMENT: everything
you see has a mechanism under it that you can inspect, rewire, or replace.
This manual follows vim's help conventions: a `*tag*` marks a section and a
`|link|` names one (CTRL-] follows it, CTRL-T comes back). `:help <thing>`
opens this page AT that thing — a chapter name, any tag, a key, a `:command`
with or without its colon, or an option as `:set` takes it; <Tab> completes
all of them. `:help config`, `:help window-session`, `:help zM`, `:help :delete`
and `:help nosecrets.password_protect` all land on the line that defines
them. A key is read as vim reads one: `CTRL-^`, `<C-^>`, `^^` and `ctrl-^`
are one key, and `:help w` is the motion where `:help :w` is the command.
"#;

const INTRO: &str = r#"==============================================================================
1. INTRO — the ideas everything rests on                                *intro*

Shannon keeps your conversation as a GRAPH, not a scrollback. Four ideas
explain nearly every behavior in this manual; the rest is keys.

THE HIERARCHY                                                        *concepts*

  session (episode)  >  turn  >  message  >  span
    A SESSION is one line of conversation — what you see in the transcript
    pane. A TURN is one exchange: your message and the reply it produced.
    A MESSAGE is one side of that exchange. A SPAN is any character range
    within a message. Operations act at exactly the level you point them
    at (see |wysiwyg|).

CONTENT ADDRESSING                                                *content-id*

    A turn's identity is a hash of its content (messages + the model config
    that produced it). Two identical turns ARE the same turn, stored once,
    shared by reference. Sessions are therefore cheap to branch: a fork
    stores a pointer to the shared history plus only what diverged.
    Position ("the 3rd turn") and identity (the hash) are different
    things — a repeated turn is one id at two positions.

HISTORY IS THE TRUTH                                            *history-truth*

    What happened is never silently destroyed. Compositions (merge, squash,
    edit, |return|) build NEW sessions and keep the originals
    reachable in |graph|. Work you paid for survives failure: a turn
    interrupted mid-reply is sealed and marked, not discarded
    (|lifecycle|). Only `:delete` deletes.

EVERYTHING IS CONFIG                                             *environment*

    Defaults are chosen carefully, then made yours to override: keys
    (|keymap|), glyphs, guardrails, instruction templates. The
    config file is a sparse diff over the defaults — it lists only what
    you change (|config|).

GETTING STARTED                                             *getting-started*

    The screen starts as two panes: the TRANSCRIPT on top (the active
    session) and the PROMPT below — a modal editor that starts in Normal mode
    (|editing|). It is a tree, not a fixed pair: split it, close panes,
    and move between them with CTRL-W (|windows|). To send your first
    message:

        i           enter Insert mode
        {message}   type it (<Enter> here just adds a newline)
        <Esc>       back to Normal mode
        <Enter>     send it

    (Or CTRL-<Enter> sends from any mode, in a terminal that reports it —
    |submit|.) The reply streams in above, and your session is saved as
    you go (|persist|).

    Every session starts on Claude Sonnet 5.5 unless your config says
    otherwise, and `:model` changes the one you are in (|model|).

    Two doors open everything else: `:` for commands (|commands|) and
    `:graph` for history itself (|graph|). Where a command wants an
    id (`:open`, `:child --stop`), it comes from `:graph` — select a row and
    read its short id in the inspector, or just press <Tab> to complete one.
"#;

const EDITING: &str = r#"==============================================================================
2. EDITING — the modal prompt                                         *editing*

The prompt pane is a real modal editor. The defaults are canonical vim — a
faithful subset, not a flavor. Most of the keys below are remappable through
`[keymap.*]` in the config file (|keymap|); `:keymap` lists exactly
which, beside the action name each is bound to. The exceptions are the keys
the terminal or the app owns rather than the keymap — CTRL-C, CTRL-L,
PageUp/PageDown, CTRL-X CTRL-E, and Insert-mode line editing. The subset
grows; what is listed here is what exists.

MODES                                                                   *modes*

    Normal (default; `[tui.prompt] start_mode` can start in insert), Insert
    (`i` `a` `A` `I` `o` `O`), Replace (`R`), Visual (`v` characterwise,
    `V` linewise, CTRL-V blockwise). <Esc> returns to Normal. The grammar
    composes as [count][operator][count][motion|text-object].

    i  a                      Insert before / after the cursor
    I  A                      Insert at the line's first non-blank / its end
    o  O                      a new line below / above, and Insert
    v  V  CTRL-V              Visual by character / by line / blockwise
    <Esc>  CTRL-[             back to Normal
    <CR>                      in Normal, send the prompt; in Insert, a newline
    CTRL-X CTRL-E             the prompt in $EDITOR (|editor|)

    CTRL-[ is <Esc>. Not a Shannon feature — it is what the terminal sends,
    and it always has — but it is the least-known key in vim, and it is
    right under your left hand. Worth trying once: the same press works in
    vim, and in any shell running `set -o vi`.

MOTIONS                                                               *motions*

    h j k l (and arrows)      left / down / up / right
    w b e                     word forward / back / to word end
    W B E                     the same over WORDs
    0  $   (Home / End)       line start / line end
    ^                         the line's first non-blank
    f{c}  F{c}                to the next / previous {c} on the line
    t{c}  T{c}                to just before / just after it
    ;  ,                      the last f F t T again / the other way
    ge  gE                    back to a word's / a WORD's end
    }  {                      next / previous paragraph (an empty line)
    %                         the bracket matching the first at or after
                              the cursor; `50%` the line half way down
    gg G                      buffer start / buffer end
    ]]  [[                    next / previous section
    ][  []                    next / previous section's end
    '{a-z}                    to a mark (|mark-motions|)
    Counts multiply: `3w`, `d2j`, `2d3fx` (the sixth x).

    The arrows move as `h` `j` `k` `l` do. With `tui.arrows = false` they do
    nothing, and an arrow says why. The `:` line keeps them: there they
    recall history and move the cursor, and no other key does.

    The last `f` `F` `t` `T` is Shannon's, not a pane's, as vim's is the
    editor's: `fx` in the prompt, then `;` in a transcript finds the next
    `x` there. `;` after `t` steps past the char it stands before, as vim's
    does (`'cpoptions'` without `;`). `:graph` refuses the four, and `;`
    `,`: its rows have no columns to find in.

    `j` and `k` keep the column, as vim's do: through a shorter line the
    cursor goes to its end and comes back to the column on a longer one —
    a screen column, so a wide character holds two. After `$` they keep to
    every line's end. Any other move or edit sets the column where the
    cursor is; a move that fails leaves it. `gg` and `G` keep it too (vim's
    `nostartofline`; under its default they go to the first non-blank).

    A word is vim's: a run of letters, digits and `_`, or a run of other
    non-blanks — so `w` stops at each `/` of a path. A WORD is any run of
    non-blanks — `W` crosses the path whole. CTRL-W in Insert mode and on
    the command line cuts back one word.

    The word motions cross lines as vim's: an empty line is a word, a line
    of blanks is not. After an operator vim's special cases hold too: `cw`
    in a word changes to its end (from its last char, that char alone);
    `dw` on a line's last word stops at the line's end; `db` at a line's
    start takes the word before it; and a `d` that would leave only blanks
    behind takes the lines whole.

    A section is a TURN in a transcript, as each shell prompt is one in
    neovim's terminal: `]]` goes to the rule above the next message you
    sent, `3]]` three on, `y]]` copies one turn and `V]]` selects it; `][`
    goes to the end of a turn, its last line of text. In the prompt and the
    pages a section is vim's — it begins at a line that begins with `{` and
    ends at one that begins with `}`. With none left it goes to the last
    (first) line; a count the sections run out before does nothing, as
    vim's beeps.

MARKS IN A TRANSCRIPT                                   *mark-motions* *marks*

    m{a-z}                    set mark {a-z} where the cursor is (in Visual
                              too, at the selection's moving end; it stays)
    '{a-z}                    to it: its line, and its column
    y'a  v'a                  copy the lines to it / grow a selection to it
    `{a-z}                    to the AGENT's mark {a-z} (below)

    A mark is the SESSION's, as vim's lowercase marks are the buffer's:
    every window on the session has it — mark a line while a reply
    streams, split, and come back to it from the split. It is a place in
    the conversation (a turn, a block of it, a place in that block), not a
    line number, so what streams in below it, a tool's record landing
    ahead of it, or a resize does not move it.

    `'` goes to the exact place, column too. In vim `'` goes to the line
    and the backtick to the place; here the backtick is kept for the marks
    the agent makes. Under an operator `'` is linewise, as vim's is.

    Setting one says its turn ("mark `a` — turn 3"). A mark whose text is
    gone — a turn undone before anything ran — is not set, and going to
    a mark that is not set says so, as vim's E20. Marks are a
    transcript's for now: elsewhere `m`, `'` and ` say so — but in
    |graph|, whose own `m` marks its rows. They are saved
    with the session, yours and the agent's apart, so a session reopened
    has them where they were; `''` comes next. (A marked PANE, for
    `:Send`, is |window-marks|.)

    A line a reopened session would not draw keeps a mark for this run
    only, and setting one there says "not saved": a call's `⚙ running:`
    line (what reopens is the record that closed the call), or a turn not
    in the conversation, drawn in the danger colour (|interrupt|).
    A mark whose turn is dropped after you set it says so when it leaves
    what is saved.

    A session made from another starts with its marks, as its own from
    there: a `:clone`, `:fork` or `:vfork` all of them, a checkout those
    on the turns it shares, an edit's branch those on the turns it left as
    they were, `:continue` every one, `:compact` those on the turns it
    keeps. A mark whose turn changed or was folded into a summary is not
    carried, and that is said. Marks set or gone in one after are that
    one's alone.

    :marks                    every mark, as a live page

    A table per session with a mark: one row a mark — the mark, its
    turn, line and column, the word it stands on, and the line's context,
    faint and cut to the window (the agent's begins with its note). Yours,
    a soft rule, then the agent's, each in the order they stand in the
    conversation; every mark in one colour, the one `⚙ marked` records
    have.
    `<CR>` on one shows its session in this pane, as `:b N` does, the
    cursor on the mark. A mark saved that a session reopened does not draw
    again is listed under it, not dropped; one saved in a shape this build
    does not read (another build's, a hand's edit) is counted there, and
    kept as it was when the session is saved again.

    THE AGENT'S MARKS are its own letters, apart from yours: `a` and
    `'a` are not the same mark. On the API the model has a `mark` tool
    for a place you will want again — a question it leaves open for you,
    a result, where a plan was agreed — with a note saying what it is.
    It marks where the call falls, or an earlier line by quoting a
    phrase from it. Shannon names each one, the next free letter (the
    oldest's past 26), and the model is told which, to name it in its
    reply ("mark `b`"). The call is in the transcript as a record —
    `⚙ marked: mark "the plan"` — and ` goes to that record, or to the
    phrase it quoted. It runs nothing, so nothing asks first. A quote is
    matched within a line of the text as the model was sent it: a secret
    you see drawn — over several lines too, a key's — is only its handle to
    the model, and no text it can find (|secrets|). Claude Code
    sessions have no such tool: theirs are Claude Code's own.

SEARCHING                                                              *search*

    /{pattern}<CR>            to the next match
    ?{pattern}<CR>            to the one before
    n  N                      the last search again, its way / the other
    *  #                      the word under the cursor, whole: next / before
    /<CR>                     the last pattern again, forward

    In every pane with text — the prompt, the transcript, the manual and
    every other page — from the cursor, round the end and on from the top,
    which it says ("search hit BOTTOM, continuing at TOP"). Counts work:
    `3n`. In Visual a search moves the selection's end. One last pattern
    for every pane, as vim's: `/` in the manual, then `n` in the transcript.

    The `/` line edits as the `:` line does; <Up> recalls earlier patterns,
    kept apart from commands. A {pattern} is a Rust regex — the dialect of
    the gate's `re:` rules (|rules-re|), not vim's — so `a|b` and
    `foo.*bar` mean what they say. vim's `\<` `\>` (a word's edges) and
    `\c` `\C` (ignore / match case) work too. Otherwise case matters, as in
    vim, unless you set vim's two options:

    [tui] ignorecase = true    case ignored
    [tui] smartcase = true     …but matched for a pattern you type with a
                               capital in it: `/mara` finds `Mara`, `/Mara`
                               only `Mara`. Not for `*` and `#`, nor `n`
                               after them: their word was not typed

    Both are off by default, as in vim; `:set tui.ignorecase` turns one on
    for the session. A `\c` or `\C` in a pattern decides over both. A pattern
    found nowhere, or not a pattern, is said and moves nothing.

    In `:graph` the rows are searched as they read — a name, an
    id, a prompt's opening words — and the cursor moves to the row; a
    folded row is searched too, and the fold opens, as vim's search opens
    one ('foldopen'): a session forked from a turn hangs in that turn's
    fold, and `/` and its id finds it. A Claude Code conversation's turns
    are searched once they are open (`l`), not before.

OPERATORS AND EDITS                                                 *operators*

    d c y                     delete / change / yank (+ motion or object)
    dd cc yy                  linewise forms
    x X                       delete char under / before the cursor
    D C Y                     to end-of-line forms (Y yanks the line)
    p P                       paste after / before
    u  CTRL-R                 undo / redo
    ~                         toggle case, [count] chars, stepping past them
    r{char}                   replace [count] chars with {char}, all or
                              nothing; `r<CR>` breaks the line
    R                         Replace mode: typing overwrites, <BS> puts back
                              what it covered, <Esc> leaves; `2R` repeats
    Operators double in Visual mode over the selection (`d x c s y`), and
    there `~ u U` toggle / lower / upper the case and `r{char}` replaces
    every selected char (not `r<CR>`, which in vim fills the selection with
    literal carriage returns; here it cancels).

TEXT OBJECTS                                                     *text-objects*

    i / a  +  w W  " ' `  ( ) b  { } B  [ ]  < >  p
    Inner or around: `ciw`, `di"`, `ya(`. Same objects in Visual (`vi"`).
    `aw` takes the white space after the word, or before it when none
    follows; from white space, the word after — on the next line if need
    be. `a"` takes the white space after the closing quote, or else before
    the opening one.
    `p` is a paragraph — whole lines, up to a blank one (empty or white
    space only); `ap` takes the blank lines after it, as vim's: `vap`,
    `yap`, `dip`. It works in every pane with text, the decision's report
    included — `yap` or `vapy` copies a paragraph of it and answers
    nothing; there `d` is deny, so `dip` denies (|decision|).

VISUAL MODE                                                            *visual*

    v V CTRL-V begin (or switch) a selection; `o` swaps cursor and anchor;
    `I`/`A` block-insert at the block's left/right edge (blockwise only).

THE REGISTER AND THE CLIPBOARD          *register* *clipboard* *copy* *yank*

    To copy out of Shannon, yank: `yy`, `yiw`, `V2jy`, or a drag with the
    mouse and then `y`. The text goes to your SYSTEM clipboard, for `pbpaste`
    or a paste anywhere else. A `y` of two lines or fewer says nothing, as
    vim's does; the copy was still sent, and the terminal has the last word
    (below).

    One unnamed register, shared between the prompt and the chat pane.
    Yanks additionally mirror to the SYSTEM clipboard (OSC 52 — works over
    ssh, and inside tmux with `set -g allow-passthrough on`; the terminal
    must allow it); deletes do not touch the system clipboard. With
    `tui.copy_on_yank = false` a yank stays in the register, as vim's does
    without `clipboard=unnamedplus`.

    A copy that touches a marked secret — any part of it, so half a key
    dragged over is the key — goes as `tui.copy_secrets` says:

      value    the value, as shown (the default). The clipboard is yours,
               and the model never sees it: a command that prints the
               clipboard (`pbpaste`) asks first, and a held value in any
               command's output comes back as its handle. A transformed
               copy (`pbpaste | base64`) would not — the ask is the guard.
      handle   the secret's handle in place of every part of it taken.
      skip     nothing; the notice says which secret stopped it.

    The register `p` pastes from always keeps what the screen shows. A
    secret pasted from it into your message leaves as its handle; so does
    PART of one: a copy that takes part of a held value marks that part
    as a secret of its own (`<name>-part`) and says so. A part shorter than
    four characters is not marked — it would hide every one in what you
    write — and that is said instead: pasted and sent, it goes as written.
    A `y` says nothing else unless the clipboard got something other than
    what it took — or it took more than two lines (vim's 'report'): `yy`
    on a shut fold takes every line under it, drawn or not, and says "7
    lines yanked", and a value among them too. A drag always says what it
    sent.

INSERT MODE                                                            *insert*

    Plain typing; <Enter> is a newline. Pasting is bracketed: a multi-line
    paste arrives verbatim as text, never executed as keys. Control
    characters in pastes are stripped.

SUBMITTING                                                             *submit*

    <Enter> in Normal mode, or CTRL-<Enter> anywhere, submits the prompt as
    a turn. Submitting while THAT session is busy — its reply streaming, a
    decision of its waiting, its summary being drafted — QUEUES the prompt;
    it is sent when that ends. A prompt for another session starts at once
    (|sessions-apart|). An all-whitespace prompt is ignored. A
    prompt starting with the word `/child` is a command, not a message —
    see |child|.

    CTRL-<Enter> needs a terminal that reports it apart from <Enter>:
    Shannon asks for the kitty keyboard protocol. Where the terminal does
    not speak it — inside tmux, for one — CTRL-<Enter> arrives as a plain
    <Enter>, a newline in Insert mode. ALT-<Enter> submits from Insert and
    Replace mode, and gets through tmux; on macOS the terminal must send
    Option as Alt (ghostty's `macos-option-as-alt`, iTerm2's "Esc+").
    <Esc> then <Enter> works everywhere.

THE EXTERNAL EDITOR                                                   *editor*

    CTRL-X CTRL-E opens the prompt in $EDITOR (vim's readline idiom); save
    and quit to bring the result back — nothing auto-submits. Everywhere
    Shannon opens $EDITOR for you to APPROVE something (squash summaries,
    child briefings), the contract is save-to-apply: `:wq` applies,
    closing without saving keeps a draft or cancels — see
    |save-to-apply|.

THE MOUSE                                                               *mouse*

    A click focuses the pane under it and puts the cursor on the character
    it points at, the wheel scrolls the pane under the pointer, and a drag
    selects text in that pane — the prompt, the transcript, the manual or
    any other page — for `y` to copy (|chat|). A drag stays in its
    pane: it never takes the pane beside it.

    SHIFT-DRAG is your terminal's own selection, not Shannon's: it reaches
    anything on screen — a pane's title and the session id in it, the
    status line, the `:memories` header. `y` cannot see it: let go of Shift
    and use your terminal's own copy key (Cmd-C on a Mac; Cmd-Shift-C is
    another key). Some terminals take Option or Alt instead of Shift. With
    tui.mouse = false Shannon takes no mouse at all, and a plain drag is the
    terminal's.

THE CHAT PANE                                                            *chat*

    CTRL-W w (or a mouse click) moves focus to the transcript, which becomes
    a read-only navigable buffer: motions move, `v` selects, `y` yanks
    (into the shared register + system clipboard). The mouse wheel scrolls
    the pane under the POINTER, focused or not: here, as in vim, a notch
    moves the WINDOW three rows, and the cursor stays where it was until
    the window would leave it ('scrolloff' from the edge). PageUp/PageDown
    and CTRL-U/CTRL-D scroll the same way, and so does a reply streaming
    into a window that follows the end — except that a cursor on the last
    line rides it, as a terminal's does. A transcript is entered at its
    end, where its window is. Drag selects, and `y` copies it; with
    `tui.copy_on_select = true` releasing the drag copies it too (the
    selection stays up and `p`'s register is untouched; a secret in it goes
    as |register| says). Shift-drag is the terminal's own
    selection, reaching anything on screen (|mouse|). CTRL-L
    forces a repaint; CTRL-C interrupts and never quits
    (|interrupt|).

    A reply's markdown is styled where it is written: a `#` heading, a
    fenced block in its language's colours, an inline `code` span,
    `**bold**`, `*italic*` or `_italic_`, and `***both***`. The marks stay,
    dim: what is drawn is what the model wrote, so the cursor, a search and
    a yank see the same characters (vim's 'conceallevel' 0). A `*` with a
    blank beside it (`* item`, `2 * 3`) and an `_` inside a word are just
    text.

    Every tool call is on the record, where it happened in the reply: a
    `⚙` line in cyan saying what happened to it; a command beneath it as
    a fenced `bash` block — laid out as the gate showed it, as shell you
    can copy (each continued line ends ` \`), coloured as the gate reads it
    (see |analyze|); one of Claude Code's other tools by its name
    and file on the `⚙` line (`Edit src/a.rs`), a file tool's change as a
    diff (red `-`, green `+`, the text in its language's colours); and what
    came of it as a block of its own, fenced as `output`, in the
    record's cyan, dim — or, for a `Read`, in its file's colours. So the
    three read apart wherever you scroll: the reply plain, a thought grey
    and italic, a tool's output cyan. While a call runs it is one line
    (`⚙ running: cargo test`); the record that closes it takes that line's
    place, the whole one, shut until you open it (`zo`, |folds|) —
    a command's saying what ran and how it ended, `+ ⚙ ran: cargo test ·
    9 lines`, or `failed` for a non-zero exit, a timeout or a command that
    could not start. A reopened or imported session shows its calls the
    same way, but a failed command of Shannon's reads `refused or failed`:
    what the session keeps says only that the call failed.

    What the model THOUGHT is on the record too, where it thought it: a
    block of its own above the words or the call it led to, in dim grey
    italic (`ui.text.quiet.thinking`). It is what the API sends of it —
    under `[turn] thinking = "summarized"`, the default, a summary of the
    reasoning and the notes between calls; under "omitted" the blocks come
    empty, as Claude Code's do, and draw nothing. One the API sent
    encrypted says so.
    A thought cut off by a stop or a failure says so — shut, on its row,
    `· cut off`: the turn keeps none of it, so a reopened session does not
    show it, and a mark there is not saved. The agent's `mark` never lands
    a quote in thinking — what is drawn is a summary, not its words. A
    thought starts shut, its first line showing (|folds|).

    Two transcript panes are INDEPENDENT: they show the same conversation
    but each keeps its own cursor, selection and folds, so you can hold one
    place while reading another. That is vim's rule for two windows on one
    file.

FOLDS                                                                 *folds*

    A transcript folds by its blocks: what the model thought, a tool's
    record, a reply, your message. A shut block is one row — `+`, its first
    line (a command's record: the command, and how it ended), how many
    lines it holds — and the rest is still there: a yank or a search takes
    it whole. Thinking and tool records have a fold, and start shut. A
    reply and your message have none until you make one: `zc` on one shuts
    it, and from then `zR` and `zM` open and shut it with the rest, as
    vim's do a fold made by hand. The folds are the window's, as vim's
    are: two windows on one session fold apart.

    A row says what it hides that you would want to see: `· cut off` for
    a thought the turn ended before, and `· leaked value`, in the colour
    a value the model wrote itself is drawn in, when one is under it
    (|secrets|).

        zo  zO          open the fold at the cursor
        zc  zC          shut it — on a reply or your message, make it
        za  zA          open it if shut, shut it if open
        zR              open every fold, and those still to come
        zM              shut every fold, and those still to come

    `j` and `k` step over a shut fold as one line, onto its first; `12G`
    goes to line 12 inside one; an operator takes it whole (`yy` on one
    yanks every line of it). A search, a mark, `]]` or a motion along the
    line (`l` `w` `$`) that lands in one opens it; `j` `k` `G` do not (vim's
    'foldopen'). A block that fits one row has no fold, and `zc` there says
    so — one row at the window's width, as vim's `foldminlines` counts
    screen lines: a one-line thought that wraps in a narrow split folds,
    `· 1 line`, and is open again when the window is wide. `zo` on a reply
    with no fold says so too, and makes none.

    How each kind starts is `[tui.fold]`. A `:set` of a kind's rule shows
    at once, and puts that kind's folds back to it — what `zo` `zc` `zR`
    `zM` set on its blocks is undone, in every window, as vim's
    `:set foldlevel` does, even set to the value it had; other kinds keep
    theirs:

        [tui.fold]
        thinking = "shut"           "shut", "open", "manual", or
        tool = "shut"               { shut_over = N }: shut when it
        reply = "manual"            holds more than N lines; "manual",
        message = "manual"          no fold until `zc` makes one

    `reply = "open"` gives every reply a fold that starts open, so `zM`
    shuts the replies too.

    `shut_over` counts a block's own lines, the count its row shows, not
    the rows they wrap to: `{ shut_over = 2 }` starts a thought of two
    lines open and one of three shut.

    What you open and shut is your window's: a split starts with them, and
    a window that shows another session and comes back finds them as it
    left them, as vim's do. They are not kept when the session is
    reopened: each block starts as its kind does.

    Two panes can also hold DIFFERENT sessions. Split (`:vs`), start a
    session in the new pane (`:new`), and the prompt follows the pane you
    are standing in — its title reads `prompt → <id>` and the target pane
    wears the accent, so where your next message is going is on screen.

    From there one question can go to several sessions at once: mark panes
    with CTRL-W <Space> and press <Enter>, or name them directly with
    `:Send 1 3` (or every one: `:Send all`). Set the two to different
    models (|model|) and you are comparing them side by side; set
    them to the same one and you are measuring how much a reply varies on
    its own. The full rules — what a mark does, what happens to a session
    already mid-turn — are in |window-marks|.
"#;

const WINDOWS: &str = r#"==============================================================================
4. WINDOWS — panes, splits, focus                              *windows* *panes*

The screen is a tree of PANES. There is no fixed number of them and no fixed
order: every pane is a viewport onto some content, and the same commands move,
size and close all of them.

    output      a session's transcript (|window-session|)
    prompt      what you type into
    graph       the session tree (|graph|)
    inspector   details of whatever the graph pane has selected
    analyze     a command-effect report (`:analyze`)
    help        this manual (`:help`)
    messages    every notice this session (|messages|)
    decision    a tool call awaiting your answer (|decision|)

A window shows one session at a time; the ones it is not showing, and tab
pages, are in |buffers|.

FOCUS                                                     *window-focus* *focus*

Exactly one pane has focus. Its title is drawn in reverse video and its border
in `ui.border.focused`, it shows the cursor, and it is where your keys go —
the status line names it. Nothing is modal: opening a decision or a viewer
moves focus there, but never traps it.

    CTRL-W w           next pane          CTRL-W W    previous pane
    {count}CTRL-W w    go straight to pane {count} (the number in its corner)
    CTRL-W p           back to the pane you came from (press again to return)
    CTRL-W h/j/k/l     the pane left/below/above/right
    <click>            the pane under the pointer

`W` and `p` answer different questions, and only look alike with two panes open:
`W` steps BACKWARD THROUGH THE RING, `p` returns to WHERE YOU JUST WERE. With
three panes they part company. `p` says so when there is nowhere to go back to.

Every pane shows its NUMBER in the top-right corner, and that is the number
`{count}CTRL-W w` takes — `3<C-w>w` goes to pane 3. Numbering runs in the same
order `<C-w>w` cycles (top-left first), so it renumbers as you split and close;
it describes where things are now, not when you opened them.

`h/j/k/l` move to the pane that is actually THERE: it must start past this
pane's edge and share rows (or columns) with it. A pane sitting diagonally is
not a neighbour and is never reached by one press. Where two panes qualify —
you are beside a column split at a different height — the topmost wins.

Every pane takes the same Normal-mode keys, minus anything that would write.
So `:` works everywhere, not only in the prompt: `:sg` while reading the
transcript is exactly when you want it. `<Enter>` is the exception, and
deliberately: it would send the PROMPT's text, which you cannot see from
another pane, so it stays put rather than firing a turn you did not compose.

SPLITTING                                                        *window-split*

    CTRL-W s     `:split`  `:sp`     split the focused pane, stacked
    CTRL-W v     `:vsplit` `:vs`     split it side by side
    CTRL-W q     `:quit`   `:q`      close this pane — QUIT if it is the last
    CTRL-W c     `:close`  `:clo`    close this pane, never quitting
    CTRL-W o     `:only`   `:on`     close every pane but this one
                 `:hide`   `:hid`    stop showing this VIEW here
                 `:qall`   `:qa`     quit, however many panes are open
                 `:quitall`          the long spelling of `:qall`
                 `:wq`     `:x`      save this session, THEN close the pane

`:wq` and `:x` are one command here, not vim's two. Vim's `:x` writes only
when the buffer is modified and `:wq` writes always; `:write` here is already
idempotent — it says "already on disk" and does nothing — so there is nothing
for the distinction to attach to. Both save first and quit only if the save
succeeded: if it could not be written, you keep the pane and the reason,
because throwing away turns it just said it could not save is the one outcome
this command exists to prevent.

`:q` and `:close` differ in exactly one case, and it is vim's distinction:
on the LAST window, `:q` leaves Shannon and `:close` does not. The prompt bar
is no window `:q` closes while it is the only prompt, so the last is the one
beside it — on the default layout, the transcript — and `:q` in either
leaves. In a window showing a view over the transcript, `:q` gives the
transcript back first; from the bar, it leaves, the view and all. `:close`
there closes the transcript and keeps the bar, and on the bar alone
refuses. So `:q` is the one to reach for — it closes windows until there
are none, then quits — and `:close` is for when you want to be sure you
cannot exit by accident.

                                                             *quitting*
While anything is running (a reply, a command, a `:Send` job, a child), the
ways out that would end it ask first, as vim's `:q` does about a modified
buffer: `:q` on the last window, `:qa`, `:wq` and CTRL-W q refuse and name
what is running. Add `!` to go anyway: `:q!`, `:qa!`, `:wq!`. Going stops
every session's work, the way CTRL-C stops the one your prompt goes to. A
reply is sealed as `interrupted`, and a command is killed with every process
it started, short of one that detached itself (`setsid`). Nothing keeps
running after Shannon has gone. Closing
the terminal (SIGHUP) or a `kill` (SIGTERM) is taken as `:qa!`, since there
is no one left to ask.

A session that is not on disk counts too (|write|): one made and
not yet written that holds something of its own — a turn, a name, a mark
you set in it, or a merge's or a prune's choice of turns — or one whose
write failed. The refusal names it — `b4 7a9c1234, unwritten (:wa writes
it)` — and `!` leaves it behind. A checkout or a clone you only looked at
holds nothing a quit loses, and does not count.

A command that FINISHED may have left something running: `server &` ends
when its shell does, and the server runs on. Its result says so, `:ls`
lists it (|buffers|), and quitting counts it as running — `:qa`
names it, `:qa!` ends it. It is asked first (SIGTERM), so a server can
clean up after itself, and killed a second later if it has not gone. Its
output is still read once the command has finished, slowly: unread, a
server blocks when the pipe fills; closed, it would die at its next line.
On macOS for now; elsewhere, what a finished command leaves is not tracked
yet, and outlives Shannon. One that left the command's process group
(`setsid`) cannot be seen; if it still holds the output, the result says
something does.

`:hide` is the third one, and it is about the VIEW rather than the window. In
vim, `:hide` closes the window and leaves its buffer loaded, as against
`:bdelete`, which unloads it. Here a viewer BORROWS a pane rather than owning
a window (|window-where|), so `:hide` gives the pane back to whatever
it was showing — your transcript, and where you were reading in it — and a
pane opened FOR a view (`:sg`) closes, because there is nothing behind it.

To a viewer it does what `:q` and `:close` do — none of them takes the
transcript underneath with it — and it is the narrower-intent spelling of
that. Sessions live for the run and a page is a buffer, so a close loses
neither. A DECISION is where the verbs part, as vim's do: `:q` and `:close`
abandon it, which for a call is a deny, and `:hide` keeps it, hidden, until
`:decide` brings it back (|decision|).

THE LAST PROMPT IS NOT CLOSEABLE. It is the only pane you can type into, so
a window with none has no way in — and, unlike vim, no other pane could take
over the job. `:only` from anywhere leaves the pane you kept AND the prompt
you write in — and what belongs with it: its decision pane, a tree's
inspector, `:themes`' preview; from one of those, the window it belongs to —
while `:q` and `:close` on the last prompt refuse and say why (a tab's last
pane closes the tab instead). A second prompt closes like any pane, but not
over its draft. Everything else about it is an ordinary pane: split it,
resize it, move focus through it.

A bare split shows the same thing the focused pane does. To open something
else, use the `s`/`vs` family — vim's `:sbuffer`/`:sfind` lineage:

    `:sg`  `:sgraph`      the graph, in a stacked split
    `:vsg` `:vsgraph`     the graph, side by side

(`:split graph` and `:vsplit graph` are the long way round, and <Tab>
completes the names.)

WHERE THINGS OPEN                                                *window-where*

One rule, and it is the one you can see: EVERYTHING ACTS ON THE PANE YOU ARE
IN. `:graph`, `:help` and `:analyze` show their thing in that pane; `:sp`,
`:vs`, `:sg` and `:vsg` divide that pane. Nothing is ever placed for you
somewhere else on the screen.

So the two families differ only in whether you get a new pane:

    `:graph`   look at the graph HERE — `:q` gives the pane back
    `:sg`      keep this view AND look at the graph, in a new split

A new pane appears ABOVE (`:sp`) or to the LEFT (`:vs`) of the one you split
and takes the cursor — vim's default. `[tui] splitbelow` and `splitright`
flip each axis if you prefer the other way round.

Because a viewer only changes what a pane DISPLAYS, closing it (`:q`, `:close`
or `:hide`) restores whatever was there before — the transcript AND where you
were reading in it — rather than destroying the pane; the arrangement you built
stays put. That holds on the last window too: on the default layout, one
transcript and the prompt bar, a viewer's `:q` hands the transcript back,
and the next leaves. A pane you
opened FOR a viewer (with `:sg`) has nothing behind it, so closing it closes
the pane.

A view replaced is SET ASIDE, not lost: `:help` in the graph's pane shows the
manual there, and `:q` gives the graph back as you left it — cursor, marks,
folds — with an inspector beside it again (a new one: at its first width and
the top of its page); another `:q` gives back the transcript under it. A pane
keeps each thing it showed once, the latest: `:b` to another session keeps
the first, and CTRL-^ brings it back.

A COMPANION — the tree's inspector, `:themes`' preview — is its host's
window: what you ask for in one shows there. `:help` in the inspector puts
the manual in the tree's window, the inspector leaves with the tree and
gives its space back, and `:q` gives the tree back with an inspector
beside it. `:b N` and `:new` there show the session in the tree's window,
and you are in it; CTRL-^ brings the tree back, since `:q` on a
transcript closes it, as vim's does.

A SESSION opened there (`:open`, `:b N`, `<CR>` in the graph, `:new`) keeps
it too, as vim's `:b` keeps the buffer it leaves: the view, and the session
it lay over, stay in the window's history. Show that session in the window
again and it is where you were in it — the cursor, the scroll and its
folds, not the end. A window that never showed a session starts at its
end, its cursor on the last line: a transcript is live, and its end is
where the news arrives. That is
Shannon's default, not vim's — vim opens a buffer new to a window where
you last were in it, as a page and the tree open here. On a transcript
`:q` still closes the window, as vim's does, history or not.

A session MADE from the one a window reads — `:clone`, `:fork`, an edit,
a compaction or a squash landing over it — starts where that window was
reading: the cursor, the line at the top and the folds, found on the
turns it kept, as its marks are. What it did not keep starts at the end.
A `:fork`'s new window starts where the one it splits from is, as
`:split` does. A checkout is not one: `<CR>` on a turn in the graph
lands at the end of that turn — on a session's last turn, the session
itself at its end, not where its window was reading.

A window keeps the line at its top when its width changes — a `:vsplit`,
a `:vfork`, a terminal resized — as vim's does, and keeps its cursor on
screen.

                                                                 *alternate*
CTRL-^ (or `:b#`) takes the window back to what it showed last before what
it shows — vim's alternate file — as it was: a session where you were in
it, the tree with its cursor, marks and folds. Again, and it goes back
again: the two toggle. From the prompt bar it is the aimed window's — of two
wearing the accent, the lower-numbered — as `:b N` there is — and as there,
a view brought back takes the focus (its
keys are its own) while a session leaves you in the prompt (you type to it
there). With a count it is `:b N`: `3 CTRL-^` shows buffer 3, as
vim's does. With nothing to go back to, it says so ("no buffer to go
back to in this window", from the prompt bar "in pane 2, the aimed
window", from a companion "in pane 2, the window this one describes" —
vim's `:b#` says nothing there). A split starts
with no history of its own: its `:q` closes it, where a history copied
from the window split would give back what lay under that one.

One pane is never replaced: the DECISION pane, which belongs to its
conversation and must stay answerable. A view asked for there opens as one
asked for in the prompt bar does — in the pane the prompt is aimed at.

Running a view command AGAIN opens ANOTHER one, in the pane you are in now. It
does not jump you to the one already open: how many `:help` panes you want is
your decision, and reading two chapters at once is ordinary. Each is its own
reader with its own scroll.

The one exception is a pane that cannot host anything — the prompt bar and the
decision pane are sized to a declaration rather than to a share, so a manual
opened in one gets about three lines and costs you the only place you can type;
and a prompt window you split off is still a prompt, with a draft of its own,
so it shows nothing else either. Asked for from one of those, the view opens
in the pane the prompt is aimed at — the lowest-numbered wearing the accent,
whose session the title `prompt → <id>` names, or the pane a viewer has
borrowed from that session — and only when there is none of those, ACROSS
THE TOP of the frame (where vim puts `:help`).

ONE PANE, ONE SESSION                                          *window-session*

Each transcript pane shows a session of its own. `:open <hash>`, `:buffer N`,
`:new` — and <CR> on a session in |graph| — point THE PANE YOU ARE IN
at that session, by the same rule as a view; the other panes stay where they
are. Any pane can: standing in `:ls` or `:help`, the session takes the pane,
and the listing or the manual is kept in its history, as vim's `:b` keeps
the buffer it leaves (|alternate|).

TWO SESSIONS SIDE BY SIDE, which is what the panes are for:

    :vnew           a new pane beside the window you are in, on a fresh
                    session (`:snew` stacks it); from the prompt, beside
                    the window it is aimed at; from the inspector, its
                    graph's. With no window on the aim, the pane opens
                    where `:new` would open one

Or by hand, which also takes an existing session:

    CTRL-W k        step up into the transcript FIRST — you start in the
                    prompt, and `:vs` splits the pane you are standing in
    :vs             a second window — on the SAME session, so far
    :new            give THIS pane a session of its own, or
    :open <hash>    ...an existing one, by short id

Both steps matter. `:vs` from the prompt gives you a second PROMPT, which is
consistent (everything acts on the pane you are in) and not what you wanted.
And `:vs` in the transcript leaves you with one session in two windows, which
is vim's rule for a split — until you run `:new` or `:open` in one of them,
both windows show the same conversation and both update together. After that,
comparing a branch against the trunk needs no switching back and forth.

Vim's `:new` splits as well; Shannon's is what vim calls enew, the pane you
are in, so the split forms are `:snew` and `:vnew`. Each shows a fresh
session: the one your prompt is aimed at while nothing has called it — blank,
unnamed, in no window, as `shannon tutor`'s chat is — or else a new one, as
vim's `:vnew` makes a new buffer, so no empty session is left behind. The new
window keeps the session it split from as its alternate (|alternate|).

Every transcript pane's title names the session it shows, with the same short
id `:open` takes, so what you read off a border is what you can type:

    ─ a1b2c3d4 ────────────────────  a root session
    ─ 9f8e7d6c — branch ← 4b5a6978 ─  a branch, and where it forked
    ─ 51ce27ab · running ───────────  a child session mid-turn

The PROMPT follows the pane you stand in: a prompt, one session it commits
to. A running child is a session like any other, so `:open`ing one lets you
WATCH it work rather than waiting for the notice — and typing into it queues
behind its turn instead of interleaving with it.

Two panes on the SAME session share the text and nothing else: one transcript,
a cursor and a scroll position EACH. That is vim's rule for two windows on one
buffer, and it is why `:sp` on a transcript gives you a second view of what you
were already reading rather than a copy — hold one place while you read another,
and the status line's `$follow` (|statusline|) describes the window you
are standing in.

Splitting a pane inherits its session, so `:sp` and `:vs` always give you a
second window on what you were looking at.

The PROMPT can be split too, and each prompt window is its own: its own draft,
its own cursor, and its own session it commits to — so two conversations can
be drafted side by side. A new one starts empty, going where the one it was
split from goes. Standing in a transcript aims the prompt you used LAST at
it, and that one only; the other keeps where it was going. Each title reads
`prompt → <id>` while there are two, naming its own, and the one you write
in — the one you stand in, else the one you used last; what `<Enter>`,
`:Send` and `:aim` take — wears `%`, as `:ls` marks the current buffer:
`% prompt → <id>`. What you yank in one, `p` pastes in another: the register
is the editor's, as vim's is. A tab's prompts are its own, so a draft stays
in the tab you wrote it in.

When the aim is not where you want it, say where: `:aim N` aims the prompt
you write in at session N — as `:ls` numbers it, or by id or name — shown
or not, and the title names it while no pane here shows it. It stays until
you stand in a transcript, or the pane that showed it closes. A bare `:aim`
says where the prompt goes.

A prompt holding a draft is not closed — vim's refusal for a modified
buffer: `:q`, `:close`, `CTRL-W c` and `:hide` (a draft has nowhere to wait
hidden) say so; send it or clear it, or `:q!` drops it, and says it did.
`:only`, `:tabclose` and `:tabonly` refuse over a draft they would close the
same way, their `!` closing anyway. Quitting does too — `:qa`, and `:q` on
the last window, name the pane whose draft would go; `:qa!` quits over it.

SAVING                                                               *write*

    Turns reach the store as they COMMIT, so `:w` usually answers
    `already on disk`. It is for two things: a write that FAILED — the
    message said so, and this is the retry — and a session made but not
    yet written. A squash, a `:compact` and an `:import` hold text nothing
    else does, and are written as they are made. A `:clone`, a `:fork`, a
    checkout, a straight merge and a prune only share another session's
    turns, and are written on their first turn, or here. Quitting names
    any such session that holds something of its own rather than lose it,
    and `!` is how to leave it behind (|quitting|); one you only
    looked at, it lets go.

        :write  :w    save this session now
        :w {name}     name it, then save it — vim's `:w {file}` on a
                      buffer with no name (|names|)
        :wall   :wa   save every open session

    `:w` on a session with nothing pending answers `already on disk`
    rather than nothing, because a verb that works and a verb that does
    nothing must not look the same. A name is a SESSION's name, never a
    file: `:w notes.md` names the session `notes.md` and says so first.
    A session already named otherwise is refused toward `:rename` — vim
    would write a copy there, and a copy is `:clone`'s. Exporting a
    transcript to a file is `:Edit`, which opens it read-only in $EDITOR.

READING A PANE                                                  *window-read*

    Every pane you can only READ — a transcript, the graph, the manual,
    an analysis, the inspector, the message log, the report in a decision
    pane — navigates the same way, with the same keys, from the same keymap.
    There is a CURSOR in each of them; `j` and `k` move it and the window
    follows only when it has to, so the page stays still while you read down
    it. In all but the graph it is on a character, as in vim's read-only
    buffers, so every motion of |motions| works there too.

        j  k             down / up one line
        w  b  e  W  B  E  0  $  ^  f  t  ;  ,  ge  %
                         along the line, as in the prompt
        }  {             the next / previous paragraph
        /  ?  n  N  *  #
                         search (|search|)
        {count}j         five lines is `5j`, as anywhere else
        CTRL-F  CTRL-B   forward / back one screen (the cursor goes too)
        CTRL-D  CTRL-U   half a screen
        gg  G            first / last line
        ]]  [[           in a transcript, the next / previous turn
        ][  []           the next / previous turn's end (|motions|)
        m{a-z}  '{a-z}   in a transcript, set / go to a mark
                         (|mark-motions|)
        H  M  L          top / middle / bottom line of the window; `5H` is
                         the fifth from the top (with 'scrolloff' at 5, the
                         same line `H` finds — both keep five lines of room)
        zt  zz  zb       put the cursor's line at the top / middle / bottom;
                         `20zt` goes to line 20 first. Near the end this
                         scrolls PAST it, and `~` marks rows past the end
        zo  zc  za  zR  zM
                         in a transcript, open / shut / flip a fold, or every
                         one (|folds|)
        CTRL-]           in the manual and the tutor, follow the link under
                         the cursor, or the next on its line; so does <CR>
        CTRL-T           back to where it was followed from ({count} of them)
        :q  CTRL-W q     close it, giving the pane back to its session
        :               the command line, from anywhere

    `H` `M` `L` and `zt` `zz` `zb` work in every one of them. In the
    transcript a line cut off at an edge of the window does not count as on
    screen, as in the manual; and near the end `zt` shows the last page
    rather than scrolling past it. The prompt takes them too, and its
    cursor pushes its window at 'scrolloff' from an edge as every other
    window's does. An operator over them (`yH`) is refused in the
    transcript and the prompt out loud — select with `V`, then `y`.

    SELECTING works in every one of them but the decision pane, whose `o`
    and `d` are its answers. In a page — the manual, an analysis, the
    inspector, the message log — it is the transcript's: `v`, `V` and
    CTRL-V start one, any motion grows it, `iw` `aW` `i"` select a text
    object, `o` swaps its ends, <Esc> drops it, and `y` copies it to the
    system clipboard and to the register `p` pastes in the prompt
    (|register|) — so a sentence of the manual is `v`, a motion,
    `y`, then `p` in the prompt. `yy`, `yj`, `yw` and `yiw` copy without
    one. In the graph a selection is a range of rows
    (|graph-visual|).

    A selection belongs to its window: moving focus — `CTRL-W`, a click, a
    decision arriving — ends it, as leaving a window ends Visual in vim.

    A VIEWER IS A WINDOW, so you close it the way you close a window. No
    bare key does it — not `q`, not `<Esc>`. That is vim: the `:help` window
    it ships binds nothing, and in netrw `q` is a PREFIX (`qb`, `qf`) rather
    than a close. Binding it here would spend a key vim keeps free and make
    these panes the one kind that closes differently from all the others.

    Nothing is bound to a bare letter that vim binds to a chord: `f` is
    not `CTRL-F`, and `g` is a prefix rather than a command. If a motion
    you expect is missing it is missing everywhere, which is the point —
    one vocabulary, not one per pane.

    A key that WRITES — `i`, `p`, `x`, `d` — is refused here rather than
    ignored, and says so on the message line: these panes have no buffer
    to write through, and `<C-w>w` is the way back to the one that does.

    Two panes keep a few keys of their own on top of these: :graph's verbs
    (`d` deletes, `p` picks — |graph|) and a decision's answers
    (|decision|). A key typed after a prefix — `z`, `g`, `CTRL-W` —
    belongs to the prefix, never to them: `zd` is not a delete, and `za`
    is not an allow. A count before one of their keys is dropped: `3d` is
    `d`.

THE MESSAGE LINE                                                  *messages*

    The bottom row doubles as Shannon's message line, and it is where
    every refusal lands: a guard that stops something you asked for says
    why THERE, never only in a log.

    It is one row, so a long message is CUT — and when it is, the cut
    end reads `…:messages` in grey. That is not decoration: the tail of
    a message is usually where the recovery advice is, so a message that
    merely stopped would be worse than none. Several at once are joined
    with ` · `, a refusal first: the row takes the colour of the loudest,
    and is cut from the end.

        :messages   every notice this session, oldest first
        :mes        same

    The pane is a still, not a feed. Run `:messages` again to pick up
    what has been said since — in every pane showing the history, each
    where it was; nothing moves under you while you read.

TRACING A RUN AFTERWARDS                                               *log*

    `:messages` ends with the run. What a run did is also written down,
    owner-only, under `$XDG_STATE_HOME/shannon/log/` (by default
    `~/.local/state/shannon/log/`), a file a day:

      shannon.log.<date>    every notice the status row showed (as the
                            log keeps text: a secret by its handle), each
                            request's shape — model, effort, max tokens,
                            the thinking asked for and how many thinking
                            blocks went back, the beta headers — the tokens
                            each reported and how much came from the cache,
                            and each command the gate let run or asked
                            about, under the session it belongs to
      shannon.audit.<date>  what the agent was allowed to do and did — a
                            trail to hand on without the rest

    A line starts with the time, the run's four-digit id and the session's
    short id, so `rg eed01df2 shannon.log.*` follows one session. The
    diary's detail is `RUST_LOG`'s: `info` unless set, `RUST_LOG=debug`
    for more; the audit trail is written whatever it says. A session's
    turns themselves are its store (|persist|). Both places are
    guarded from the model's commands as a credential path is
    (|secrets|).

MARKS: ONE QUESTION, SEVERAL SESSIONS                         *window-marks*

    CTRL-W <Space>    mark / unmark this transcript pane
    CTRL-W u          clear every mark
    :Send [1,3]       send the prompt to the marked panes, or to those
                      pane numbers (which marks them, so you can see it)
    :Send all         send it to every transcript pane (and mark them)
    :Send b2 3f2a91c0 send it to buffer 2 and to the session whose id
                      starts 3f2a91c0, shown or not (marks are left alone:
                      a session in no pane has nothing to mark)

A target of four or more hex characters is a session id, so it is never read
as a pane number; an id is typed from its first 8 characters. One that
matches two sessions is refused with both, never sent to whichever came
first.

A marked pane wears a `*` before its id, and while anything is marked your
prompt goes to ALL of them instead of to the aimed one — an explicit statement
beats an implicit one, and you can see which it is.

A marked send is a background turn, but it is NOT unattended: you just pressed
<Enter>, so a command it wants to run stops and asks you exactly as a
foreground turn would (|decision|). Only a `:child` (|child|)
runs unwatched, and only a child holds instead of asking.

This is the reason for the whole window model. Put two sessions side by side
(|window-session|), give them different models (|model|), mark
both, and one question is answered twice at once. Give them the SAME model and
you are measuring how much a reply varies on its own, which is worth knowing
before you trust one.

Each send is a background job, not a foreground turn, so the replies stream
into their own panes together and no reply waits for another. Their QUESTIONS
do: there is one decision at a time, so a second send that asks waits until
the first is answered (|decision|). Two panes on
ONE session count once: a mark names a pane, but a turn belongs to a session.
A session already mid-turn is skipped and says so, and the others still go.

SIZING                                                            *window-size*

    {count}CTRL-W +/-          taller / shorter
    {count}CTRL-W >/<          wider / narrower
    CTRL-W =                   make every pane the same size
    CTRL-W H/J/K/L             move this pane to that edge

`+`/`-` change height and `>`/`<` change width, and each finds the nearest
divider on that axis — so `CTRL-W +` on one of three side-by-side transcripts
takes rows from the prompt, which is the divider above them all. `=` evens the
panes out and leaves the prompt bar at its declared height, because a bar is not
a window.

Height and width are remembered separately, so flipping a pane from stacked to
side-by-side and back restores the size you had on each axis.

The DECISION pane opens sized to its content, a band under the session that
asked. Size or move it like any other — `CTRL-W +` for a long command,
`CTRL-W L` to stand it beside the conversation — and the conversation beside
it too. Its first resize or move makes the size it is drawn at yours, and the
next decision opens where you left it, at that size, for the rest of the
run. `CTRL-W =` puts it back to fitting its content, below. It is never
split: each session's decision has one pane.

In a terminal too short for everything, the decision pane shrinks first, then
the prompt — a prompt one row tall is still a prompt — so the conversation
keeps a few rows to read.

VIEWERS ARE PANES                                                *window-viewer*

`:graph`, `:help` and `:analyze` are ordinary panes, not screens that take
over. They join the focus ring, `<C-w>` moves and sizes them like anything
else, and `:q` puts back whatever the pane was showing. You can run `:`
commands while looking at one — which is the point, since `:open <hash>` is
exactly the command you want with the tree in front of you, and the manual is
worth more beside your work than instead of it.
"#;

const BUFFERS: &str = r#"==============================================================================
5. BUFFERS AND TABS                                                   *buffers*

A session does not end when its pane moves on. Prompt one, `:new` or `:open`
another in the same pane, and the first keeps working out of sight, as a
hidden buffer does in vim:

    :ls             every session open this run, numbered b1, b2, ... — a
                    number missing is a page's or a tree's, or a session
                    not called yet, as `:ls!` shows (below); and the tutor,
                    a page you edit
    :buffer N       show session N here                              (:b)
    :b#  CTRL-^     back to what this window showed before (|alternate|)
    :Send bN        send the prompt to it without showing it
    :aim N          aim the prompt you write in at it, without showing it
    :decide N       go to the decision waiting on it (|decision|)

`:ls` is a view like `:messages`, shown in the pane you are in
(|window-where|), and it is LIVE: a session that starts
or stops needing you changes its line while you read, and you keep your
place — on the LINE you were on, wherever lines come or go above it, as in
every listing (`:alias`, `:rules`, `:scriptnames`, `:themes`). <CR> on a
line shows that buffer here, as `:buffer N` would, netrw's <CR> in a
directory: `:ls` lies behind it then, and CTRL-^ brings it back, on that
line. `:buffer N` and `:Send bN` are typed right there too. It marks each line
as vim does: `%` for the session your prompt goes to, `a` for one a pane
draws, `h` for one hidden — a session whose pane a viewer is lent over is
hidden there, and its pane is named under what (`pane 1 under ls`). There is
no `#` yet: CTRL-^'s alternate is each window's (|alternate|). Right
after the marks it says what it is doing, first so a narrow window still
shows it:
`running`, `needs you` when a decision waits on it (`:decide N` goes to
it, and is named there when no pane shows the decision), or `held — needs
you` when a job stopped for you. Through Claude Code a decision also says
how long its hook will wait for you — `needs you · 52m left`, counting
down while you read (|timeouts|). A session with news you have
not seen — a reply that ended where no pane showed it — wears a 🔔 in a
column of its own, as does each session an alert of yours with `mark`
holds for (|alerts|). Below the sessions, it lists
what finished
commands LEFT RUNNING — each process and its pid, the command that started
it and which session's call that was (<CR> on it shows that session), how
long ago, how much it has
printed since, and the last line it printed — which Shannon ends when it
exits (|quitting|). That last line is the program's own output,
shown as the transcript shows output: control characters removed, a
secret replaced by its handle.

Every command that takes a session — `:buffer`, `:Send`, `:open`, `:resume`,
`:delete`, `:clone`, `:merge`, `:return`, `:rename`, `:decide` — reads it
the same four ways: `b<N>`, as `:ls` numbers it; the first 8 characters of
its id or more, in any case (as every id is shown; Tab completes them); the
name
`:rename` gave it (|names|); or `%`, the session your prompt goes
to, as vim's `%` is the current file. A start two sessions' ids share is
refused, naming both; it is never read as the first. (A buffer's `b<N>` is
never an id: it is shorter than 8.) `b<N>` is the buffer that number
names, whatever it holds. A bare number of up to six digits is a buffer
to `:buffer`, `:decide` and `:tabnew` (vim's `:b 2`), and a PANE to
`:Send` (`:Send 2`); longer, it can be no buffer's and is read as an id,
so eight digits copied off a border reach their session. The notice after
a send names who it reached.

This manual is a buffer too, as vim's help is: ONE, however many `:help`
windows read it, and closing the last keeps it — with where you were in it.
So are `:analyze`'s report, which each `:analyze` replaces in every pane
showing it, the `:messages` history, which each `:messages` reads again,
the session tree `:graph` shows, and every listing — `:ls`, `:alias`,
`:rules`, `:scriptnames`, `:themes` — one buffer each, however many windows
show it, kept current while one does. `:themes` reads its themes again each
time it is typed.

    :ls!            the sessions, and the pages and the tree beside them
    :buffer N       a page, back where you were, that line mid-window;
                    the tree, back on the row you were on, mid-window — or,
                    either, in a window that held it, as that window left it

`:ls!` is the same list showing more, as Emacs's buffer list is: the ONE
`:ls` buffer naming the unlisted too, until `:ls` is typed again — in every
window on it, each titled `ls!` while it does. It lists each page in its
place by number, the listings among them, marked `u` (unlisted) where a
session has `%`, with the line you were on — but the tutor, which you edit,
as vim lists a file and not its help: `:ls` lists it too, unmarked. And a
session you have not called yet — nothing in it, no name, in no window, as
`shannon tutor`'s is until `:new` or a prompt — `:ls!` lists alone.
Sessions, pages and the tree are numbered from one count: with b1 open and
the manual made b2, the next session is b3; `shannon tutor` is b1, its
session b2. A command that takes a session, given a page's or the tree's
number, says which it is rather than finding no session. `:graph` still
opens the tree on your session; `:buffer N` is where you were.

THE TREE IS NETRW'S LISTING, as vim treats a directory: ONE buffer, however
many windows show it, and its MARKS are the tree's, so every window on it
shows them, made or spent in any. `:sp` of a tree copies the window — its
cursor, folds and scroll; `:sg` and `:graph` elsewhere open a new window on
it, on your session; `:graph` in a pane showing it lists it again. Closed
everywhere it is kept, marks and all, and the next `:graph` shows it again.
The cursor, the folds and the scroll are each window's.

TAB PAGES                                                                *tabs*

A tab is a layout of panes, as vim's tab page is — your own arrangement, as
you would use another monitor or another workspace. The sessions are not
the tab's: one shown in two tabs is one buffer in two windows, and closing a
tab closes its windows, never its sessions (`:ls` still lists them). Each
tab keeps its own prompt — what you half wrote there and where it is aimed —
and the pane you were in: go back to it, and you stand where you stood,
typing to what you typed to there — in a decision pane, its keys waiting
their delay again.

    :tabnew [N]     a new tab after this one, on a fresh session — or on
                    session N, by its `:ls` number, its id or its name;
                    `:{N}tabnew` puts it after tab N (`:0tabnew`: first)
    :tabclose[!] [N]
                    close this tab and show the one to its right (the
                    left, from the last) — or close tab N and stay where
                    you are; never the only one, and not over a request
                    waiting in it, a call or a summary's review, or a
                    prompt's draft: answer or send it first, or `!` closes
                    anyway — the request waits, hidden; the draft is dropped
    gt  :tabnext    the next tab, the first after the last; `{N}gt` and
                    `:tabnext N` go to tab N
    gT  :tabprevious
                    the previous tab, the last before the first; `{N}gT`
                    and `:tabprevious N` go N back
    :tabmove [N]    move this tab after tab N (`0`: to the front); bare or
                    `$`, to the end; `+N` and `-N` (`+`, `-`: one) move it
                    N places — never past either end
    :tabonly[!] [N] close every tab but this one (or but tab N) — not over
                    a request or a draft in one of them, but with `!`
    :tabrename [name]
                    name this tab: the tab line shows the name instead of
                    what its pane shows, and it stays with the tab wherever
                    it moves; bare says it, `:tabrename!` takes it off
    :tabs           list the tab pages and the windows in each, live, as
                    vim's `:tabs` prints them: `>` the window you are in,
                    `%` the prompt a tab writes in, `+` a prompt holding a
                    draft; `<CR>` on a tab's heading goes to the tab, on a
                    window stands you in it
    :tab {cmd}      run {cmd} with the window it opens in a new tab after
                    this one — `:tab help`, `:tab graph`, `:tab b 2`, `:tab
                    sp`, `:tab fork`; a command that opens no window opens
                    no tab. A tab is never made around a prompt or a
                    companion: `:tab sp` from the prompt copies the window
                    it is aimed at, from an inspector the tree's
    CTRL-W T        move this window to a new tab of its own, where you
                    were in it and all — its companions made again beside
                    it, a decision on its session following it there, its
                    `:Send` mark kept; `{N}CTRL-W T` puts the tab before tab
                    N; not the prompt, a decision pane, or a tab's only
                    window (`:tab sp` copies that one)
    :ball [N]       a window for each session `:ls` lists that no window
                    here shows, split below in `:ls` order — at most N, the
                    one you are in counted first; `:ba`, `:sball` too
    :tab ball [N]   a tab for each session no tab shows, after this one in
                    `:ls` order — vim's way to open every buffer in a tab.
                    Neither closes a window: vim's `:ball` also closes what
                    it does not need, and here a window carries its
                    session's decision

`:tab b 2` opens a tab where vim's opens none (vim's `:b` opens no window):
here what `:b` and `:help` show come by one door, the pane you stand in,
and `:tab` sends both to a tab of their own.

Where a tab command takes a tab — `:tabnext`, `:tabclose`, `:tabonly` — it
is N (from 1), `+N` or `-N` (N to the right or the left; `+` and `-` alone,
one), or `$` (the last): vim's, and never round the end, as `gt` goes.
`:tabprevious` takes a count only. A count or range before the command is
its argument: `:2tabnext` is `:tabnext 2`, `:3tabclose` closes tab 3,
`:+tabmove` moves one right, `:$tabnext` goes to the last, `:.tabclose`
closes this one, `:-tabnew` opens just before this one; `:2tabprevious` goes
two back. `:tabmove` counts the tabs before the move, as vim's does, so
from tab 2 `:tabmove 1` and `:tabmove 2` leave it where it is — said. With
one tab, `gt` and `:tabmove` say so. Closing another tab says which closed.

`:tabc`, `:tabn`, `:tabp`, `:tabN`, `:tabm` and `:tabo` are the short forms.
`:q` on a tab's last window closes the tab while another is open, as vim's
does, and so do `:close` and CTRL-W c on its prompt bar alone; on the last
tab `:q` leaves. A tab number
that is not there is said ("there is no tab 9"), from `9gt` too, where vim
says nothing; so is `gt` with one tab.

With more than one tab, a line along the top names each — its number and
the name you gave it, else what its focused pane shows (the session's id or
name, or the view; standing in a companion, its host's) — the one you are
in in `ui.tabline.selected`, the rest in `ui.tabline`. Narrow, it keeps the
numbers and cuts the names — a name whole, or not at all below six cells;
too narrow for every number, it shows the tab you are in and those around
it, `<` and `>` where some are cut — in `ui.caution` when a decision waits
in one of those (an alert's glyph on a cut tab is not drawn; `:ls` marks
its session). With `tui.mouse` on, a click on a tab's label goes to that
tab.

A DECISION opens beside its session: in this tab when a window here shows
it, else in the tab that does — never gone to for you. That tab's label
turns `ui.caution` while it waits, the status row says `DECIDE in tab N ·
Ngt`, `:ls` says `needs you — in tab N`, and its keys wait their delay
when you get there, as a call just arrived does. `:decide N` takes you
there — to its pane's tab, or, its call hidden, to the tab that shows its
session. A tab holding a session an alert marks wears the alerts' glyph
beside its number, as `:ls` marks the session (|alerts|); `:ls`
names a pane in another tab `t2:3`, tab 2's pane 3.

SESSIONS RUN APART                                            *sessions-apart*

Each session answers on its own. A turn runs in the session it was sent to,
and a prompt for another session starts at once — beside it, not behind it:
`:vfork` a side question and both conversations go on together, each with
its own agent. What waits, waits per session:

    a second prompt     queues behind THAT session's turn, and is sent when
                        it ends (`N queued` on the status line)
    a decision          holds the session that asked it, and no other
                        (|decision|); another session's turn goes
                        on meanwhile — with the calls the rules allow; one
                        that asks waits its turn in the one queue of calls
    CTRL-C              stops the work of the session you are aimed at, and
                        of every marked one — pressed anywhere but the
                        decision pane (|interrupt|)
    :qa!                stops every session's

The status line is about the session your prompt goes to; when work runs in
another, the sessions running are counted — `2 sessions running`, this one
among them — so a turn in a pane off-screen is never invisible
(|statusline|). A notice about work in a session you are not aimed
at names it — `b3 fix-parser: stopped — …`.
"#;

const GRAPH: &str = r#"==============================================================================
6. THE GRAPH VIEWER                                                     *graph*

`:graph` (alias `:g`) opens history itself: every session, turn, and message
as a collapsible tree, with an inspector for whatever is selected. It opens
on the turn your cursor is on in the transcript you open it from — the
last, if you have not moved it (a window starts at its end, its cursor
there too) — on its row in the tree: a turn a branch shares with the
session it came from is drawn under that session. The viewer is a
snapshot — reopen it (or act through it) and it rebuilds.
History operations live HERE, on the thing they act on, because an
operation should mean what the screen shows (|wysiwyg|).

KEYS AND COMMANDS                                                 *graph-keys*

    `:help graph-keys` typed in the tree opens this in the tree's pane, and
    `:q` gives the tree back as it was. Moving is vim's — `j` `k` `gg` `G`,
    counts, `H` `M` `L`, `zt` — and `/` `?` `n` `N` search the rows as they
    read, to move to one, opening a fold to reach it. A session forked from
    a turn hangs under that turn, folded: `/` and its id, <Tab>, or
    `:resume <id>` reach it. A verb acts on a Visual range, else the marks,
    else the row; so does its command, typed here.

    Open and fold
      <CR>                      open the session HERE, the tree kept in
                                this window's history; on a turn, check
                                it out
      <Tab>                     the next session
      <S-Tab>                   the previous session
      l h                       unfold / fold the row (the arrows too)

    Mark, as netrw does
      mf <Space>                mark the row, or unmark it
      mu                        unmark every row
      V                         a range of rows; `mf` marks all of it

    Compose a new session — the originals stay
      p           :pick         the marked rows, in the tree's order
      S           :distill      a summary the model drafts; `!` reviews it
      <Esc>                     stop a summary the model is drafting
                  :squash       a summary you write, in $EDITOR
                  :clone        a sibling sharing the whole history
                  :merge        a branch's turns, back onto its base
                  :return       a finished child, as one turn on its parent
                  :compact      its history, summarized in place

    Change what is here
      e                         edit it in $EDITOR, as Messages-API JSON
      r           :rename       name the session (|names|)
                  :description  say in a line what it is
      d D         :delete       delete the sessions or turns, asking first

WHY A GRAPH                                                        *graph-why*

    Most coding agents keep one growing transcript: every turn is permanent,
    a wrong turn sits in the context forever, and trying a second approach
    means starting over and losing the first. Shannon keeps the whole
    conversation as a TREE instead — and that single change is what the rest
    of this chapter is really about.

    Because history is a tree, you can:
      - BRANCH at any past turn and try another approach without losing the
        first — both lines live side by side (|branch|).
      - EXPLORE in parallel — spawn children (|child|) down several
        paths at once, then keep only what worked.
      - COMPOSE the good parts of different lines into one: merge a branch,
        pick scattered turns, or squash a dead end into a one-line note so
        it stops costing context.
      - DISTILL — replace a long exchange with a summary that still points
        back at the originals, trimming what the model re-reads each turn
        without throwing the record away.

    None of this destroys anything (|history-truth|): every operation
    composes a NEW session and leaves the originals reachable right here. You
    can experiment freely precisely because you cannot lose work.

READING THE TREE                                                  *graph-tree*

    ● session      a root or branch head (one color per branch line)
    ◆ child        a spawned child session (|child|)
    ▶ in gutter    the session a launch resumes (HEAD), in the leftmost
                   column — not where you are (|head|)
    ▶ on a child   that child is mid-turn (same glyph, beside the row)
    ⊚ turn         a summary: where a compaction happened (|compact|)
    grayed turns   history shared by reference from another session
    ◇ Claude Code  one of its conversations, below the tree
    ⧓ Claude Code  one a Shannon session continues (|graph-claude-code|)
    A turn's messages (`l` on it) say who spoke: `you:` before your words,
    `reply:` before the model's; a tool call or its output speaks for
    itself, in brackets — `[Bash: rg …]`, `[result: …]`, `[error: …]`.
    Glyphs are configurable (`[tui] graph_glyphs`) — each role, one
    character one cell wide, as the rows line up by it; another is said
    and the default kept — and so are their colours, in a theme
    (`ui.graph.continued` and the rest, |themes|); `graph_chars =
    ascii` switches the whole set. Two styles exist: `git` (default; lanes and
    decorations like git log --graph --oneline) and `tree` (indented),
    via `[tui] graph_style`. A named session's row leads with its name,
    the id after it; `[tui] graph_ids = "hidden"` drops the id there
    (a session with no name, and a turn, keep theirs).

NAVIGATION                                                         *graph-nav*

    j k             move        gg G        top / bottom
    Tab S-Tab       next / previous session (reveals buried branches)
    l h             unfold / fold          (arrows work too)
    :q              close       :           command line stays available

    The moving keys are every read pane's (|window-read|), so
    counts (`5j`, `12G`), `H` `M` `L` and `zt` `zz` `zb` work here too.

    <CR> ACTS on the selection — it is not a fold key. On a SESSION it
    opens it (makes it active); on a TURN it checks out that point, so the
    transcript shows history up to there and continuing forks a branch
    (|sessions|). Either way the viewer closes — it is the take-me-
    there key, and the only operation that leaves the tree. Folding is
    `l`/`h`.

THE INSPECTOR                                                *graph-inspector*

    The right pane details the selection, and it is an ordinary read-only
    page: `j`/`k`, counts, `CTRL-F`, `gg`/`G` and `:q` all work in it
    (|window-read|); what you open from it (`:help`, `:b N`)
    shows over the tree and it, and `:q` there gives both back
    (|window-where|). It shows ids, origin (`branch ←`,
    `child ←`, `merge ←` + the fork turn; `at` + the turn of a checkout
    nothing was sent in; `clone of` + the session copied), model, per-turn
    token usage, timestamps, how a turn ended (|stop-reasons|),
    what a summary distills (`distills: …`), child-session depth and
    count, and a subtree usage sum (what a session and everything it
    spawned cost). A session with anything of Claude Code's says what:
    the conversation an import continues and how many turns came in; or,
    for one born here from such a session (a clone, a branch), the turns
    it holds that came in and through which session — and that
    `:delete all shannon-only` deletes it, as it was born here. A
    message's words are painted as the conversation paints them: the
    reply's markdown and code styled, the question dim.

    `[tui] time_format` says how a time is shown: `"locale"` (the
    default — your locale's date and time), `"unix"` (the seconds), or a
    `strftime` pattern such as `"%Y-%m-%d %H:%M"`. What is saved is always
    the seconds; this is only how they read.

MARKS                                                             *graph-marks*

    mf          mark / unmark the selected row
    <Space>     the same thing, one key
    mu          clear every mark

    The tree is a file browser over your sessions, so it borrows netrw's
    mark vocabulary rather than inventing one: `mf` marks a row, `mu`
    unmarks them all. If you already drive netrw, these are the keys you
    already know. `<Space>` is an alias because netrw leaves it unmapped,
    so the shorthand collides with nothing.

    Core vim has no non-contiguous Visual selection — a marked SET is how
    vim itself expresses "these ones, not a range". For a range, use Visual
    (|graph-visual|); `mf` there marks the whole range, so ranges
    build a set.

    Marking a session row means exactly the turns displayed under it.
    Marks drive `d` `S` `p` and `:squash` below; their SHAPE picks the
    semantics (contiguous? one session?). A verb spends the marks it acted
    on — after `d`, even one it refused — and leaves any others standing.
    Marks are the TREE's, as netrw's marked list is its listing's: every
    window on it — `:sp`, `:sg`, a second `:graph` — shows the same marks,
    made or spent in any (|buffers|).

VISUAL                                                           *graph-visual*

    V           select a RANGE of rows from here (`v` and CTRL-V too: a row
                has no columns, so all three select whole rows)
    j k G …     grow or shrink it — every moving key works
    o           swap its ends
    mf <Space>  mark the whole range, and end Visual
    <Esc>       end Visual (while `S` is generating, a second <Esc>
                cancels it — the first only drops the range)

    A range is what the next verb acts on — `d` `r` `p` `S` `e` <CR> —
    ahead of the marks, and pressing the verb ends Visual whatever it then
    does, as an operator does in vim. A folded session in the range is
    that session; the turns its fold hides are not separately in it. `e`
    edits one row, and says so when more are selected. `y` has no text to
    copy here, and says so.

CLAUDE CODE                                                 *graph-claude-code*

    Below the tree, Claude Code's own conversations for this directory,
    newest first — what `claude --resume` would offer, with their titles.
    This directory's only: another's are not listed, and `:import <id>`
    reads one of them by name.

    They are another program's records, so Shannon reads them only where
    you say it may. The first :graph in a directory asks: `y` reads them
    for this run, `a` always (kept, never asked again here), `n` not — the
    heading then says `not read — l asks`, and `l` on it asks again.
    `:import <id>` of one of this directory's asks the same, and imports
    on `y`; of a file anywhere else — a path, another directory's id — it
    asks about that file alone (`y/N`), and reads nothing else there.
    `:unread` takes your leave back — as `n` for this
    run, `always` forgotten — and lets go of what was read; `:unread all`
    does so for every directory you gave it for.

    l       open one a level: what hangs from it (the sessions going
            on with it, the conversations resumed from it), then its
            turns, in its lane, oldest first — every compaction kept,
            its summary the turn marked ⊚. `h` closes a level. Until
            its turns are open its row ends in ⋯. Read when first
            opened, in the background — its row says `reading…` until
            it is — and kept: a file grown since is read on from where
            it was, a secret marked since scrubs it again. On a turn,
            the inspector shows its prompt and its reply — scrubbed, as
            an import would bring them in.
    <CR>    import it as a Shannon session — a copy, from its latest
            compaction (`:import --all <id>` reads all of it). On one
            a session goes on with (⧓), open that session instead —
            the most recently active, if several do. On one of its
            turns, a copy up to that turn: the conversation as it
            stood there, from the compaction before it

    They are Claude Code's files, and Shannon only reads them. `d` on one
    refuses and says why; a range over them deletes only Shannon's
    sessions, and the question says the rest stay; they cannot be marked.
    The copy an import makes is a Shannon session — yours to delete.

    A conversation a Shannon session CONTINUES — one you imported — wears
    ⧓ in its own colour (`ui.graph.continued`) where the others wear ◇, so
    a fold above cannot hide that it went on. The session hangs UNDER it,
    at the turn it parted at, as a branch hangs under the turn it forked
    from — not among the sessions above. Its imported turns are the
    conversation's, drawn once, there; its row says what it continues
    (`continues <id>`), and a branch cut from an imported turn hangs under
    that turn of the conversation's — once its turns are open. Until then
    (by default: a long conversation's turns at once are too many to see
    past) the session hangs from the conversation's row, which is open to
    it, so the move hides nothing. A session whose turn cannot be found
    hangs from the conversation's row, every turn its own.

    A conversation `claude --resume` went on with in a new file hangs under
    the one it was resumed from, at the turn it was picked up — so you see
    where you picked it up, and how far each has gone since. The turns the
    two share are drawn once, as the first's; a session or a branch at one
    of them hangs there. Until both are read (`l` on each) and the first's
    turns are open, the resumed one hangs from the first's row. A lineage
    is listed by its newest conversation.

    An import records the conversation it came from, and the turn it
    parted at; one made before imports did is found by its turns, once,
    the first time :graph opens after (this directory's conversations are
    read for it, in the background, and the session moves under its
    conversation when they are; a session found or not is not read for
    again).

    A conversation Shannon itself ran in Claude Code (a `claude-code/…`
    session's turns, its summaries, a turn that stopped or failed) is not
    listed: it is a Shannon session's already, in the tree above. Shannon
    records each one it starts — and, of those, the finished turns' are how
    a session resumes its Claude Code session after a restart. One run
    before Shannon recorded them is found the same way, once: every turn of
    it one Shannon ran. One a session continues stays listed — it is that
    session's root.

OPERATIONS                                                         *graph-ops*

Every operation composes a NEW session and keeps the originals — nothing
here destroys history except `d`. The composed session becomes active, and
a notice on the status line names it. It takes a buffer of its own, shown
in the window that showed the session your prompt was aimed at, as vim
shows a file you edit in the window you were in: that session keeps its
buffer (|buffers|), and whatever it was doing — a reply streaming, a
decision waiting, a `:Send` — goes on there. A session no pane shows says
so: "still answering, in no pane — `:b2` shows it". Only a fresh session
with nothing in it (`:new`) is filled instead, since nobody could tell.
`:compact` and `:continue` are the exceptions on purpose: they REPLACE a
session, which goes on as the result, in its own buffer — and so does a run
of turns squashed in place (`s`, `S`), in the buffer of the session the run
is in. And an `S` whose summary composes unreviewed arrives later, while
you may be typing: it takes the aim of nothing, and a result that stands
beside its sources lands in a buffer of its own in no pane, its notice
saying the `:b N` that shows it.

THE TREE IS A WORKBENCH: every operation below leaves it OPEN, rebuilt so
the session you just composed is in it. Clone three sessions, merge two
branches, squash a dead end — without reopening `:graph` between each.
`<CR>` is the one key that leaves, because it means take me there, and
`:q` closes when you are done. (Run one of these as a `:` command
from a pane showing something ELSE — the manual, say — and that pane hands
its viewport back instead, since it has no way to show you the result.)

    e   edit          Edit exactly the selected thing — a message, a turn,
                      or a session's own turns — in $EDITOR as
                      Messages-API JSON. Unchanged neighbors survive by
                      reference (copy-on-write); an emptied list deletes
                      those turns; malformed JSON cancels. Editing what
                      the model "said" is allowed — history is yours.
    r   rename        Name the selected session: the `:` line opens
                      holding `:rename <id> ` and its name, to edit
                      (|names|). Enter names it.
    :clone            Fork the selected session into a sibling sharing
                      its entire history (O(1) — nothing is copied).
                      This was the `c` key, which now says so.
    :merge            Splice the selected branch's divergent turns into
                      its base at the fork point. `:merge <hash>` names a
                      branch instead of pointing at one. This was the `m`
                      key; it moved so `m` could be the mark prefix.
    :squash           Replace turns with a summary YOU write in $EDITOR.
                      No marks: the selected branch into its base. Marks
                      in one contiguous run: compacted in place. Any
                      other mark shape: GATHERED into a new session
                      holding only the summary. This was the `s` key; it
                      moved beside `:merge`.
    S   :distill      `:squash`'s targets, but the model drafts the
                      summary ([merge] squash_instruction), streaming
                      off-transcript, and it composes when it is done,
                      moving no pane: turns squashed in place replace
                      their session; a summary beside its sources opens
                      in a buffer of its own, and the notice gives its
                      `:b N`. `:distill!` puts the draft to you for review
                      first, in a pane beside its session (or, with
                      `[merge] distill_review = true`, does not), as
                      `:compact!` does (|review|). <Esc> cancels
                      the aimed session's while it is generating; another
                      session's stops with CTRL-C in its pane.
    p   :pick         Compose the selected range or the marked rows, in
                      display order, into a new session — the free-form
                      assembly primitive.
    :return           Collapse a finished child (or any branch) into ONE
                      briefing→answer turn appended to its parent
                      (|return|). This was the `r` key, which
                      renames now.
    d   delete        Delete the selected range, else the marked rows,
                      else the row under the cursor (confirmed unless
                      `confirm_delete = false`). Acts on SESSIONS and on
                      individual TURNS, and a mixed selection deletes
                      both — the confirmation names each count. A turn
                      inside a session that is also going is not counted
                      twice.
                          A session that shares what goes — a fork, a
                      clone, a merge, folded out of sight or not — KEEPS
                      its history: the turns it borrowed become its own
                      copy first, as a git branch keeps commits another
                      branch's deletion would take. The notice says how
                      many kept a copy. Deleting a fork along with what it
                      forked from copies nothing. A message row inside a
                      selected turn goes with it; one on its own is
                      refused (`h` to its turn).
                          Deleting a turn that something later still
                      answers is allowed, but the confirmation says so
                      and carries the WARNING sign. A trailing turn is
                      an ordinary delete: nothing is left dangling.
                          The cursor then stands on the row now in the
                      deleted one's place, as after vim's `dd` — the one
                      above when nothing follows, the fold when its place
                      is folded away.

    Every verb but `e` has a command (`e` takes the selection itself into
    $EDITOR; `:Edit` is the transcript's, read-only), and a command typed
    in the tree acts on what it has picked — its marks, else the cursor's
    row. A verb that takes one session (`r`, `:clone`, `:merge`,
    `:return`, `:rename`, `:description`, `:compact`) refuses marks naming
    several rather than picking one. Bare `:delete` deletes what `d` does,
    turns included; `:delete <id>` names a session, and a turn's id is
    refused there.

WYSIWYG                                                               *wysiwyg*

    An operation on a row means exactly what the row shows. A marked
    session row means the turns drawn under it (a branch row = its
    divergent turns, not its full derived history). `e` on a message
    edits one message. If a guard refuses, the reason appears on the
    status line — never only in a log.

SAVE-TO-APPLY                                                   *save-to-apply*

    Reviews and briefings open in $EDITOR with one contract: SAVING is
    approval. `:wq` applies immediately. `:cq` (nonzero exit) or emptying
    the file cancels. Closing WITHOUT saving is ambiguous on disk — vim's
    `:x` and `ZZ` write only when modified, so accepting an unmodified
    draft looks identical to `:q!` — therefore a generated draft stays in
    its review pane, `closed unsaved`, for `a` to apply as it was or `d` to
    drop (|review|), and no-save cancels a hand-authored seed (it
    was never a summary yet). A generated draft's `:cq`, an emptied file,
    or a save that does not compose keeps it too: it was paid for.

    Model-generated summaries record their sources: the inspector shows
    `distills:` with the ids they stand in for, and the originals stay
    resolvable.

EXAMPLES                                                      *graph-examples*

    `e` on a MESSAGE opens JSON in the Messages-API shape — a role and its
    content. Rewriting a reply, then saving to apply:

        {
          "role": "assistant",
          "content": "Use ripgrep, not grep -r, for the search."
        }

    `e` on a TURN (or a session's own turns) opens an ARRAY of those objects
    in order: delete an element to drop that message, empty the array to
    delete the turn. Malformed JSON cancels and nothing changes.

    `:squash` reads the SHAPE of your marks (|graph-marks|). Three
    cases, each keeping the originals in the graph:

      no marks   Select a branch row and run `:squash`: its divergent turns
                 collapse into one summary, spliced into the base at the
                 fork. "This branch, summarized, back onto the main line."

      a run      Mark a contiguous stretch in ONE session (say turns 4-9)
                 and run `:squash`: they become a single summary IN PLACE;
                 turns 1-3 and 10+ are untouched. Context compaction.

      scattered  Mark turns across sessions, or with gaps, and run
                 `:squash`: they are GATHERED — distilled into a NEW
                 session holding only the summary, sources left intact.

    `S` is the same three cases with the model drafting the summary; it
    composes when written, or waits for your review first with `:distill!`
    (`[merge] distill_review`, |review|). A run squashed in
    place is distilled ON its session, wherever your prompt is: CTRL-C there
    stops it, and while that session answers `S` is refused.

WORKFLOWS                                                    *graph-workflows*

    A few things the tree makes routine that a flat transcript cannot.

    Try two approaches, keep the winner.
        Ask your question, then select that turn and `:clone` — two
        sessions now share the history. Take one down approach A, the other
        down approach B, and compare them side by side here. `:merge` the
        winner's branch back onto the base, or `:squash` the loser into a
        one-line note so its dead end stops costing context. Nothing is
        lost.

    Fan out, then fold in.
        `:child` (|child|) three subagents on three files at once and
        keep working while they run. As each finishes, `:return` folds it into
        your session as one briefing->answer turn whose `distills:` still
        links the child's full transcript — the results without the noise.

    Compact a long session before it gets expensive.
        A large session re-sends its whole history every turn. `:compact`
        (|compact|) has the model write a summary of it, in place,
        as Claude Code's `/compact` does — or mark the early, settled turns
        here and `S` just those. The model now reads the summary, not the
        raw exchange, and the originals stay one fold away in `:graph`.

    Assemble a clean start from scattered good turns.
        Across several dead-end sessions, mark just the turns that landed
        and `p` (pick) composes them, in display order, into a new session
        to continue from. The junk stays behind, still inspectable.
"#;

const SESSIONS: &str = r#"==============================================================================
7. SESSIONS                                                          *sessions*

A session is one line of conversation — an episode in the graph. Shannon
starts FRESH by default (`[tui] resume = "last"` resumes instead); the
previous session is never lost, only left in |graph|.
`shannon --resume <name|id>` starts on that session, as `:resume` does
inside; bare `shannon --resume` (or `-r`) opens :graph to choose one.

LIFE OF A SESSION                                                    *session*

    :new            show a fresh session in this pane (the old one stays
                    on disk). On a session you have not typed into yet you
                    stay put rather than piling up empty episodes — unless
                    another pane is showing it too, where staying put would
                    look like a key that did nothing
    :vnew           a fresh session in a new pane beside this one
    :snew           ...stacked (|window-session|)
    :clone [hash]   fork a session into a sibling sharing all history
    :open <hash>    jump to a session (opens it) or a turn (checks out
                    that point) by short id — ids Tab-complete
    :resume [name]  Claude Code's `/resume`: bare, the sessions to choose
                    from (`:graph`, where <Enter> opens one); with a
                    session, that one — sessions only, a turn is `:open`'s
    :rename <name>  name the session (|names|); `:rename!` takes
                    the name off, bare `:rename` says it
    :description    say in a line what it is; the inspector shows it
    :delete         delete the current session (y/N); `:delete all`
                    prunes everything, asking again, by name, about
                    sessions that go on from Claude Code (their
                    conversations stay in Claude Code); `:delete all
                    shannon-only` keeps those; `:delete <hash>` one by id
    :continue       resume a truncated turn (|continue|)
    :model [name]   set the model THIS session talks to; bare, `:models`
    :models         the model, effort and context budget on one screen
    :effort [level] how hard THIS session's model works (|effort|)
    :setup          choose how Shannon reaches a model (|setup|)

NAMES                                                                  *names*

    A session's id is its identity, and a hash is no name to remember.
    `:rename` gives it one — display only: the id, and everything that
    points at it, never changes.

        :rename auth-fix          name this session
        :rename b3 auth-fix       name another, by any session token
        :rename                   say its name
        :rename!                  take the name off
        :description the token refresh race     one line saying what it is
        :description!             take it off
        :w auth-fix               name it as you save it (|write|)

    With `:graph` focused, the bare forms act on what it has selected, as
    bare `:delete` and `:clone` do; `r` there opens `:rename <id> ` on
    the session it picked — the id written on the line, so Enter names
    that one whatever is marked by then.

    A name is one word — the letters, marks and digits of any script
    (`인증-수정`, `क्षेत्र`), `-`, `_`, `.`; no other ASCII punctuation, and
    nothing invisible — and one
    session's: it is refused when another has it, and when it would read
    as something else, so a token never means two things. Not a pane
    number (`3`), not a buffer (`b3`), not four or more hex characters (an
    id: `cafe`, `beef`), not starting with `-`. After that, every command
    that takes a session takes the name (|buffers|), and `<Tab>`
    completes it.

    It shows where the id did: the pane's border and the prompt's, `:ls`,
    the tree's rows (the name first, the id after it — `[tui] graph_ids =
    "hidden"` leaves the id to the inspector), the inspector's first lines
    with its description, and every message naming a session —
    `auth-fix 1a2b3c4d`, the name and the id the log keeps.

    A name follows the WORK. A clone, a fork, a branch, a child, a pick or
    a merge starts unnamed — it is a new line beside the old. But
    `:continue` and a squash of a session's own turns REPLACE the session
    you are in, so the name goes on with you, as a rebase moves a branch's
    name; the original stays in `:graph`, unnamed. The replacement is
    written at once, so quitting cannot lose it and its name.

WHICH MODEL ANSWERS                                                    *model*

    `:model` is per SESSION, not per program, and that is the point: put
    two sessions side by side (|window-session|), set one to
    `:model opus` and the other to `:model haiku`, and one prompt marked
    to both answers the same question twice. Set both to the SAME model
    and you are measuring how much the answer varies on its own.

    Short names — `stub`, and `haiku`, `sonnet`, `opus`, `fable`, each the
    latest of its kind; the older ones still served as `sonnet-5`,
    `opus-5`, `fable-5`, `opus-4-8`, `opus-4-7`, `opus-4-6`, `sonnet-4-6`
    and `haiku-4-5` — Tab-complete.
    A full model id is accepted verbatim, so a model this build has never
    heard of still works; the table is a convenience, not a gate. A bare
    `:model` opens `:models` to choose from (|models|); the status
    line names the model that answers, which is the stub when there is no
    key.

    `stub` is the offline canned responder — no key, no network, no spend
    (|stub|).
    It is a model name rather than a separate switch because it answers the
    same question (what answers me), and because being per-session is the
    useful part: point one pane at `stub` while its neighbour keeps talking
    to a real model, and you can exercise the interface without paying for
    the half of it you are not testing. Turns it produced record
    `canned-stub` in their provenance, so a session you come back to says
    plainly which answers were real.

    `[tui] model` in your config sets what NEW sessions start on. Changing
    a session's model takes effect on the NEXT turn: turns already
    committed keep the model that produced them, because provenance is
    history rather than a preference, and `:graph` will tell you which
    model wrote which turn. A turn in flight refuses the change — it was
    sent under the old model, and sealing it under another would say the
    wrong thing about what answered.

    Child sessions inherit their parent's model unless `:child -m <name>`
    says otherwise (|child|).

    `:new` and `:open` both answer the same question — what session does
    THIS pane show — so both work from a |graph| or |help|
    pane and hand the viewport back on the way. `:new` is the way home
    when a viewer has borrowed your only transcript.

THE STUB                                                                *stub*

    The stub says back what you wrote, so the interface can be tried with
    no key, no network and no spend. It also asks for the tool calls you
    write out: a fenced block whose first line is ```tool and the tool's
    name. For `bash` the block is the command, as it stands; for any other
    tool it is the input, as JSON:

        ```tool bash
        ls -la
        ```

        ```tool Read
        {"file_path": "notes.txt", "limit": 20}
        ```

    Send that and the stub asks for each call as a model would: the gate
    judges it, the pane asks where it would (|decision|), and what
    runs is recorded. Once the calls are answered, the stub says what each
    returned AS THE MODEL WAS HANDED IT — a secret by its handle, a
    refusal's reason, a file reached through a link named. `:analyze`
    judges a line and runs nothing (|analyze|); this is the whole
    path, end to end.

    A block it cannot read (not JSON, or not an object) is said, and no
    call is made, the readable ones beside it included. A ```bash block is
    code, not a call.

THE TUTOR                                                              *tutor*

    `shannon tutor` (from a shell) teaches Shannon in Shannon. Its lessons
    are a page you may change, as vimtutor's copy is: every key edits it,
    undo included, a yank there is a `p` in a prompt, and nothing is
    written. Its <CR> sends nothing; the prompt below it does, to the stub
    (|stub|), so a lesson's tool call is asked for and answered
    with nothing spent. `:models` still reaches a real model.

    It runs in a directory of its own, made fresh under your system's
    temporary directory, so a call a lesson allows touches nothing of
    yours, and its chats are kept in memory and go when you quit. Like
    vimtutor, it reads none of your config files: the lessons describe the
    Shannon you get.

HOW HARD IT WORKS                                                     *effort*
                                                                      *models*

    Effort is how much work the model is asked to put into a reply:
    `low`, `medium`, `high`, `xhigh`, `max`. Higher thinks longer and costs
    more. Opus and Sonnet 4.6 take all but `xhigh`; Haiku 4.5 takes none.

    Effort is the SESSION's, as its model is: `:effort` sets this
    session's, it is kept with the session, and it comes back when you
    resume or `:open` it. Two panes can compare efforts the way they
    compare models (|model|). A child starts at its parent's.

        :effort max            this session's (Tab completes the levels)
        :effort default        follow `[turn] effort` again
        :effort                choose in a panel at the bottom
        :set effort=high       the DEFAULT, for sessions that never chose

    `[turn] effort` in your config (|config|), or `:set effort=`
    for this run, is the default: what a session that never chose one
    gets. A `CLAUDE_CODE_EFFORT_LEVEL` in the environment Shannon starts
    from sets it at launch, over your config, as a `:set` would (so a
    `:reload` whose file names `[turn] effort` takes it back); the launch
    says so, and says when the value is not a level. A session's own
    outranks the default, and `:set` says so when the one in front has its
    own. With no default either, none is sent: through the
    API the model works at its own (high on Sonnet 5.5 and Fable 5.1,
    medium on Opus 5.5); through Claude Code, at YOUR Claude Code setting
    (its `/effort`).

    A bare `:effort` is vim's command-line window (`q:`): a few lines at
    the bottom, the session still on screen above them, one `:effort` you
    could type per line and the cursor on this session's. `j`/`k` (and
    `gg`, `G`) move; <Enter> runs the line under the cursor; `q` or <Esc>
    closes it having run nothing. Its bar takes the status row, its keys
    first. Over a decision waiting for you its bar says so — it opens over
    one, as `:models`, `:setup` and `:memories` do, and nothing it sets
    reaches the turn waiting on it: that turn goes on as it began, and a
    new model for its session is refused until it ends. Over a y/N, which
    takes your next key, none of them opens. It sets the session it opened
    on, and if another comes to the front meanwhile, <Enter> refuses
    rather than set that one.

    A session on a model that does not take its level is sent none, and
    `:effort` says so — the level stays for when it moves to a model that
    does. The status line shows the effort a turn would be sent beside the
    model (`opus max`), and nothing when none is. A turn records the
    effort it was sent, which `:graph`'s inspector reads back. A change is
    the NEXT turn's: a tool round keeps its turn's effort, and so does a
    prefill `:continue`'s request (|continue|); an instruct
    `:continue` is a new turn, at the session's effort now. A session
    composed from another — by `S`, a prefill `:continue`, a squash — keeps
    that session's own effort as it is when it lands, one set meanwhile
    included; a turn nothing was sent for (typed by hand, returned,
    imported) records none. Changing the level actually sent re-caches
    the prompt on the next API turn.

    Through Claude Code the level goes as `claude --effort` on every turn.
    By Claude Code's docs (10-04, not checked here): it does not save the
    level in its own session, so `claude --resume` of a Shannon session
    runs at your Claude Code setting; and a `maxEffortLevel` cap in its
    settings outranks Shannon's level without a word. Its
    `CLAUDE_CODE_EFFORT_LEVEL` would too, so Shannon keeps it from
    `claude`: it is Shannon's starting level instead (above).

    `:models` puts the three choices on one screen: this session's model
    and effort, and the context budget the `ctx` gauge measures against
    (|cost|) — the run's, as `:set` sets it. The models are
    grouped by how they answer — the API, Claude Code, the demo — under a
    rule each. The API's older models are a FOLD, shut on one line —
    `+ older models (8)` — and open (`-`) when the session is on one:
    vim's `zo`, `zc`, `za`, `zR` and `zM` open and shut it from anywhere
    in the list, and on its line <Enter> or `l` opens it and `h` shuts it
    (`h` from inside too, back to its line). `j`/`k` move — down the
    models, then on to the effort and the context rows; `h`/`l` change a
    row; <Tab> jumps between them. The screen says what <Enter> would
    change before you press it — each model by its id, `claude-sonnet-5`,
    not `sonnet-5`, as the effort note does — and applies it through
    `:model`, `:effort` and `:set` — to the session it opened on, as the
    panel does. While a turn runs, an <Enter> that changes the model is
    refused whole, rather than applying the half the others allow. <Esc>
    or `q` leaves, changing nothing.

SETTING UP                                                             *setup*

    `:setup` (or `shannon setup` from a shell) chooses how Shannon
    reaches a model. It opens by itself on a first launch — no key
    anywhere and no config file — and when a session is switched to a
    model that is not set up: `:model opus` with no key opens it on the
    key, `:model claude-code/sonnet` with no login on Claude Code. Esc
    leaves any screen, and leaving changes nothing.

    Anthropic API   Fully featured. Paste a key, then Use (it answers
                    now; nothing is written) or Save (written to
                    `~/.config/shannon/api_key`, readable by you alone).
                    With nothing pasted, both act on the key in use — so
                    an `ANTHROPIC_API_KEY` can be saved. At launch,
                    `ANTHROPIC_API_KEY` and `SHANNON_API_KEY_FILE` are
                    read ahead of a saved key.
    Claude Code     Limited: you get the TUI features, but Claude Code
                    runs the tools (|claude-code|). Uses your
                    `claude` login.
    Demo mode       The canned stub — no agent, no key, no spend.

    Chosen from the menu, the choice is your default: it is written to
    `[tui] model` in your config — that key alone, the rest of the file
    as it was — and this session switches to it. Opened by a `:model`
    switch, setup only makes that model work.

CLAUDE CODE AS A PROVIDER                                      *claude-code*

    A `claude-code/<model>` session is answered by Claude Code
    (`claude -p`), under your own `claude` login — Shannon's API key is
    never handed to it. Claude Code runs its tools, and Shannon's gate
    decides every call before it runs: shell commands, reads and writes
    by your rules; a subagent, an MCP tool or the web asks every time
    (`[claude_code] web` can allow the web).

    The gate checks that it is in the loop. A call that worked without
    asking it stops the turn, which says so: a Claude Code setting such
    as `disableAllHooks` — in your settings or the project's — removes
    the gate. And every call runs in the directory Shannon started in;
    one Claude Code would run anywhere else is refused.

    It answers only for a call Claude Code's own output named. Every
    shell Claude Code starts could reach the gate, so a question about a
    call never named is refused, and said — it never reaches you. Nothing
    documents that a background task's calls are named: one that is not
    is refused (Claude Code's model is told to run it in the foreground
    instead).
    `[claude_code] background_tasks = false` turns background work off
    (agents, and Bash's run_in_background too), so it runs where each call
    is shown and decided here.

    What you give up, against the API:

    - Claude Code's model sees its tools' output AS IS. Shannon's
      redaction is not between them, so the gate is the only layer, and
      a credential path is refused by default (`[claude_code]
      path_policy`) — one a command names; one it only MAY name (a `$`,
      a `${X:-…}` default, a glob aimed at it) asks, naming it.
    - Only a Bash, Write or Edit call can be edited. For any other, `e`
      is struck through in the decision pane, and pressing it says why.
    - If you run Claude Code's own sandbox: a Bash call can ask to run
      outside it (`dangerouslyDisableSandbox`). Such a call asks every
      time: the pane shows the command analysed, says it would leave the
      sandbox, and offers no rule — a rule that allows the command does
      not allow it there. A deny still denies; a `:child` holds on it.
    - A turn is slower.

    Claude Code reads your CLAUDE.md files, as it always does, and the
    project's own Claude Code settings apply — its hooks and MCP servers
    included. Shannon adds nothing to its system prompt.

    So a directory is TRUSTED before Claude Code runs there, as Claude
    Code asks when you run it yourself — and, as it does, before you start:
    a screen asks when Shannon opens on a `claude-code/…` session in a
    directory it does not trust, and when setup or `:model` brings Claude
    Code in. Three answers:

        1  Yes, and remember     here and under here, from now on
        2  Yes, this time only   until Shannon quits
        3  No                    and <Esc>, `q`, CTRL-C — the safe keys

    A no leaves the question for later: the first prompt you send asks
    again, and a no there leaves your message in the prompt. A yes asked
    up front never sends what you have typed — you did not press <Enter>
    on it; a yes asked at the send sends it.

    Anything else that would start Claude Code in an untrusted directory
    (`:continue`, a `:child`, a queued prompt) is refused and says so.
    `:trust` says what is trusted, `:trust .` trusts this directory (kept),
    `:trust ~/work` another; `:untrust` stops trusting this one, kept or
    for this run. Kept in `trusted_dirs` in Shannon's state directory,
    readable by you alone; "this time only" is never written.

    Trust says whether a directory's own files may configure what runs —
    not what a command may touch: the rules and the inside default say
    that, trusted or not. So it also decides whether Shannon reads what the
    directory's .shannon/ holds — its config, models and skills
    (|config-project|) — whichever backend answers: `:trust` reads
    them at once, `:untrust` takes them back off.

    `:import <id>` brings a Claude Code conversation in as a session in
    this pane — by its id, from this directory's conversations or any
    other's, or `:import <file>` by its path. An id is its first 8
    characters or more, as Shannon's own are, and Tab lists this
    directory's (another's, never). It
    brings what `claude --resume` would: from the latest compaction on —
    its summary, then the last messages that compaction kept, then what
    followed; `:import --all <id>` reads back to the first prompt. Every
    tool output is scrubbed on the way in, and the import says what it left
    out (thinking, images, Claude Code's own entries). The session is yours
    from then on — continue it on any model. `:graph` lists them too, below
    the tree, and <Enter> there imports one (|graph-claude-code|).
    Either reads only where you let Shannon read Claude Code's
    conversations, and asks first where you have not; `:unread` takes the
    leave back.

WHAT A TURN COSTS                                                       *cost*

    The status line carries the model answering (and its effort, when one
    is sent — |effort|), then `ctx N%` — how full its window is —
    then `tok ↑N ↓N` once a session has spent anything — the whole prompt
    sent, and the reply received, counted by the provider rather than
    estimated by us. `:graph`'s inspector breaks the same numbers down per
    turn, which is where you look when one answer cost five times its
    neighbour.

    `ctx` turns yellow at 70% and red at 90%, so the limit is seen coming.
    `[tui] ctx_colors` says so, as stops — `["70:yellow", "90:red"]`, each
    colour from its percent up, a terminal colour's name, number or
    `#rrggbb` — so more stops make a stepped gradient, and `[]` none.

    A reply is bounded by the MODEL's maximum, not by ours — `[turn]
    max_output_tokens = 0` is the default and means exactly that. Set a
    number if you want a hard bound on what one answer can cost. Either
    way, a reply that hits its ceiling says so on the status line and
    `:continue` picks it up; it is never cut off in silence.

    `ctx` measures against the model's context window: 1M on every model
    the short names name, 200k on Haiku 4.5. `[turn] max_context_tokens`
    sets a smaller budget to work inside, and there are two good reasons
    to. A long context is re-sent on every turn, so it costs more the
    longer it gets; and long contexts get less reliable well before they
    get full. A million tokens is a lot of rope.

    When the budget is YOURS the gauge marks it — `ctx 83%*` — because
    83% of a bound you chose means something different from 83% of the
    model's hard limit. Setting it ABOVE the model's window does not buy
    room the model does not have; it only makes the gauge optimistic.

    `ctx ~6%` is an ESTIMATE. A window is measured on one history; after
    `:compact` (or any change to what came before the last turn) no request
    has measured the history you now have, so `ctx` is its text at four
    characters a token — low by the system prompt and tool definitions. The
    next answer measures it again and the `~` goes.

    `tok ↑` and `ctx` answer different questions. The arrow is CUMULATIVE:
    everything this session has ever sent, which is what you paid and
    grows without limit. `ctx` is how full the window is RIGHT NOW, which
    is one exchange — the last prompt plus the reply it produced, because
    that is what the next request re-sends. A long session can read
    `↑200000 · ctx 6%` with nothing wrong.

    `↑` is the WHOLE prompt. A conversation re-sends everything said so
    far on every turn, so that number grows with the session and not with
    what you just typed; a long thread whose next question is three words
    still sends the thread.

    PROMPT CACHING is what stops you paying full price for that, and it
    is on by default. Each request asks the provider to keep its prompt,
    and the next one — or a branch taken from any earlier turn, which is
    the shape |graph| makes cheap — re-reads what they already
    hold instead of sending it again. Reads cost a tenth of fresh input;
    the writes that make them possible cost a quarter more. A prefix
    written once and read once has already paid for itself.

    You see it in two forms. The FIRST request that caches has nothing to
    read yet, so it reports what it put away: `tok ↑6149 (6147 stored)
    ↓2312`. Every request after that reads it back, and the reading is
    what you want: `tok ↑12480 (94% cached) ↓310`.

    Silence means neither happened, and the usual reason is honest: a
    prompt shorter than the provider's minimum is not cached at all, no
    error and no surcharge. The minimum is per model and Haiku 4.5's is
    the highest of them at a few thousand tokens, which is why the model
    is named on the status line beside these numbers — a short Haiku 4.5
    session showing no cache segment is working correctly, and you can
    only tell that if you can see which model is answering.

    Entries live five minutes by default, and every read restarts the
    clock — an active conversation stays warm indefinitely. `[cache] ttl
    = "1h"` buys an hour instead, at twice the write price; it is for
    coming back to a branch point much later, not for a session you are
    sitting in. `[cache] enabled = false` turns the whole thing off.

    One honest limit: several children spawned at the same instant off
    one turn (|child|) all miss, because an entry only exists once
    a reply has begun. Spawn one, let it answer, then spawn the rest.

WHAT ENDS A TURN BY ITSELF                                         *timeouts*

    No clock cuts a reply for being long. Its ceiling can (`max_tokens`,
    |cost|; `:continue` resumes it), going quiet can, and so can
    you. The time bounds a turn meets, and what you see when one fires:

    API, reaching it      30 s to connect. "could not reach the API
                          within 30 s".
    API, a reply          5 minutes with NOTHING arriving, before the
                          first word or between two. "no data from the API
                          for 300 s — the reply was taken as stalled". What
                          had arrived is kept, marked interrupted, and
                          `:continue` picks it up (|continue|). A
                          very long prompt at a high effort can take a
                          while to begin; five minutes of silence is the
                          limit.
    Claude Code, a turn   No bound of Shannon's: by Claude Code's docs it
                          keeps its own watch on a quiet connection; and
                          you can stop a turn.
    Claude Code, a call   A decision left unanswered for 59 minutes is
                          refused — a no, never a yes — under the hour
                          Claude Code allows its gate (|decision|).
                          `:ls` counts it down: `needs you · 52m left`.
    A command             `[exec] timeout_secs`, 120 by default, for a
                          command Shannon runs (|config|).

WHERE YOUR NEXT MESSAGE GOES                                  *prompt-target*

    The prompt commits to ONE session, and STANDING IN A TRANSCRIPT PANE
    AIMS IT THERE — `<C-w>w`, `<C-w>h/j/k/l`, `<C-w>p`, a click, all of
    them. Act on the thing you are pointing at. A pane lent to a viewer
    is not a target: you went there to read, not to talk.

    Two things show you the aim, and you should learn to read both:
      - the target transcript's border wears a brighter accent than the
        other unfocused panes;
      - the prompt's title reads `prompt → <id>` whenever the answer is
        not obvious (more than one transcript pane, or none showing it).

    `:delete`, `:clone`, `:continue` and `:child` act on the session you
    are aimed at, for the same reason: `delete THIS session` has to mean
    the one on screen.

    If the pane you were aimed at closes, the prompt re-aims at one that
    is still visible and SAYS SO on the notice line. A target you cannot
    see is how a message reaches the wrong conversation.

THREADS                                                              *thread*

    :reply          reply to the turn under the cursor

A THREAD IS A BRANCH. Put the cursor on an earlier turn in the transcript,
`:reply`, and your next message continues from THERE instead of from the end —
the conversation forks, and both lines stay. That is all threading is here: the
graph already stored it, `:graph` could already do it, and what was missing was
a way to start one where you are reading rather than from the tree.

It costs nothing until you speak. Checking out is lazy, so the branch becomes
real on your first message and the shared prefix stays shared by reference.
Fold a thread back with `:merge`, `:squash` or `:return` (|graph|), or
keep both and compare them side by side (|window-session|).

FORKS                                                                  *fork*

    :fork           split the pane, the conversation going on in both
    :vfork          the same, side by side

A side question without touching the conversation you are having — Claude
Code's `/btw`, except that it is a whole conversation and you can keep going.
`:fork` clones the session in the pane you are in (or the one your prompt goes
to, from the prompt or a viewer) into a new pane beside it, focused, the
prompt aimed there. Ask; the original never sees it. `:q` closes the pane and
leaves the fork in |graph|; `:return` brings its result back into the
original as one turn (|return|). A fork is a clone, not a branch, so
`:merge` refuses it. `:fork` is a pane's verb, like `:split`: it forks what a
pane shows, never a row selected in `:graph`.

It shares the whole history by reference, so it costs nothing until you
speak — and when you do, the first request's prefix is the same history the
original sends, so the provider's cache serves it (within the cache's
lifetime) and you pay for the new words. A session not yet on disk is written
first; one with no turns has nothing to fork. The fork starts unnamed
(|names|).

COMPACTING                                                          *compact*

    :compact        this session's history into one summary the model
                    writes, replacing it when it is written — as Claude
                    Code's `/compact` does
    :compact {text} the same, told what to keep: `:compact keep the API
                    decisions` — as `/compact {text}`
    :compact --keep N   keep the last N turns as they were, this once
    :compact -- {text}  {text} as words, though it begins with `-` or is
                    a number (a bare number is refused: it was `--keep`)
    :compact!       the other way for once: the summary waits for your
                    review first (or, with `compact_review = true`, it
                    does not)

    [merge] compact_keep = 0         last turns kept as they were (none)
    [merge] compact_review = false   `true` reviews every summary first
    [merge] distill_review = false   the same, for `S` and `:distill`

A long session re-sends its whole history with every turn, so it grows dearer
and, near its window (`ctx` on the status line), worse. `:compact` asks the
session's model to summarise its turns — all but the last `[merge]
compact_keep` (0) — with `[merge] squash_instruction`, the same request as `S`
in |graph|, your words after it added for that one summary; and the
summary REPLACES them: the session goes on as the summary and any kept turns,
under its name, and the original stays in `:graph`, whole. A relaunch opens
the compacted session, as `claude --continue` does after `/compact`. The
result does not move your view when it lands — you may be reading something
else by then. Reviewed, the review is `S`'s (|review|): it waits in a
pane beside the session, and `<CR>` there opens it in $EDITOR, where `:wq`
applies it. Typed with `:graph` focused, it compacts the session the tree has
picked, as `:rename` does; anywhere else, the one your prompt goes to.

On a Claude Code session the summary is asked of the session Claude Code holds
for exactly those turns, forked, so it reads them whole, and the conversation
it continues next is untouched (|claude-code|).

Claude Code also compacts on its own near its window's end. When it does, it
is said on the status line: its model works from its summary from there, while
this session keeps every turn. `[claude_code] auto_compact = false` stops it
(its window then fills until a request is refused, and `:compact` is yours).
After any compaction `ctx` reads `~N%` — estimated — until the next answer.

BRANCHES                                                             *branch*

    Checking out a past turn (<CR> on it in :graph, or `:open <turn>`)
    detaches onto that point — in a buffer of its own, where `:open` lands,
    as vim's edit command opens a file: what that window showed stays a buffer
    (`:ls` lists it, `:b N` brings it back), its work running on, and the
    prompt is aimed at the checkout. The transcript pane's title reads
    `— at <turn>` after the session id, and landing says what that means:
    `at 515d5dd3 (turn 2 of c3f7e628) — sending continues from here as a
    branch; leaving keeps nothing`. CONTINUING from there forks a real
    branch: the shared prefix stays on the original by reference; only new
    turns are yours, and the title then reads `— branch ← <turn>`, so you
    always know you are off the main line — and you can keep the trunk
    open beside it (|window-session|). Branches merge back with
    `:merge`, `:squash` or `:return` (|graph|).

HEAD AND LAZINESS                                                       *head*

    HEAD is the session a fresh launch resumes (`resume = last`), flagged
    `(HEAD)` in :graph. It moves when you `:open` a session, and when a
    session is first written while you are in it, or replaces the one HEAD
    names (a `:compact` of it) — never because something finished in a pane
    you have left. HEAD is not where you are: that is the window's title
    and the prompt's `→`. A clone or checkout you open but never continue
    is not written, so it leaves no trace on disk, and quitting does not
    ask about it (|quitting|).

PERSISTENCE                                                          *persist*

    Sessions live under $XDG_STATE_HOME/shannon/projects/<hash-of-cwd>/ as
    append-only JSONL, one file per session, integrity-checked on load
    (each turn's content hash is re-verified). A branch's file stores
    ONLY its divergent turns. Files are owner-only (0600/0700). Delete a
    session and its file is gone — everything else is never rewritten.

CHILD SESSIONS                                               *child* *subagent*

A child session — also called a SUBAGENT — is a background conversation
spawned off your current turn. It is an ordinary session in every way
(inspect it, open it, merge it); "child" describes how it was born, not what
it is. Children run while you keep working; nothing they do touches your
transcript unless you compose it in.

SPAWNING                                                          *child-spawn*

    :child <prompt>          Normal mode
    /child <prompt>          typed as your message in Insert mode (`/` is
                             not special in insert — the prefix is just
                             text that Shannon recognizes at submit)

    Flags come first, CLI-style, space- or =-delimited:
        -m <model>, --model=<model>    run the child on another model
        --stop <id>                    cancel a running child (a flag, not
                                       a `stop` word, so a briefing that
                                       STARTS with "stop" cannot be
                                       misread)
    With flags but no prompt (or no arguments at all), $EDITOR opens for
    the briefing — save-to-apply (|save-to-apply|).

WATCHING AND RESULTS                                              *child-run*

    The status line counts running children; in |graph| the child
    hangs off the turn that spawned it with a ◆ head and `▶ running`
    while mid-turn. When it finishes (announced on the status line), open
    it like any session — or `:return` it:

RETURN                                                                *return*

    `:return` — on the child `:graph` has selected, the session you are
    in, or one named (`:return b3`) — collapses it into ONE turn appended
    to its parent: the
    briefing as the user side, its final reply as the assistant side,
    with `distills:` pointing at the child's full transcript (one hop
    away in the inspector, forever). Merge/squash/pick remain available
    when you want more than the summary shape.

WHAT A CHILD MAY RUN                                            *child-tools*

    A child can run shell commands, and the bar is deliberately not the
    one your own turns face. **A child never asks you anything.** That is
    the whole difference: a background session you spawned and walked away
    from must not be able to put a (y/N) in front of you, because a stream
    of questions you did not go looking for is how consent gets worn down.

    So it runs only what is safe with nobody watching — a command every
    stage of which merely OBSERVES state, with no rule and no credential
    policy having anything to say about it. That is stricter than merely
    being allowed:

        a rule you wrote (`allow git *`), or one `A` taught against a
        decision you read once, was written thinking about your own work
        at the keyboard, not about an agent running unattended.

    It does not carry over. Reading a credential does not either, even
    though reading is observing (|secrets|).

    Anything else HOLDS. The child stops, says once what it wanted, and
    waits — its turn stays open and it keeps its session's slot. It does
    not route around the block and it does not try something else, not
    even the next command in the same message: stopped means stopped.
    On Claude Code, which runs its own tools, two things differ: a call it
    asks for while held — a parallel one, a subagent's — is refused at
    once, said; and the call it holds on waits at most the hook's 59
    minutes, then is refused, and the child carries on, refused.

    A held child is never reported as a running one. Its `:graph` row
    reads `held — needs you` with the held marker (`|` by default, see
    `graph_glyphs`), the status line counts it apart from the children
    that are working, and any refusal it causes names the command and
    points at `:child --stop <id>` rather than telling you to wait —
    waiting is precisely what will not help, since the thing holding the
    session is waiting on you.

    There is no in-app approval for a held child yet: it has no decision
    pane, by
    design (see above). Your options are to deal with the thing yourself
    and let the child go, or stop it.

    In practice the reach is set by two things, and neither is the
    policy. First, which programs Shannon can CLASSIFY: an unmodelled
    program is unclassifiable, so it holds even when it plainly only
    reads. Second, whether every path in the line can be ACCOUNTED for:
    a glob, a variable or a `..` means the credential check cannot tell
    which file is meant, and a check that cannot look is not a check
    that passed. So `cat notes.txt` runs and `cat *.txt` holds.

    Both sets grow, and growing them is safe precisely because the
    default is closed.

GUARDRAILS                                                            *agents*

    `[agents]` bounds recursion and fan-out — children spawning children,
    and how many children one session may spawn. Only SPAWNS count:
    your own branches and clones never eat the budget. At a limit,
    `on_limit` picks the posture: "confirm" (y/N with the reason, the
    default), "block" (refuse, visibly), or "advisory" (spawn and
    say so). See |config|.

Stopping a child (`:child --stop <id>`) keeps whatever it had already
produced, sealed and marked (|lifecycle|); an untouched child
session is deletable in :graph like anything else.

TURN LIFECYCLE                                                      *lifecycle*

A turn does not simply exist or not — it ends some WAY, and the way is
recorded. Knowing how a turn ended is what lets you resume it, trust it, or
redo it — and Shannon never throws away work you were already billed for.

STOP REASONS                                                    *stop-reasons*

    Every finished turn records why the model stopped; the inspector
    (|graph-inspector|) shows it verbatim.

      end_turn       finished on its own — a whole, ordinary reply
      max_tokens     hit the response-length cap before finishing
      stop_sequence  emitted a configured stop string
      tool_use       paused to call a tool (|permissions|)
      pause_turn     a long turn the server paused for continuation
      refusal        the model declined to continue
      interrupted    a partial kept after a stream was cut short (below) —
                     Shannon's own reason, not one the API returns
      other:<x>      any future API reason, passed through untouched so a
                     new vocabulary word is never silently dropped

    end_turn is a complete reply. max_tokens and interrupted are genuine
    but UNFINISHED — the resumable pair (|continue|).

INTERRUPTING                                                      *interrupt*

    A reply can stop before it finishes — the backend drops mid-stream, or
    you `:child --stop` a running child (|child|). Whatever already
    arrived is SEALED into a real turn: billed tokens bought real text, so
    the text is kept, marked `interrupted`, and left in history like any
    other turn — never a silent draft, never discarded. Read it, edit it
    (|graph-ops|), `:continue` it (below), or delete it.

    If NOTHING arrived yet — you stopped it in the first moment, before the
    first chunk — there is nothing to seal, and the turn is dropped whole:
    your question goes back out of the conversation and off the transcript,
    so the two agree and the model is never left half-told. The status line
    says which happened, and only offers `:continue` when there is something
    to continue.

    Two such turns stay on screen, though the conversation drops them: one
    whose request FAILED before any reply, and one stopped while a command
    it asked for was running (its round never finished, so nothing can be
    sealed). The failure, or the record of what ran on your machine, is
    yours to read. The whole turn is drawn in the danger colour (`ui.danger`),
    your words with it, and ends saying so and why:

        [error: rate limited]
        [not in the conversation: the request failed]

    The model's next turn does not see it. It is no turn: `]]` passes it,
    `:reply` on it says so, and a mark on it is not saved
    (|mark-motions|).

                                                                    *CTRL-C*
    CTRL-C INTERRUPTS, and never quits — vim's rule. It cancels the
    innermost thing you are in: an open `:` line, a pending (y/N), or
    whatever is RUNNING, in that order.

    That third rung stops what runs in THE SESSION YOU ARE AIMED AT — and,
    while panes are marked, in every marked one too, so one press stops
    what one multi-send started. There it stops the work: the reply, any
    command its turn asked for that is still running (killed part-way — it
    may have done some of its work), a `:Send` into it, a distillation and
    a continuation. A `:tool` you typed yourself is not the model's work
    and runs on until it ends, or until `:qa!`. Other sessions run on:
    they are independent, and a key pressed in one does not end another's
    work. Aim there and press it again — with nothing to stop here, CTRL-C
    says what runs elsewhere and where. A child works in a session of its
    own (|child|): CTRL-C in that session stops it, or `:child
    --stop <id>` from anywhere.

    In the DECISION PANE it stops nothing: the call there may be another
    session's than the one you are aimed at, so a press there would stop
    work that is not the call in front of you. It says so, and where that
    session's stop is — the prompt, when your prompt goes to it; else its
    own pane, named — and that `d` denies the call.

    The status line says what CTRL-C will do — it reads `^C:stop` while
    something runs that it would stop, and nothing of it when nothing does
    — and names what is running beside it (`answering…` while this
    session's reply streams, `running: <cmd>` while a command of its runs,
    `N sessions running` when another session works too, `N sessions
    answering` for `:Send`s, `N children running`, `distilling`,
    `continuing`). With
    nothing to interrupt anywhere it tells you how to leave (`:qa`) rather
    than leaving for you, which is also what vim does with the question it
    is asked most often.

    Stopping is not undoing. Whatever streamed was billed, so it is sealed
    as an `interrupted` turn and `:continue` picks it up. There is no
    per-turn discard: `:delete` drops the WHOLE session, in-flight turn and
    all, so branch past a bad turn instead (|graph-ops|).

    `:new` and `:open` do NOT stop it — the turn keeps running, and keeps
    billing, in the session it started in (|window-session|).

CONTINUING                                                          *continue*

    `:continue` resumes the last turn from where it stopped, when that turn
    ended `max_tokens` or `interrupted`; it refuses a whole turn, with the
    reason on the status line. The resumed text folds into ONE turn, so
    history reads as if the reply had arrived in a single piece. Continuing
    always acts on the session's real tip, never on a point you were merely
    inspecting (|head|).

    `[continue] strategy` picks how (|config|):

      instruct   (default) A fresh request asks the model to carry on from
                 where it stopped. Works on every model; because the seam
                 is a new request, wording can shift slightly at the join.
      prefill    The partial reply is handed back as the opening of the
                 assistant's message and extended in place — an exact
                 splice. Only some models accept a trailing assistant
                 message; one that refuses leaves the turn untouched with
                 a note to switch strategies.
"#;

const DECISIONS: &str = r#"==============================================================================
8. DECISIONS                               *decisions* *permissions* *security*

The model can propose a shell command. It never simply runs: the proposal
stops at a DECISION PANE, you read what it would actually do, and you answer.
The thesis is that verification has to be CHEAP, or it stops happening — so
the pane's job is to make the dangerous part obvious in one glance, not to
make you audit a wall of shell.

Be clear about the trust model: an allowed command runs AS YOU, in your
shell, in your working directory. There is no sandbox yet. Shannon's controls
decide WHETHER a command runs, not what it can reach once it does.

What answers a call before you are asked is in |rules|; what never reaches
the model at all is in |secrets|.

THE DECISION                                                         *decision*

    A command's keys. A summary's review has keys of its own, and its `d`
    drops paid work: |review|.

    d  q             deny — the model is told, and re-plans
    <Esc>            nothing: pressed by reflex, it answers no call
    a                allow ONCE; standing policy is unchanged
    A                always: opens `:allow <rule>` for you to edit; Enter
                     teaches it for the session and answers this call
                     (|rules|). Never a rule holding a credential:
                     it says where the value sat instead
                     (|secrets|)
    e                edit the command, then decide again on the result
    n  N             cycle: the next call waiting (`n`) or the last (`N`),
                     this one set aside, edit and all — it answers nothing
    j  k  gg  G      move through the report — it reads like any page
                     (|window-read|): counts, `H` `M` `L`, `zt`
    v  V             select in the report, as on any page; `y` copies.
                     While a selection is up NO key answers — `vap` selects
                     a paragraph, it does not allow — and the last row says
                     so; `<Esc>` or `y` ends it
    y  Y             copy from the report, as on any page. The keys after
                     `y` are its own — `yap` copies a paragraph, it does not
                     allow — and the last row says so until it ends or
                     `<Esc>` drops it

    The decision opens under the conversation that asked: the pane you are
    standing in if it shows that session, else the first pane that does.
    It does NOT take your cursor: what you were typing stays where you
    typed it, and no key meant for it can answer a call you have not read.
    Nor does closing a pane drop you into it: focus that would fall there
    goes to the prompt. You enter it only on purpose.
    `{n}<C-w>w` reaches it, `{n}` the number on its border.

    A session NO window shows has no pane: its call waits, and its turn
    with it, said at the status row's right — `decision hidden · :decide
    3` (${alert.hidden}, |statusline|) — and on its `:ls` line (`needs
    you`). `:decide 3` goes to it: the session shown here if no window
    shows it, as `:b 3` would, and you in its decision pane, its keys
    waiting a moment as for a call just shown (below). `:decide` alone goes
    to this session's, else the first with no pane, else the first on
    screen. A pane opens for it, too, as soon as any window shows the
    session again. A command you type with `:tool` shows its session
    first, so its call asks under it.

    Each conversation's decisions are its own, in its own pane: two
    sessions asking at once have two panes, each under its conversation.
    A pane holds ITS session only: a prompt for that session waits behind
    it until it is answered, and every other session goes on — sessions
    run apart, each its own agent. The title names the session whose call
    it shows — `run this? · fix-parser 3f2a91c0` — so which conversation
    you are allowing is said, not only where the pane opened.

    A pane stays where it opened. Moving your cursor, or opening the
    conversation in another window, moves nothing; only the window it sits
    under turning to another session does, and then it opens under a
    window still showing its own — with its keys waiting again, as for a
    call just shown (below), since what is under your cursor changed — or,
    with none, its call waits, as above.

    A session's calls are asked in the order they came. While others of
    its own wait, the title says how many — `2 queued` — and the last row
    adds `n/N cycle`: `n` shows the next one and sets this one at the back,
    `N` shows the last one and sets this one at the front, answering
    nothing — each undoes the other, and either, pressed in turn, walks the
    whole queue. Another session's calls are never in it: they are in that
    session's pane. Answer one from the pane — or cycle — and, while its
    session has another call, you stay in it: a deny, `n` or `N` shows the
    next at once; an allow runs the command first, and the pane says so
    (`running …`, and how many are queued) — no key in it answers until the
    next call is there, and `q` leaves it. With nothing else to ask, it
    closes, and you are back at the prompt.

    A call just shown takes no answer for a moment — `tui.approval_delay_ms`,
    300 by default — so the second tap of the `a` that allowed the last one,
    or a key held down, cannot answer a call you have not read. Every answer
    key waits, `d` and `q` too; one pressed too soon says so and starts the
    moment again, so a held key never answers, and the last row is drawn
    dim until the keys answer. `j`, `k` and `v` work at once; so does
    closing the pane (`:close`, `<C-w>c`), which denies — a command, not a
    reflex. `tui.approval_delay_ms = 0` turns the wait off.

    Dismissal denies. `q` — the key you press when you want a thing to go
    away — means NO, so the reflexive answer and the safe answer are the
    same key. (`<Esc>`, pressed by reflex before anything else, answers
    nothing at all.) Closing the pane, in it, denies too: `:close`, `:q`,
    `<C-w>c`, `<C-w>q` there are a deny, not an escape that leaves the call
    hanging — vim's `:q`, which abandons its buffer. Each waits the delay
    first, as `q` does.

    `:hide` keeps it, as vim's `:hide` keeps a buffer, and so does `:q!`:
    forced, a close answers nothing. The pane goes and the call waits,
    hidden, its turn with it — said at the status row's right, as a call in
    no window is, and on its `:ls` line. The pane does not come back when
    the conversation is shown again; you hid the decision, not the session.
    `:decide` brings it back; a command you type with `:tool` in that
    session does too, as it asks behind it.

    A pane goes with the window it sits under. `:only` there — or in the
    pane itself — keeps both, so `:only` makes room rather than answering.
    No close made anywhere else answers it, and none takes it from you:
    `:only` elsewhere, `:tabclose` and `:tabonly` over its tab, and `:q`,
    `:close`, `<C-w>c` or `<C-w>q` on the conversation it sits under — the
    one you decide it by — are refused, saying whose request waits. Add `!`
    (`:only!`, `:tabclose!`, `:tabonly!`, `:q!` on the conversation), or
    `:hide` the conversation, and the close goes ahead without answering:
    the call waits, hidden, as `:hide` in its pane leaves it, until
    `:decide` brings it back. With a second window on the same session, the
    conversation stays in view in it, so its window closes and the pane
    goes under the other — `:only` from that other window keeps it the same
    way. `:b` to another session in that window leaves it waiting too, its
    pane gone with its session and back with it: you changed a window, not
    the question. And it holds no window but its own: the one beside it,
    and every tab, change as ever.

    And nothing times out into a yes: on the API a decision waits as long
    as you do; through Claude Code, one left unanswered for 59 minutes — a
    call waiting in no window or hidden too — is refused by the gate's own
    deadline, under the hour Claude Code gives it (|timeouts|). Its
    `:ls` line counts that down (`needs you · 52m left`), and when it comes
    the notice says the deadline refused it.

    A key after a prefix or an operator belongs to it: `za` is a `z` command
    that does not exist, `yap` copies a paragraph, `<C-w>a` is a window
    command — none is an allow. A key that means nothing here (`x`, `c`,
    `"`, `m`) says so and starts the wait over, so the key typed after it,
    as its argument, is not an answer either. `d` is the one operator that
    is also an answer: here it denies, so `dap` denies.

    A decision you cannot SEE cannot be answered either. On a window too
    short to draw the pane, the keys that would resolve it refuse and say
    why; navigation keys still work, so you can make room.

    Each thing is said once. The pane's last row is its keys (`answer:`),
    and the command is in its body; the title is only the question. A pane
    squeezed too short for the command puts it in the title, and one too
    short for its keys leaves them to the status row (`DECIDE: d deny ·
    a allow`).

    Some keys are deliberately absent rather than refused-with-a-message.
    Against a DENY verdict there is no allow key at all — the way past a
    standing deny is to change the rule, deliberately, not to answer a
    prompt. And `A` is offered only when a rule exists that answers THIS
    call: the legend shows it (`A always, this session: allow rm -r -f`),
    and the report's last line shows it in full, where a long rule fits. A
    glob or a variable, or a call that asks whatever a rule allows
    (|rules|), has none; `A` says which, and `a` still runs it once.

    A call Claude Code makes is titled as what it is. A command is `run
    this?`; a file tool is `write this? — Claude Code Edit <file>`, and the
    pane shows what it writes — a Write's lines, each edit's old lines (`-`)
    then new (`+`) — under the analysis of how Shannon reads it (`cat >
    <file>`). Its `A` grants the write, `:write <file>`, not a `cat`. Its `e`
    opens what it writes in $EDITOR — a Write's content, an Edit's new text,
    as the file's language — and the pane then shows your text, which is
    what Claude Code is told to write. A MultiEdit takes no edit (its new
    texts are several), nor does a tool that writes nothing.

    The rule names the call as written and nothing more. The one `*` it
    writes is for a program Shannon does not model: no rule word can name
    its flags, so `uv run pytest -x foo.py` offers `allow uv run pytest
    foo.py *`, which still requires each word it names. It never offers the
    program alone (`allow uv *`); type that yourself if you mean it.

    `e` (edit) keeps the SAME call open and re-gates your edited text from
    scratch. It is not an escape from the gate; it is answering differently.
    A long command opens laid out as the pane shows it, as shell — each
    run of flags, `&&` or `|` a continuation line ending ` \`, which the
    pane draws quiet — and only where that reads back as the very same
    command; otherwise as written. Where a command has one row — the
    pane's title, a call still running, a shut record — a continuation is
    a blank, ` ⏎ ` marks only a line break the shell itself sees, and a
    heredoc's body is said by its size: `cat > t.py <<'EOF' ⟨74 lines⟩ ⏎
    python3 t.py`, so what runs after it stays on the row.

A SUMMARY'S REVIEW                                                     *review*

    A summary you asked to review — `:compact!`, `:distill!`, or either
    with `[merge] compact_review` / `distill_review` on — is its session's
    decision when it is written. It takes nothing: no $EDITOR, no cursor.
    Its pane opens beside the session, as a call's does, titled `review
    this summary? — 2 turns of b3 5da64e5a`, the draft in it, and you go to
    it (`{n}<C-w>w`, or `:decide 3`). A session's calls come first: one you
    type while its review is up is asked in that pane, and the review comes
    back to it after.

    <CR>  e      review it in $EDITOR — the instruction sent and the
                 draft, as one exchange; `:wq` composes what you saved
    a            apply it as the model drafted it, unreviewed
    d            drop it; nothing composes, and the notice says what went.
                 It answers at its first key: `dd` and `dap` drop too, as
                 a call's `d` denies
    :hide        leave it for later, hidden (|decision|)
    j k / n N    move through and search the draft, as any page

    Its keys wait a moment after it appears, as a call's do. `q`, `:q`,
    `:close`, `<C-w>c` and `<C-w>q` do NOT close it — a draft is paid work,
    and as vim refuses `:q` on a buffer with changes, the pane says what
    keeps it and what drops it; `:q!` drops it, as vim's `:q!` abandons.
    `:wq!` and `:x!` do not: their `!` forces the write, not the abandon.

    Closed in $EDITOR without a save, or saved empty, it stays, marked
    `closed unsaved`: `:x` on an unchanged draft writes nothing, which no
    file can tell from `:q!`, so `a` applies it as it was and `<CR>` opens
    it again. An editor that fails, or `:cq`, keeps it too, and says why.
    So does a save that cannot compose — the file is the exchange as JSON,
    so a newline typed inside its text is one — marked `your edit not
    applied`, and `<CR>` opens what you wrote; `a` then applies the
    model's draft instead, and says your edit was not used. While Shannon
    runs, only `d` or `:q!` throws it away.

    Until it is applied or dropped it holds its session, as its
    distillation did, and the session it replaces when that is another (a
    branch folded by `S`): a prompt for it waits (`its summary draft is
    waiting for your review — :decide 3 goes to it`), and goes, when the
    review does, to whichever session the answer made; a turn cut from it
    is refused. Deleting its session, or the one it replaces, drops it,
    said with the rest; `:qa` names it, and `:qa!` drops it with
    everything else. Composed from its pane, it lands where you were
    working — the `:graph` you pressed `S` in stays.

FILE TOOLS                                                             *files*

    On Shannon's own backend the model has three tools for files beside
    `bash` — Claude Code's names and arguments, run by Shannon:

        Read   file_path [offset] [limit]       a text file, its lines
                                                numbered
        Edit   file_path old_string new_string [replace_all]
        Write  file_path content                a whole file, made or
                                                replaced

    Each is judged as the shell line of what it does to its file, so your
    rules, the inside/outside default and `:analyze` read it as they read
    a command: a Read is `cat 'f'`, a Write `cat > 'f'`, an Edit both,
    `cat 'f' > 'f'` — its answers say what the file holds. Inside your
    working directory a read or a change runs unasked; anywhere else it
    asks, and the pane shows the change itself: `e` edits the text it
    writes, and the model is told you did. On Claude Code its own tools
    run, judged the same way (|claude-code|).

    A path through a link is judged where it lands too — `cat 'notes';
    cat '/elsewhere/x'`, the stricter verdict winning — and the pane's
    title names both: `Read notes → /elsewhere/x`. The run opens the file
    judged; a link pointed elsewhere meanwhile, or put where there was
    none, refuses it, said. A device, a pipe or a socket is refused before
    anything opens it.

    A file with more than one name (a hard link: a package store's file
    under `.venv`, a dotfile linked in) is changed under every name when it
    is written, and the others can be anywhere. So an Edit or a Write of
    one asks, wherever it is, its line saying the reach — `cat > $(find /
    -samefile 'P')` — and its pane how many names the file has; no rule
    answers it. A Read of one is judged as any read.

    READ FIRST. An Edit, or a Write over a text file that exists, asks the
    model to have read the file first: what the conversation it was sent
    still holds of it, of the file as it is now, read in this run of
    Shannon — after a restart, it reads the file again. A file changed
    since, by anyone, is unread again; a fork carries the reads, a
    compaction drops them. An Edit of a file never read is told so before
    any pane asks you. A Read of lines it was shown, the file unchanged,
    is answered that its earlier result is still current, not with the
    text again.

        [files]
        read_first = "any"          some of it (the default); "whole",
                                    every line; "off", nothing

    SECRETS. A secret's handle in what a Write or an Edit writes is a hard
    ask, as a command spending one is (|secrets|): it asks wherever
    the file is, its line saying the spend, and the value goes into the
    file only at the run, said (`secret spent: db`); a new file it makes
    is yours alone (0600). What a Read returns is scrubbed as a command's
    output is — credentials out, a secret you hold as its handle — so the
    model reads the handle back: before its lines are numbered or cut, so
    a value over several lines is its handle on the first and
    `[… continued]` on each after, the line numbers still the file's.

    A Read takes 2,000 lines or 256 KiB at most, and a longer file comes
    back in part, saying where to read on; a line is cut at 2,000
    characters, and a file over 64 MiB is not read at all. Text only
    (UTF-8): a directory is listed with `ls`, anything else looked at with
    `bash`. Try them with no model at all: the stub asks for the calls you
    write out (|stub|).

READING THE REPORT                                                    *analyze*

    The command is shown AS WRITTEN, pretty-printed — control flow on its
    own lines; a line too long broken before a chain's `&&`/`||`, a
    pipeline's `|`, or a run of a command's flags (a flag keeps its value),
    each hung under what it continues — with each command coloured by what
    it DOES. "Too long" is `tui.command_width` columns (72; `0` never
    breaks), and the flags pack to that width, or at most
    `tui.flags_per_line` to a line (`1`: each on its own) — the pane, the
    transcript and `e` alike. The traffic light tells you where to look:

      green    read-only — safe
      yellow   mutating, or UNKNOWN (we could not account for it)
      red      destructive or an executor — look here

    Unknown is yellow on purpose: not-understood is a warning, never a
    silent pass. Yellow and red fill the whole command; green is a claim
    about the command, so it marks the command's NAME. Everything else is
    ordinary shell colouring, in hues the light never uses: a command's
    name bold, flags light blue, strings light cyan, what expands (`$X`,
    `$(…)`) light magenta, reserved words blue, and the plumbing — `|`,
    `&&`, redirects — grey, so your eye lands on the commands. When an
    unresolved value flows into a dangerous command, the producer is raised
    to a warning and the carrier variable is underlined at both ends, so
    you can follow source -> $VAR -> sink.

    `:analyze <command>` shows exactly this report for any command WITHOUT
    running it and without spending a token — the same producer as the live
    pane, so the preview cannot drift from the real thing. It opens as a
    pane: `j`/`k` scroll, `gg`/`G` jump, `:` runs a command, `:q` closes.
    `:tool <command>` is the opposite: it routes a command through the gate
    the way a model call would, INCLUDING running it if policy allows.
    Typed while a reply streams, its record goes in when the reply's words
    stop, below them, not in the middle of a sentence; it is drawn this run
    only, since the conversation keeps no command you typed.

WHAT BOUNDS EXECUTION                                                   *exec*

    `[exec]` bounds an allowed command: a timeout, an output cap, and the
    shell it runs under. A timed-out command is killed by PROCESS GROUP, so
    every stage of a pipeline dies, not just the shell we spawned.

THE ENVIRONMENT A COMMAND SEES                                      *exec-env*

    A command starts from a MINIMAL environment — PATH, HOME, USER, SHELL,
    TERM, TMPDIR, the locale, TZ, PWD, each as yours — not your whole one,
    so it does not see your credentials. `[exec] env` gives it more:

        [exec.env]
        PATH = "$HOME/.cargo/bin:$PATH"
        SSH_AUTH_SOCK = "$SSH_AUTH_SOCK"

    A $NAME in a value is read from Shannon's own environment. PATH set
    here is the one commands run with; unset, it is yours. Claude Code's
    process is given these too — over your WHOLE environment, which its
    commands inherit, less Shannon's key and every variable [secrets]
    lends: it does not start from the minimal set, unless your config says
    `[claude_code] env = "minimal"` — then it gets what Shannon's own
    commands do, and Claude Code's own config directory, so a credential
    Shannon does not hold (`AWS_SECRET_ACCESS_KEY`) is not there to print.
    `:set exec.env.PATH=…` holds from the next command. The shell and
    `claude` themselves are found on Shannon's own PATH, never this one.

    `[exec] withhold = ["AWS_SECRET_ACCESS_KEY"]` names variables NO command
    is handed — Shannon's own or Claude Code's — whatever else names them;
    the gate reads them as unset. A project's config may add to it, never
    take from it.

    Shannon's own key variables, and every variable [secrets] lends, are
    never handed on — not by name, nor as a $NAME in a value, which is
    left empty there; a command is given a value through [secrets]
    (|secrets|) — and a PATH entry that is relative or empty is
    said when it is set: a file found there runs under a name your rules
    allow — a repository's own ls, as yours. Only your own config sets it.
"#;

const RULES: &str = r#"==============================================================================
9. RULES                                                                *rules*

    A rule says what to do when a command matches it:

      allow uv run pytest *     a subcommand, with any arguments
      deny rm -r                a flag, however it is spelled (`-R`, `-rf`)
      deny find -name -delete   both flags together; two rules mean either
      deny bat -l|--language    either spelling of one flag (below)
      ask git push --force      ask for the flag, beside any allow of the rest
      allow find *              a program, with any arguments
      allow find                the program with its model's benign flags only
      allow :read ~/notes       a place, at an operation (|rules-paths|)
      allow :fetch example.org  a host, in a direction (|rules-hosts|)

    Through a program that runs another (`uv run`, `sudo`), a rule names
    the one it runs, then that one's words, in their order: `allow uv run
    pytest *` is not `uv run ls pytest`.

    Older spellings still read: a `:` joins a constraint to the program or
    subcommand before it (`gh api:-X` is `gh api -X`, `find:*` is `find
    *`), and `,` joins flags (`find:-name,-delete`). After a subcommand,
    `push:*` is refused: it also reads as a glob over an argument like
    `ubuntu:22.04`, so write `push *`, or `ubuntu:?*` for the glob. A colon
    that is not followed by a flag is the argument's own (`main:main`,
    `localhost:8080`).

    A program is named, never pathed: `deny rm` already covers `/bin/rm`,
    and a command run by a path asks whatever a rule allows.

    A program rule names the program, its subcommands in order, its flags
    and its arguments; a path rule names places, at an operation (below:
    `:read`, `:write`, `:delete`, `:exec`, `:*`). A program rule never
    grants a place: a path word on an allow is an argument the command must
    carry, as written (`allow make test`), and the place is the path
    rules' to judge — `allow rm` with `allow :delete passwd` (Steven,
    10-10).

    A rule is read where the line runs. A path follows the line's `cd` —
    `deny rm /etc/*` stops `cd /etc && rm passwd` — and a bare `cd` goes
    home, as `cd ~` does. A path rule's place is read from where Shannon
    started: under `allow :delete passwd`, `cd /etc && rm passwd` removes
    another file, and asks.

    A `cd` may fail, so past `;`, a newline or `||` the next command may
    run where the line went or where it was: `cd build; rm x` removes
    `build/x`, or `./x` when there is no `build`. It is judged in both
    places — a deny or a credential path is met in either, an allow covers
    it only in both, and a word holds only if both are inside.

    A relative path pattern names files from where Shannon started:
    `build/*`, or `./**` for everything below. Its wildcards never stand
    for `..` or for that directory itself — write `../sibling/*` to reach a
    sibling. A pattern led by `/`, `~` or a written `**` reaches anywhere:
    `**/*.pem` is every .pem file, and `allow :delete /**` every place
    below the root.

    A deny or an ask on a path names the directory itself too: under `deny
    rm /etc/**`, `rm -rf /etc` is refused, as `/etc` holds what is below
    it. The directories ABOVE it only ASK, naming the rule: `rm -rf /`
    removes `/etc` with the rest, and whether a command acts on what a
    directory holds is not in its words. A pattern that reaches anywhere
    (`**/*.pem`, an unanchored `re:`) has no directory above it, so it
    does not.

    An allow names the flags it approves, and each it names must be there:
    `allow rm` covers `rm x` and not `rm -rf x`, which `allow rm -r -f` or
    `allow rm *` (any flag) covers. A path rule grants a place and no flag:
    under `allow :delete build` alone, `rm build` runs and `rm -rf build`
    asks. A command that only reads takes the harmless flags its model
    knows without their being named — `allow find -name` covers `find .
    -name x -L` — as it would with no rule at all. `allow git push origin *`
    covers `git push --force origin main`; `ask git push --force` keeps
    asking for that one. A flag is known by every spelling only for a
    program Shannon models (`rm`, `find`, `gh`, …): `deny gh api -X` stops
    `gh api --method GET user` too. For any other it is matched as
    written, so `deny bat -l` does not catch `--language`. Name
    both with `|`: `deny bat -l|--language`. The `|` lists spellings of ONE
    flag — flags, or flag values (`-X=GET|--method=GET`), never a mix — so
    `-a|--all -l` is (`-a` or `--all`) and `-l`. For an allow, every
    spelling given must match: `allow x --out=a|--output=a` does not cover
    `--out=a --output=b`. It is never a shell pipe, and a rule command
    takes the rest of its line, `|` and all.

    For a program Shannon does not model (|programs| models it),
    nothing says which flags take a value, so in `bat -l rust` the word
    `rust` is an argument and `allow bat -l` asks for it. `-f=` says the
    flag takes one, any value: `allow bat -l=|--language=` covers `bat -l
    rust` and `bat --language rust`,
    and `-f=VALUE` names one (`deny bat -l=*` refuses every value). One
    rule saying so is enough for every rule on the program: beside it,
    `allow bat -l` covers `-l` with any value. A declared value is still a
    place to the path rules wherever it could be one, so `bat -l
    /srv/x/a` asks until `allow :* /srv/x`. For a program Shannon models,
    the model says which flags take a value, and a rule giving a value to
    one that takes none is refused (`-r=x` for `rm`).

PATH RULES                                                      *rules-paths*

    A path rule names places, at an operation — `:read`, `:write`,
    `:delete`, `:exec`, or `:*` for any — for every program:

      allow :write build         read and write in build, not delete
      deny :write /srv/out       no write, delete or run in /srv/out
      deny :read re:/\.ssh/      not even a read of a .ssh directory

    An allow grants its operation and those below it — `:write` reads,
    `:delete` writes, `:exec` reads — never one above. A deny or an ask
    meets its operation and every one above (read, write, delete, run): a
    deny at `:read` stops everything there. A word of a program Shannon
    does not model is at no operation: only `:*` grants it, and every deny
    or ask meets it. Relative paths are from where Shannon started, and
    each path is where the line's `cd` put it. A word is a path where its
    program reads one there: `echo`'s words and `grep`'s or `rg`'s pattern
    are not, so `allow :write /srv/out` covers `echo hi > /srv/out/log`,
    and `allow :read ~/notes` covers `rg TODO ~/notes/a.md`. A directory
    holds what is below it: that rule covers `rg TODO ~/notes` and `cd
    ~/notes && rg TODO` too, and `~/notes/**` says the same. For a
    DELETE, `build/**` is what is below build and not build: `allow
    :delete build/**` does not cover `rm -rf build`, nor does `allow
    :delete build/*` (the entries in it); `allow :delete build` does (the
    directory and all below it), and `allow :delete ./**` never covers the
    project itself.

    THE MOST SPECIFIC RULE DECIDES — the one whose program, flags and
    words, or whose place, lie inside the other's: `allow :read /mnt/foo`
    is an exception to `deny :* /mnt`, and `allow rm -r -f` to `deny rm
    -r`. Where neither lies inside the other, or both name the same, the
    strictest wins: deny, then ask, then allow. The order they are written
    in never matters.

    What Shannon ships is a tier below yours (your config and this
    session), and a lower tier only narrows: a shipped rule takes the place
    of one of yours only by being more specific AND stricter. The shipped
    guard on `~/.ssh` still asks beneath `allow :read ~`, and `deny :* ~`
    still refuses `cat ~/.ssh/config`.

    Shannon ships GUARDS, which only a rule of yours at least as specific
    lifts: `deny rm --no-preserve-root` (`allow rm --no-preserve-root`
    lifts it, `allow rm *` does not); an ask before a write into what
    instructs an agent — `**/.claude`, `**/.shannon`, `**/.git/hooks` — or
    into Shannon's config, `~/.config/shannon`; each `[secrets]` path
    (|secrets|); and Shannon's own state. It ships one allow read in
    full (|rules-re|) — `date` that prints, never `date` that could
    set the clock — and a grant of the scratch directory (below). `:rules`
    lists them all, by kind, and `[permissions] remove` drops any of them.

    What makes a program more than it seems is its model's to say: a flag
    that deletes, writes or runs asks until an allow names it, or says any
    flag (`allow find -delete`, `allow find -exec`, `allow find *`) —
    `allow find src` leaves `find src -delete` asking — and what the flag
    acts on is judged still. Its places are the path rules': `find build
    -delete` runs beside `allow :delete build`, as `rm -rf build` does. What
    `-exec` RUNS is a command of its own: under `allow find *`, `find .
    -exec cat {} \;` reads and runs, and `find . -exec rm {} \;` asks until
    a rule on `rm` says otherwise.

    The APPROVED PLACES are where Shannon started, everything below it, the
    scratch directory, and what an allow grants. The scratch directory is
    the agent's own place for temporary files — `[permissions] scratch`,
    /tmp/shannon-<uid> unless set — used only if it is a directory, yours,
    and closed to other accounts; the model, and Claude Code, are told it.
    A read runs unasked only in them: `rg foo src`,
    `cat notes.txt`, `ls`. One that reads elsewhere asks — `rg foo /`, `cat
    /etc/hosts`, `cd .. && ls`, home spelled `~` or in full alike — and so
    does one whose path still expands (`cat $F`), which could be anywhere.

    A program allow holds only in them — `allow rm *`, `allow
    rg:--hidden`, a `re:` — so `allow cat *` does not cover `cat
    /etc/passwd > notes.txt`, nor `allow rm *` a `cd /etc && rm passwd`. A
    path rule names other places: `allow :read /**` every one below the
    root.

    What a command names is read every way it may name a file: a value
    attached to a flag (`cp -t/etc`, `--target-directory=/etc`, read at
    each letter a bundle could end on), a glob by what it picks (`rm
    /srv/out/*` is a file in /srv/out, not the directory), each at what it
    does to it (`cat granted/a > b.txt` reads there and writes here). A `~`
    after the line moves HOME may be anywhere. A place's own root holds a
    read and a write into it, not its delete: `rm -rf /tmp/shannon-<uid>`
    asks; `.` names what is there.

    A path rule grants places, at its operation, to every program; a
    program rule grants none:

      allow :read ~/notes/**       reads there, by any program
      allow :write /srv/out/**     reads and writes there, never a delete
      allow :delete /srv/out/**    deletes there too
      allow * /tmp/x/**            anything there (`:* /tmp/x/**`)
      allow rm /srv/out/a          `rm` with that argument — the delete
                                   still asks, until a path rule grants it

    They compose: under `allow cat *` and `allow :write /srv/out/**`, `cat
    a > /srv/out/b` runs, which neither covers alone.

    `rg` is a read: it skips hidden files and what .gitignore ignores,
    which is where `.env` and keys live, so a search runs unasked, and a
    credential it is NAMED is met as any file is. What makes it more asks,
    and says why: --hidden, -u, --no-ignore… and -L (they read what it
    skips); --pre, -z and --hostname-bin (they run a program); and
    RIPGREP_CONFIG_PATH set on the line (a file of its flags). `-r` is
    --replace, which changes what it prints, not any file.

    Six programs an agent reaches for all day are modelled, so their plain
    forms run and what makes each more asks, saying why. Read them as
    examples of what a model says (|programs|):

      grep   a read of the files it names. `-r`, and any flag not
             modelled, asks: a recursive grep reads hidden and ignored
             files — `.env` under `.`.
      rg     a read that skips hidden and ignored files, as above.
      fd     lists names and reads none, so even --hidden runs. -x, -X
             and -l ask: they run a command (`ls`, for -l) on what it finds.
      find   a read until -exec, -execdir, -ok, -okdir, -delete or
             -fprint…, which ask; what -delete removes is a place, for
             the path rules (|rules-paths|).
      sed    its script is read: a substitution or a print runs; `w`
             (writes), `r` (reads another file), `e` (runs a command),
             -i (edits in place) and -f (a script from a file) ask.
      awk    its program is read: one that prints runs; `system` (as a
             word, however it is called), an `@` before a name (gawk's
             `@load`, `@include`, an indirect call), a pipe (`| "sort"`),
             a redirection (`> "out"`) and -f (a program from a file)
             ask.

    Each reads only in the approved places. To let more run, say what:

      allow rg --no-ignore-vcs    a mode rg leaves unmodelled on purpose
      allow :read ~/notes/**      reads of another place
      allow fd *                  fd however it is called, in the approved
                                  places — -x, -X and -l included
      allow fd -x                 fd's -x with no other flag: a call that
                                  runs a command takes only the flags its
                                  rule names
      allow fd -x -e              and -e beside it (`fd -e rs -x wc`)

    Some commands ask WHATEVER a rule allows, because a rule names a program
    and these may not run the program named, or reach further than it says:

      a name that may not      `./ls`, `/tmp/ls`, `/usr/bin/ls` (any path),
      run what it says         `PATH=. ls`, `LD_PRELOAD=… ls`,
                               `GIT_CONFIG_GLOBAL=… git`, `alias ls=rm; ls`
      a privilege wrapper      `sudo`, `doas`, `pkexec`, `run0` — unless an
                               allow names the program it runs and every
                               flag given the wrapper: `allow sudo ls *`,
                               `allow sudo -u=pg psql *`; never through a
                               shell (`allow sudo -s ls *` lifts nothing),
                               nor `su`, which takes a shell line
      a secret handle          `{{…}}` is spent when it runs
      an argument list from    `… | xargs wc -l`: the words come from
      a stream                 the stream, and the gate never sees them
      a credential path        `.env`, `~/.ssh` — unless a rule names the
                               file (`allow :read ~/.ssh/config`), which
                               `A` never offers, or `[secrets]` says
      a call leaving Claude    its sandbox is Claude Code's to lift; a deny
      Code's sandbox           still denies

    A wider rule never stops these asking — `allow sudo *` names no program,
    and lifts nothing; `a` runs the call once.

    One reach no rule can see: a repository's own `.git/config` can name
    programs git runs — `core.fsmonitor` on `git status`, a diff or filter
    driver, an `alias.*` — and the gate does not read that file, so under
    `allow git *` they run unasked. (Shannon sets `GIT_PAGER=cat`, so a
    `core.pager` does not run.) In a repository you did not write, read its
    `.git/config` before you allow git broadly.

    Keep rules in your config (|config-file|) — one list per file,
    which may span lines (a second `rules =` is refused, naming its line):

      [permissions]
      rules = [
        "allow uv run pytest *",
        "deny rm -r",
      ]
      remove = ["ask :write **/.claude"]

    Rules STACK: each file's list is added to the ones read before it — the
    system file's, yours, any you `:source` — rather than replacing them,
    and a rule two files name is in force once. Which one wins is never the
    order: the files are one tier, and the most specific rule decides, the
    strictest between equals — so a file read later cannot loosen a deny by
    allowing the same thing, while a narrower allow is an exception to a
    wider deny whichever file holds each. A file read again
    (`:reload`) counts through its new text alone, so a rule you deleted
    from it is gone. `:verbose set permissions.rules?` lists each file's.

    `remove` drops a shipped rule, written as `:rules` lists it, and only the
    ones you name. A rule that cannot be read, and a `remove` that names no
    shipped rule, are named when the config loads; nothing changes quietly.

    :allow <rule>      add a rule for THIS SESSION, like `:set`; the file is
    :deny <rule>       untouched (to keep a rule, add it to [permissions]
    :ask <rule>        rules)
    :unallow <rule>    take a rule OUT for this session, whoever wrote it —
    :unask[!] <rule>   shipped, your config, or this session; the file is
    :undeny[!] <rule>  untouched. Removing an ask or a deny you did not
                       add this session (shipped, or your config's) takes
                       `!`: it loosens the standing policy, so a call it
                       asks about or stops could run. Removing an allow
                       only tightens, and never takes `!`
    :rules             the policy in force: yours by where each rule came
                       from, then what ships by kind — guards, the
                       credential paths `[secrets]` guards, Shannon's own
                       state, allows, the scratch directory's grant — and
                       what was removed. A LIVE pane, like `:ls`: a
                       rule taught or removed beside it shows as you read

    A rule is named as `:rules` lists it, by any spelling of it (`ask sort:-o`
    is `ask sort:--output`). Adding one already in force says so; adding one
    you removed this session puts it back.

    A rule added while a decision is open answers it too: covered, it runs;
    denied, it is denied; otherwise it stays open and says why. That is what
    `A` on a decision is for (|decision|): it opens the `:` line
    holding a rule that names the call as written, to read and edit before
    Enter; widen it there if you mean to (`uv run pytest *`). A call that
    needs a program's flags and a place takes one rule each — `rm -r -f`,
    then `:delete build`, for `rm -rf build`; `:write out.txt` for `echo hi
    > out.txt` — and the pane lists them all before the first `A`: press it
    once per rule. Nothing is granted by the key alone.

    A rule in your config applies in every project; one in a project's
    .shannon/config.toml, in that project, as yours do
    (|config-project|); a session rule, only until you quit.

HOST RULES                                                      *rules-hosts*

    A host rule names where a command connects, in a direction: `:fetch`
    when it only receives, `:send` when it sends — a body, an upload, a
    method that writes — and `:web` for either. A host is a name, or
    `*.example.com` for the hosts under one (not example.com itself), or
    `*` for any:

      allow :fetch api.example.com    receive from it
      allow :web *.example.com        either way, any host under it
      deny :send *                    send nothing, anywhere

    With no rule a host asks — never a deny, which would leave `curl`
    unusable — and a rule on the program decides the program, not where it
    connects: under `allow curl *`, `curl https://api.example.com/x` asks
    until a host rule says. A host a program reaches without naming it
    (`gh`'s api.github.com) is its model's: the call is judged by what it
    does, and the pane says it (`GET api.github.com/user — the
    authenticated user's profile`).

PROGRAMS SHANNON READS                                           *programs*

    The gate reads a command by its program's model: which words are flags,
    which flags take a value, which words are files and what is done to
    them. Shannon ships models for the programs it knows (`rm`, `find`,
    `git`, `tar`, …). For any other, a rule's flags are matched as written
    and every plain word is an argument, so `bat -p notes.txt` asks under a
    rule naming only `-p` — `notes.txt` is a word it did not name.

    Write the model yourself, in programs/<name>.toml beside your config
    file, in the format Shannon's own are written in:

      name = "bat"
      effect = "read"
      examples.match = ["bat -p notes.txt", "bat -l rust notes.txt"]
      examples.not_match = ["bat --frob notes.txt"]
      positional = [{ at = "rest", role = "read_file" }]
      flag = [
        { names = ["-p", "--plain"] },
        { names = ["-l", "--language"], arity = 1, role = "data" },
      ]

    With it, `bat` is a read: `bat notes.txt -p` runs inside the project as
    `cat` would, with no rule, its flag harmless; `deny bat --language
    rust` stops `bat -l rust` too; and the files it reads are the path
    rules' to judge: `bat -p /srv/notes/a.md` asks until `allow :read
    /srv/notes`.

    A program that only LISTS — prints names, never what files hold — says
    so: `role = "list"` for the places it is given, and `reads_here =
    "list"` when, given none, it lists where it runs (`reads_here = true`
    searches there instead; a `[command.<name>]` table may set its own).
    `ls`, `find`, `fd` and `git status` are written so. A listing of a
    directory reaches no guard below it: `ls` or `git status` where a key
    or Shannon's own state sits beneath runs, while `rg foo` there asks.
    Named outright, the guarded file is still guarded (`ls key.api`); and
    once a flag deletes, writes or runs (`find -delete`, `-exec`), the call
    acts on what is below and is judged as such.

    A file replaces the model Shannon ships under its name, whole, and must
    be named after the program it models. It is read at launch and at every
    config change (`:reload` after an edit), and every rule is read by it —
    your config's and `:allow`'s. What a file could not honour is said:
    one that does not parse or is misnamed (not loaded); an example it
    misreads (each `match` line must read in full and each `not_match` line
    leave a word unknown — the model's own test); a rule it refuses.

    `trust = "completion"` marks a model imported from shell completions:
    it says what the words are, not what they do, so it allows nothing by
    itself — no read by default, no harmless flag riding on an allow, its
    files at no operation (`:*` grants them). Your rules still apply, read
    by it. Without `trust`, a model is the author's word.

    Shannon's own models say `verified` where written and checked by hand,
    and `generated` where carried over from its older table and not yet
    checked flag by flag. Both are relied on alike; the decision pane names
    which beside a verdict no rule made (`model: shipped, generated`).

    A project may keep models in .shannon/models/, read where you trust its
    directory (|config-project|): each at `trust = "completion"`,
    whatever it says. One may replace a model Shannon ships, as yours may;
    what Shannon's reads as code, or as another program it runs, stands
    under it — a `find` modelled flat still runs its `-exec`.

REGEX RULES                                                        *rules-re*

    For what the rule words cannot say, `re:` takes a regex. It is the
    advanced form, and says exactly what you write:

      deny re:^(?:\S*/|\\)?rm(?:\s+--?[A-Za-z0-9][\w-]*)*(?:\s|$)

    is "any `rm`": bare, pathed (`/bin/rm`), escaped (`\rm`), with flags.
    (`\S*/`, not `.*/`: a `.*` runs across spaces, so the program part met
    `ls /bin/rm` and `cat notes/rm` — and any `$X` or glob that might.)
    The rest of the line is the pattern — its spaces, `,` and `|` are the
    regex's own — and a `#` in it is the pattern's, not a comment.

    It is matched against EACH command the line runs, as the shell runs it:
    the program and its arguments, quotes removed, wrappers like `sudo` and
    `env` peeled, joined by single spaces (an argument holding a space reads
    as two). So the rule above also catches `cd x && rm -rf y`, `sudo rm -rf
    /`, `"rm" foo` and `r\m foo` — the commands the line runs, not the text
    it was typed as. A deny fires when any of them matches; an allow covers
    only the commands it matches, so `allow re:^make test$` does not cover
    the `rm` in `make test; rm -rf ~`. What always asks still asks.

    An allow covers only what it READ. Not a command whose words still
    expand (`$X`, `*` — what they become is not what it read; a quoted
    `'*.rs'` does not expand, and is read as the text it is), nor one
    whose arguments come from a stream (`… | xargs rm build/x`), nor a
    wrapper's own arguments (`flock /etc/passwd …`), nor a redirect (`cat
    notes.txt > out.txt` — `2>/dev/null` is fine), nor the environment it
    is given (`X=1 make test`). And its words hold where
    Shannon started, as a rule's words do (|rules|). A deny
    on words that still expand asks when some expansion COULD match it —
    `git push $F` under a deny on `--force` — and stays quiet when none can
    (`ls *.md` under `deny re:^rm(\s|$)`).

    The engine is Rust's `regex` crate: no look-around, no backreferences,
    and matching takes time linear in the command, so no pattern can hang
    the gate. A pattern is kept under a megabyte compiled: a repeated
    Unicode class is the usual way past it (`\w{50}` is), and the ASCII
    one is small — `[A-Za-z0-9_]{50}`.

    The footguns, each of them real:

      deny re:rm          matches anywhere in the command — `echo rm` too
      deny re:^rm         no boundary: `rmdir` begins with `rm`
      deny re:^rm(\s|$)   right — `rm`, then a space or the end
      allow re:git        covers ANY command containing `git`, `rm -rf
                          git-x` included (where Shannon started; it names
                          no place). Anchor an allow at both ends:
                          `allow re:^git status$`
      deny re:rm -rf      unanchored, so ANY `$X` or glob could become it:
                          `echo $X` asks. Anchor a deny too: `^rm -rf`

    A path can be a regex too: one word of a rule, `re:` and the pattern —
    a path rule's place, or a deny's or an ask's (an allow on a program
    names no place, so it takes none).

      allow :delete re:^build/.*\.o$   `.o` files under build/, nothing else
      deny * re:(^|/)\.git(/|$)        anything touching a .git directory

    It is a path pattern, as `/tmp/*` is: an allow needs EVERY path the
    command touches to match, and a deny fires when ANY does. It reads each
    path as the shell will open it — where the line's `cd` put it, in each
    spelling of that one file: relative to where Shannon started
    (`build/x`) and absolute (`/home/you/proj/build/x`), with `.` and `..`
    resolved, so `build/../../etc` is not under build/ — and a path in
    your home both ways, `~/notes` and its full spelling. Anchor it as
    the rule needs: `/\.git/` misses `.git/config`, which has no `/` before
    it. A space in it is `\s`, since a rule's words are split on spaces.

    In your config, write a regex as a TOML literal string, in single
    quotes, so a backslash stays a backslash (`"\s"` is not a TOML escape,
    and the file is refused):

      rules = ['deny re:^rm(\s|$)']

    A long pattern can be laid out to be read. A TOML multi-line literal
    string (`'''`) may span lines, and the regex's own `(?x)` flag skips
    whitespace and reads `#` to the end of its line as a comment:

      rules = [
        '''deny re:(?x)
           ^(?:\S*/|\\)?rm                 # the program, bare or pathed
           (?:\s+--?[A-Za-z0-9][\w-]*)*    # then its flags
           (?:\s|$)                        # then a space, or the end
        ''',
      ]

    `:rules` lists it as you wrote it; a reason or a notice shows it on one
    line. Without `(?x)`, a line break is part of the pattern, and matches
    only an argument holding one. Any rule may span lines this way, and a
    `#` comment in it ends at its line.
"#;

const SECRETS: &str = r#"==============================================================================
10. SECRETS                                                           *secrets*

    A credential that reaches the model is a credential you have disclosed.
    Shannon redacts on the way OUT: known issuer formats (checksum-validated
    where the issuer defines one), high-entropy strings matching no known
    format, and anything you name yourself.

    :secret [--no-spend] <text>   mark <text> a secret: it leaves as a
                                  HANDLE, never as itself. --no-spend also
                                  forbids commands that would expand it.
                                  Any text, a sentence too; it need not be
                                  typed yet. <Tab> completes from the
                                  prompt, then the chat, latest first
    :nosecret <handle|value>      it is no longer a secret: from now on its
                                  value is sent and saved as itself. Off
                                  this session and out of the secure store;
                                  one lent by `[secrets] env` or config
                                  returns at the next launch. Earlier
                                  messages keep its handle, which no longer
                                  works. <Tab> completes handles, and values
                                  once you start typing one
    :secrets, :alias              what this session substitutes, in a live
                                  pane: each handle BESIDE ITS VALUE,
                                  secret or alias, whether a command may
                                  spend it, and where it came from (`env
                                  NAME`, your config, marked by you)

    A marked secret still shows as ITSELF on your screen, styled — you are
    not the one it is hidden from, and `:secrets` shows what you hid so you
    can check it is what you meant (a typo marks the wrong text). Your own
    line stays exactly what you typed; where a reply or a command's output
    names it by its handle, it shows as its value, styled as yours is. A
    value the model writes ITSELF — it had it: given before you marked it,
    or read where it was not masked — is drawn apart, underlined in
    `ui.secret.leaked`, and a shut block holding one says `· leaked value`
    on its row (|folds|). The offline stub, a test tool, shows
    both, `value ({{handle}})`, so you can see the handle went. An alias
    follows the same rule, uncoloured. Values appear in that pane, and in
    `:nosecret`'s completion once you start typing one — never in the log
    or the wire.
    A notice that quotes what you typed quotes it as you typed it, value
    and all, on the status row and in `:messages` (the log has the
    handle); `:ls` shows a session's first prompt as stored, by its
    handle. A selection you copy holds what you see, values included,
    unless `tui.copy_secrets` says otherwise — see |register|.

    A mark hides the value where Shannon sees it WHOLE — what you type, what
    the model writes, a command's output. A COPY of it kept in a file the
    model may read (a project's config, a `.git/config` a token was written
    into) is beyond the mark: a command that reshapes what it reads (`cut`,
    `tr`, `base64`) or only counts it (`grep -c`) carries the value, or
    facts about it, past the mask — and on Claude Code a plain `cat` does,
    since its model reads its own commands' output as it is. Put such files
    on `[secrets] paths` (below): reading one then asks you first, and on
    Claude Code is refused, by default.

    A credential the model typed INTO a command — one it had before you
    marked it — is never written into a rule. A rule keeps its words as
    written, and `:config save` puts them in the project's file, beside its
    code: the way other agents' "don't ask again" lists have published
    passwords. So `A` offers no rule for a call carrying one, `:allow`,
    `:ask` and `:deny` refuse a rule you type holding one, and each says
    where it sat — never the value: the value after a flag whose name says so
    (`--password`, `--token`, `--api-key`), at any length; a URL's
    password; a key, or a high-entropy run. Write `*` in its place to cover
    any value. A rule may hold a `:secret` handle, but a call holding one
    always asks, whatever the rule allows. Not guessed: the value after a
    flag that does not say so (`-p`, `-a`) of a program Shannon does not
    model.

    A mark is KEPT between runs, in Shannon's own store — owner-only files
    under `$XDG_STATE_HOME/shannon/secrets/`. When you resume or `:open` a
    session whose history holds its handle, it comes back — shown to you as
    itself, and its commands still work. The model still has only the
    handle. Only that exact handle brings it back — never a look-alike
    written into a file the model read. Secrets lent from the environment
    or the config are lent again each launch, under new handles. The gate
    asks before any command the model runs reads that directory, and the
    model is never told the path.

    `secrets.password_protect` (off by default; your config file only —
    `:set` refuses it, since it re-stores your secrets on disk) will seal
    them under a passphrase Shannon asks for before a session holding one
    opens: given, it opens with its secrets; not given, it does not open.
    A forgotten passphrase loses the sealed secrets. NOT BUILT YET: today
    the setting changes nothing, and says so.

    `[secrets]` configures the rest: literal values to scrub, extra issuer
    prefixes, what to do when a command touches a path that usually holds
    credentials (~/.ssh, .env, *.pem), and more such paths of your own —
    `paths = ["~/work/vault", "*.age", "/srv/keys/**"]`. An entry is a
    glob, read as bash reads one with `globstar`: `*`, `?` and `[...]` stay
    inside one directory, `**` crosses them, `{a,b}` is either. A NAME (no
    `/`) matches anywhere — `id_*.pub`; a literal name is a file or a
    directory, all of it. A PATH is anchored — `~/` at a home, `/` at the
    root, otherwise where Shannon started — and holds everything below what
    it names, so `/srv/keys/*.api` is those files and `/srv/keys/**` the
    whole place. A `re:` entry is a regex read as a path reads
    (|rules-re|), and like every entry here it ignores case unless
    it says `(?-i)`. A glob in a command is
    checked against every path it could expand to, so `re:^/srv/keys/.*pem`
    stops `cat /srv/keys/*`; a word that names nothing in particular (`cat
    *`, `cat $X`) does not ask on its own account. What a variable's braces
    write is read as written: `${X:-~/.ssh/id_rsa}` is that key whenever X
    is unset, and `${HOME%/}` is still your home. Where that cannot be
    worked out — a pattern too large to check, words too long to check on
    every decision, a word of more expansions than are read — the command
    asks and says so; it never denies, and never names your entry, on a
    guess.
    Environment variables can be LENT to a command it may use but never see:
    `env = ["GITHUB_TOKEN"]` gives the model a handle in its place, and no
    command is handed the variable itself — Claude Code's included, whose
    commands otherwise inherit yours — so the value is reached only by
    spending the handle, which asks you first. A command that names the
    variable itself — `$GITHUB_TOKEN`, `printenv GITHUB_TOKEN`, a script's
    `ENVIRON["GITHUB_TOKEN"]` — asks too, whatever a rule allows: nothing
    hands it on, but a shell that sets it again (Claude Code's Bash replays
    your shell's setup) would print it.

    The shipped lists are a SEED, not the list: an internal token format is
    exactly the shape nobody has published, so add yours. And `paths_except`
    carves places OUT of them, in the same grammar: a pattern that holds
    the whole place (`~/.claude` holds `~/.claude/skills`) does not count
    inside it; one naming a file there (`*.pem`) still does. It ships
    holding Claude Code's files that hold no credential
    (|memory-claude|); `paths_except = []` makes all of ~/.claude a
    credential path again. Only your own config sets it.

    {claude-home} leading an entry, in either list, is Claude Code's home
    wherever this run found it: $CLAUDE_CONFIG_DIR when that is set, else
    ~/.claude — and the place a link there leads to, too. The seed holds
    all of it, beside ~/.claude itself (a home moved away still holds what
    it held), and carves its memories, CLAUDE.md and skills out the same
    way. A WRITE there asks even under path_policy = "allow": its
    settings.json runs hooks. Written anywhere but first, or in a run that
    knows no home, an entry names nothing, and the config load says so.

    A redacted value is REMOVED, not hidden: output is scrubbed before
    anything records it, so the transcript, the session file and the logs
    all hold `[redacted: …]`. The notice says so. For a file that is fine —
    the value is still in the file. For a command that prints a new secret
    once, it is gone unless you set `keep_redacted = true` in `[secrets]`:
    then each redacted output is also written, unredacted, to its own
    owner-only file under $XDG_STATE_HOME/shannon/redacted/, and the notice
    gives you the path. OFF by default — it is a plaintext file of your
    secrets on disk, not encrypted. The model is never told the path, and a
    command that reads that directory always asks.

    A call the model makes is kept scrubbed the same way — in the session,
    its file and every later request — and still runs as the model typed
    it. The text of its reply is kept as it wrote it.
"#;

// `##`: a theme's `"#61afef"` would end a `#` raw string.
const CONFIG: &str = r##"==============================================================================
11. CONFIG                                                            *config*

Every default in this manual is yours to change. The config file is TOML.
Your file is a SPARSE diff over the built-in defaults: list only what you
change — anything absent keeps its default, so the file stays small and a
new release's better defaults reach you untouched.

At launch a machine-wide file is read first, if one is installed (the first
shannon/config.toml under $XDG_CONFIG_DIRS, /etc/xdg by default), then yours
at $XDG_CONFIG_HOME/shannon/config.toml, then the project's .shannon/
config.toml where Shannon started, if you trust that directory
(|config-project|). Each is a diff over what came before it; the
later wins where two say something.

EDITING IT                                                        *config-file*

    :config edit   open your file in $EDITOR, and CHECK it when you save
                   — saving does not apply it
    :reload        apply it: source your file again (|source|)
    :config dump   print the full defaults, every field commented — the
                   authoritative list to copy knobs from
    :config save   save this session's settings into the project's file
                   (|config-project|); `:config! save` where the
                   directory is not trusted
    :keymap        the keys in force and their action names
                   (|keymap|)

    Editing never changes the running session by itself: a half-finished
    edit applied mid-conversation is how a working session breaks. Save as
    often as you like; `:reload` when you mean it. A new file starts with a
    short header rather than a copy of the defaults — a copied default
    would override every better one a later release ships.

    A malformed file is always reported, and the difference in what happens
    next matters: on save you are told `:reload` would refuse it, and the
    running config is untouched; a file that will not parse AT LAUNCH is
    left out — so its [secrets] settings (sensitive paths, names, redaction)
    are not in effect. The notice says so; it is not a typo you can leave
    until later.

    A `:reload` or `:source` says which settings it changed. Of those, some
    only apply on relaunch, and the notice names the ones you touched. These
    need a relaunch: mouse, keymaps, aliases, [secrets] env, scratch —
    "aliases" meaning [[alias]] and alias_home. (A session's handles carry a
    per-run nonce, so rebuilding the table mid-session would invalidate every
    handle already in your transcript; the scratch directory is checked, and
    handed to Claude Code, once.)
    The rest applies at once — including the rest of [secrets], [cache]
    (from the next request), and `tui.prompt.height`, which resizes the
    prompt and nothing else — except that `resume` and `start_mode`
    describe a launch. Anything a file could not
    use as written (an unknown key, an out-of-range value, a setting that
    can never take effect) is listed with it.

SOURCING FILES                                                        *source*

    :source <file>     apply a config file over the running one  (:so)
    :reload            source your own file again                (:rel)
    :unsource <file>   take a file's settings back out           (:uns)
    :scriptnames       the files in effect, in order, in a pane  (:scr)

    `:source` is vim's: a file's settings run against what is CURRENTLY in
    effect, later settings winning. It is additive — take a key out of a
    file and source it again, and the value you had stays until relaunch.
    `:reload` is `:source` of your own file and then of the project's
    (|config-project|), so the same is true of it. `:source` of
    your own file is `:reload`; of the project's, it reads that one again
    as the project's. A bare `:source` is an error rather than a guess at
    which file.

    Unlike vim, a sourced file can be taken back out. `:unsource` removes
    everything that file set (every time it was sourced), and each of its
    settings falls back to what the files still loaded say — down to the
    defaults. That is what makes a file of per-project settings something
    you can put on for a project and take off again. `~` in a path is your
    home; a relative path is from the directory Shannon runs in.

    Every file loaded or taken back out is written to the log with the
    settings it changed — their names, never their values — at launch as
    well, so the log answers "which file set this?".

    A file that is not yours (anything you `:source` but your own config)
    that changes a safety setting says so, and names each one. It still
    applies; the note is for a file someone else wrote, where "it turned
    redaction off" should not be one key in a list of ten. It names the
    change and does not judge whether it is looser or stricter. Safety
    settings are those under: secrets, permissions, exec, agents, alias,
    continue, merge, keymap, tui.confirm_delete, tui.approval_delay_ms,
    memory, skills, tui.alerts, tui.alert. A project's file that turns
    back on a bell or a notification you turned off wins, and is named.

    A file that is not yours may ADD memory and skill locations, but it
    never says where the model writes: its `memory.write` is ignored, and
    said so (|memory-places|). So with secrets: it may add to what
    counts as one (secrets.paths, names, prefixes, extra), but never take
    caution away — its secrets.names_exclude, value_delimiters, env and
    paths_except are ignored, and said so.

    Permission rules live under [permissions] (|rules|).

THE PROJECT'S FILE                                            *config-project*

    A project keeps Shannon's settings for it in .shannon/config.toml, its
    program models in .shannon/models/ (|programs|) and its skills
    in .shannon/skills/ (|memory-places|), beside its code. All of
    .shannon/ is read only where you TRUST the directory holding it
    (|claude-code|): a repository's files are someone else's
    writing until you say otherwise, and a rule allowing everything is not
    yours by being cloned. Not trusted, Shannon says what it did not read:
    at launch (the trust screen names it, when it asks), when you answer
    that screen no, or when you leave setup — and a skill there
    is listed with how to read it, and never told the model. `:trust .`
    reads them at once and `:untrust` takes them back off, as though
    Shannon had started that way.

    Its settings win over yours where both speak, and stand under any
    `:set` typed before they arrived. `:reload` reads yours and then it —
    one made since you started, too — so an edit to either is taken; a
    file that is not there is left out, and one gone since is taken off
    and said. `:scriptnames` lists it as `project`.

    `:config save` writes what you did this session into it, making
    .shannon/ if there is none: each option you `:set`, at its value now,
    and each rule you taught — `:allow`, `:ask` and `:deny` among its rules,
    a shipped rule you took out (`:unask!`) in its `remove`, one of its own
    taken out of it. A list the files add to (rules, memory and skill
    locations) is changed, never written whole: what you added goes in,
    what you took out of its own entries comes out, the rest stays. The
    file is edited where it stands, its comments kept, and read back at
    once: the rules come from it now, and `:rules` lists them under it.
    What it cannot hold is said instead — a setting only your own config
    makes, a `-=` of an entry another file gives, your config's rule taken
    out. The effort `CLAUDE_CODE_EFFORT_LEVEL` set is not written: you did
    not type it. In a directory you do not trust it would not be read, so
    it refuses; `:config! save` writes it anyway, and it is read once you
    `:trust .`. A .shannon/ or a config.toml that is a link is refused.

    `:config save <file>` writes the same into a file you name — to keep,
    or to `:source` in another run — trusted or not, and reads nothing
    back: it is in force here already. Only what you added goes in; a `-=`
    or a rule taken out of the project's own entries is that file's to
    lose, and is said instead. A file that is a link is refused (saving
    replaces it): name the file it points to.

    Its [permissions] rules are yours to keep: they decide as yours do, the
    more specific rule winning whichever file wrote it (|rules|),
    and may lift a shipped guard or `remove` a shipped rule as yours may.
    `:rules` lists them apart. What only your own files set stays yours —
    where memories are written, how the project is named and what marks
    its root, what is carved out of the credential paths, the command
    environment — and a safety setting it changes is said, as for any file
    not yours: when you trust it, and after that only when what it says
    about safety changes (an edit, a rule a pull brought), not at every
    launch. Shannon keeps what it said beside the trust; `:untrust`
    forgets it, so a trust again says it again.

SETTING OPTIONS                                                          *set*

    :set <option>=<value>    change it, at once                      (:se)
    :set <option>?           show it
    :set <option>            a switch: turn it on; anything else: show it
    :set no<option>          turn a switch off (inv<option> or <option>!
                             flips it)
    :set <option>&           back to the default
    :set <option>+=<value>   add to a list or a number; -= takes away,
                             ^= puts first (for a number, multiplies)
    :set                     the options you have changed
    :set all                 every option, in $EDITOR
    :verbose set <option>?   and every layer that sets it            (:verb)

    An option is its config key, dotted — `:set tui.prompt.height=15` — and
    Tab completes the names. Every option, its default and what it does is
    in |options|, and `:help` takes one as `:set` does (`:help
    notui.mouse`). A value takes the type the option holds, so
    nothing is quoted: a list is a,b,c (or TOML's ["a", "b"]), and `\ ` is
    a space. On a list your files and a project's add to (the [memory] and
    [skills] locations, what is disabled, the rules, what counts as a
    secret), `-=` takes the item
    out wherever it came from and `=` sets the whole list — for the
    session: a file read again adds its own back. What `:set` answers
    with is the value you got — adjusted, if the config had to.
    `tui.set_report` chooses: "default" (that), "quiet"
    (no echo, as vim — a note or a warning still shows), or "verbose" (the
    `:verbose` form). An `x?` is always answered.

    `:verbose` lists what lies under the value, top first, down to the
    default — what `:unsource`, `:set x&` or quitting would give back:

        tui.prompt.height=15 (:set 15, ~/.config/shannon/config.toml 20,
        default 10)

    A `:set` is a layer, like a sourced file: it holds until a file that
    names the same option is sourced after it — `:reload` included, as
    running your vimrc again resets what it sets. It lasts until you quit;
    to keep a setting, put it in your config. One that is read only at
    launch says so when you set it. A line applies left to right, as vim's
    does: `:set tui.scrolloff=10 tui.scrolloff+=1` is 11.

    An option is one key; a section is set a key at a time (`:set tui?` is
    refused — `:set tui.<Tab>` lists its keys). Two things are not `:set`'s
    at all. [secrets] extra and [[alias]] hold credentials, and a value
    typed here is echoed and kept in the command history, so they live in
    your config file only — which is also why `:set secrets?` is refused.
    And keys are mapped in your config file, for now. Rules are SHOWN by
    `:set permissions.rules?` but taught with `:allow`, `:deny` and `:ask`
    (|rules|): they stack across files, so `=` would add, not set.

THE SECTIONS                                                      *config-tree*

    [tui]           glyphs, graph style, mouse, resume policy, and the
                    theme (|themes|)
    [tui.graph_glyphs]  `:graph`'s node glyphs, one character a role
    [tui.prompt]    the prompt pane: height, and start_mode (normal|insert)
    [tui.fold]      which of a transcript's blocks fold, and how each
                    starts (|folds|)
    [tui.status]    what the status row says, per kind of pane
                    (|statusline|)
    [tui.alerts]    every alert at once: which are off, the mark, the
                    bell and notifications (|alerts|)
    [tui.alert.*]   one alert each: when it holds, what the row says
                    (|alerts|)
    [keymap.*]      key remaps, per context (|keymap|)
    [merge]         squash_instruction — the brief `S` hands the model;
                    whether `:compact` and `S` open $EDITOR first
                    (|compact|)
    [continue]      the `:continue` strategy (|continue|)
    [agents]        child-session guardrails (|agents|)
    [turn]          max_output_tokens / max_context_tokens — ceilings on
                    one reply and on the context you work inside, or 0
                    for the model's own (|cost|)
    [cache]         prompt caching: on/off and entry lifetime
                    (|cost|)
    [exec]          bounds on an allowed command: timeout, output cap,
                    shell (|exec|)
    [files]         Shannon's file tools: what an edit must have read
                    first (|files|)
    [secrets]       redaction, issuer prefixes, sensitive paths, and env
                    vars lent to commands (|secrets|)
    [permissions]   your rules, and the shipped rules you drop
                    (|rules|)
    [claude_code]   Claude Code as a provider (a `claude-code/…` model).
                    Its model sees its tools' output UNREDACTED, so the
                    gate is the only layer: path_policy (a credential
                    path named; "deny" by default, while one it MAY
                    name asks) and web ("ask" by default,
                    or "allow" — which lets the model send what it has
                    read to any URL)
    [[alias]]       named substitutions, one table entry each — the
                    handles `:secrets` lists
    [memory]        instruction files, where memories are read and
                    written, and Claude Code's files (|memory|)
    [skills]        where skills are found (|skills|)
    [mcp]           MCP servers not used (today Claude Code's)

KEY REMAPPING                                                         *keymap*

    Keys are data, not wiring. A `[keymap.<context>]` table maps a key to
    an action by name; an unmapped key keeps its canonical-vim default.
    Remap only what you want changed — like the rest of the file, the
    table is a diff over the defaults. The ten contexts:

      normal  insert  visual        the three modes
      motion  text-object           operator-pending leaves (`d{motion}`)
      goto    window  z             after `g` / after CTRL-W / after `z`
      open-bracket  close-bracket   after `[` / after `]`

    :keymap             every context, every key in force, and the action
                        name each one is bound to
    :keymap <context>   one context on its own

    Read out of the table the program is RUNNING, so it already includes
    your own remaps — and every chord it prints is spelled the way the
    config accepts it, so a line can be pasted straight into your file.
    The bottom section lists actions nothing presses by default; those are
    bindable too. (`:config dump` will not help you here: the keymap is
    code, so the template carries examples and no action list.)

    An unknown action or context, or an unparseable chord, is reported and
    SKIPPED — the rest of your table still applies, so one typo never
    costs you every remap.

THE STATUS LINE                                                   *statusline*

    [tui.status]
    prompt = '[$mode] $pane{ · $model}{ · ctx $ctx}'   the prompt's row
    output, decision, graph                           those panes' rows
    page                       every read-only page — :help, :rules, :ls,
                               :messages, the inspector, …

    The bottom row says which pane you are in and what waits on you, in
    words you choose; `:config dump` shows the shipped rows. It is
    starship's shape, with brackets and parentheses left as text:

    $name  ${name}             a value, listed below
    { … }                      a group: shown when a value in it is, and
                               always when it holds none
    [text](style)              text in a style: bold italic dim underline
                               reverse strikethrough, fg:<colour>,
                               bg:<colour>, or a colour alone — a colour
                               as a theme writes one (|themes|) —
                               or a theme's role: `ui.caution` alone is
                               all of its style, `fg:ui.caution` its
                               colour, as the theme in force says it
    $fill                      the room the rest leaves, as spaces: what
                               follows sits at the right (vim's `%=`);
                               several share it evenly
    $alerts                    what waits on you: every alert that holds,
                               joined with ` · ` (|alerts|)
    ${alert.name}              one alert, placed here
    \$ \{ \} \[ \] \( \) \\    the character itself

    A `[` not written as [text](style) is text, so `[$mode]` is the mode
    in brackets, and `($readonly)` is the word in parentheses. A value the
    row does not have, a style word it cannot read, a `{` never closed:
    each is said when the config loads, and drawn as written.

    WHEN IT DOES NOT FIT the rightmost group goes first, then the next,
    and what is left is cut with `…` — so order is priority: put what you
    would keep on the left. A group showing what waits on you — $alerts,
    or an ${alert.name} — is never dropped, only cut: a row that clips is
    honest, one that hides a pending decision is not. A $fill gives its
    room up first.

    $alerts leaves out an alert the row places itself: the shipped rows put
    ${alert.hidden} at the right, past a $fill, so $alerts beside the
    pane's name does not say it again. The value written bare — $hidden —
    is a value like any other: dropped with its group when the row
    narrows, and said by $alerts as well.

    $activity is several values said as one, each that the row does not
    write itself.

    THE ROW IS ABOUT ONE SESSION: the one your prompt goes to, wherever
    you stand. Its model, its tokens, its `answering…`; what runs in other
    sessions is counted (`$sessions`), so a turn off-screen is never
    invisible. A row with no session to describe — `shannon tutor`, before
    `:new` or `:vnew` opens one — reads `no agent` in red, rather than
    describing nothing; a prompt is not sent then, and a command that acts
    on a session says why.

    WHICH SESSION the row describes — model, ctx, the token counts, lines,
    follow — is the one your next message goes to (|prompt-target|),
    wherever you stand. In a transcript that is the one you are in; in a
    viewer, even one opened over another transcript, it is the aim.

    The values, each empty when it has nothing to say:
      mode           NORMAL, INSERT, VISUAL, …
      pane           prompt, output, decision, graph, rules, …
      readonly       `read-only` where no text can be typed in (vim's
                     'nomodifiable', its [-], not its [RO] 'readonly',
                     which lets you type): every pane but the prompt,
                     though the graph's and a decision's keys still act.
                     No shipped row carries it: a key that would type
                     says so, as vim's E21 does
      help           the pane's section of this manual, as `:help` takes it
      position       the cursor's row of how many: `3/5` (graph, pages)
      model          the model answering, and the effort it is sent
      ctx            how full the context window is: `43%` (`*` for a
                     ceiling you set), coloured by tui.ctx_colors
      tokens_in      tokens this session has sent, cached or not: `10.4M`
      tokens_out     tokens it has been sent back: `40k`
      cache          what the prompt cache did: `85% cached`, `6.1k stored`
      size           the terminal's size: `183x59`
      lines          how many lines the transcript has
      follow         `↧` while the transcript follows its end, `‖` once not
      decide         a decision whose pane cannot show its keys
      queued         prompts waiting to be sent: `2 queued`
      running        this session's allowed command, running:
                     `running: cargo test`
      interrupt      `^C:stop` while CTRL-C would stop something — this
                     session's work, or a marked one's
      hidden         decisions no pane shows — hidden by `:hide`, or in
                     a session no window shows: `decision hidden ·
                     :decide 3` (|decision|)
      unshown        sessions with news no pane has drawn — a reply, a
                     turn ending, a summary landing, once nothing runs in
                     them — until one does: `news in session 3 · :b 3`
                     (|alerts|)
      activity       what is running: answering, distilling, continuing,
                     sessions, children, sends — in another session's
                     decision pane, led by the aimed session's name, as
                     it is that session's
      answering      `answering…` while this session's reply streams
      distilling     `distilling <session>…` while its summary is drafted
      continuing     `continuing…` while its cut-off reply is continued
      sessions       `2 sessions running` — sessions with a turn, a summary
                     or a continuation, when one is not this one
      children       child sessions running or held: `2 children running`
      sends          `:Send` sessions answering or held

    `:set tui.status.prompt=[$mode]\ $pane` changes a row at once (a space
    in the value is `\ `, as in vim); `:set tui.status.prompt?` shows it,
    and `:set tui.status.prompt&` puts the shipped one back.

    tui.status.tokens = "short"    token counts to the nearest tenth of a
                                   unit: `999`, `1.2k`, `40k`, `10.4M`
    tui.status.tokens = "exact"    every digit: `10415096`

    The inspector always shows the exact counts
    (|graph-inspector|).

ALERTS                                                                *alerts*

    [tui.alert.ctx]
    when   = 'ctx_percent >= 80'         when it holds
    say    = '[ctx $ctx](bold fg:red)'   what the status row says of it
    border = 'ui.caution'                the windows of each session it
                                         holds for: a heavy line
    mark   = true                        each session it holds for, in
                                         `:ls`: the glyph beside it
    bell   = true                        the terminal's bell, as it begins
                                         to hold
    notify = '$session is nearly full'   a desktop notification, as it
                                         begins to hold: true for
                                         Shannon's words, or yours

    [tui.alerts]
    disabled   = ["queued"]              alerts drawn nowhere
    glyph      = "🔔"                   what a mark draws: one to four
                                         cells, no control characters
    bell       = true                    every alert's bell: false for none
    notify     = true                    every alert's notification: false
                                         for none
    notify_via = "osc9"                  how a notification is sent:
                                         "osc9", or "tmux" inside tmux

    An alert is something waiting on you, by a rule you write: WHEN it
    holds, and what the row SAYS of it where $alerts or ${alert.name} is
    written (|statusline|) — a status format, styles and all.
    Seven ship. What waits on you: decide, queued, running, interrupt and
    hidden, each with no `when`, so each holds whenever its value
    ($decide, …) has something to say. And news you have not seen:
    `unshown` says on the row the first session that has it, and how many
    more, and
    `unshown_session` marks each in `:ls`, rings the bell and sends a
    notification as it arrives. $alerts says them in that order, then
    yours by name. A table of yours with a shipped alert's
    name changes only the keys it writes; `disabled`, which every config
    file adds to, draws an alert nowhere.

    A `when` is a fact, which holds when it is not 0, or a fact compared
    with a whole number: `>` `>=` `==` `<` `<=`. One fact to an alert —
    for either of two, write two alerts. A `when` that cannot be read is
    said when the config loads, and drawn where its alert would be, as
    `[name: when?]`: never silently dead.

    A `border` marks each window showing a session the alert holds for, and
    a `mark` each such session's `:ls` line, so each needs a `when` that
    names a session's fact. A border's colour or role (a style's words)
    shows where nothing else colours the border: the focused window keeps
    focus's colour, and the window your prompt goes to its accent. The heavy
    line shows on every one of them, since neither of those draws one.

    A `bell` and a `notify` go when the alert BEGINS to hold — once for
    each session it holds for, or once for the app's fact — so each needs a
    `when`. Nothing goes for what already holds when Shannon starts, when
    you change your alerts, or when you open a session: only what begins to
    hold after a frame has seen it. However many begin at once, one bell
    rings and one notification says the first, and how many more.

    A notification's words are Shannon's — `Shannon: <alert> · <session>`,
    or a shipped alert's own (`unshown_session`'s `news in`) — or yours,
    with `$alert` (the alert's name) and `$session` (`b3` and its name, or
    its id while it has none) in them. Never a `say`: what a
    value holds can be what the model wrote, and a notification leaves the
    terminal. Its words are stripped of control characters and of what
    would reorder or break them where your OS shows them (a bidi override,
    a line separator, a tag character), a secret's value said as its
    handle, and cut at 200 characters. Every language's letters, accents
    and joiners are kept. A notice on the row shows such a character as its
    escape (`\u{202e}`) instead of obeying it.

    It is sent as `OSC 9`, which ghostty, iTerm2 and WezTerm show. Inside
    tmux, set `notify_via = "tmux"` and add `set -g allow-passthrough on`
    to your tmux.conf; tmux drops it otherwise. Shannon cannot tell either
    for you. The bell is the terminal's own: what it does is your
    terminal's setting.

    A pane draws nothing you see while the terminal is not in front: a
    reply that ends while you work in another window is news, and rings.
    Shannon asks the terminal to say when it gains and loses focus; inside
    tmux, add `set -g focus-events on` to your tmux.conf. A terminal that
    never says is taken as in front, and any key you press there is.

    To quiet them for this run, `:set notui.alerts.bell` and `:set
    notui.alerts.notify`; to keep it, `bell = false` and `notify = false`
    under `[tui.alerts]` in your config file (`:config edit`).

    `:set tui.alert.queued.say=…` changes an alert already written, yours
    or one that ships. A new one is made whole, for this run:
    `:set tui.alert.built={when="running", bell=true}` — or written in a
    config file to keep it. A name with no alert takes nothing else, so a
    misspelt one is refused, not made.

    A session's facts are asked of each session by a border, a mark, a bell
    or a notification, and on the row of the session it is about — the one
    your prompt goes to:
      decision       decisions waiting on you: its calls, and its review
      review         1 while its summary waits for your review
      ctx_percent    how full its context window is, in percent: 0 before
                     its first answer
      queued         prompts queued for it
      running        its allowed commands, running
      answering      1 while its reply streams
      unshown        1 once something has arrived in its transcript since
                     a pane last drew it — a reply, a summary — and
                     nothing runs in it, until one does; what you did
                     there (your prompt, a stop, a delete) is not news.
                     Drawn, with room to show something, while the
                     terminal is in front — a viewer over
                     it, a screen over the whole frame (setup,
                     `:models`), a float over the panes (`:memories`,
                     `:skills`), or a pane squeezed to nothing, shows it
                     not; above `:effort`'s panel it is shown

    The app's own:
      sessions       sessions with work running: a turn, a summary, a
                     continuation
      children       child sessions running or held
      sends          `:Send` sessions answering or held
      unshown_count  sessions `unshown`

                                                          *news* *unshown*
    News is that fact: something arrived in a session — a reply, a turn's
    end, a summary — where you did not see it, and nothing runs there now.
    The row says `news in session 3`, `:ls` marks it, the notification
    says `news in` it. What you did yourself is not news: your prompt, a
    `<C-c>`, a delete. Nor is what a session held when Shannon started or
    you opened it: only what arrives after a frame has seen it counts. A
    reply you stopped half-read is marked, and rings nothing.
    The shipped pair, as a config writes it:

      [tui.alert.unshown]
      when = 'unshown_count'
      say  = '$unshown'

      [tui.alert.unshown_session]
      when   = 'unshown'
      mark   = true
      bell   = true
      notify = 'Shannon: news in $session'

THEMES                                                                *themes*

    tui.theme = "terminal"     your terminal's own colours, and the default
    tui.theme = "<name>"       themes/<name>.toml beside your config file,
                               else one Shannon ships by that name

    Shipped, each its authors' palette (themes/NOTICE.md says whose):

        terminal  catppuccin-mocha  catppuccin-macchiato  catppuccin-frappe
        catppuccin-latte  tokyonight-night  tokyonight-storm  tokyonight-moon
        tokyonight-day  gruvbox-dark  gruvbox-light  nord  dracula  one-dark
        solarized-dark  solarized-light  rose-pine  rose-pine-moon
        rose-pine-dawn  kanagawa-wave  kanagawa-dragon  kanagawa-lotus

    A file of yours named for a shipped theme is yours; `inherits =
    "nord"` starts one of yours from a shipped one.

    tui.theme_import = ["ghostty"]

    Directories of terminal palettes — a ghostty or iTerm2 theme file,
    sixteen colours and a ground — each a theme by its file's name, after
    yours and the shipped ones: `tui.theme = "Catppuccin Mocha"`.
    `ghostty` is ghostty's own theme directories, wherever this machine
    keeps them (its 463 bundled themes, and ~/.config/ghostty/themes); any
    other entry is a directory of yours, `~/` your home:

        tui.theme_import = ["ghostty", "~/palettes"]

    Only colours are read — a ghostty theme may set any ghostty option,
    and none of those is Shannon's.

    :themes            every theme, and a preview of one, in two panes

    The list holds every theme there is — shipped, yours, imported, under
    where each is from — and beside it a preview drawn in the theme under
    the cursor: a reply with its markdown and code, a tool's record, the
    prompt with its secrets, the status row, a refusal and a question, a
    decision with both its fills, the terminal's sixteen as the theme says
    them, `:graph`'s lanes, the manual's markup, and what loading it said.
    The preview pane's ground and border are the theme's too.

    The list is a page like any other: `j` and `k` move, and the preview
    follows; `/` and `n` find a theme by name (a search moves to it; the
    list stays whole). `<CR>` wears the theme under the cursor, as `:set
    tui.theme=<name>` would — for this run, and `:config edit` keeps one.
    The list is read when it opens: a file added since shows at the next
    `:themes`.

    Highlighting says what a piece of text IS — a keyword, a string, a
    command's flag, a line a write adds — and a theme says what each of
    those looks like. Every highlighter speaks the one vocabulary: the
    shell, in the decision pane and the transcript; a model's code blocks
    and a file a Read returned; markdown. So one theme paints them all
    alike, and a new one recolours what is already on screen.

    The shipped theme names TERMINAL colours, never values — `keyword` is
    your terminal's magenta, whatever its own theme makes magenta — so
    Shannon wears your terminal's palette, as `bat --theme=ansi` does.

    A theme file is TOML, a role per key:

        inherits = "terminal"           # the default; or another of yours
        keyword = "magenta"             # a terminal colour, by name
        comment = { fg = 8, italic = true }
        "function.call" = "#61afef"     # 24-bit
        "decision.string" = "light-cyan"

        [palette]                       # names for colours, used above
        lavender = "#b3a1e6"

    A colour is a name — black red green yellow blue magenta cyan white,
    each with a bright- form, and gray and dark-gray — or 0-255, #rrggbb, a
    [palette] name, or "default" for the terminal's own text colour. A
    table adds bold, italic and dim (`bold = true`, or helix's `modifiers =
    ["bold"]`). A key is one role however TOML spells it: quoted
    "function.call", unquoted function.call, or `call` under a [function]
    table — and a [decision] table holds the pane's.

    The roles are named as tree-sitter's highlight captures are, which is
    the vocabulary neovim, helix and zed themes share. A key may be any
    prefix of a role: `function` paints `function.call` unless that is
    named itself. The most specific key wins, inherited or yours.

        comment  keyword  operator  punctuation  label  tag  attribute
        string  string.special  number  constant  constant.builtin
        function  function.call  function.builtin  function.macro
        type  type.builtin  module
        variable  variable.builtin  variable.parameter  variable.special
        variable.member
        markup.heading  markup.raw  markup.raw.fence
        diff.plus  diff.minus  ui.tool  ui.tool.output  ui.muted
        diagnostic.error  diagnostic.warning  diagnostic.info
        diagnostic.hint
        ui.background  ui.text  ui.text.quiet  ui.text.quiet.thinking
        ui.text.faint  ui.nontext  ui.cursor
        ui.statusline  ui.tabline  ui.tabline.selected
        ui.border  ui.border.focused  ui.border.aimed
        ui.border.user  ui.chosen  ui.bar  ui.bar.waiting
        ui.menu.selected  ui.safe  ui.caution  ui.danger  ui.danger.sign
        ui.caution.fill  ui.danger.fill  ui.question  ui.operand  ui.secret
        ui.secret.leaked
        ui.graph.tree  ui.graph.running  ui.graph.held  ui.graph.shared
        ui.graph.mark  ui.graph.continued  ui.graph.lane.1  ui.graph.lane.2
        ui.graph.lane.3  ui.graph.lane.4  ui.graph.lane.5  ui.graph.lane.6
        ui.analyze.carrier.1  ui.analyze.carrier.2  ui.analyze.carrier.3
        ui.analyze.carrier.4
        ui.help.heading  ui.help.tag  ui.help.ref  ui.help.code
        ui.listing.heading

    The `ui.` roles are the screen's own: every border, the verdict's
    colours, the status row, `:graph`'s lanes, the manual's markup. A
    table may give a background too, `{ fg = "…", bg = "…" }`;
    `ui.background` is every pane's ground and the colour of text with no
    role — unset, as shipped, your terminal's own. `ui.border.user` is the
    rules above and below your message in a transcript — unset, as shipped,
    so it is whatever `ui.border` is. `ui.text.quiet.thinking` is what the
    model thought, in a transcript — unset, as shipped, so it is
    `ui.text.quiet`'s colour, always in italic and dim. `ui.tool.output` is
    what came of a tool call — unset, as shipped, so it is `ui.tool`'s
    colour, always dim; `ui.muted` is a read's line numbers.

    `ui.cursor` is the cursor, in every pane — there is one, the
    terminal's, where your keys go: its `bg`, or a bare colour, is the
    block's (vim's `Cursor` group), told to the terminal as #rrggbb and
    given back when Shannon exits or `$EDITOR` opens. Unset, a theme that
    paints `ui.background` draws it in that ground's text colour, since
    your terminal chose its own cursor for its own ground; unset with no
    ground, as shipped, or "default", it is your terminal's. Inside a
    Visual range the cell under it is drawn plain, so it is not lost in
    the selection.

    Its shape says the mode your keys go to — a block in Normal, a bar in
    Insert — and `tui.guicursor` sets it, as neovim's 'guicursor' does
    (|cursor|).

    A [palette] entry named for a terminal colour — black … bright-white —
    says what that colour IS, for every role, the inherited ones too:

        [palette]
        magenta = "#cba6f7"             # every role drawn magenta, now this

    So a theme drawn for the sixteen colours of a terminal is a theme here
    with sixteen entries and a `ui.background`.

    In shell, a command's name is `function.call`, a flag
    `variable.parameter`, a quoted string `string`, and what the shell
    expands (`$X`, `$(…)`, `X=1`) `variable.special`.

    THE DECISION PANE is the one place green, yellow and red are a verdict
    (|decision|), so a string painted green there would read as
    "this runs". A `decision.` key paints the pane; a role it does not name
    is painted as in code. The pane shows a command and, for a Write, the
    file's own syntax: the shipped theme keeps every role it paints there
    off the verdict's hues, and a theme that puts one on them — or on a
    withheld value's colour — is told so when it loads. Not diff.* and
    diagnostic.*: their colour is a severity. The verdict's own colours
    (`ui.safe`, `ui.caution`, `ui.danger`) are a theme's too, and a theme
    that paints two of them alike is told so: a refusal would read as an
    allow.

    `:reload` reads the theme file again, so an edit to it shows without a
    change to the setting. A theme that cannot be read — no such file, not
    TOML, a name that is not a plain name — is refused with the reason,
    and the theme in use stays. Every pane shows a new theme at once — an
    open decision and an open `:analyze` too.

CURSOR                                                     *cursor* *guicursor*

    There is one cursor, your terminal's, where your keys go, in every
    pane. Its colour is the theme's `ui.cursor` (|themes|). Its
    shape says the mode your keys go to, as neovim's does, so a change of
    mode shows where you type as well as on the status row:

      Normal, Visual                  a block
      Insert                          a bar
      an operator (`d`, `c`, `y`)     an underline, until the next key
        or `r` waiting
      the `:` line                    a block at the end of what you
                                      typed, a bar inside it

    tui.guicursor = "n-v-c-sm:block,i-ci-ve:ver25,r-cr-o:hor20"

    This is neovim's 'guicursor', read by neovim's rules, so a value from
    your init.vim works here. A value is parts joined by commas; a part is
    modes, a colon, then arguments; modes are joined by `-`, and so are
    arguments. The default, part by part:

      n-v-c-sm:block      Normal, Visual, the `:` line: a block
      i-ci-ve:ver25       Insert, inside the `:` line: a bar
      r-cr-o:hor20        Replace, an operator waiting: an underline

    Modes:
      n           Normal
      v           Visual, every kind
      o           an operator waiting for its motion: `d`, `c`, `y`
      i           Insert
      r           Replace, and `r` waiting for its character
      c           the `:` line, its cursor at the end
      ci          the `:` line, its cursor inside what you typed
      a           every mode
      ve sm cr t  neovim's, for modes Shannon never enters (an exclusive
                  selection, showmatch, overstrike on the `:` line, a
                  terminal): read, and never met

    Arguments:
      block       a block
      verN        a bar, N percent of a cell wide
      horN        an underline, N percent of a cell high
      blinkonN    blinking: N milliseconds shown
      blinkoffN   blinking: N milliseconds hidden
      blinkwaitN  N milliseconds before the blinking starts
      any other   a highlight group (`Cursor/lCursor`): read, and not
      word        used, since the colour is `ui.cursor`'s

    Every mode starts as a steady block. Parts are read in order, and a
    part sets only what it names, for the modes it lists. So a last part
    that names every mode and no shape keeps each mode's shape, and makes
    them all blink:

      :set tui.guicursor+=a:blinkon500-blinkoff500

    `+=` adds a part with its comma, `-=` takes one out, `^=` puts one
    first, as vim's do on 'guicursor'; `:set tui.guicursor&` is the
    default again. `:set tui.guicursor?` shows the whole value, which is
    what `guicursor` under [tui] in a config file takes.

    A terminal is told less than this says. It hears one of three shapes,
    blinking or steady (DECSCUSR, as neovim's terminal UI tells it), and
    draws that shape its own way, blinking on its own clock. So `ver25` and
    `ver90` are the same bar, blinkwait is never told, and blinkon and
    blinkoff say one thing only: both above 0, it blinks; either 0 or
    unset, it is steady — `a:blinkon500` alone is steady, as in neovim.
    The default names no blink, so the cursor is steady whatever your
    terminal's own setting; neovim's default does the same.

    "" tells the terminal nothing: its own shape and blink stay, as
    neovim's `:set guicursor=` leaves them.

    A value neovim refuses is refused, with the reason, and the shapes in
    use stay: a part with no `:`, a mode it does not know, `ver`, `hor`
    or a `blink` word with no number after it, `ver0` or `hor0`.

    The shape and the colour are given back when Shannon exits or
    `$EDITOR` opens, and taken again after.

PLUGINS                                            *plugins* *local-additions*

    Shannon is built to be MODIFIED, not merely configured — the value that
    makes vim vim. A plugin surface (custom commands, merge and summary
    strategies, keymaps, glyph sets) is planned; when it lands, installed
    plugins document themselves HERE, following vim's local-additions
    convention.

    Here to CONTRIBUTE? That is the intent. The pluggable strategies and the
    data-driven command and keymap tables exist so behavior can be added at
    the edges without forking the core. Contributors are who these internals
    are written for — see AGENTS.md and the design docs in the repository.

For the authoritative, always-current list, run `:config dump` — it is
generated from the very defaults the program runs on, so it can never drift
from this summary.
"##;

const MEMORY: &str = r#"==============================================================================
12. MEMORY AND SKILLS                                                 *memory*

What the model is told beyond the conversation: INSTRUCTION files, how to
work here (AGENTS.md); MEMORIES, what an earlier session learned and kept;
and SKILLS, a procedure loaded by name when it fits the task. Each is a
file you can read and edit, [memory] and [skills] say where they are, and a
session records what it was told, as it was sent.

    :memories             the instruction files and memories a session is
                          told, where the model writes, and what was not
                          read
    :skills               the skills found, and those not used
    :skill <name> [args]  run a skill now (|skills|)
    :set memory=<mode>    whether the model writes memories: auto, ask or
                          off (|memory-write|)

    `:memories` and `:skills` open one screen, a float over your panes
    (fzf's shape): a file a row, under the place it was found, with what
    becomes of it — and, when the float is wide enough, the file under the
    cursor shown beside the list —

        sent      told whole with every turn
        index     a memory location's MEMORY.md, told; its lines name
                  memories the model reads when one is relevant
        listed    named to the model, read when it is wanted
        claude's  Claude Code reads it itself, in its session
        not read  and why: a credential path, a skill another shadows
        disabled  you disabled it (|disabled|)

    It moves as every pane does, by the same keys: j and k with a count, gg,
    G and {n}G, CTRL-D and CTRL-F (Home and End too). Tab turns to the next
    list — memories, skills, MCP — and BackTab (h) back; <Enter> opens the
    file under the cursor in $EDITOR — the real file; the screen stays
    open, read again — x disables it or enables it again, and q leaves (Esc
    does not). Its keys are on the status row while it is open, and the
    panes behind it take none. What a key did, or why it did nothing, is
    said at the top of the screen, and kept in `:messages`. A file the
    model is not shown is not shown beside the list either; it says why.

    The MCP list is Claude Code's servers, as its last turn here started
    them (connected, or down and why); none are known before one. A server
    is Claude Code's to configure (`claude mcp`), so x is the one action
    there: disabled, a call to any of its tools is refused.

    d REMOVES it, asked first on the screen, saying what goes: a memory
    and its lines in the MEMORY.md beside it, or a skill's directory
    whole. y removes; any other key, Esc included, keeps it. It is
    deleted, not hidden — x disables instead. An instruction file, an
    index, a location you configured and a file not read are not removed
    here, and the screen says why; a link is unlinked, never followed.

    n makes a NEW one: type its name, Enter, and its template opens in
    $EDITOR — a memory where the model writes them (frontmatter: name,
    description, type; then the fact), a skill as <name>/SKILL.md in the
    first [skills] location that is one place ({config}/skills) — never a
    name a skill found already has. Saved, it is made, and a memory with a
    name and a description gets its line in its MEMORY.md; left as it
    was or deleted, nothing is made — an editor that fails keeps what it
    saved, and says it failed. N
    asks the MODEL instead: the screen closes and a request is drafted in
    the prompt — finish it (`A`) and send it, or not. A draft of yours
    already there is never replaced.

DISABLED                                                             *disabled*

    A file you disable is not told, not listed, and a read of it is
    refused at the gate, whatever approves its place. What the model
    already has stays in its context — nothing is taken out of a session
    for you; the next turn says a file it was told is gone.

        [memory]
        disabled = ["~/notes/team-memory/old.md"]   # a file, or a directory
        [skills]
        disabled = ["review", "~/.claude/skills"]   # a name, or a place
        [mcp]
        disabled = ["github"]                       # a server, by name

    A skill by its name disables every skill of that name — its directory's
    or the name its frontmatter gives — called any way: `review` holds
    /review and a plugin's p:review; p:review holds only that plugin's.
    Loading one — `:skill`, the model's `skill` tool, Claude Code's own
    Skill tool — is refused, saying which entry disables it. In a Claude
    Code session claude still LISTS its own skills, and still reads its
    own memory index itself; loading the skill, and reading a memory's
    file, are what is refused.

    Disabling is of files and places: a memory disabled is refused, and
    the MEMORY.md beside it is still told as it is — its line too. To
    silence a whole memory location (a global one, or a project's), put
    the location itself in [memory] disabled.

    Claude Code's own CLAUDE.md is read by claude itself, in its session,
    and nothing Shannon refuses reaches that: x there says so. [memory]
    claude = "off" stops it, with the rest of Claude Code's files.

    A server disabled still starts, and its tools are still listed to the
    model; a call to any of them is refused, saying how to use it again —
    and so is its prompt loaded as a skill, and a read or listing of its
    resources. A name is matched as Claude Code spells its tools, so
    my_server names "my server". A listing of every server's resources
    names none, and runs.

    x on the screen disables for this session, as `:set
    skills.disabled+=review` would. On a disabled row it takes back the
    entry naming that row; one naming a place that holds more is said,
    with the `:set` line that takes it back. The lists STACK, and your
    config keeps them: Shannon never writes your config, so add the line
    there (`:config edit`). x enables an entry a config file holds for
    the session, and says that file disables it again at the next launch
    or `:reload`.

WHAT A TURN TELLS                                                       *told*

    A session's first turn tells the model everything found; each later
    turn tells only what changed. A file edited is sent again, whole; one
    no longer found is said to be gone. It goes on the turn's own message,
    ahead of your prompt, so the session holds exactly what the model saw,
    and the transcript says what in one line:

        ⚙ Shannon told the model: AGENTS.md · memory MEMORY.md · the skills list

    Nothing is cached: edit a file from `:memories`, in another editor, or
    let the model write one, and the next turn carries the change. A file
    is read as text, an instruction or index file up to 40 KB, all of them
    together up to 128 KB — past it a file is named, not read — and the
    skills list up to 8 KB.

    What a file says is redacted as a command's output is
    (|secrets|): a value you marked secret leaves as its handle, a
    credential's shape as [redacted: …], and the line says the file was,
    `AGENTS.md (redacted)`. A file in a credential path — as named, or
    where a link in it leads — is never read, and `:memories` lists it
    under Not read.

WHERE THEY ARE                                                 *memory-places*

    [memory] and [skills] name LOCATIONS, each a file or a directory:

        [memory]
        instructions = ["AGENTS.md", "SHANNON.md"]
        paths = ["{config}/memory", "{config}/projects/{project}/memory"]
        write = "{config}/projects/{project}/memory"

        [skills]
        paths = ["{config}/skills", ".shannon/skills"]

    A location that begins with ~, /, {config} (Shannon's config
    directory), {start} (where it started), {claude-home} (Claude Code's
    home: $CLAUDE_CONFIG_DIR, else ~/.claude) or {claude-memory} (Claude
    Code's memory directory for this project, |memory-claude|) is
    one place. Anything else
    is looked for where Shannon started and in each directory above it up
    to the repository's root, and read at every level it is found, the
    root's first — but under a .shannon/ only where you trust the directory
    holding it (|config-project|). {project} is filled in wherever
    it stands: this
    project's name, as memory.project says —

      "hash"         the directory Shannon started in: its name and a short
                     hash of its path (my-project-3f9a2c71), each directory
                     its own project. The default.
      "claude-code"  as Claude Code names it, from the repository's root
                     (-Users-me-my-project): a subdirectory and every
                     worktree are one project.

    Changing it moves where memories are kept — move the old directory
    yourself. Like memory.write, only your own config sets it.

    The project's root is the nearest directory, from where Shannon
    started upward, holding one of memory.project_root's markers —
    gutentags' list, each a file or a directory:

        project_root = [".git", ".hg", ".svn", ".bzr", "_darcs",
                        "_FOSSIL_", ".fslckout"]

    `["package.json"]` makes a package its own project; `[]` (or no
    marker found) reads only where Shannon started. It decides what a
    relative location may read, so only your own config sets it. Claude
    Code's name for a project ("claude-code" above) is git's either way.

    What a location holds depends on its list:

      instructions   a file, read whole; a directory, each *.md in it
      memory.paths   a directory: its MEMORY.md, read, and its other files,
                     listed for the model to read when one is relevant; a
                     file, read whole
      skills.paths   a directory: each <name>/SKILL.md in it; a file, one
                     skill

    The three lists STACK: your config's list REPLACES the default, and a
    file that is not yours, one you `:source`, ADDS to what is there — so
    a project's file brings its own skills without dropping yours. Yours
    come first, whatever order the files were read in: the order is which
    skill a name means.
    `memory.write` is yours only: another file's is ignored, and said so
    (|source|).

    Every memory and skill location that exists is an approved place for
    reads, so the model reads a memory, or a skill's own files, without
    asking. (An instruction file is told whole and needs no rule.) `:rules`
    lists those rules apart, under FROM [memory] AND [skills] — the rules of
    the session in front, so a Claude Code session lists its own; `:analyze`
    judges by them too.

SHARING MEMORIES                                               *memory-share*

    A location without {project} is the same place wherever Shannon starts:
    one memory for many projects. {config}/memory is one by default — a
    global memory every session is told. Add your own (your list replaces
    the default, so keep what you want of it):

        [memory]
        paths = ["{config}/memory", "{config}/projects/{project}/memory",
                 "~/notes/team-memory",
                 "~/.claude/projects/-Users-me-work/memory"]

    The last is the memory Claude Code kept for another project, readable
    as its memories are carved out of ~/.claude (|memory-claude|).
    A Claude Code session is told each of these as a session through the
    API is; its own memory, claude reads itself.

WHERE THE MODEL WRITES                                          *memory-write*

    memory.write is one directory, yours and writable by no other account,
    made when it is missing. The model is told how to keep memories there:
    a file per memory, and a line for each in its MEMORY.md. In a Claude
    Code session its own memory directory takes that place instead
    (|memory-claude|). memory.mode, or `:set memory=`, says what a
    write there does:

      "auto"   it runs unasked; deleting a memory still asks. The default.
      "ask"    each write there asks
      "off"    the model is not told how, writes there are not approved,
               and the directory is not made; what `paths` names is still
               read. Claude Code keeps no memories of its own either.

    "auto" holds only where the directory is yours: one another account
    owns, one another account can write in, or a link, is not approved,
    and each write asks — the message line says why, and the model and
    `:memories` are told so. Others READING it is your umask's business:
    what an unasked write must not meet is a name another account planted.

    The model writes in its own turn, through the gate, so every memory it
    keeps is a tool call in the transcript. To change one, edit its file
    from `:memories`; to forget one, delete the file.

CLAUDE CODE'S FILES                                            *memory-claude*

    memory.claude says who reads Claude Code's own files: CLAUDE.md (found
    as AGENTS.md is, and {claude-home}/CLAUDE.md), .claude/skills and
    {claude-home}/skills, and its memories —
    {claude-home}/projects/<the repository, its / as ->/memory,
    {claude-memory}. {claude-home} is Claude Code's home as it finds it:
    $CLAUDE_CONFIG_DIR when set, else ~/.claude.

      "auto"   a Claude Code session: claude reads them itself, and
               Shannon never sends them again; its memory directory is
               where the model writes memories, as claude's own prompt
               says, so Shannon tells it of no other. A session through the
               API does not read them. The default.
      "on"     a session through the API reads them too, Claude Code's
               memories as any memory location (it writes in its own)
      "off"    neither: a Claude Code session runs with its CLAUDE.md and
               its memories turned off (its skills it still reads), and
               keeps Shannon's memory as a session through the API does

    Where they are read, their places are approved for reads, and under
    memory = "auto" Claude Code's memory directory for writes, in a Claude
    Code session only — once Claude Code has kept the project, from the end
    of its first turn there. Shannon makes nothing under ~/.claude for a
    project Claude Code never ran in. Under memory = "off", claude runs with
    its memory off, and :memories says so.

    Its home is a credential path (|secrets|) — {claude-home} and
    ~/.claude both — but for what holds no credential: [secrets]
    paths_except carves its memories, CLAUDE.md and skills out of it by
    default. The rest — its credentials, settings, and every other
    project's conversations — is still refused; so is ~/.claude.json
    beside it, which holds an API key, and its backups. A project's own
    CLAUDE.md and .claude/skills are not under it.

    Not followed: a memory directory moved by Claude Code's own
    autoMemoryDirectory setting — Shannon does not read Claude Code's
    settings.

SKILLS                                                                *skills*

    A skill is a SKILL.md in a directory named for it: frontmatter saying
    what it is for, then the procedure.

        ---
        description: Review the working tree before a commit.
        ---
        Read `git diff`, then for each file ... $ARGUMENTS

    The model is told each skill's name and description, not its body. In
    a session through the API it loads one with the `skill` tool when one
    fits; the call and its answer are in the transcript, as any tool
    call's are. In a Claude Code session the list names each skill's file,
    which claude reads.

    :skill <name> [args]   run one now: a turn on the session in front,
                           whose prompt says to run it and which carries
                           its text, so the session records what the model
                           was given. $ARGUMENTS in the body becomes
                           [args]; without one, they follow the body.
                           Refused while that session is busy. <Tab>
                           completes the names. Claude Code's own skill,
                           in its session, goes as the prompt alone:
                           claude loads it

    A name found in two locations is the first one's, in the order the
    list gives them; `:skills` lists the other under Not used, with the
    one that shadows it. A body, its arguments placed, is sent up to
    64 KB; a skill that does not load is recorded as NOT LOADED, with why.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// **A chord is one key, however it is spelled** — vim writes `CTRL-W q`
    /// in a table and tags it `CTRL-W_q`; read as two keys, the row defined
    /// a bare `q`, and `:help q` opened `<C-w>q` under SPLITTING (Steven,
    /// 11-08). `h/j/k/l` after one is four chords.
    #[test]
    fn a_chord_is_one_key_however_it_is_spelled() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        let at =
            |asked: &str| anchor_line(&page, asked).unwrap_or_else(|| panic!("`:help {asked}`"));
        let close = at("<C-w>q");
        assert!(
            lines[close].trim_start().starts_with("CTRL-W q"),
            "{}",
            lines[close]
        );
        for spelled in ["CTRL-W q", "CTRL-W_q", "^Wq", "ctrl-w q"] {
            assert_eq!(at(spelled), close, "`:help {spelled}`");
        }
        assert_eq!(anchor_line(&page, "q"), None, "a bare `q` is no chord's");
        // A chord's first half, asked alone, is the chords' table — and a
        // `{count}` ahead of a chord is no part of its name (stop 17, A M2).
        let chords = at("CTRL-W");
        assert!(lines[chords].contains("CTRL-W"), "{}", lines[chords]);
        for spelled in ["<C-w>", "ctrl-w", "^w"] {
            assert_eq!(at(spelled), chords, "`:help {spelled}`");
        }
        for (asked, opens) in [
            ("<C-w>+", "taller"),
            ("<C-w>-", "taller"),
            ("<C-w><", "wider"),
        ] {
            assert!(lines[at(asked)].contains(opens), "`:help {asked}`");
        }
        let below = at("<C-w>j");
        assert!(
            lines[below].trim_start().starts_with("CTRL-W h/j/k/l"),
            "{}",
            lines[below]
        );
        assert_eq!(keys_defined("    CTRL-W q     close it"), ["CTRL-Wq"]);
        assert_eq!(
            keys_defined("    CTRL-W h/j/k/l     move"),
            ["CTRL-Wh", "CTRL-Wj", "CTRL-Wk", "CTRL-Wl"]
        );
        assert_eq!(
            keys_defined("    j k / n N    move"),
            ["j", "k", "/", "n", "N"],
            "a `/` alone is a key"
        );
        assert_eq!(key_spelling("CTRL-W _"), "CTRL-W_", "`_` alone is the key");
    }

    /// **A table of words defines no keys** — `rg` is shaped like `gg`, so
    /// a row alone cannot tell; its table can: `grep` beside it, `green`
    /// beside `red`, "no marks" beside "a run" (`:help run` opened the
    /// squash table). A blank line between a definition list's entries does
    /// not end its table, a group's heading does not stop a key table being
    /// one, and prose at the rows' indent ends it.
    #[test]
    fn a_table_of_words_defines_no_keys() {
        let cells = |text: &str| -> Vec<bool> {
            key_cells(text.lines())
                .iter()
                .map(Option::is_some)
                .collect()
        };
        let programs =
            "      grep   a read of the files it names\n      rg     a read that skips\n";
        assert_eq!(cells(programs), [false, false], "rg is a program here");
        let list = "      no marks   Select a branch\n\n      a run      Mark a stretch\n";
        assert_eq!(
            cells(list),
            [false, false, false],
            "a blank line keeps the table"
        );
        let keys = "    Motions:\n        w       a word\n        gg      the top\n";
        assert_eq!(cells(keys), [false, true, true], "a heading is no row");
        let two =
            "    grep   a program\n    rg     one too\n    The prose after.\n    gg     the top\n";
        assert_eq!(
            cells(two),
            [false, false, false, true],
            "prose ends a table"
        );
        // A key-shaped row first: the table's words still decide it.
        let first = "      rg     a read\n      grep   a program\n";
        assert_eq!(cells(first), [false, false], "a word later unmakes it");
        // A shallower row closes the deeper table: `rg` opens its own.
        let nested = "      grep   a\n    gg     top\n      rg     b\n";
        assert_eq!(cells(nested), [false, true, true]);
        assert_eq!(cells("w       a word\n"), [false], "a row is indented");
        // Over every table in the manual, not only Normal mode's.
        let page = compose(&crate::command::commands_chapter());
        let defined: Vec<String> = (key_cells(page.lines()).into_iter().flatten())
            .flat_map(|cell| cell.keys)
            .collect();
        for word in ["run", "rg", "sed", "red"] {
            assert!(
                !defined.iter().any(|key| key == word),
                "`{word}` is a word in its table"
            );
        }
    }

    /// **The words people arrive with find their place** — vim gives a
    /// place several tags: `focus` is the window-focus section (Steven,
    /// 11-08: "Can we have `:help focus` also point to `:help
    /// window-focus`?"), and `clipboard`, `copy` and `yank` the register's
    /// ("I was struggling to figure out how to copy to my clipboard").
    #[test]
    fn the_words_people_arrive_with_find_their_place() {
        let page = compose(&crate::command::commands_chapter());
        let at =
            |asked: &str| anchor_line(&page, asked).unwrap_or_else(|| panic!("`:help {asked}`"));
        assert_eq!(at("focus"), at("window-focus"));
        for word in ["clipboard", "copy", "yank"] {
            assert_eq!(at(word), at("register"), "`:help {word}`");
        }
    }

    /// **The keys the tutor teaches have rows** (review stop 17, F L2): these
    /// opened the chapter list, though the manual's header promises "a key".
    #[test]
    fn the_keys_the_tutor_teaches_have_rows() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        let row = |asked: &str| anchor_line(&page, asked).map(|at| lines[at].trim_start());
        for (asked, opens) in [
            ("<Esc>", "<Esc>  CTRL-["),
            ("CTRL-[", "<Esc>  CTRL-["),
            ("<C-[>", "<Esc>  CTRL-["),
            ("<CR>", "<CR> "),
            ("CTRL-X CTRL-E", "CTRL-X CTRL-E"),
            ("<C-x><C-e>", "CTRL-X CTRL-E"),
            ("CTRL-W j", "CTRL-W h/j/k/l"),
        ] {
            assert!(
                row(asked).is_some_and(|row| row.starts_with(opens)),
                "`:help {asked}`: {:?}",
                row(asked)
            );
        }
    }

    /// **A bare key is Normal mode's, or nothing** (Steven, 11-08): `:help q`
    /// maps to nothing and `:help :q` to quit; a pane's own keys answer only
    /// from their chapter. The measured class: `a`, `A`, `v` opened the
    /// decision pane's, `o`, `r`, `V` the graph's, `m` an `M` by case; `f`
    /// found nothing beside `f{c}`, and `*` and `/` opened any prose holding
    /// the characters of `***` or `*/*`.
    #[test]
    fn a_bare_key_is_normal_modes_or_nothing() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        let row = |asked: &str| anchor_line(&page, asked).map(|at| lines[at].trim_start());
        // `U` and `g` found `u` and `G` by case: one keystroke's case is the
        // key (stop 17, A L5).
        for nothing in ["q", "Q", "s", "S", ")", "U", "g"] {
            assert_eq!(row(nothing), None, "`:help {nothing}`");
        }
        let quit = row(":q").expect("`:help :q`");
        assert!(quit.starts_with(":q[!], :quit[!]"), "{quit}");
        assert_eq!(row(":quit"), Some(quit));
        assert_eq!(row("quit"), Some(quit), "a command's whole name reads bare");
        assert!(
            row("ls").is_some_and(|row| row.contains(":ls[!]")),
            "and an abbreviation of two keys"
        );
        for (asked, opens) in [
            ("a", "i  a "),
            ("i", "i  a "),
            ("A", "I  A "),
            ("I", "I  A "),
            ("o", "o  O "),
            ("O", "o  O "),
            ("v", "v  V "),
            ("V", "v  V "),
            ("m", "m{a-z}"),
            ("f", "f{c}"),
            ("F", "f{c}"),
            ("r", "r{char}"),
            ("*", "*  #"),
            ("/", "/{pattern}<CR>"),
            ("?", "?{pattern}<CR>"),
            ("CTRL-F", "CTRL-F  CTRL-B"),
            ("CTRL-U", "CTRL-D  CTRL-U"),
        ] {
            let found = row(asked).unwrap_or_else(|| panic!("`:help {asked}`"));
            assert!(found.starts_with(opens), "`:help {asked}` opened {found:?}");
        }
        assert!(row("decision").is_some_and(|row| row.starts_with("THE DECISION")));
        assert!(
            no_such_topic("q").contains("`:help :q`"),
            "{}",
            no_such_topic("q")
        );
        assert!(!no_such_topic("nosuch").contains("for the command"));
        assert!(
            !no_such_topic("w foo").contains("for the command"),
            "arguments: that is no form of `:help :w` (stop 17, A L3)"
        );
    }

    /// **A tag is defined once** — two with one name, and `:help` opens
    /// whichever comes first (vim's `:helptags` refuses them; stop 17, C).
    #[test]
    fn no_tag_is_defined_twice() {
        let page = compose(&crate::command::commands_chapter());
        let mut tags = marked(&page, Mark::Tag);
        assert!(tags.len() > 100, "premise: the manual's tags");
        tags.sort_unstable();
        let doubled: Vec<&&str> = tags
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| &w[0])
            .collect();
        assert!(doubled.is_empty(), "{doubled:?}");
    }

    /// **Regrouping the chapters moved text, not names** (11-08): each name
    /// the old chapters answered to opens on its own heading, chapter or
    /// section now — and `:help marks` on the marks a lesson teaches.
    #[test]
    fn every_old_chapter_name_still_opens_its_heading() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        for (asked, heading) in [
            ("buffers", "5. BUFFERS AND TABS"),
            ("tabs", "TAB PAGES"),
            ("child", "CHILD SESSIONS"),
            ("subagent", "CHILD SESSIONS"),
            ("lifecycle", "TURN LIFECYCLE"),
            ("permissions", "8. DECISIONS"),
            ("security", "8. DECISIONS"),
            ("rules", "9. RULES"),
            ("secrets", "10. SECRETS"),
            ("marks", "MARKS IN A TRANSCRIPT"),
            ("window-marks", "MARKS: ONE QUESTION"),
        ] {
            let at = anchor_line(&page, asked).unwrap_or_else(|| panic!("`:help {asked}`"));
            assert!(
                lines[at].starts_with(heading),
                "`:help {asked}` opened {:?}",
                lines[at]
            );
        }
    }

    /// **A key no table lists is found at its tag** (`*CTRL-C*`, vim's), and
    /// however it is spelled — `CTRL-C` sat in a table of situations, which
    /// is words.
    #[test]
    fn a_key_is_found_at_its_tag_however_it_is_spelled() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        let at = anchor_line(&page, "CTRL-C").expect("`:help CTRL-C`");
        assert!(lines[at].contains("*CTRL-C*"), "{}", lines[at]);
        for spelled in ["<C-c>", "^c", "ctrl-c"] {
            assert_eq!(anchor_line(&page, spelled), Some(at), "`:help {spelled}`");
        }
    }

    /// **The manual's marks, as vim reads them** — a tag and a link hold a
    /// name; a literal is read first, so what it quotes is not marked again;
    /// a key is vim's notation, never a shell's `${VAR}`, a `{{handle}}` or
    /// a comparison; the tutor's `*…*` is emphasis.
    #[test]
    fn the_manual_is_marked_as_vim_reads_help() {
        fn read(line: &str, dialect: Dialect) -> Vec<(&str, Mark)> {
            let marked = marks(line, dialect);
            marked
                .into_iter()
                .map(|(at, mark)| (&line[at], mark))
                .collect()
        }
        let manual = |line| read(line, Dialect::Manual);
        assert_eq!(
            manual("allow * /tmp/x/**   anything there"),
            [],
            "no tag spans a space"
        );
        assert_eq!(
            manual("deny * re:(^|/)\\.git(/|$)"),
            [],
            "a regex's pipes link nothing"
        );
        assert_eq!(
            manual("see |graph| and *graph-keys*"),
            [("|graph|", Mark::Link), ("*graph-keys*", Mark::Tag)]
        );
        assert_eq!(
            manual("a `*not a tag*` here"),
            [("`*not a tag*`", Mark::Literal)]
        );
        assert_eq!(
            manual("CTRL-W then <Esc>, {count}x, <C-w>q, <id>"),
            [
                ("CTRL-W", Mark::Key),
                ("<Esc>", Mark::Key),
                ("{count}", Mark::Key),
                ("<C-w>", Mark::Key),
                ("<id>", Mark::Key)
            ]
        );
        assert_eq!(manual("${HOME} and {{secret.1a2b}} and a < b > c"), []);
        assert_eq!(manual("{{x}} is a handle"), [], "its inner braces too");
        assert_eq!(manual("a*b*c"), [], "a tag stands apart");
        assert_eq!(manual("NOCTRL-X is a word"), [], "a key starts a word");
        let tutor = |line| read(line, Dialect::Tutor);
        assert_eq!(
            tutor("works *with* you, **NOTE** it, *every* time"),
            [
                ("*with*", Mark::Emphasis { strong: false }),
                ("**NOTE**", Mark::Emphasis { strong: true }),
                ("*every*", Mark::Emphasis { strong: false })
            ]
        );
        assert_eq!(tutor("2 * 3 * 4"), [], "a spaced `*` is arithmetic");
    }

    /// **The manual lists every status-line value, and only values** —
    /// counted from the type (`status_line::Var::ALL`), never from the page:
    /// a value added without its line, or a line naming no value, fails.
    #[test]
    fn the_manuals_status_values_are_the_vars() {
        use crate::status_line::Var;
        let section = CONFIG
            .split("*statusline*")
            .nth(1)
            .and_then(|rest| rest.split("*themes*").next())
            .expect("the STATUS LINE section");
        let listed: Vec<&str> = section
            .split("each empty when it has nothing to say:")
            .nth(1)
            .expect("the list's lead-in")
            .lines()
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            // A value's line is indented six; its continuation further.
            .filter(|line| line.starts_with("      ") && !line.starts_with("       "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let vars: Vec<&str> = Var::ALL.iter().map(|v| v.name()).collect();
        assert_eq!(listed, vars, "the manual's list, in the type's order");
    }

    /// **The manual lists every fact a `when` can ask, under its scope, and
    /// only facts** — counted from the types (`facts::SessionFact::ALL`,
    /// `AppFact::ALL`), never from the page.
    #[test]
    fn the_manuals_facts_are_the_facts() {
        use crate::facts::{AppFact, SessionFact};
        let section = CONFIG
            .split("*alerts*")
            .nth(1)
            .and_then(|rest| rest.split("*themes*").next())
            .expect("the ALERTS section");
        let listed = |lead_in: &str| -> Vec<&str> {
            (section.split(lead_in).nth(1).expect(lead_in).lines())
                .skip(1)
                .take_while(|line| !line.trim().is_empty())
                // A fact's line is indented six; its continuation further.
                .filter(|line| line.starts_with("      ") && !line.starts_with("       "))
                .filter_map(|line| line.split_whitespace().next())
                .collect()
        };
        let session: Vec<&str> = SessionFact::ALL.iter().map(|f| f.name()).collect();
        let app: Vec<&str> = AppFact::ALL.iter().map(|f| f.name()).collect();
        assert_eq!(
            listed("your prompt goes to:"),
            session,
            "a session's, in order"
        );
        assert_eq!(listed("The app's own:"), app, "the app's, in order");
    }

    /// **The manual's shipped alerts are the ones shipped** — the pair it
    /// writes out, read as TOML and compared key by key with the defaults'
    /// (not through the config's fold, which would fill a key the page left
    /// out), and every shipped alert named, as many as it says. It was a hand
    /// copy: `bell = false` in it passed (review stop 9, C 8 and F9).
    #[test]
    fn the_manuals_shipped_alerts_are_the_shipped_ones() {
        use shannon_core::config::{AlertConfig, SHIPPED_ALERTS};
        let section = CONFIG
            .split("*alerts*")
            .nth(1)
            .and_then(|rest| rest.split("*themes*").next())
            .expect("the ALERTS section");
        let pair: String = (section.split("The shipped pair, as a config writes it:"))
            .nth(1)
            .expect("the pair")
            .lines()
            .skip(1)
            .take_while(|line| line.is_empty() || line.starts_with("      "))
            .map(|line| format!("{}\n", line.trim()))
            .collect();
        let written: toml::Table = toml::from_str(&pair).expect("the pair reads as TOML");
        let shipped = shannon_core::Config::default().tui.alert;
        for name in ["unshown", "unshown_session"] {
            let page: AlertConfig = written["tui"]["alert"][name].clone().try_into().unwrap();
            assert_eq!(page, shipped[name], "{name}");
        }
        assert_eq!(SHIPPED_ALERTS.len(), 7, "the page says \"Seven ship.\"");
        assert!(section.contains("Seven ship."));
        for alert in SHIPPED_ALERTS {
            assert!(section.contains(alert.name), "{} named", alert.name);
        }
    }

    /// **The manual's blinking example does what it says** — run through
    /// `:set` onto the default, as `:help cursor` writes it (Steven,
    /// 11-02: "how do I do this?"): every mode keeps its shape and blinks,
    /// DECSCUSR's blinking number being the steady one less one.
    #[test]
    fn the_manuals_blink_example_blinks_every_shape() {
        use crate::guicursor::{CursorMode, Shapes};
        use shannon_core::config::{Config, set};
        let section = CONFIG
            .split("*cursor*")
            .nth(1)
            .and_then(|rest| rest.split("*plugins*").next())
            .expect("the CURSOR section");
        let example = section
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(":set "))
            .expect("the section's `:set` example");
        let plan = set::plan(set::parse(example).unwrap(), &Config::default()).unwrap();
        let value = plan.table["tui"]["guicursor"].as_str().expect("a string");
        let (shipped, blinking) = (Shapes::default(), Shapes::read(value).unwrap());
        for &mode in CursorMode::ALL {
            let steady = shipped.code(mode).expect("the default shapes every mode");
            assert_eq!(
                blinking.code(mode),
                Some(steady - 1),
                "{mode:?} in `{value}`"
            );
        }
    }

    /// **The manual's shipped themes are the ones shipped** — in order,
    /// counted from `theme::shipped_names`, not from the page.
    #[test]
    fn the_manuals_shipped_themes_are_the_shipped_ones() {
        let section = CONFIG.split("*themes*").nth(1).expect("the THEMES section");
        let listed: Vec<&str> = section
            .split("says whose):")
            .nth(1)
            .expect("the list's lead-in")
            .trim_start_matches('\n')
            .split("\n\n")
            .next()
            .expect("the list")
            .split_whitespace()
            .collect();
        let shipped: Vec<&str> = crate::theme::shipped_names().collect();
        assert_eq!(listed, shipped);
    }

    /// **The manual's role list is `Ink`'s** — every role named, and only
    /// roles: a theme author writes the keys this list gives, and a theme
    /// file that names no role is told so at load.
    #[test]
    fn the_manuals_roles_are_the_inks() {
        use crate::style::Ink;
        let section = CONFIG
            .split("*themes*")
            .nth(1)
            .and_then(|rest| rest.split("*plugins*").next())
            .expect("the THEMES section");
        // The list: the indented block after "inherited or yours.", up to
        // the blank line that ends it.
        let listed: Vec<&str> = section
            .split("inherited or yours.")
            .nth(1)
            .expect("the list's lead-in")
            .trim_start_matches('\n')
            .split("\n\n")
            .next()
            .expect("the list")
            .split_whitespace()
            .collect();
        let names: Vec<&str> = Ink::ALL.iter().map(|ink| ink.name()).collect();
        for name in &names {
            assert!(listed.contains(name), "{name} is not in the manual");
        }
        for word in &listed {
            assert!(names.contains(word), "{word} is not a role");
        }
    }

    /// A line's rendered width with tabs expanded to 8 (how the pager draws
    /// them) — a tag-aligned header can't cheat the width by using a tab.
    fn rendered_width(line: &str) -> usize {
        line.chars().fold(0, |w, c| match c {
            '\t' => (w / 8 + 1) * 8,
            _ => w + 1,
        })
    }

    /// Every chapter name and alias opens something: read from [`TOPICS`],
    /// the one list, so none can be offered that the manual does not hold.
    #[test]
    fn every_topic_word_opens_something() {
        let page = compose(&crate::command::commands_chapter());
        assert!(topic_words().count() > TOPICS.len(), "premise: aliases too");
        for word in topic_words() {
            assert!(anchor_line(&page, word).is_some(), "{word}");
        }
    }

    /// An alias resolves to the chapter that declares it (`:help security`
    /// opens the `decisions` chapter), and the chapter carries the matching
    /// tag.
    #[test]
    fn aliases_resolve_and_are_tagged() {
        for t in TOPICS {
            for alias in t.aliases {
                let by_alias = compose_topic(alias, "").expect("alias should resolve");
                let by_name = compose_topic(t.name, "").expect("name should resolve");
                assert_eq!(by_alias, by_name, "alias {alias:?} != its chapter");
                assert!(
                    by_name.contains(&format!("*{alias}*")),
                    "the {} chapter is missing the *{alias}* tag",
                    t.name
                );
            }
        }
    }

    /// **Every `:help` argument lands on the thing it names.**
    ///
    /// Steven: *"`:help config` lands in the wrong place. Near, but not quite
    /// right. Which makes me think anchors are being done wrong."* They were —
    /// there were two mechanisms, and a chapter used the one with no anchor at
    /// all, so it opened on ten lines of the manual's general blurb.
    ///
    /// Checked over the whole table rather than one example, because "near but
    /// not quite" is exactly the failure a single hand-picked case misses.
    #[test]
    fn every_chapter_name_and_alias_anchors_on_its_own_heading() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        for t in TOPICS {
            for name in std::iter::once(t.name).chain(t.aliases.iter().copied()) {
                let at = anchor_line(&page, name)
                    .unwrap_or_else(|| panic!("`:help {name}` resolves to nothing"));
                let landed = lines[at];
                assert!(
                    landed.contains(&format!("*{}*", t.name)),
                    "`:help {name}` landed on {landed:?}, not the {} heading",
                    t.name
                );
                // The heading, not the blurb: a chapter heading is ALL-CAPS and
                // sits under a `===` rule. Landing anywhere in the first
                // screenful of the manual is the bug this test exists for.
                assert!(
                    at > 20,
                    "`:help {name}` landed at line {at}, inside the manual header"
                );
            }
        }
    }

    /// **`:help news` and `:help unshown` land where news is said** — the
    /// row says "changed", the notification "news in", the alert `unshown`,
    /// and none of the three words found anything (review stop 9, F8).
    #[test]
    fn news_has_a_place_in_the_manual() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        for word in ["news", "unshown"] {
            let at = anchor_line(&page, word).unwrap_or_else(|| panic!("`:help {word}`"));
            assert!(
                lines[at + 1].contains("News is"),
                "{word}: {}",
                lines[at + 1]
            );
        }
    }

    /// A tag and a command resolve too, and a chapter name is never captured by
    /// a same-named `:command` row (`:help config` must not land on `:config`).
    #[test]
    fn tags_and_commands_anchor_without_shadowing_chapters() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();

        let section = anchor_line(&page, "window-session").expect("a tag");
        assert!(lines[section].contains("*window-session*"));
        assert_eq!(
            anchor_line(&page, "shannon-window-session"),
            Some(section),
            "the old spelling still reads"
        );
        assert!(!page.contains("*shannon-"), "and no tag is written in it");

        let delete = anchor_line(&page, "delete").expect("a command");
        assert!(lines[delete].trim_start().starts_with(":delete"));
        assert_eq!(
            anchor_line(&page, ":delete"),
            Some(delete),
            "colon optional"
        );

        // `config` is BOTH a chapter and a `:config` command. The chapter wins,
        // because that is what the reader typing a chapter name means.
        let config = anchor_line(&page, "config").expect("the config chapter");
        assert!(
            lines[config].contains("*config*"),
            "`:help config` landed on {:?}",
            lines[config]
        );

        assert_eq!(anchor_line(&page, "definitely-not-a-tag"), None);
        assert_eq!(anchor_line(&page, ""), None);
    }

    /// **`:help` finds a key** (S21-Q14, Steven: "They will often know the
    /// keys but not the name of things… I might do something like `:help
    /// zM`") — on the key table row that lists it; a control key however it
    /// is spelled; another case only where none matches (Neovim's `:help GG`
    /// opens `gg`, measured); a bare key before a same-named command, the
    /// colon asking for the command (Neovim's `:help w`, `:help :w`).
    #[test]
    fn help_finds_a_key_however_it_is_spelled() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        let key = |asked: &str| {
            let at = anchor_line(&page, asked).unwrap_or_else(|| panic!("`:help {asked}`"));
            (at, lines[at])
        };
        let (_, zm) = key("zM");
        assert!(
            zm.trim_start().starts_with("zM") && zm.contains("shut every"),
            "{zm}"
        );
        let (alternate, row) = key("CTRL-^");
        assert!(row.contains("CTRL-^"), "{row}");
        for spelled in ["<C-^>", "<c-^>", "^^", "ctrl-^", "Ctrl-^"] {
            assert_eq!(key(spelled).0, alternate, "`:help {spelled}`");
        }
        let (_, brackets) = key("[[");
        assert!(brackets.trim_start().starts_with("]]  [["), "{brackets}");
        let (gg, _) = key("gg");
        assert_eq!(key("GG").0, gg, "another case, where none matches");
        let (w, motion) = key("w");
        assert!(
            motion.trim_start().starts_with("w b e"),
            "the motion: {motion}"
        );
        let (command, row) = key(":w");
        assert_ne!(command, w, "the colon asks for the command");
        assert!(row.trim_start().starts_with(":w"), "{row}");
        // A command no key table lists stays the command's, bare too; a
        // rule's words in a table are not keys.
        assert!(key("ask").1.trim_start().starts_with(":ask"), "the command");
        assert!(key("on").1.trim_start().starts_with(":on"), "the command");
        assert_eq!(
            keys_defined("    ask git push --force      ask for the flag"),
            Vec::<String>::new()
        );
        assert_eq!(
            keys_defined("    :set <option>     what it is"),
            Vec::<String>::new()
        );
        let defined = keys_defined("    :b#  CTRL-^     back to what this window showed");
        assert_eq!(defined, vec!["CTRL-^".to_string()]);
        let noted = keys_defined("    h j k l (and arrows)      move");
        assert_eq!(noted, ["h", "j", "k", "l"].map(String::from), "a note");
        // And they complete, as every name `:help` takes does.
        let targets = targets();
        for key in ["zM", "CTRL-^", "[["] {
            assert!(targets.iter().any(|t| t == key), "{key} completes");
        }
    }

    /// **Each chapter opens with a contents of every section it tags**, in
    /// order — counted from the TAGS (each `*…*` on a line that
    /// starts with a capital, below the chapter's heading), not from the
    /// generator, so a chapter the contents misses fails here.
    #[test]
    fn each_chapter_opens_with_its_own_sections() {
        let page = compose(&crate::command::commands_chapter());
        let mut chapters = 0;
        for chapter in page.split(
            "==============================================================================\n",
        ) {
            let mut lines = chapter.lines();
            let Some(heading) = lines.next() else {
                continue;
            };
            if !heading.starts_with(|c: char| c.is_ascii_digit()) {
                continue; // the header and CONTENTS
            }
            chapters += 1;
            let rest: Vec<&str> = lines.collect();
            let tagged: Vec<&str> = rest
                .iter()
                .filter(|l| l.starts_with(|c: char| c.is_ascii_uppercase()))
                .filter_map(|l| marked(l, Mark::Tag).into_iter().next())
                .collect();
            // The contents: the lines after the blank under the heading, up
            // to the next blank.
            let listed: Vec<&str> = rest
                .iter()
                .skip(1)
                .take_while(|l| !l.is_empty())
                .filter_map(|l| marked(l, Mark::Link).into_iter().next())
                .collect();
            assert_eq!(listed, tagged, "{heading}");
        }
        assert_eq!(chapters, TOPICS.len());
        // And it says so where a reader starts: the intro's own sections.
        let intro = &page[page.find("1. INTRO").unwrap()..];
        let opens: Vec<&str> = intro.lines().skip(2).take(5).collect();
        assert!(opens[0].contains("|concepts|") && opens[0].contains("THE HIERARCHY"));
        assert!(opens[4].contains("|getting-started|"), "{opens:?}");
    }

    /// The width contract, over the WHOLE manual — hand-written chapters AND
    /// the generated COMMANDS chapter — since the page is read beside a live
    /// prompt at half a screen. Written to vim's help textwidth (78); 80 is
    /// the hard ceiling.
    #[test]
    fn every_line_fits_the_help_width() {
        let page = compose(&crate::command::commands_chapter());
        for line in page.lines() {
            let w = rendered_width(line);
            assert!(w <= 80, "manual line renders {w} cols (max 80): {line:?}");
        }
    }

    /// Each chapter holds the width **on its own**, not merely inside the
    /// composed manual — so a chapter is checked even if `compose` ever stops
    /// including it. (`:help <chapter>` no longer renders a chapter-only page;
    /// this composes one to measure it.)
    #[test]
    fn each_chapter_page_fits_the_help_width() {
        let commands = crate::command::commands_chapter();
        for name in topic_words() {
            let page = compose_topic(name, &commands)
                .unwrap_or_else(|| panic!("topic {name:?} should compose"));
            for line in page.lines() {
                let w = rendered_width(line);
                assert!(
                    w <= 80,
                    "{name} page line renders {w} cols (max 80): {line:?}"
                );
            }
        }
    }

    /// Every topic's `|<name>|` TOC link resolves — the chapter that
    /// owns it defines the matching `*<name>*` tag, so the contents
    /// list can never point at a section that doesn't exist.
    #[test]
    fn every_topic_link_has_its_tag() {
        let page = compose(&crate::command::commands_chapter());
        for name in topic_words() {
            assert!(
                page.contains(&format!("*{name}*")),
                "manual is missing the *{name}* tag its TOC links to"
            );
        }
    }

    /// Each chapter's own heading number matches its position in the table of
    /// contents.
    ///
    /// The TOC is *generated* from `TOPICS` (`1.`, `2.`, …) while each chapter
    /// body hand-writes its own `N.` in a string literal — so inserting a
    /// chapter renumbers one and not the other, and the contents list then
    /// disagrees with the page it points at. Inserting a chapter mid-list is
    /// exactly that edit; this is the check that makes the next one safe.
    #[test]
    fn chapter_numbers_match_their_contents_position() {
        let commands = crate::command::commands_chapter();
        for (i, t) in TOPICS.iter().enumerate() {
            // A generated chapter carries its own heading.
            let body = chapter_body(t, &commands);
            let want = format!("{}. ", i + 1);
            let heading = body
                .lines()
                .find(|l| l.contains(&format!("*{}*", t.name)))
                .unwrap_or_else(|| panic!("the {} chapter has no self-tag", t.name));
            assert!(
                heading.starts_with(&want),
                "the {} chapter is numbered {:?} but sits at {} in the contents",
                t.name,
                heading.split_whitespace().next().unwrap_or(""),
                i + 1
            );
        }
    }

    /// **Every** `|…|` cross-reference in the manual resolves to a
    /// `*…*` tag — not just the TOC's chapter links.
    ///
    /// The narrower test above passed for weeks while the WINDOWS chapter
    /// pointed at `|permissions|`, a chapter that did not exist: a
    /// reader following it searched the page and found nothing. A dangling
    /// link is the manual's version of a dead key, and vim's help gets this
    /// right because a broken `|tag|` is *checkable* — so check it. Scanning
    /// the composed page (rather than the chapter constants) also covers the
    /// generated COMMANDS chapter.
    #[test]
    fn every_cross_reference_resolves_to_a_tag() {
        let page = compose(&crate::command::commands_chapter());
        // The one definition of a tag and a link, `marked` — shared with
        // `:help` completion rather than kept here as a second copy.
        let tags = marked(&page, Mark::Tag);
        assert!(!tags.is_empty(), "the scanner found no tags at all");
        for link in marked(&page, Mark::Link) {
            assert!(
                tags.contains(&link),
                "|{link}| points at a tag no chapter defines"
            );
        }
    }

    /// An unknown topic is a clean `None` (the caller turns it into a refusal
    /// listing the valid names), never a panic or a wrong chapter.
    #[test]
    fn unknown_topic_is_none() {
        assert!(compose_topic("nope", "").is_none());
    }

    /// Does `prose` mention `:word` as a WHOLE command name?
    ///
    /// Substring matching is not enough, and the reason is structural rather
    /// than hypothetical: twelve spellings in this table are strict prefixes of
    /// a *different* command's spelling — `:w`⊂`:wall`, `:clo`⊂`:clone`,
    /// `:h`⊂`:hide`, `:q`⊂`:quitall`, `:secret`⊂`:secrets`. Under
    /// `prose.contains(":w")`, `:write` counts as documented the moment `:wall`
    /// is, and could be described nowhere at all (Reviewer C).
    fn mentions_command(prose: &str, word: &str) -> bool {
        let needle = format!(":{word}");
        prose.match_indices(&needle).any(|(i, _)| {
            let after = prose[i + needle.len()..].chars().next();
            // A command name ends where an identifier character stops.
            !after.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
    }

    /// Pins `mentions_command` against synthetic prose, so it does not depend
    /// on what the manual happens to say today — the cases that matter are
    /// exactly the ones the manual is not currently exercising.
    #[test]
    fn a_command_is_not_documented_by_a_longer_one_that_starts_the_same() {
        // The real pairs from the table.
        assert!(!mentions_command("run `:wall` to save every session", "w"));
        assert!(!mentions_command("`:clone` forks a session", "clo"));
        assert!(!mentions_command("`:hide` stops showing a view", "h"));
        assert!(!mentions_command("`:quitall` leaves Shannon", "q"));
        assert!(!mentions_command("`:secrets` lists the table", "secret"));
        // …while a genuine mention still counts, in the punctuation the manual
        // actually uses around commands.
        assert!(mentions_command("run `:w` to save", "w"));
        assert!(mentions_command("    :w  :write   save this session", "w"));
        assert!(mentions_command("see :quit for the difference.", "quit"));
    }

    /// **Every `:command` the manual NAMES exists** — the other direction.
    ///
    /// Its sibling, `every_command_is_explained_in_the_prose_not_only_the_table`,
    /// checks that every command in the table is explained.
    /// Nothing checked the reverse, and the reverse is the one that bites: the
    /// shipped config template promised `:keymap names` for months against a
    /// command that did not exist, so following the documentation produced
    /// *not a command*. A manual that names a verb you cannot type is worse
    /// than one that omits it — the user assumes they mistyped.
    ///
    /// Foreign commands are allowed, and listed: the manual deliberately
    /// compares against vim's `:bdelete`/`:sbuffer`/`:sfind` lineage and
    /// documents the `$EDITOR` contract in the spawned editor's own verbs
    /// (`:wq`, `:cq`, `:x`). The list is short so it can be read; adding to it
    /// to silence this gate is the mistake it exists to catch, so each entry
    /// says whose command it is.
    #[test]
    fn every_command_the_manual_names_exists() {
        // Commands of another program, named as such.
        // `:wq` and `:x` used to be here — commands only the spawned `$EDITOR`
        // understood. They are ours now, so the allowlist must not keep
        // excusing them: an entry for a name that DOES resolve would hide a
        // future typo of it. `:cq` stays, because it is vim's "quit with an
        // error" and we have no equivalent.
        const FOREIGN: &[&str] = &[
            "bdelete", "sbuffer", "sfind", // vim, as lineage/contrast
            "cq",    // the $EDITOR we spawn, in the save-to-apply contract
        ];
        let prose: String = TOPICS.iter().map(|t| t.body).collect();
        let mut missing = Vec::new();
        for word in prose
            .split(|c: char| !(c.is_alphanumeric() || c == ':'))
            .filter_map(|w| w.strip_prefix(':'))
            .filter(|w| !w.is_empty() && w.chars().next().is_some_and(char::is_alphabetic))
        {
            if FOREIGN.contains(&word) {
                continue;
            }
            // A rule's head (`:write`, `:fetch`), as the gate reads one —
            // asked of the rule grammar, not listed, so a misspelt head is
            // still caught here, and so is a class head (`:readonly`) the
            // grammar refuses (P7, 10-13: the manual taught them after the
            // acts engine had refused them).
            // With a target, since a head takes one (`:read docs`, `:fetch
            // example.org`).
            let head = ["", " docs", " example.org"].iter().any(|target| {
                shannon_core::permission::Rule::parse_by(
                    &format!("deny :{word}{target}"),
                    shannon_core::permission::model::Models::shipped(),
                )
                .is_ok()
            });
            if head {
                continue;
            }
            if matches!(
                crate::command::resolve(word),
                crate::command::Resolved::Unknown
            ) {
                missing.push(word.to_string());
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "the manual names commands that do not resolve: {missing:?}"
        );
    }

    /// **The manual must not tell you to press a key that does nothing.**
    ///
    /// When `m` and `s` became `:merge` and `:squash`, the graph chapter's
    /// reference table was corrected and the EXAMPLES and WORKFLOWS below it —
    /// the parts a new user actually follows — kept saying "press `s`". At HEAD
    /// `s` is bound in no context the graph consults, so it does nothing AND
    /// says nothing: the reader's conclusion is that the program is frozen, and
    /// the splash sends them to `:help` to begin with (Reviewer F, CRITICAL).
    ///
    /// Scoped to the imperative form on purpose. The chapter still mentions
    /// both keys historically — "this was the `m` key; it moved" — and that
    /// prose is worth keeping, so a blanket "every key named must be live"
    /// would fail on exactly the sentences that fix the problem.
    ///
    /// The graph's key vocabulary is read from its one table, the key page
    /// (`*graph-keys*`), which `the_graph_keys_page_lists_every_verb`
    /// holds to the handler. It was read from the status row's legend until
    /// the legend became a pointer to the page (Steven, 10-14).
    #[test]
    fn the_manual_never_says_press_a_key_the_graph_does_not_answer() {
        // **The page's KEY TOKENS, not its text.** A `contains` over a whole
        // string answers yes to every letter that appears in "session" or
        // "unmark", so the first version of this test passed with the stale
        // "press `s`" put back — an assertion measuring a superset of its
        // subject, which is the shape it exists to catch
        // ([[feedback_assert_the_subject]]).
        let keys: std::collections::HashSet<&str> = graph_key_rows()
            .into_iter()
            .flat_map(|row| row.keys)
            .collect();
        assert!(
            keys.contains("d") && !keys.contains("s"),
            "premise: the token set is keys, not words: {keys:?}"
        );
        let prose: String = TOPICS.iter().map(|t| t.body).collect();
        let mut dead = Vec::new();
        // `press \`X\`` and `press \`X\`:` — the shapes an instruction takes.
        for caps in prose.match_indices("press `") {
            let rest = &prose[caps.0 + "press `".len()..];
            let Some((key, _)) = rest.split_once('`') else {
                continue;
            };
            // Multi-character names (`CTRL-F`, `gg`) are spelled out in the
            // legend or are keymap-wide; only single keys are graph-local.
            if key.chars().count() == 1 && !keys.contains(key) {
                dead.push(key.to_string());
            }
        }
        dead.sort();
        dead.dedup();
        assert!(
            dead.is_empty(),
            "the manual says to press {dead:?}, which the graph's key page does \
             not list — a key that does nothing and says nothing reads as a \
             frozen program"
        );
    }

    /// **Every command is EXPLAINED somewhere, not just listed.**
    ///
    /// Chapter 3 is generated from the command table, so every verb appears
    /// there by construction — which means a test that reads the composed
    /// manual would pass for a command nobody ever wrote a sentence about, and
    /// measure nothing (`[[feedback_mutation_check_guards]]`: a list cannot
    /// check that it lists everything, and a completeness test that starts
    /// *from* the list is measuring itself).
    ///
    /// So this reads only the HAND-WRITTEN chapters. `:hide` shipped as a
    /// deliberate design decision — vim's verb, chosen over an alias for
    /// `:close` for a stated reason — and the manual never mentioned it, so the
    /// reasoning existed only in a doc comment no user reads.
    ///
    /// A command may be documented under any of its spellings: the manual says
    /// `:qall` where the table's canonical name is `quitall`, and that is fine
    /// — a reader looking either up finds it.
    #[test]
    fn every_command_is_explained_in_the_prose_not_only_the_table() {
        let prose: String = TOPICS.iter().map(|t| t.body).collect();
        // **Whole words, not substrings.** `prose.contains(":mes")` is true of
        // any prose containing `:messages`, and in a vim-style table almost
        // every short alias is a prefix of its own canonical name (`:q`⊂`:quit`,
        // `:hid`⊂`:hide`, `:clo`⊂`:close`, `:sp`⊂`:split`…). So the
        // "documented under any of its spellings" half of this test matched
        // itself and checked nothing — deleting the `:mes` line from the manual
        // left it green (Reviewer C).
        let mentioned = |word: &str| mentions_command(&prose, word);
        let mut missing = Vec::new();
        for spec in crate::command::TABLE {
            let spellings = std::iter::once(spec.name).chain(spec.aliases.iter().copied());
            if !spellings.into_iter().any(mentioned) {
                missing.push(spec.name);
            }
        }
        assert!(
            missing.is_empty(),
            "listed in the table and explained nowhere: {missing:?}"
        );
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;

    /// **What `:help` completes is what `:help` accepts** — every completion
    /// resolves through `anchor_line`, the function `:help` itself calls, and
    /// every tag the manual defines is offered. They were two lists: the
    /// chapter names completed while every tag was accepted.
    /// **`:help :cmd` lands on that command's row, by any form** — `:hide`,
    /// `:bu`, `:on`. The lookup parsed the forms back out of the rendered
    /// row, and the day the row learned vim's `:hid[e]` it found nothing. The
    /// test above asks only that something opens; this asks WHERE. **Every
    /// form to the one row**, with no exception for a chapter or tag of the
    /// same name: `:help :write` and `:help :w` were two places, by whether a
    /// tag of that spelling existed (review stop 17, D M4; 26 of 127 forms).
    #[test]
    fn help_for_a_command_lands_on_its_row_by_any_form() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        for spec in crate::command::TABLE {
            let row = crate::command::synopsis_of(spec.cmd).unwrap();
            let mut landed = std::collections::BTreeSet::new();
            for form in std::iter::once(&spec.name).chain(spec.aliases) {
                let at = anchor_line(&page, &format!(":{form}"))
                    .unwrap_or_else(|| panic!("`:help :{form}` opens nothing"));
                assert!(
                    lines[at].trim_start().starts_with(&row),
                    "`:help :{form}` landed on {:?}",
                    lines[at]
                );
                landed.insert(at);
            }
            assert_eq!(landed.len(), 1, "`:{}`'s forms: {landed:?}", spec.name);
        }
    }

    /// **A follow takes the link under the cursor, else the next on the
    /// line**, as vim's `CTRL-]` takes its keyword; past the last, none.
    /// Counted in characters, as a cursor is: a wide one before the link.
    #[test]
    fn a_follow_takes_the_link_under_or_after_the_cursor() {
        let line = "see |one| — then |two| end";
        let at = |column| link_at(line, column, Dialect::Manual);
        assert_eq!(at(0), Some("one"), "after");
        assert_eq!(at(5), Some("one"), "under");
        assert_eq!(at(8), Some("one"), "its closing bar");
        assert_eq!(at(9), Some("two"), "past the first");
        assert_eq!(at(19), Some("two"), "under the second, past `—`");
        assert_eq!(at(line.chars().count() - 1), None, "past the last");
        assert_eq!(at(999), None);
        assert_eq!(link_at("- |motions|", 0, Dialect::Tutor), Some("motions"));
        assert_eq!(link_at("words alone", 0, Dialect::Tutor), None);
        assert_eq!(
            link_at("`:q` *tag* then |one|", 0, Dialect::Manual),
            Some("one"),
            "a literal and a tag are no links"
        );
    }

    /// **The link keys are found however they are spelled**, on their row.
    #[test]
    fn help_finds_the_link_keys() {
        let page = compose(&crate::command::commands_chapter());
        let lines: Vec<&str> = page.lines().collect();
        for (spelled, row) in [
            ("CTRL-]", "CTRL-]"),
            ("<C-]>", "CTRL-]"),
            ("^]", "CTRL-]"),
            ("CTRL-T", "CTRL-T"),
            ("<C-t>", "CTRL-T"),
        ] {
            let at = anchor_line(&page, spelled).unwrap_or_else(|| panic!("{spelled}"));
            assert!(lines[at].trim_start().starts_with(row), "{}", lines[at]);
        }
    }

    /// **The bare word keeps what it always named** — a colon is what asks
    /// for the command, so `:help write` still reaches the SAVING prose and
    /// `:help :write` the row.
    #[test]
    fn a_bare_word_still_finds_its_tag_and_a_colon_the_row() {
        let page = compose(&crate::command::commands_chapter());
        let (bare, colon) = (anchor_line(&page, "write"), anchor_line(&page, ":write"));
        assert_eq!(bare, tag_line(&page, "write"), "the tag");
        assert_ne!(bare, colon);
        assert_eq!(colon, anchor_line(&page, ":w"));
    }

    #[test]
    fn every_help_target_resolves_and_every_tag_is_a_target() {
        let page = compose(&crate::command::commands_chapter());
        let targets = targets();
        assert!(
            targets.len() > topic_words().count(),
            "premise: more than the chapter names"
        );
        for t in &targets {
            assert!(
                anchor_line(&page, t).is_some(),
                "`:help {t}` completes but opens nothing"
            );
        }
        for tag in marked(&page, Mark::Tag) {
            assert!(
                targets.iter().any(|t| t == tag),
                "*{tag}* is accepted but never offered"
            );
        }
        assert!(
            targets.iter().any(|t| t == ":delete"),
            "a :command is offered"
        );
    }

    /// **`:help` takes an option in every form `:set` does** (Steven, 10-19:
    /// `:help nosecrets` found nothing — no setting had a tag), each landing
    /// on the option's own line. Counted from `:set`'s options, never from
    /// the chapter.
    #[test]
    fn every_option_opens_in_every_form_set_takes() {
        let page = compose(&crate::command::commands_chapter());
        let line_of = |tag: &str| {
            page.lines()
                .position(|l| l.contains(&format!("*{tag}*")))
                .unwrap_or_else(|| panic!("no *{tag}*"))
        };
        let options = shannon_core::config::set::option_names();
        let mut switches = 0;
        for key in &options {
            let at = line_of(key);
            let mut forms = vec![
                key.clone(),
                format!("'{key}'"),
                format!("{key}?"),
                format!("{key}&"),
                format!("{key}=1"),
                format!("{key}+=1"),
            ];
            if let Some(toml::Value::Boolean(_)) = shannon_core::config::set::default_of(key) {
                switches += 1;
                forms.extend([format!("no{key}"), format!("inv{key}"), format!("{key}!")]);
            }
            for form in forms {
                assert_eq!(anchor_line(&page, &form), Some(at), "`:help {form}`");
            }
        }
        assert!(
            switches > 5,
            "premise: switches have `no` forms ({switches})"
        );
    }

    /// **The rest of what a reader may type for a setting**: a section
    /// (`nosecrets`, Steven's), a key the defaults leave out (at its
    /// section), a setting's last word alone where it is one setting's, and
    /// — where it is several settings' — a refusal naming them.
    #[test]
    fn a_setting_is_found_by_its_section_or_its_last_word() {
        let page = compose(&crate::command::commands_chapter());
        let at = |tag: &str| page.lines().position(|l| l.contains(tag));
        for (asked, lands) in [
            ("nosecrets", "*secrets*"),
            ("password_protect", "*secrets.password_protect*"),
            ("scrolloff", "*tui.scrolloff*"),
            ("nomouse", "*tui.mouse*"),
            ("keymap.normal.Q", "*keymap*"),
            ("tui.graph_glyphs.child=x", "*config.tui.graph_glyphs*"),
            ("config.tui", "*config.tui*"),
            ("notui", "*config.tui*"),
        ] {
            assert_eq!(anchor_line(&page, asked), at(lands), "`:help {asked}`");
            assert!(at(lands).is_some(), "premise: {lands}");
        }
        assert_eq!(anchor_line(&page, "paths"), None, "several settings'");
        let said = no_such_topic("paths");
        assert!(
            ["memory.paths", "skills.paths", "secrets.paths"]
                .iter()
                .all(|k| said.contains(k)),
            "{said}"
        );
        assert_eq!(anchor_line(&page, "frobnicate"), None);
        // A word of thousands of dots walks six ancestors, not all of them
        // (review stop 6, B 5: 3 s on the UI thread at 4,000, debug).
        let long = vec!["x"; 4_000].join(".");
        let started = std::time::Instant::now();
        assert_eq!(anchor_line(&page, &long), None);
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
        // `<Tab>` offers a setting's name, `_` and all.
        let offered = targets();
        for name in [
            "secrets.password_protect",
            "tui.prompt.height",
            "config.tui",
        ] {
            assert!(offered.iter().any(|t| t == name), "{name}");
        }
    }

    /// A command whose aliases are written first lands on ITS row: `:alias`
    /// on `:secrets, :alias`, `:quit` on `:q[!], :quit[!]` — with or without
    /// the colon, and through vim's optional bang. Only the first form on a
    /// row used to match.
    #[test]
    fn a_command_named_after_its_alias_finds_its_row() {
        let page = compose(&crate::command::commands_chapter());
        for (name, row) in [
            (":alias", ":secrets, :alias"),
            (":quit", ":q[!], :quit[!]"),
            ("quit", ":q[!], :quit[!]"),
            (":qall", ":qa[!], :quitall[!], :qall[!]"),
        ] {
            let at =
                anchor_line(&page, name).unwrap_or_else(|| panic!("`:help {name}` opens nothing"));
            let line = page.lines().nth(at).unwrap();
            assert!(
                line.trim_start().starts_with(row),
                "`:help {name}` landed on {line:?}"
            );
        }
    }

    /// A rule the manual shows — and whether it shows it to `remove`, which
    /// is a different claim from adding it (review 20, Reviewer C: a removal
    /// was tested as an added rule, beside the shipped one it names).
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct Shown {
        text: String,
        removes: bool,
    }

    /// Every rule the manual writes, as the gate would be handed it:
    /// - a config list (`rules = [`…`]`, `remove = [`…`]`), parsed AS TOML —
    ///   a `'''` string spans lines, and flattened, the first `#` comment of
    ///   a `(?x)` pattern would swallow the rest of it;
    /// - a table row (six spaces in, its gloss two spaces after it);
    /// - a `` ` `` span that opens with a verb, read across line wraps.
    fn rules_shown(page: &str) -> Vec<Shown> {
        let opens = |s: &str| ["allow ", "ask ", "deny "].iter().any(|v| s.starts_with(v));
        let rule = |text: String| Shown {
            text,
            removes: false,
        };
        let mut shown = Vec::new();
        let mut prose = Vec::new();
        let mut lines = page.lines();
        while let Some(line) = lines.next() {
            let opened = line.trim();
            if opened.starts_with("rules = [") || opened.starts_with("remove = [") {
                let mut block = line.to_string();
                if !opened.ends_with(']') {
                    for more in lines.by_ref() {
                        block = format!("{block}\n{more}");
                        if more.trim() == "]" {
                            break;
                        }
                    }
                }
                let config =
                    shannon_core::config::Config::parse(&format!("[permissions]\n{block}\n"))
                        .unwrap_or_else(|e| {
                            panic!("the manual's TOML does not parse: {e}\n{block}")
                        });
                shown.extend(config.permissions.rules.into_iter().map(rule));
                shown.extend(config.permissions.remove.into_iter().map(|text| Shown {
                    text,
                    removes: true,
                }));
                continue;
            }
            if let Some(row) = line.strip_prefix("      ")
                && opens(row)
            {
                shown.push(rule(row.split("  ").next().unwrap_or(row).to_string()));
            }
            // A row's gloss is prose too (`(= `allow find`)`).
            prose.push(opened);
        }
        let flat = prose.join(" ");
        for (at, _) in flat.match_indices('`') {
            let span = &flat[at + 1..];
            if opens(span)
                && let Some(end) = span.find('`')
            {
                shown.push(rule(span[..end].to_string()));
            }
        }
        shown.sort();
        shown.dedup();
        shown
    }

    /// A rule as this test's table names it: on one line, every run of
    /// whitespace one space — a key, never what the gate is handed.
    fn key(rule: &str) -> String {
        rule.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// **Every rule the manual shows does what the manual says it does.**
    ///
    /// The rules chapter taught `deny gh api:-X|--method` and `ask git
    /// push:--force`, and neither could fire: the `:` after a subcommand was
    /// read as part of the word. Steven copied the first from the manual,
    /// and his `gh api -X GET user` asked. Each example is a claim; this is
    /// the table of what each claims, built against the manual both ways — a
    /// rule it shows with no case here fails, and so does a case for a rule
    /// it no longer shows. Each runs over the shipped rules, as a user's
    /// would.
    #[test]
    fn every_rule_the_manual_shows_decides_as_it_says() {
        use Decision::{Allow, Ask, Deny};
        use shannon_core::permission::{Decision, RuleSet};
        const RM_RE: &str = r"deny re:^(?:\S*/|\\)?rm(?:\s+--?[A-Za-z0-9][\w-]*)*(?:\s|$)";
        const RM_RE_X: &str = r"deny re:(?x) ^(?:\S*/|\\)?rm # the program, bare or pathed (?:\s+--?[A-Za-z0-9][\w-]*)* # then its flags (?:\s|$) # then a space, or the end";
        // (the rule as shown, rules it is shown beside, a command, the verdict)
        let cases: &[(&str, &[&str], &str, Decision)] = &[
            ("allow git *", &[], "git commit -m x", Allow),
            ("allow uv *", &[], "uv sync", Allow),
            (
                "allow uv run pytest *",
                &[],
                "uv run pytest -x foo.py",
                Allow,
            ),
            ("allow uv run pytest *", &[], "uv run ruff check", Ask),
            ("allow uv run pytest *", &[], "uv run ls pytest", Ask),
            (
                "allow uv run pytest foo.py *",
                &[],
                "uv run pytest -x foo.py",
                Allow,
            ),
            ("deny rm -r", &[], "rm -R build", Deny),
            (
                "deny bat -l|--language",
                &["allow bat *"],
                "bat -l rust x",
                Deny,
            ),
            (
                "ask git push --force",
                &["allow git *"],
                "git push --force origin main",
                Ask,
            ),
            // OPTIONS' `[permissions]` example: a push asks beside `allow git *`.
            (
                "ask git push *",
                &["allow git *"],
                "git push origin main",
                Ask,
            ),
            // Unmodelled, a flag takes a value only where a rule declares
            // it; the value is still a place.
            ("allow bat -l", &[], "bat -l rust", Ask),
            ("allow bat -l", &["ask bat -l=go"], "bat -l rust", Allow),
            ("allow bat -l=|--language=", &[], "bat -l rust", Allow),
            (
                "allow bat -l=|--language=",
                &[],
                "bat --language rust",
                Allow,
            ),
            ("allow bat -l=|--language=", &[], "bat -l /srv/x/a", Ask),
            ("deny bat -l=*", &["allow bat *"], "bat -l rust", Deny),
            (
                "allow :* /srv/x",
                &["allow bat -l=|--language="],
                "bat -l /srv/x/a",
                Allow,
            ),
            ("allow find *", &[], "find . -name x -fprint out", Allow),
            // Any arguments, every flag (Steven, 10-06).
            ("allow find *", &[], "find . -exec cat {} ;", Allow),
            // Without `*`: a flag that makes `find` more than a read is not
            // covered (the model carries what the shipped carvings said).
            ("allow find", &[], "find . -name x -fprint out", Ask),
            ("deny rm", &[], "/bin/rm x", Deny),
            (
                "deny rm /etc/*",
                &["allow rm *"],
                "cd /etc && rm passwd",
                Deny,
            ),
            // A place is a path rule's, read from where Shannon started.
            ("allow :delete passwd", &["allow rm"], "rm passwd", Allow),
            (
                "allow :delete passwd",
                &["allow rm"],
                "cd /etc && rm passwd",
                Ask,
            ),
            ("allow make test", &[], "cd sub && make test", Allow),
            ("allow make test", &[], "cd .. && make test", Ask),
            // An allow names the flags it approves (Steven, 10-10); a read
            // takes its harmless flags unnamed.
            ("allow rm", &["allow :delete build"], "rm -rf build", Ask),
            (
                "allow rm -r -f",
                &["allow :delete build"],
                "rm -rf build",
                Allow,
            ),
            // (Beside an ask on `find`, so the allow is what answers.)
            (
                "allow find -name",
                &["ask find"],
                "find . -name x -L",
                Allow,
            ),
            (
                "allow git push origin *",
                &[],
                "git push --force origin main",
                Allow,
            ),
            ("deny gh api -X", &[], "gh api -X GET user", Deny),
            // `gh` is modelled: `-X` and `--method` are one flag, so the
            // deny holds under either spelling. (For a program Shannon does
            // not model, the spelling is matched as written — `x` below.)
            ("deny gh api -X", &[], "gh api --method GET user", Deny),
            ("allow x --out=a|--output=a", &[], "x --out=a", Allow),
            (
                "allow x --out=a|--output=a",
                &[],
                "x --out=a --output=b",
                Ask,
            ),
            ("ask sort:-o", &["allow sort:*"], "sort -o out in", Ask),
            (
                "ask sort:--output",
                &["allow sort:*"],
                "sort --output=out in",
                Ask,
            ),
            (RM_RE, &[], "/bin/rm -r x", Deny),
            (RM_RE, &[], "cd x && rm -rf y", Deny),
            (RM_RE, &[], "rmdir x", Ask),
            // Not a file NAMED rm, nor a word that might be one (review 20,
            // Reviewer F: `.*/` met these, and every `$X` and glob).
            // (`cat notes/rm`, the manual's other example: `ls /bin/rm` reads
            // outside where these rules start, which asks for that alone.)
            (RM_RE, &[], "cat notes/rm", Allow),
            (RM_RE, &[], "echo $PATH", Allow),
            (RM_RE, &[], "cat src/*.rs", Allow),
            (RM_RE, &[], "sudo rm -rf /", Deny),
            // (`echo $X` under it is allowed: a `re:` reads the words as the
            // shell passes them, and a word that is data to its program is
            // not read as what it may become — design §8.7.)
            ("deny re:rm -rf", &[], "echo hi", Allow),
            (RM_RE_X, &[], "/bin/rm -r x", Deny),
            (RM_RE_X, &[], "cd x && rm -rf y", Deny),
            (RM_RE_X, &[], "rmdir x", Ask),
            (RM_RE_X, &[], "echo rm", Allow),
            ("allow re:^make test$", &[], "make test", Allow),
            ("allow re:^make test$", &[], "make test; rm -rf ~", Ask),
            ("deny re:rm", &[], "echo rm", Deny),
            ("deny re:^rm", &[], "rmdir x", Deny),
            (r"deny re:^rm(\s|$)", &[], "rm x", Deny),
            (r"deny re:^rm(\s|$)", &[], "rmdir x", Ask),
            // Unanchored, it matches more than it meant: a `frob` that names
            // `git-x`. (A delete would still ask for its path: a `re:` allow
            // covers the command and none of its paths.)
            ("allow re:git", &[], "frob git-x", Allow),
            (
                r"allow :delete re:^build/.*\.o$",
                &["allow rm *"],
                "rm build/a.o build/sub/b.o",
                Allow,
            ),
            (
                r"allow :delete re:^build/.*\.o$",
                &["allow rm *"],
                "rm build/a.o src/x.rs",
                Ask,
            ),
            (
                r"allow :delete re:^build/.*\.o$",
                &["allow rm *"],
                "rm build/../../etc/x.o",
                Ask,
            ),
            // A writer with no rule of its own runs where its writes are
            // granted — with no flag a program rule would have to approve
            // (Steven, 10-10) — and the grant does not reach outside its
            // place.
            (
                "allow :write /srv/out/**",
                &[],
                "uniq a.txt /srv/out/b",
                Allow,
            ),
            (
                "allow :write /srv/out/**",
                &["allow sed -i"],
                "sed -i '' s/a/b/ /srv/out/x",
                Allow,
            ),
            ("allow :write build", &[], "rm build/x", Ask),
            ("allow :write build", &[], "sed -i '' s/a/b/ /tmp/x", Ask),
            // A place a read may go: its files, the directory, and a `cd`
            // into it — and nowhere else.
            ("allow :read ~/notes/**", &[], "rg TODO ~/notes/a.md", Allow),
            ("allow :read ~/notes/**", &[], "rg TODO ~/notes", Allow),
            (
                "allow :read ~/notes/**",
                &[],
                "cd ~/notes && rg TODO",
                Allow,
            ),
            ("allow :read ~/notes/**", &[], "cat ~/other/a.md", Ask),
            // A program allow holds only in the approved places; a path
            // rule names others, `/**` every one below the root.
            ("allow cat *", &[], "cat /etc/passwd > notes.txt", Ask),
            // A program allow grants no place: a delete needs one (Q7).
            ("allow rm *", &[], "rm -rf build", Ask),
            ("allow rm *", &[], "cd /etc && rm passwd", Ask),
            ("allow rg:--hidden", &[], "rg --hidden foo src", Allow),
            ("allow rg:--hidden", &[], "rg --hidden foo /srv", Ask),
            ("allow :read /**", &[], "cat /srv/a.txt", Allow),
            // A rule naming only places grants them, at its level.
            (
                "allow :write /srv/out/**",
                &[],
                "echo hi > /srv/out/b",
                Allow,
            ),
            ("allow :write /srv/out/**", &[], "rm /srv/out/b", Ask),
            (
                "allow :write /srv/out/**",
                &["allow cat *"],
                "cat a > /srv/out/b",
                Allow,
            ),
            ("allow * /tmp/x/**", &[], "rm /tmp/x/a", Allow),
            (
                "allow :delete /srv/out/**",
                &["allow rm *"],
                "rm /srv/out/a /srv/out/b",
                Allow,
            ),
            // A program rule's path word is an argument, never a grant.
            ("allow rm /srv/out/a", &[], "rm /srv/out/a", Ask),
            // The programs' examples: a guarded mode named, a broad rung
            // that answers the shipped asks, a flag named.
            (
                "allow rg --no-ignore-vcs",
                &[],
                "rg --no-ignore-vcs foo src",
                Allow,
            ),
            ("allow rg --no-ignore-vcs", &[], "rg --hidden foo src", Ask),
            // A flag Shannon does not know (a newer fd's) asks without it.
            ("allow fd *", &[], "fd --frobnicate foo src", Allow),
            ("allow fd *", &[], "fd foo /srv", Ask),
            ("allow fd *", &[], "fd -e rs -x wc -l", Allow),
            ("allow fd -x", &[], "fd -x wc", Allow),
            ("allow fd -x", &[], "fd -e rs -x wc", Ask),
            ("allow fd -x -e", &[], "fd -e rs -x wc", Allow),
            ("allow fd -x", &[], "fd -l foo", Ask),
            ("allow find src", &[], "find src -delete", Ask),
            ("allow find *", &[], "find . -exec rm {} \\;", Ask),
            ("allow find *", &[], "find . -exec cat {} \\;", Allow),
            ("allow find *", &[], "find build -delete", Ask),
            (
                "allow find *",
                &["allow :delete build"],
                "find build -delete",
                Allow,
            ),
            ("allow find -exec", &[], "find . -exec cat {} \\;", Allow),
            ("allow find -exec", &[], "find . -exec rm {} \\;", Ask),
            ("allow find -exec", &[], "find . -delete", Ask),
            // The examples in today's spellings (P7, 10-13).
            ("deny rm -r", &[], "rm -rf build", Deny),
            (
                "deny find -name -delete",
                &[],
                "find . -name x -delete",
                Deny,
            ),
            ("deny find -name -delete", &[], "find . -delete", Ask),
            (
                "deny bat -l|--language",
                &["allow bat *"],
                "bat --language rust x",
                Deny,
            ),
            ("deny bat -l", &["allow bat *"], "bat -l rust x", Deny),
            (
                "deny bat -l",
                &["allow bat *"],
                "bat --language rust x",
                Allow,
            ),
            (
                "ask git push --force",
                &["allow git push origin *"],
                "git push --force origin main",
                Ask,
            ),
            (
                "allow git push origin *",
                &[],
                "git push origin main",
                Allow,
            ),
            // The most specific decides; the strictest between equals.
            (
                "allow :read /mnt/foo",
                &["deny :* /mnt"],
                "cat /mnt/foo/x",
                Allow,
            ),
            (
                "deny :* /mnt",
                &["allow :read /mnt/foo"],
                "cat /mnt/bar",
                Deny,
            ),
            (
                "allow rm -r -f",
                &["deny rm -r", "allow :delete build"],
                "rm -rf build",
                Allow,
            ),
            // Operations: an allow grants its own and those below.
            (
                "allow :write /srv/out",
                &[],
                "echo hi > /srv/out/log",
                Allow,
            ),
            ("allow :write build", &["allow rm *"], "rm build/a", Ask),
            ("allow :delete build", &["allow rm *"], "rm build/a", Allow),
            (
                "deny :write /srv/out",
                &["allow rm *"],
                "rm /srv/out/a",
                Deny,
            ),
            ("deny :write /srv/out", &[], "cat /srv/out/a", Ask),
            // A lower tier only narrows.
            ("allow :read ~", &[], "cat ~/.ssh/config", Ask),
            ("deny :* ~", &[], "cat ~/.ssh/config", Deny),
            // A credential path: a rule naming the file lifts it.
            ("allow :read ~/.ssh/config", &[], "cat ~/.ssh/config", Allow),
            // The shipped deny: lifted by a rule naming the flag alone.
            (
                "deny rm --no-preserve-root",
                &["allow rm *", "allow :delete build"],
                "rm --no-preserve-root build",
                Deny,
            ),
            (
                "allow rm *",
                &["allow :delete build"],
                "rm --no-preserve-root build",
                Deny,
            ),
            (
                "allow rm --no-preserve-root",
                &["allow :delete build"],
                "rm --no-preserve-root build",
                Allow,
            ),
            // A flag's places are the path rules'.
            ("allow find -delete", &[], "find build -delete", Ask),
            (
                "allow find -delete",
                &["allow :delete build"],
                "find build -delete",
                Allow,
            ),
            // Hosts.
            (
                "allow :fetch api.example.com",
                &["allow curl *"],
                "curl https://api.example.com/x",
                Allow,
            ),
            (
                "allow :fetch api.example.com",
                &["allow curl *"],
                "curl -d x https://api.example.com/x",
                Ask,
            ),
            (
                "allow :web *.example.com",
                &["allow curl *"],
                "curl https://api.example.com/x",
                Allow,
            ),
            (
                "allow :web *.example.com",
                &["allow curl *"],
                "curl https://example.com/x",
                Ask,
            ),
            (
                "deny :send *",
                &["allow curl *"],
                "curl -d x https://api.example.com/x",
                Deny,
            ),
            ("allow curl *", &[], "curl https://api.example.com/x", Ask),
            (
                "allow :fetch example.org",
                &["allow curl *"],
                "curl https://example.org/x",
                Allow,
            ),
            ("allow :read ~/notes", &[], "rg TODO ~/notes/a.md", Allow),
            ("allow :read ~/notes", &[], "rg TODO ~/notes", Allow),
            // A privilege wrapper: only an allow naming what it runs, and
            // every flag given it, lifts the ask.
            ("allow sudo ls *", &[], "sudo ls src", Allow),
            ("allow sudo -u=pg psql *", &[], "sudo -u pg psql -l", Allow),
            ("allow sudo -u=pg psql *", &[], "sudo psql -l", Ask),
            ("allow sudo *", &["allow ls *"], "sudo ls src", Ask),
            ("allow sudo -s ls *", &[], "sudo -s ls src", Ask),
            // `build/*` names what is in build, not build itself; for a
            // delete `build/**` is what is below it, `build` the directory
            // too. (Beside any flag, so only the place decides.)
            (
                "allow :delete build/*",
                &["allow rm *"],
                "rm -rf build/x",
                Allow,
            ),
            (
                "allow :delete build/*",
                &["allow rm *"],
                "rm -rf build",
                Ask,
            ),
            (
                "allow :delete build/**",
                &["allow rm *"],
                "rm -rf build",
                Ask,
            ),
            (
                "allow :delete build/**",
                &["allow rm *"],
                "rm -rf build/x",
                Allow,
            ),
            (
                "allow :delete build",
                &["allow rm *"],
                "rm -rf build",
                Allow,
            ),
            ("allow :delete ./**", &["allow rm *"], "rm -rf .", Ask),
            ("allow :delete ./**", &["allow rm *"], "rm -rf src", Allow),
            ("deny :write /srv/out", &["allow *"], "rm /srv/out/x", Deny),
            // (`**`: outside the approved places, an allow must name one.)
            (
                "deny :write /srv/out",
                &["allow * **"],
                "cat /srv/out/a",
                Allow,
            ),
            // `/**` reaches every place below the root.
            ("allow :delete /**", &["allow rm *"], "rm -rf /tmp/x", Allow),
            // Above what it names, it asks rather than denies. (Beside an
            // allow that reaches anywhere, so the deny is what answers.)
            (
                "deny rm /etc/**",
                &["allow rm *", "allow :delete /**"],
                "rm -rf /etc",
                Deny,
            ),
            (
                "deny rm /etc/**",
                &["allow rm *", "allow :delete /**"],
                "rm -rf /",
                Ask,
            ),
            (
                "deny rm /etc/**",
                &["allow rm *", "allow :delete /**"],
                "rm -rf /etc/x",
                Deny,
            ),
            (
                "deny rm /etc/**",
                &["allow rm *", "allow :delete /**"],
                "rm -rf /tmp",
                Allow,
            ),
            (
                r"deny :read re:/\.ssh/",
                &["allow *"],
                "cat ~/.ssh/config",
                Deny,
            ),
            (r"deny * re:(^|/)\.git(/|$)", &[], "rm -rf .git", Deny),
            (
                r"deny * re:(^|/)\.git(/|$)",
                &[],
                "cat repo/.git/config",
                Deny,
            ),
            (r"deny * re:(^|/)\.git(/|$)", &[], "cat .gitignore", Allow),
            // (Its covering `git status` shows nothing: a read runs anyway.)
            (
                "allow re:^git status$",
                &[],
                "git status; rm -rf ~/git-x",
                Ask,
            ),
        ];
        // What a `remove` the manual shows does: the shipped guard it names
        // is gone, so the write it asked about runs as any write inside.
        let removals: &[(&str, &[&str], &str, Decision)] =
            &[("ask :write **/.claude", &[], "echo x > .claude/x", Allow)];
        // Shown beside the manual's own model of `bat` (|programs|),
        // and decided with THAT model loaded as a user's file is: the
        // template a user copies is held to what the page says it does.
        let modelled: &[(&str, &[&str], &str, Decision)] = &[
            (
                "deny bat --language rust",
                &["allow bat *"],
                "bat -l rust notes.txt",
                Deny,
            ),
            (
                "allow :read /srv/notes",
                &[],
                "bat -p /srv/notes/a.md",
                Allow,
            ),
        ];
        let page = compose(&crate::command::commands_chapter());
        let bat = {
            use shannon_core::permission::model::{Models, Source, load};
            let text: String = page
                .lines()
                .skip_while(|l| l.trim() != "name = \"bat\"")
                .take_while(|l| !l.trim().is_empty())
                .map(|l| format!("{l}\n"))
                .collect();
            let model = load::parse(&text, Source::User("bat.toml".into()))
                .unwrap_or_else(|e| panic!("the manual's model: {e}\n{text}"));
            let mut models = Models::shipped().clone();
            assert!(
                shannon_core::permission::acts::example_misreads(&model, &models).is_empty(),
                "the manual's model misreads its own examples"
            );
            assert!(models.get("bat").is_none(), "premise: `bat` ships no model");
            models.insert(model);
            std::sync::Arc::new(models)
        };
        let shown = rules_shown(&page);
        // The subject first: a table row, a wrapped span, a TOML literal, a
        // TOML `'''` string across lines, a removal.
        for rule in [
            RM_RE,
            "allow uv run pytest foo.py *",
            r"deny re:^rm(\s|$)",
            RM_RE_X,
        ] {
            assert!(
                shown.iter().any(|s| key(&s.text) == rule && !s.removes),
                "`{rule}` is not read"
            );
        }
        assert!(shown.iter().any(|s| s.removes), "no removal is read");
        for s in &shown {
            let mut table = match s.removes {
                true => removals.iter().chain(&[][..]),
                false => cases.iter().chain(modelled),
            };
            assert!(
                table.any(|(r, ..)| *r == key(&s.text)),
                "the manual shows `{}` (removes: {}), and no case here says what it does",
                s.text,
                s.removes
            );
        }
        let shipped = RuleSet::defaults();
        let passes = [
            (cases, false, None),
            (removals, true, None),
            (modelled, false, Some(bat)),
        ];
        for (table, removes, models) in passes {
            for &(rule, beside, command, want) in table {
                // As the manual writes it — across lines, if it does.
                let written = shown
                    .iter()
                    .find(|s| key(&s.text) == rule && s.removes == removes)
                    .map(|s| s.text.clone())
                    .unwrap_or_else(|| {
                        panic!("a case for `{rule}`, which the manual no longer shows")
                    });
                let decide = |with: bool| {
                    let entry = with.then(|| written.clone());
                    let permissions = shannon_core::config::PermissionsConfig {
                        rules: beside
                            .iter()
                            .map(|b| b.to_string())
                            .chain(entry.clone().filter(|_| !removes))
                            .collect(),
                        remove: entry.filter(|_| removes).into_iter().collect(),
                        ..Default::default()
                    };
                    let (rules, problems) =
                        RuleSet::configured_by(&permissions, &[], &[], models.clone());
                    assert!(problems.is_empty(), "`{rule}`: {problems:?}");
                    rules.decide_line(command).decision
                };
                assert_eq!(
                    decide(true),
                    want,
                    "`{rule}` (removes: {removes}, beside {beside:?}) on `{command}`"
                );
                // **And the entry is what decides it** (review 20, Reviewer C:
                // five cases gave their verdict without their rule). A case
                // that claims the rule's own verb, or any removal, must decide
                // otherwise without it — unless the manual is showing a
                // shipped rule, which is then held to BE one.
                let verb = rule.split_whitespace().next().unwrap_or("");
                let claims_its_verb = format!("{want:?}").eq_ignore_ascii_case(verb);
                if (removes || claims_its_verb) && decide(false) == want {
                    let parsed = shannon_core::permission::Rule::parse_by(
                        &written,
                        shannon_core::permission::model::Models::shipped(),
                    )
                    .unwrap_or_else(|e| panic!("`{rule}`: {e}"));
                    assert!(
                        !removes && shipped.rules().iter().any(|r| r.same(&parsed)),
                        "`{rule}` on `{command}` decides {want:?} without the rule too — \
                         the case shows nothing about it"
                    );
                }
            }
        }
    }
}
