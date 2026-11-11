//! **The OPTIONS chapter of `:help`** — every setting at its default, read
//! from the one reference held complete against the schema
//! (`shannon_core::DEFAULT_TOML`; `default_toml_is_complete`), as the
//! COMMANDS chapter is read from the command table. A setting carries the
//! tag `*<key>*` and a section `*config.<section>*`, so
//! `:help tui.scrolloff` and `:help nosecrets.password_protect` land on it
//! (`help::anchor_line`) and `<Tab>` completes it (`help::targets`) —
//! where no setting had a tag at all (Steven, 10-19: `:help nosecrets`
//! found nothing).
//!
//! The file says a setting's comment ABOVE it; a help page says it under
//! the line a jump lands on. So a comment that touches a setting moves
//! under it, and one a blank line ends stays where it stands: under a
//! section's header it is the SECTION's words, under the header a jump to
//! `config.<section>` lands on — which stood bare, the section's words filed
//! under its first setting, since the file did not tell them apart (Steven,
//! 11-09: `:help config.tui.prompt`, "a generalization problem"). A comment
//! before a header (an example, a setting the defaults leave out) stays
//! where it stands too.

use std::sync::LazyLock;

use crate::command::{HELP_WIDTH, wrap_words};

/// A section's tag namespace — `*config.tui*` — apart from the
/// prose chapters' tags (`*secrets*`), which a section of the same
/// name would otherwise collide with.
pub(crate) const SECTION_TAG: &str = "config.";

/// The chapter, rendered once: the reference is a constant.
pub(crate) fn options_chapter() -> &'static str {
    static CHAPTER: LazyLock<String> = LazyLock::new(render);
    &CHAPTER
}

fn render() -> String {
    render_from(shannon_core::DEFAULT_TOML)
}

/// The chapter of `toml` — the shipped reference's, or a test's fixture,
/// for a shape the reference does not hold today.
fn render_from(toml: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "==============================================================================\n",
    );
    out.push_str(&tagged(
        "13. OPTIONS — every setting",
        "*options* *settings*",
    ));
    for para in INTRO.split("\n\n") {
        out.push('\n');
        for line in wrap_words(para, HELP_WIDTH) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let mut section = None::<&str>;
    // Comment lines waiting for the setting they describe.
    let mut said: Vec<&str> = Vec::new();
    // Just past a header: words there are the section's, under it.
    let mut opened = false;
    for line in toml.lines() {
        if let Some(name) = header(line) {
            // Words before a header stand apart from the setting above them.
            // The file's own preamble is this chapter's intro, said above.
            if section.is_some() {
                stand(&mut out, &mut said, false);
            }
            said.clear();
            section = Some(name);
            out.push('\n');
            out.push_str(&tagged(
                &format!("[{name}]"),
                &format!("*{SECTION_TAG}{name}*"),
            ));
            opened = true;
        } else if let Some(text) = line.strip_prefix('#') {
            said.push(text.strip_prefix(' ').unwrap_or(text));
        } else if line.trim().is_empty() {
            // A blank line ends the words: they touch no setting.
            if section.is_some() {
                stand(&mut out, &mut said, opened);
            }
            said.clear();
            opened = false;
        } else if let Some(key) = line.split_once(" = ").map(|(key, _)| key.trim()) {
            opened = false;
            // The value is read from the defaults: a list's further lines are
            // passed over as neither a comment nor a setting.
            let key = match section {
                Some(section) => format!("{section}.{key}"),
                None => key.to_string(),
            };
            out.push('\n');
            setting(&mut out, &key);
            comment(&mut out, &mut said);
        }
    }
    // The file's last words touch no setting either: apart, as any block a
    // blank line ends. They were glued to the block before (V2b L8).
    stand(&mut out, &mut said, opened);
    out
}

/// Two paragraphs, the first with no `|link|`: a chapter's first paragraph
/// is where its contents stand (`each_chapter_opens_with_its_own_sections`).
const INTRO: &str = "Every setting, at the value it has when nothing sets it — what \
    `:config dump` shows as a file, here as a reference.\n\n\
    Your config holds only what you change (|config-file|); `:set` changes \
    one for the session (|set|). \
    `:help` reads an option as `:set` does: `:help tui.scrolloff`, `:help \
    nosecrets.password_protect`, `:help turn.effort=high` — or its last word, where no \
    other option ends with it (`:help scrolloff`). A section is `config.` and its \
    name (`:help config.tui`); a setting not listed here opens at its section.";

/// `[name]` when `line` is a section's header.
fn header(line: &str) -> Option<&str> {
    let name = line.strip_prefix('[')?.strip_suffix(']')?;
    (!name.is_empty() && !name.starts_with('[')).then_some(name)
}

/// `key = value  (default)`, tagged: the tag beside it where both fit, on a
/// line of its own above where they do not, and a value longer than the
/// page wrapped under its key. Said to be the default on the line a jump
/// lands on: `tui.scrolloff = 5` alone read as the value you have (review
/// stop 6, F 6).
fn setting(out: &mut String, key: &str) {
    let value = shannon_core::config::set::default_of(key)
        .map(|v| v.to_string())
        .unwrap_or_default();
    let tag = format!("*{key}*");
    let line = format!("{key} = {value}  (default)");
    if width(&line) + 2 + width(&tag) <= HELP_WIDTH {
        out.push_str(&tagged(&line, &tag));
        return;
    }
    out.push_str(&tagged("", &tag));
    if width(&line) <= HELP_WIDTH {
        out.push_str(&line);
        out.push('\n');
        return;
    }
    out.push_str(&format!("{key} =  (default)\n"));
    for part in wrap_words(&value, HELP_WIDTH - 4) {
        out.push_str(&format!("    {part}\n"));
    }
}

/// The comment lines waiting, under what they describe, and none waiting
/// after. Indented two, so they read as the setting's above them — which
/// the file's 78 columns leave room for in the page's 80.
fn comment(out: &mut String, said: &mut Vec<&str>) {
    let lines = std::mem::take(said);
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let end = lines.iter().rposition(|l| !l.trim().is_empty());
    let (Some(start), Some(end)) = (start, end) else {
        return;
    };
    for line in &lines[start..=end] {
        match line.trim().is_empty() {
            true => out.push('\n'),
            false => out.push_str(&format!("  {}\n", line.trim_end())),
        }
    }
}

/// **Words that touch no setting, where they stand** — a section's under
/// its header (`opened`), else apart, after a blank line; none waiting
/// after.
fn stand(out: &mut String, said: &mut Vec<&str>, opened: bool) {
    if !said.iter().any(|l| !l.trim().is_empty()) {
        said.clear();
        return;
    }
    if !opened {
        out.push('\n');
    }
    comment(out, said);
}

/// `left` with `tag` at the page's right edge.
fn tagged(left: &str, tag: &str) -> String {
    let pad = HELP_WIDTH.saturating_sub(width(left) + width(tag)).max(1);
    format!("{left}{}{tag}\n", " ".repeat(pad))
}

fn width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every setting the schema has is tagged here** — counted from the
    /// type's defaults, never from this chapter, which could only list what
    /// it lists. Each once, at its default.
    #[test]
    fn every_setting_has_its_tag_and_default() {
        let chapter = options_chapter();
        let table = shannon_core::Config::default().as_table();
        let keys = shannon_core::config::set::leaf_paths(&table);
        assert!(
            keys.len() > 50,
            "premise: the schema's settings ({})",
            keys.len()
        );
        for key in keys {
            let tag = format!("*{key}*");
            assert_eq!(chapter.matches(&tag).count(), 1, "{tag}");
            let value = shannon_core::config::set::default_of(&key).unwrap();
            let line = chapter.lines().find(|l| l.starts_with(&format!("{key} ")));
            // Said to be the default, where a jump lands (review stop 6, F 6).
            assert!(
                line.is_some_and(|l| l.contains(&format!("= {value}  (default)"))
                    || l.ends_with(" =  (default)")),
                "{key} at {value}: {line:?}"
            );
            // And says what it is: a jump to it landed on a bare line for 14
            // settings, two of them made bare by S13 (stop 17, V2b L10).
            let words = after(chapter, &format!("{key} "));
            assert!(!words.trim().is_empty(), "{key} says what it is");
        }
    }

    /// The line after the first that starts with `start`.
    fn after<'a>(chapter: &'a str, start: &str) -> &'a str {
        let mut lines = chapter.lines().skip_while(|l| !l.starts_with(start));
        lines.nth(1).unwrap_or_default()
    }

    /// The sections there are — every setting's table, counted from the
    /// type's defaults, never from this chapter; and every header the file
    /// writes, for a section with no setting by default (`[tui.graph_glyphs]`,
    /// which the settings alone could never name: V2b L7). The file is held
    /// to the type (`default_toml_is_complete`).
    fn sections() -> std::collections::BTreeSet<String> {
        let table = shannon_core::Config::default().as_table();
        let settings = (shannon_core::config::set::leaf_paths(&table).into_iter())
            .filter_map(|key| key.rsplit_once('.').map(|(section, _)| section.to_string()));
        let headers =
            (shannon_core::DEFAULT_TOML.lines()).filter_map(|l| header(l).map(String::from));
        settings.chain(headers).collect()
    }

    /// **No word of the file is lost, and none is glued to another block**
    /// — every comment line of the reference is in the chapter, in order,
    /// and every block a blank line ends stands after a blank line of its
    /// own unless it is a section's, under its header. The last block went
    /// missing with the end's flush, and blocks lost their blank lines with
    /// `stand`'s, both passing every test (stop 17, V2b M4, L8).
    #[test]
    fn every_word_of_the_file_is_in_the_chapter_in_its_place() {
        let chapter = options_chapter();
        let toml: Vec<&str> = shannon_core::DEFAULT_TOML.lines().collect();
        let said = |line: &str| {
            let text = line.strip_prefix('#')?;
            let text = text.strip_prefix(' ').unwrap_or(text).trim_end();
            (!text.trim().is_empty()).then(|| format!("  {text}\n"))
        };
        // Every comment line, in order: each found past the one before, its
        // place kept by its line in the file (a line can repeat). The file's
        // preamble, above its first header, is the chapter's intro instead.
        let preamble = toml.iter().position(|l| header(l).is_some()).unwrap_or(0);
        let mut at = 0;
        let mut found = std::collections::HashMap::new();
        for (n, line) in toml.iter().enumerate().skip(preamble) {
            let Some(needle) = said(line) else {
                continue;
            };
            let here = chapter[at..]
                .find(&needle)
                .unwrap_or_else(|| panic!("{needle:?} not in the chapter, in order"));
            found.insert(n, at + here);
            at += here + needle.len();
        }
        assert!(!chapter.contains("\n\n\n"), "no doubled blank line");
        // A block a blank line ends (or the file's end) touches no setting:
        // its first line stands after a blank line, or under its header.
        let first_lines = (0..toml.len()).filter(|&n| {
            let starts = toml[n].starts_with('#') && (n == 0 || !toml[n - 1].starts_with('#'));
            let run = (n..toml.len())
                .take_while(|&m| toml[m].starts_with('#'))
                .count();
            let next = toml.get(n + run).map_or("", |l| l.trim());
            starts && (next.is_empty() || header(next).is_some())
        });
        let mut checked = 0;
        for n in first_lines.filter(|&n| n > 0 && toml[..n].iter().any(|l| header(l).is_some())) {
            let first = (n..toml.len()).find(|&m| said(toml[m]).is_some());
            let Some(&pos) = first.and_then(|m| found.get(&m)) else {
                continue;
            };
            checked += 1;
            let before = &chapter[..pos];
            let under_header = (before.strip_suffix('\n'))
                .and_then(|b| b.rsplit('\n').next())
                .is_some_and(|prev| prev.starts_with('['));
            assert!(
                before.ends_with("\n\n") || under_header,
                "glued to the block before: {:?}",
                first.map(|m| toml[m])
            );
        }
        assert!(
            checked > 20,
            "premise: the blocks a blank line ends ({checked})"
        );
    }

    /// **Words before a header stand apart from the setting above them** —
    /// a shape today's reference does not hold (every header follows a blank
    /// line), so the flush there was held by nothing (V2b M4).
    #[test]
    fn words_before_a_header_stand_apart() {
        let chapter =
            render_from("[tui]\nmouse = true\n# words for the next\n[tui.prompt]\nheight = 10\n");
        let words = chapter.find("  words for the next").expect("not lost");
        let header = chapter.find("[tui.prompt]").expect("the header");
        assert!(words < header, "before the header: {chapter}");
        assert!(
            !after(&chapter, "tui.mouse =").contains("words for the next"),
            "not the setting's above: {chapter}"
        );
    }

    /// **A setting's words are under it** — moved from above it, where the
    /// file says them, to below the line a jump lands on — and words a blank
    /// line ends stay where they stand: under a header, the section's; before
    /// one, apart.
    #[test]
    fn a_settings_words_follow_it() {
        let chapter = options_chapter();
        assert!(after(chapter, "tui.scrolloff =").starts_with("  Rows of context"));
        assert!(after(chapter, "secrets.password_protect =").starts_with("  Password-protect"));
        assert!(
            after(chapter, "[tui.graph_glyphs]").starts_with("  :graph node glyphs"),
            "an example, the section's: {}",
            after(chapter, "[tui.graph_glyphs]")
        );
        let family = chapter.find("What waits on you").expect("said");
        let first = chapter.find("[tui.alert.decide]").expect("a section");
        assert!(family < first, "words before a header stay there");
        assert!(
            after(chapter, "tui.alerts.notify_via =").starts_with("  How a notification"),
            "and are not the setting's above them"
        );
    }

    /// **Every section says what it is, under its header** — the line a jump
    /// to `config.<section>` lands on. `[tui.prompt]` stood bare, its words
    /// filed under its first setting (Steven, 11-09: "We should make these
    /// sections (or in this case, subsection) clearer").
    #[test]
    fn every_section_says_what_it_is() {
        let chapter = options_chapter();
        let sections = sections();
        assert!(
            sections.len() > 20,
            "premise: the schema's sections ({sections:?})"
        );
        for section in sections {
            let header = format!("[{section}]");
            let line = chapter
                .lines()
                .find(|l| l.starts_with(&format!("{header} ")));
            assert!(
                line.is_some_and(|l| l.ends_with(&format!("*{SECTION_TAG}{section}*"))),
                "{header} tagged: {line:?}"
            );
            let words = after(chapter, &format!("{header} "));
            assert!(
                words.starts_with("  ") && !words.trim().is_empty(),
                "{header} says what it is: {words:?}"
            );
        }
    }

    /// **The manual's list of sections names every one** — itself, or its
    /// family (`[tui.alert.*]`). A hand list the schema outgrew: six were
    /// missing, `[files]` and `[mcp]` among them (Steven, 11-09).
    #[test]
    fn the_list_of_sections_names_every_section() {
        let manual = crate::help::compose(&crate::command::commands_chapter());
        let list: String = (manual.lines())
            .skip_while(|l| !l.contains("*config-tree*"))
            .skip(1)
            .take_while(|l| !crate::help::is_heading(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(list.contains("[tui.prompt]"), "premise: the list: {list}");
        for section in sections() {
            let families = std::iter::successors(Some(section.as_str()), |s| {
                s.rsplit_once('.').map(|(up, _)| up)
            })
            .skip(1)
            .map(|up| format!("[{up}.*]"));
            let named = std::iter::once(format!("[{section}]"))
                .chain(families)
                .any(|name| list.contains(&name));
            assert!(named, "[{section}] is not in THE SECTIONS");
        }
    }

    /// **Each alert switch says what it switches, and only that** — one
    /// comment above both said `bell = false` turned notifications off too,
    /// and `:help tui.alerts.notify` landed on no words (review stop 9, F3:
    /// the switch read as broken). And a section's words are its own: each
    /// alert's under its header, not under its first setting.
    #[test]
    fn the_alert_switches_each_say_their_own() {
        let chapter = options_chapter();
        let bell = after(chapter, "tui.alerts.bell =");
        assert!(
            bell.contains("bell") && !bell.contains("notification"),
            "{bell}"
        );
        let notify = after(chapter, "tui.alerts.notify =");
        assert!(notify.contains("notification"), "{notify:?}");
        assert!(after(chapter, "[tui.alert.decide] ").contains("A decision whose pane"));
        assert!(after(chapter, "[tui.alert.unshown] ").contains("News you have not seen"));
    }
}
