//! **A tool call as the transcript shows it** — the one place a call becomes
//! lines. Read by every record of one: a live call's
//! (`App::record_tool_command`, `App::record_provider_call`), a reopened or
//! imported turn's (`App::replay_turn`), and the decision pane's view of what
//! a write writes (`permission_view::change_lines`).
//!
//! Each line carries its style, one per character, chosen here: the header
//! in [`Ink::Tool`]; a command as a fenced ```` ```bash ```` block, coloured
//! by the gate's own reading of it (`permission_view::highlighted_shell`);
//! what came of it as a fenced block of its own, ```` ```output ````, in
//! `OUTPUT` — the record's colour, dim — or the file's syntax, for a
//! read; a write's lines marked
//! [`Ink::Added`]/[`Ink::Removed`] and coloured by the file's own syntax. The
//! transcript used to paint a record one flat colour, which cannot show a
//! diff; and a reopened session showed no records at all, because replay had
//! only the text to draw (Steven, 10-05: "we also lost the tool calls which
//! used to appear in blue"). Then all of it was blue: "Let's put it in a
//! markdown code block and do syntax highlighting … Can we do the same for
//! output?" (image 93).

use serde_json::Value;
use shannon_core::backend::claude_code::{self, Change};

use crate::highlight::CodeHighlighter;
use crate::markdown::FENCE;
use crate::output::{sanitize_line, sanitize_lines};
use crate::permission_view::Breaks;
use crate::style::{CellStyle, Ink};

/// How many lines of a write a record shows before it says how many more —
/// a ten-thousand-line Write is a record, not a hang.
pub const MOST_SHOWN: usize = 400;

/// How much of an input a call with no reading of its own shows.
const MOST_INPUT: usize = 160;

/// How many lines of a read's output are coloured by its file's syntax
/// before the rest is plain: syntect is per-line state, and a long `cat`
/// is a record, not a stall. A budget of its own, not [`MOST_SHOWN`] — that
/// is how much of a write is SHOWN (review of 716f4f9, C-L1).
const MOST_HIGHLIGHTED: usize = 400;

/// A record's lines, each with one style per character.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    lines: Vec<String>,
    styles: Vec<Vec<CellStyle>>,
    /// The language what came of the call is in, when the call says: a
    /// read's file's (`Read src/a.rs` → `rs`). Its output is then that
    /// file's text, coloured as it.
    output_lang: Option<String>,
    /// **Drawn only while it is live** ([`started`](Self::started)): the
    /// record that closes the call is the one a reopened turn draws.
    live_only: bool,
    /// **A record of the agent's `mark`** ([`call`](Self::call)): it says
    /// where a mark is, and its result repeats the quote, so it is never
    /// the place a quote means (`Output::find_quote`).
    of_a_mark: bool,
    /// **What it says shut** — vim's `'foldtext'`: a command's record, as
    /// `⚙ verb: the command` on one line ([`titled`](Self::titled)), the
    /// line its call stood as while it ran. Open, the command is the block
    /// under a bare header (image 93: written on the header too, it stood
    /// twice); shut, the header alone said `⚙ output of:`, which named
    /// neither what ran nor how it ended (review stop 15, E H2). `None`:
    /// its header says it.
    title: Option<(String, Vec<CellStyle>)>,
    /// **The call it records** ([`of_call`](Self::of_call)) — what a call's
    /// running line and the record closing it share, so the closing one
    /// takes the running one's place (`Output::push_tool_record`).
    call: Option<String>,
}

/// The indent a record's body sits at under its header, so a long result
/// reads as belonging to its call.
const BODY: &str = "  ";

/// **What came of a call, as drawn** — the record's own colour, dim: apart
/// from the reply (plain) and from thinking (grey, italic, dim) wherever a
/// long output is scrolled into, without its header in view.
pub(crate) const OUTPUT: CellStyle = CellStyle::ink(Ink::ToolOutput).dim();

/// The info string on the fence around what came of a call: the block under
/// the command's says what it holds, not a second bare ```` ``` ```` beside
/// the first's close.
const OUTPUT_FENCE: &str = "output";

impl Record {
    /// **`⚙ verb:`**, the verb bold, with a one-line `subject` beside it.
    pub fn header(verb: &str, subject: Option<&str>) -> Record {
        let tool = CellStyle::ink(Ink::Tool);
        let lead = format!("⚙ {verb}:");
        let bold = sanitize_line(&lead).chars().count();
        let line = match subject {
            Some(subject) => format!("{lead} {subject}"),
            None => lead,
        };
        let mut record = Record::default();
        for (i, line) in sanitize_lines(&line).into_iter().enumerate() {
            record.push_line(line, |c| {
                if i == 0 && c < bold {
                    tool.bold()
                } else {
                    tool
                }
            });
        }
        record
    }

    /// A header as written, for a test that builds a transcript by hand.
    #[cfg(test)]
    pub fn verbatim(header: &str) -> Record {
        let mut record = Record::default();
        record.push(header, CellStyle::ink(Ink::Tool));
        record
    }

    /// **A shell command's record**: its header, and the command beneath as a
    /// ```` ```bash ```` block — the shell `e` opens, laid out to `shape`
    /// (`tui.pretty_commands`, `tui.command_width`, `tui.flags_per_line`), or
    /// as written with none — coloured by the gate's reading of it. Shut, it
    /// says the verb and the command on one line ([`title`](Self::title)).
    pub fn command(verb: &str, line: &str, shape: Option<Breaks>) -> Record {
        let mut record = Record::header(verb, None);
        let clean = sanitize_lines(line).join("\n");
        let shell = crate::permission_view::highlighted_shell(&clean, shape);
        record.fenced("bash", shell);
        let (lines, styles) = Record::titled(verb, line).into_parts();
        record.title = lines.into_iter().zip(styles).next();
        record
    }

    /// **`⚙ verb: the command`, on one line** — its line breaks marked, a
    /// continuation a blank, a heredoc's body by its size
    /// (`permission_view::one_row`), coloured by the gate's reading: what a
    /// call allowed and not yet back stands as ([`started`](Self::started)),
    /// and what the record closing it says shut ([`title`](Self::title)).
    fn titled(verb: &str, line: &str) -> Record {
        let mut record = Record::header(verb, None);
        let (text, styles) = crate::permission_view::one_row(&sanitize_lines(line).join("\n"));
        record.extend_last(" ", &[CellStyle::PLAIN]);
        record.extend_last(&text, &styles);
        record
    }

    /// **A call, named for the transcript**, and what it writes when it
    /// writes: the record a live call and a replayed one share. A command is
    /// [`command`](Self::command)'s; anything else is named on the header's
    /// line.
    pub fn call(verb: &str, name: &str, input: &Value, shape: Option<Breaks>) -> Record {
        if let Some(line) = command_of(name, input) {
            return Record::command(verb, line, shape);
        }
        let mut record = Record::header(verb, Some(&subject(name, input)));
        record.of_a_mark = name == shannon_core::MARK_TOOL;
        if let Some((path, change)) = claude_code::change(name, input) {
            record.change(&path, &change);
        }
        if name == "Read" {
            record.output_lang = language(input["file_path"].as_str().unwrap_or_default());
        }
        record
    }

    /// **A call allowed and not yet back**: one line, its command on the
    /// header's ([`titled`](Self::titled)) or its name. What it ran, and
    /// what came of it, are the record that closes it ([`call`](Self::call)),
    /// which takes this one's place (`Output::push_tool_record`) and is the
    /// one a reopened turn draws: written whole here too, a command stood in
    /// the transcript twice, one fenced block above another (image 93). So
    /// this one is [`live_only`](Self::live_only) — until it is closed.
    pub fn started(verb: &str, name: &str, input: &Value) -> Record {
        let mut record = match command_of(name, input) {
            Some(line) => Record::titled(verb, line),
            None => Record::header(verb, Some(&subject(name, input))),
        };
        record.live_only = true;
        record
    }

    /// This record, of the call `call` — the id a call's running line and
    /// the record closing it share ([`call`](Self::call)'s field doc).
    pub fn of_call(mut self, call: &str) -> Record {
        self.call = Some(call.to_string());
        self
    }

    /// What came of the call, as a block of its own under it, fenced as
    /// `output`: the file's syntax for a read of a language syntect knows
    /// (its line numbers [`Ink::Muted`]), `OUTPUT` otherwise.
    pub fn output(&mut self, detail: &str) {
        if detail.is_empty() {
            return;
        }
        let mut highlighter = self
            .output_lang
            .as_deref()
            .and_then(CodeHighlighter::for_lang);
        let muted = CellStyle::ink(Ink::Muted);
        let lines: Vec<(String, Vec<CellStyle>)> = detail
            .lines()
            .enumerate()
            .map(|(n, line)| {
                let line = sanitize_line(line);
                let width = line.chars().count();
                let styles = match highlighter.as_mut().filter(|_| n < MOST_HIGHLIGHTED) {
                    Some(h) => {
                        let (number, code) = numbered(&line);
                        let mut styles = vec![muted; number.chars().count()];
                        let code_styles = h.line(code);
                        match code_styles.len() == code.chars().count() {
                            true => styles.extend(code_styles),
                            false => styles.resize(width, OUTPUT),
                        }
                        styles
                    }
                    None => vec![OUTPUT; width],
                };
                (line, styles)
            })
            .collect();
        self.fenced(OUTPUT_FENCE, lines);
    }

    /// Lines as a fenced block under the header: ```` ```lang ````, the
    /// lines, ```` ``` ````, all at the body's indent, the fences dim as a
    /// reply's are. Each line's styles are its own, one per character.
    fn fenced(&mut self, lang: &str, lines: Vec<(String, Vec<CellStyle>)>) {
        self.push(&format!("{BODY}```{lang}"), FENCE);
        for (text, styles) in lines {
            let mut cells = vec![CellStyle::PLAIN; BODY.chars().count()];
            cells.extend(styles);
            self.lines.push(format!("{BODY}{text}"));
            self.styles.push(cells);
        }
        self.push(&format!("{BODY}```"), FENCE);
    }

    /// Text and its styles, onto the end of the last line.
    fn extend_last(&mut self, text: &str, styles: &[CellStyle]) {
        if let (Some(line), Some(cells)) = (self.lines.last_mut(), self.styles.last_mut()) {
            line.push_str(text);
            cells.extend_from_slice(styles);
        }
    }

    /// **What a file tool writes, as a diff**: a Write's content, or each
    /// edit's old lines then its new. The marker carries the verdict colour
    /// — red `-`, green `+` — and the text its file's syntax colours, both
    /// sides at full contrast: a whole red block would read as an alarm, and
    /// removed text drawn faint was hard to read in the light themes
    /// (S21-Q16, Steven: "a").
    pub fn change(&mut self, path: &str, change: &Change) {
        let heading = CellStyle::PLAIN.bold();
        // Each run is highlighted on its own: an edit's old text and its new
        // are fragments of the file, not one program.
        let mut runs: Vec<(bool, &str)> = Vec::new();
        match change {
            Change::Content(text) if text.is_empty() => {
                return self.push("  what it writes: an empty file", heading);
            }
            Change::Content(text) => {
                self.push("  what it writes:", heading);
                runs.push((true, text));
            }
            Change::Edits(edits) => {
                self.push(
                    &match edits.len() {
                        1 => "  the edit:".to_string(),
                        n => format!("  the {n} edits, in order:"),
                    },
                    heading,
                );
                for (old, new) in edits {
                    runs.push((false, old));
                    runs.push((true, new));
                }
            }
        }
        let lang = language(path);
        let total: usize = runs.iter().map(|(_, text)| text.lines().count()).sum();
        let mut shown = 0;
        for (added, text) in runs {
            // Past the cap, no run is shown, so none is highlighted (review
            // of 69f2e5b, Reviewer B).
            if shown == MOST_SHOWN {
                break;
            }
            let mut highlighter = lang.as_deref().and_then(CodeHighlighter::for_lang);
            for line in text.lines() {
                if shown == MOST_SHOWN {
                    break;
                }
                shown += 1;
                self.marked(added, &sanitize_line(line), highlighter.as_mut());
            }
        }
        if total > shown {
            self.push(
                &format!("  … {} more lines", total - shown),
                CellStyle::PLAIN.dim(),
            );
        }
    }

    /// One line of a write: `  + text` or `  - text`.
    fn marked(&mut self, added: bool, text: &str, highlighter: Option<&mut CodeHighlighter>) {
        let (mark, ink) = match added {
            true => ("  + ", Ink::Added),
            false => ("  - ", Ink::Removed),
        };
        let syntax = highlighter.map(|h| h.line(text)).unwrap_or_default();
        let width = text.chars().count();
        let mut styles = vec![CellStyle::ink(ink); mark.chars().count()];
        styles.extend((0..width).map(|c| match syntax.len() == width {
            true => syntax[c],
            false => CellStyle::PLAIN,
        }));
        self.lines.push(format!("{mark}{text}"));
        self.styles.push(styles);
    }

    /// Text in one style, each of its lines sanitized first — so the styles
    /// count the characters shown.
    fn push(&mut self, text: &str, style: CellStyle) {
        for line in sanitize_lines(text) {
            self.push_line(line, |_| style);
        }
    }

    /// A sanitized line, styled by character position.
    fn push_line(&mut self, line: String, style: impl Fn(usize) -> CellStyle) {
        self.styles
            .push((0..line.chars().count()).map(style).collect());
        self.lines.push(line);
    }

    /// The lines, and their styles in parallel.
    pub fn into_parts(self) -> (Vec<String>, Vec<Vec<CellStyle>>) {
        (self.lines, self.styles)
    }

    /// **Whether a reopened turn draws no record in its place** — a call's
    /// `⚙ running:` line ([`started`](Self::started)): a mark's kept place
    /// counts the blocks a replay draws (`output::Position`), so this one is
    /// not among them.
    pub fn live_only(&self) -> bool {
        self.live_only
    }

    /// Whether this records the agent's `mark` — never a quote's place.
    pub fn of_a_mark(&self) -> bool {
        self.of_a_mark
    }

    /// What it says shut, when its header does not ([`title`](Self::title)'s
    /// field doc): its text and one style per character.
    pub fn title(&self) -> Option<&(String, Vec<CellStyle>)> {
        self.title.as_ref()
    }

    /// The call it records, when it was given one ([`of_call`](Self::of_call)).
    pub fn call_id(&self) -> Option<&str> {
        self.call.as_deref()
    }
}

/// The command a call runs, when it is a shell call: Shannon's `bash`,
/// Claude Code's `Bash`.
fn command_of<'a>(name: &str, input: &'a Value) -> Option<&'a str> {
    match name {
        shannon_core::BASH_TOOL | "Bash" => Some(input["command"].as_str().unwrap_or_default()),
        _ => None,
    }
}

/// `     12→text` — a read's line, numbered as Claude Code numbers it —
/// split into its number and its text; all text when it is not numbered.
/// (Sanitized first, so a `cat -n` tab is a blank by now, and reads as text.)
fn numbered(line: &str) -> (&str, &str) {
    let lead = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[lead..];
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    match rest[digits..].starts_with('→') && digits > 0 {
        true => line.split_at(lead + digits + '→'.len_utf8()),
        false => ("", line),
    }
}

/// **A call that is not a command, as the transcript names it**: a file
/// tool by what it touches; anything else by its name and its input, cut
/// short. Claude Code's gate reads each of its tools as a shell line (`cat >
/// path` for an Edit), and that is the right question to judge — but the
/// record of what ran names the tool. A command never gets here: both
/// callers take it first (`command_of`), as a code block
/// ([`Record::command`]).
fn subject(name: &str, input: &Value) -> String {
    let text = |key: &str| input[key].as_str().unwrap_or_default();
    let then = |key: &str, lead: &str| match text(key) {
        "" => String::new(),
        value => format!(" {lead}{value}"),
    };
    match name {
        "Read" => format!("Read {}{}", text("file_path"), lines_read(input)),
        "Write" | "Edit" | "MultiEdit" => format!("{name} {}", text("file_path")),
        "NotebookEdit" => format!("{name} {}", text("notebook_path")),
        "Grep" => format!(
            "Grep {}{}{}",
            text("pattern"),
            then("path", "in "),
            then("glob", "--glob ")
        ),
        "Glob" => format!("Glob {}{}", text("pattern"), then("path", "in ")),
        "WebFetch" => format!("WebFetch {}", text("url")),
        "WebSearch" => format!("WebSearch {}", text("query")),
        "Task" | "Agent" => format!("{name} {}", text("description")),
        // Shannon's own (`skill`: `name`, `arguments`) and Claude Code's
        // (`Skill`: `skill`, `args`), by the skill and what it was given.
        "skill" => format!("skill {}{}", text("name"), then("arguments", "")),
        "Skill" => format!("Skill {}{}", text("skill"), then("args", "")),
        // Shannon's own, the agent's mark (`ToolDef::mark`): by its note.
        // Its letter is the result's, under it.
        "mark" => format!("mark \"{}\"", cut(text("note"), MOST_INPUT)),
        "TodoWrite" => match input["todos"].as_array() {
            Some(todos) => format!("TodoWrite, {} items", todos.len()),
            None => name.to_string(),
        },
        _ => format!("{name} {}", cut(&input.to_string(), MOST_INPUT)),
    }
}

/// **A prompt Claude Code's harness wrote, as what it says**: each
/// `<task-notification>` element — a background task's end, delivered to
/// the model as a user turn — shown as `⚙ task <id> <status>: <summary>`
/// with its result beneath, the XML gone (image 90). For the display only:
/// the stored prompt is what the model read and stays so. `None` when the
/// prompt holds no such element.
pub fn shown_prompt(prompt: &str) -> Option<String> {
    const OPEN: &str = "<task-notification>";
    const CLOSE: &str = "</task-notification>";
    if !prompt.contains(OPEN) {
        return None;
    }
    let mut out = String::new();
    let mut rest = prompt;
    while let Some(start) = rest.find(OPEN) {
        let body = &rest[start + OPEN.len()..];
        let Some(end) = body.find(CLOSE) else {
            break; // unclosed: shown as written
        };
        out.push_str(&rest[..start]);
        out.push_str(&notification(&body[..end]));
        rest = &body[end + CLOSE.len()..];
    }
    out.push_str(rest);
    Some(out)
}

/// **What a turn told the model beyond its prompt, as one line** — the
/// transcript's account of its context blocks (`Block::Context`), live and on
/// replay alike: `⚙ Shannon told the model: AGENTS.md · memory MEMORY.md ·
/// the skills list`. The text itself stays in the session (`:graph`'s
/// inspector, `$EDITOR`): an instruction file a screen long, on every new
/// session, would bury the prompt it came with. `None` when the turn told
/// nothing. Who told it is said: `⚙ told the model: how to keep memories`,
/// under your prompt, read as something you had said (Steven, 11-03, S21-Q30:
/// "name who did it").
///
/// What it is, then what became of it: an `unloaded` block of Shannon's own
/// guidance read "told the model: how to keep memories" the turn it was
/// withdrawn (Reviewers A and C, round 27). A file whose text was scrubbed
/// (`App::outbound`) says so — `[redacted: …]` is what the model read.
pub fn context_line(blocks: &[shannon_core::Block]) -> Option<String> {
    use shannon_core::{Block, ContextKind};
    let told: Vec<(ContextKind, &str, bool)> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Context { kind, source, text } => {
                Some((*kind, source.as_str(), text.contains("[redacted: ")))
            }
            _ => None,
        })
        .collect();
    // A source is a path but for Shannon's own blocks and a skill's name.
    let is_path = |kind: ContextKind, source: &str| {
        kind != ContextKind::Skill && !source.starts_with("shannon:")
    };
    let paths: Vec<&str> = told
        .iter()
        .filter(|(kind, source, _)| is_path(*kind, source))
        .map(|(_, source, _)| *source)
        .collect();
    let mut names = distinct_names(&paths).into_iter();
    let said: Vec<String> = told
        .iter()
        .map(|&(kind, source, redacted)| {
            let path = is_path(kind, source);
            let what = match source {
                // `paths` is `told` under the same filter, so in step with it.
                _ if path => names.next().expect("one name per path, in order"),
                "shannon:memory" => "how to keep memories".to_string(),
                "shannon:skills" => "the skills list".to_string(),
                name => name.to_string(),
            };
            let said = match kind {
                ContextKind::Unloaded => format!("no longer {what}"),
                ContextKind::Skill => format!("skill {what}"),
                ContextKind::Memory if path => format!("memory {what}"),
                ContextKind::Skills if path => format!("skills {what}"),
                _ => what,
            };
            match redacted {
                true => format!("{said} (redacted)"),
                false => said,
            }
        })
        .collect();
    (!said.is_empty()).then(|| format!("⚙ Shannon told the model: {}", said.join(" · ")))
}

/// **Each path by the fewest trailing components no other in `paths` shares**
/// — an editor's rule for two tabs of one name. A file name alone said
/// `AGENTS.md · AGENTS.md` for the two a repository walk finds, and both
/// default memory locations end `memory/MEMORY.md`: two files, one name.
fn distinct_names(paths: &[&str]) -> Vec<String> {
    let parts: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| p.split('/').filter(|c| !c.is_empty()).collect())
        .collect();
    let tail = |p: &[&'_ str], k: usize| p[p.len().saturating_sub(k)..].join("/");
    parts
        .iter()
        .enumerate()
        .map(|(i, mine)| {
            let shared = |k: usize| {
                parts
                    .iter()
                    .enumerate()
                    .any(|(j, other)| j != i && other != mine && tail(other, k) == tail(mine, k))
            };
            let k = (1..mine.len()).find(|&k| !shared(k)).unwrap_or(mine.len());
            tail(mine, k.max(1))
        })
        .collect()
}

/// One notification's fields, as a line and its result.
fn notification(element: &str) -> String {
    // The text between `<name>` and its closer: the first closer, or for a
    // result — which may quote anything — the last.
    let field = |name: &str, last: bool| -> Option<&str> {
        let (open, close) = (format!("<{name}>"), format!("</{name}>"));
        let from = element.find(&open)? + open.len();
        let to = match last {
            true => element.rfind(&close)?,
            false => from + element[from..].find(&close)?,
        };
        element.get(from..to).map(str::trim)
    };
    let mut line = format!(
        "⚙ task {} {}",
        field("task-id", false).unwrap_or("?"),
        field("status", false).unwrap_or("ended")
    );
    if let Some(summary) = field("summary", false) {
        line.push_str(&format!(": {summary}"));
    }
    match field("result", true) {
        Some(result) if !result.is_empty() => format!("{line}\n{result}"),
        _ => line,
    }
}

/// A `Read`'s range, when it names one: `, from line 40, 60 lines`.
fn lines_read(input: &Value) -> String {
    let mut out = String::new();
    if let Some(offset) = input["offset"].as_u64() {
        out.push_str(&format!(", from line {offset}"));
    }
    if let Some(limit) = input["limit"].as_u64() {
        out.push_str(&format!(", {limit} lines"));
    }
    out
}

/// The language a path's text is in, for syntax colours: its extension, or
/// its whole name (`Makefile`).
fn language(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    path.extension()
        .or_else(|| path.file_name())
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

/// `s`, cut to its first `most` characters, marked where cut.
fn cut(s: &str, most: usize) -> String {
    match s.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(record: &Record) -> Vec<String> {
        record.clone().into_parts().0
    }

    /// A command laid out as the pane lays it out, by the default breaks.
    fn laid() -> Option<Breaks> {
        Some(Breaks::default())
    }

    /// Every line has one style per character it shows — the invariant the
    /// renderer's per-cell path needs, or the row falls back to flat.
    fn aligned(record: &Record) {
        let (lines, styles) = record.clone().into_parts();
        assert_eq!(lines.len(), styles.len());
        for (line, styles) in lines.iter().zip(&styles) {
            assert_eq!(line.chars().count(), styles.len(), "{line:?}");
        }
    }

    /// **Two files told in one turn never read as one** — each named by as
    /// much of its path as tells it apart, and no more.
    #[test]
    fn what_a_turn_told_names_each_file_apart() {
        let told = |kind, source: &str| shannon_core::Block::Context {
            kind,
            source: source.into(),
            text: String::new(),
        };
        use shannon_core::ContextKind::{Instructions, Memory, Skill, Unloaded};
        let line = context_line(&[
            told(Instructions, "/r/AGENTS.md"),
            told(Instructions, "/r/sub/AGENTS.md"),
            told(Memory, "/c/shannon/memory/MEMORY.md"),
            told(Memory, "/c/shannon/projects/p-1/memory/MEMORY.md"),
            told(Instructions, "/r/SHANNON.md"),
            told(Memory, "shannon:memory"),
            told(Skill, "review"),
            told(Unloaded, "/r/old.md"),
        ]);
        assert_eq!(
            line.as_deref(),
            Some(
                "⚙ Shannon told the model: r/AGENTS.md · sub/AGENTS.md · memory shannon/memory/MEMORY.md \
                 · memory p-1/memory/MEMORY.md · SHANNON.md · how to keep memories · skill review \
                 · no longer old.md"
            )
        );
        // Alone, a name is enough.
        assert_eq!(
            context_line(&[told(Memory, "/c/shannon/memory/MEMORY.md")]).as_deref(),
            Some("⚙ Shannon told the model: memory MEMORY.md")
        );
        assert_eq!(context_line(&[]), None);
        // Shannon's own, withdrawn, is said as gone — never as told.
        assert_eq!(
            context_line(&[
                told(Unloaded, "shannon:memory"),
                told(Unloaded, "shannon:skills")
            ])
            .as_deref(),
            Some(
                "⚙ Shannon told the model: no longer how to keep memories · no longer the skills \
                 list"
            )
        );
        // What the model read scrubbed is said to be.
        let scrubbed = shannon_core::Block::Context {
            kind: Instructions,
            source: "/r/AGENTS.md".into(),
            text: "key: [redacted: high-entropy string]".into(),
        };
        assert_eq!(
            context_line(&[scrubbed]).as_deref(),
            Some("⚙ Shannon told the model: AGENTS.md (redacted)")
        );
    }

    #[test]
    fn a_file_tool_is_named_by_its_file_and_shows_its_change() {
        let edit = json!({"file_path": "src/a.rs", "old_string": "let x = 1;",
                          "new_string": "let x = 2;\nlet y = 3;"});
        let record = Record::call("Claude Code ran", "Edit", &edit, laid());
        assert_eq!(
            text(&record),
            [
                "⚙ Claude Code ran: Edit src/a.rs",
                "  the edit:",
                "  - let x = 1;",
                "  + let x = 2;",
                "  + let y = 3;",
            ]
        );
        aligned(&record);
        let (_, styles) = record.into_parts();
        // The markers carry the verdict colours; the text its syntax's.
        assert_eq!(styles[2][2].ink, Some(Ink::Removed));
        assert_eq!(styles[3][2].ink, Some(Ink::Added));
        assert!(
            styles[3][4..].iter().any(|s| s.ink.is_some()),
            "rust is coloured"
        );
        // Neither side faint, nor red: the marker says which it is (S21-Q16).
        assert!(styles[2][4..].iter().all(|s| !s.dim), "removed text full");
        assert!(
            styles[2][4..].iter().all(|s| s.ink != Some(Ink::Removed)),
            "a red block would read as an alarm"
        );
        assert!(styles[3][4..].iter().all(|s| !s.dim));
        // The verb is bold, the subject is not.
        assert!(styles[0][0].bold && !styles[0].last().unwrap().bold);
    }

    #[test]
    fn a_long_write_says_how_much_it_did_not_show() {
        let content: String = (0..MOST_SHOWN + 5).map(|i| format!("{i}\n")).collect();
        let write = json!({"file_path": "notes", "content": content});
        let record = Record::call("Claude Code ran", "Write", &write, laid());
        let lines = text(&record);
        assert_eq!(lines.len(), 2 + MOST_SHOWN + 1);
        assert_eq!(lines.last().unwrap(), "  … 5 more lines");
        aligned(&record);
    }

    /// **A read's syntax colours stop at a budget of their own**, and the
    /// rest is plain output (review of 716f4f9, C-L1: the cap had no test).
    #[test]
    fn a_long_read_is_coloured_up_to_its_budget() {
        let read = json!({"file_path": "a.rs"});
        let mut record = Record::call("Claude Code ran", "Read", &read, laid());
        let body: String = (0..MOST_HIGHLIGHTED + 2).map(|_| "let x = 1;\n").collect();
        record.output(&body);
        aligned(&record);
        let (lines, styles) = record.into_parts();
        // Coloured by its syntax: a role, and not the ones plain output and
        // the line numbers wear.
        let coloured = |i: usize| {
            styles[i].iter().any(|s| {
                s.ink
                    .is_some_and(|ink| ink != Ink::Muted && ink != Ink::ToolOutput)
            })
        };
        let first = lines
            .iter()
            .position(|l| l.trim() == "let x = 1;")
            .expect("the output is drawn");
        assert!(coloured(first), "rust is coloured: {:?}", styles[first]);
        assert!(coloured(first + MOST_HIGHLIGHTED - 1), "up to the budget");
        assert!(!coloured(first + MOST_HIGHLIGHTED), "and plain past it");
    }

    #[test]
    fn anything_but_a_command_is_named_by_its_input() {
        let read = json!({"file_path": "a.rs", "offset": 40, "limit": 60});
        assert_eq!(subject("Read", &read), "Read a.rs, from line 40, 60 lines");
        let grep = json!({"pattern": "fn main", "path": "src"});
        assert_eq!(subject("Grep", &grep), "Grep fn main in src");
        let named = subject("Mystery", &json!({"x": "y".repeat(400)}));
        assert!(named.starts_with("Mystery {\"x\":") && named.ends_with('…'));
        assert!(named.chars().count() < MOST_INPUT + 20);
        // A command is a code block, not a header's subject.
        let record = Record::call("ran", "Bash", &json!({"command": "ls"}), laid());
        assert_eq!(text(&record), ["⚙ ran:", "  ```bash", "  ls", "  ```"]);
    }

    /// The ink of the character where `needle` starts on `line`.
    fn ink_at(record: &Record, line: usize, needle: &str) -> Option<Ink> {
        let (lines, styles) = record.clone().into_parts();
        let at = lines[line]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} in {:?}", lines[line]));
        styles[line][lines[line][..at].chars().count()].ink
    }

    /// **A command is a ```` ```bash ```` block, coloured by what each piece
    /// is to the shell** (Steven, 10-05, image 93: "everything is in blue").
    #[test]
    fn a_command_is_a_shell_block_coloured_by_the_gates_reading() {
        let line = "rg --count 'TODO|FIXME' \"at $DIR\" 2>/dev/null | head -5";
        let record = Record::command("Claude Code ran", line, laid());
        assert_eq!(
            text(&record),
            [
                "⚙ Claude Code ran:",
                "  ```bash",
                &format!("  {line}"),
                "  ```"
            ]
        );
        aligned(&record);
        for (needle, ink) in [
            ("rg", Ink::FunctionCall),
            ("--count", Ink::Parameter),
            ("'TODO", Ink::String),
            ("\"at", Ink::String),
            ("$DIR", Ink::VariableSpecial),
            ("2>", Ink::Operator),
            ("| head", Ink::Operator),
            ("head", Ink::FunctionCall),
            ("-5", Ink::Parameter),
        ] {
            assert_eq!(ink_at(&record, 2, needle), Some(ink), "{needle}");
        }
        assert_eq!(
            ink_at(&record, 2, "/dev/null"),
            None,
            "a redirect's target is a file"
        );
        let (_, styles) = record.into_parts();
        assert_eq!(styles[1], vec![FENCE; 9], "the fences are a reply's");
        // Its weight is the theme's, not the cell's: the shipped one bolds a
        // command's name.
        assert!(!styles[2][2].bold, "the role alone");
        let shown =
            crate::theme::Theme::terminal().style(crate::theme::Context::Code, styles[2][2]);
        assert!(
            shown.add_modifier.contains(ratatui::style::Modifier::BOLD),
            "a command's name is bold"
        );

        // Where a command's name would be: an assignment is not one, a
        // substitution is, and a reserved word is its own.
        let record = Record::command("ran", "LC_ALL=C sort -u f; $(which rg) --version", None);
        for (needle, ink) in [
            ("LC_ALL=C", Ink::VariableSpecial),
            ("sort", Ink::FunctionCall),
            ("$(which", Ink::VariableSpecial),
            ("--version", Ink::Parameter),
        ] {
            assert_eq!(ink_at(&record, 2, needle), Some(ink), "{needle}");
        }
        let record = Record::command("ran", ">log.txt sort -u f", None);
        assert_eq!(ink_at(&record, 2, "log.txt"), None, "the redirect's file");
        assert_eq!(
            ink_at(&record, 2, "sort"),
            Some(Ink::FunctionCall),
            "then the command"
        );
        let record = Record::command("ran", "for x in a b; do echo $x; done", None);
        assert_eq!(ink_at(&record, 2, "for"), Some(Ink::Keyword));
        assert_eq!(ink_at(&record, 2, "done"), Some(Ink::Keyword));
        assert_eq!(ink_at(&record, 2, "echo"), Some(Ink::FunctionCall));
    }

    /// A long command is the shell `e` opens — continued with ` \`, so the
    /// block can be copied and run — and as written without `pretty`.
    #[test]
    fn a_long_command_is_the_shell_e_opens() {
        let long = "rg --no-heading --with-filename --count --ignore-case --type=rust \
                    --glob='!target/**' --regexp='TODO|FIXME' .";
        let laid = text(&Record::command("ran", long, self::laid()));
        assert!(laid.len() > 4, "{laid:?}");
        assert!(laid[2].ends_with(" \\"), "continued: {laid:?}");
        let flat = text(&Record::command("ran", long, None));
        assert_eq!(flat[2], format!("  {long}"));
    }

    /// **A call not yet back is one line** — what it ran is the closing
    /// record's, which is the one a reopened turn draws (image 93 had the
    /// command twice, block above block).
    #[test]
    fn a_call_not_yet_back_is_one_line() {
        let record = Record::started("running", "bash", &json!({"command": "echo a\necho b"}));
        assert_eq!(text(&record), ["⚙ running: echo a ⏎ echo b"]);
        aligned(&record);
        // A continuation is no break to the shell, so none is marked — the
        // shape `e` writes back (Steven, 10-31: "all these return symbols").
        let edited = "cd ~/x \\\n  && find . -type f \\\n         -name '*.md'\necho ---";
        let continued = Record::started("running", "bash", &json!({ "command": edited }));
        assert_eq!(
            text(&continued),
            ["⚙ running: cd ~/x && find . -type f -name '*.md' ⏎ echo ---"]
        );
        assert_eq!(ink_at(&record, 0, "echo b"), Some(Ink::FunctionCall));
        // A heredoc's body by its size, so what ran after it is on the row
        // (Steven, 11-02, image 45) — and so on the shut record (`title`).
        let script = "cat > t.py <<'EOF'\nimport os\nEOF\npython3 t.py";
        let heredoc = Record::started("running", "bash", &json!({ "command": script }));
        assert_eq!(
            text(&heredoc),
            ["⚙ running: cat > t.py <<'EOF' ⟨1 line⟩ ⏎ python3 t.py"]
        );
        aligned(&heredoc);
        let write = json!({"file_path": "a.rs", "content": "fn main() {}"});
        let written = Record::started("Claude Code runs", "Write", &write);
        assert_eq!(text(&written), ["⚙ Claude Code runs: Write a.rs"]);
        // Drawn live only, a command or not; the closing record is kept.
        assert!(record.live_only() && written.live_only());
        assert!(!Record::call("ran", "Write", &write, None).live_only());
    }

    /// **Shut, a command's record says the line its call ran as** — `⚙ verb:
    /// the command` on one line, in the running line's colours (`title`,
    /// vim's `'foldtext'`) — so the record that takes a running line's place
    /// says, folded, what that line said, its verb now how it ended (review
    /// stop 15, E H2: it said `⚙ output of:` of every command). Open, its
    /// header stays bare: the command is the block under it, written once
    /// (image 93).
    #[test]
    fn shut_a_commands_record_says_the_line_its_call_ran_as() {
        for command in ["echo the tide", "cd ~/x \\\n  && ls\necho ---"] {
            let input = json!({ "command": command });
            let (running, styles) = Record::started("ran", "bash", &input).into_parts();
            let record = Record::command("ran", command, laid());
            let title = Some((running[0].clone(), styles[0].clone()));
            assert_eq!(record.title(), title.as_ref(), "{command:?}");
            assert_eq!(text(&record)[0], "⚙ ran:", "open, the header bare");
        }
        let bash = Record::call("Claude Code ran", "Bash", &json!({"command": "ls"}), None);
        let title = bash.title().map(|(text, _)| text.as_str());
        assert_eq!(title, Some("⚙ Claude Code ran: ls"));
        let read = Record::call(
            "Claude Code ran",
            "Read",
            &json!({"file_path": "a.rs"}),
            None,
        );
        assert_eq!(read.title(), None, "its header names it");
    }

    /// **A read's output is its file's text**, coloured as it, the line
    /// numbers Claude Code adds kept quiet; anything else's is `OUTPUT`.
    #[test]
    fn a_reads_output_is_coloured_as_its_file() {
        let mut record = Record::call("ran", "Read", &json!({"file_path": "src/a.rs"}), laid());
        record.output("     1→fn main() {}\n     2→let x = 1;");
        let lines = text(&record);
        assert_eq!(
            lines[1..],
            [
                "  ```output",
                "       1→fn main() {}",
                "       2→let x = 1;",
                "  ```"
            ]
        );
        aligned(&record);
        let (_, styles) = record.into_parts();
        assert!(
            styles[2][..9]
                .iter()
                .skip(2)
                .all(|s| s.ink == Some(Ink::Muted))
        );
        assert!(
            styles[2][9..]
                .iter()
                .any(|s| s.ink.is_some_and(|ink| ink != Ink::Muted)),
            "rust is coloured"
        );

        let mut ran = Record::command("ran", "cat src/a.rs", laid());
        ran.output("fn main() {}");
        let (_, styles) = ran.into_parts();
        assert!(
            styles[5][2..].iter().all(|s| *s == OUTPUT),
            "a command's is not read as a file's"
        );
        let mut quiet = Record::command("ran", "true", laid());
        quiet.output("");
        assert_eq!(text(&quiet).len(), 4, "no output, no block");
    }

    #[test]
    fn a_task_notification_is_shown_as_what_it_says() {
        let prompt = "<task-notification>\n<task-id>wr2x</task-id>\n\
                      <tool-use-id>[redacted]</tool-use-id>\n<status>completed</status>\n\
                      <summary>Workflow \"review\" completed</summary>\n\
                      <result>[{\"verdict\": \"<result>ok</result>\"}]</result>\n\
                      </task-notification>\nthen this";
        assert_eq!(
            shown_prompt(prompt).as_deref(),
            Some(
                "⚙ task wr2x completed: Workflow \"review\" completed\n\
                 [{\"verdict\": \"<result>ok</result>\"}]\nthen this"
            ),
            "a result quoting its own tag is kept whole"
        );
        assert_eq!(shown_prompt("an ordinary prompt"), None);
        // Malformed, never a panic: unclosed shows as written; a closer
        // before its opener, or a missing field, is read as absent.
        let unclosed = "<task-notification><task-id>x</task-id>";
        assert_eq!(shown_prompt(unclosed).as_deref(), Some(unclosed));
        let odd = "<task-notification></result><result></task-notification>";
        assert_eq!(shown_prompt(odd).as_deref(), Some("⚙ task ? ended"));
    }

    proptest::proptest! {
        /// Untrusted text never panics the display (`[[feedback_fuzz_the_boundary]]`).
        #[test]
        fn any_prompt_is_shown_without_panicking(
            s in "(<task-notification>|</task-notification>|<result>|</result>|<summary>|é|x| ){0,40}"
        ) {
            let _ = shown_prompt(&s);
        }
    }

    /// **What came of a call is a block of its own, fenced as `output`**,
    /// in the record's colour dim (`OUTPUT`) — and a control character in it
    /// never misaligns its styles.
    #[test]
    fn output_is_its_own_block_and_control_characters_never_misalign() {
        let mut record = Record::header("ran", Some("printf 'a\\tb'"));
        record.output("a\tb\u{1b}[31m\nsecond");
        assert_eq!(
            text(&record)[1..],
            ["  ```output", "  a b[31m", "  second", "  ```"]
        );
        aligned(&record);
        let (_, styles) = record.into_parts();
        assert!(styles[2][2..].iter().all(|s| *s == OUTPUT));
        assert!(OUTPUT.dim && OUTPUT.ink == Some(Ink::ToolOutput));
        assert!(styles[0].iter().all(|s| s.ink == Some(Ink::Tool)));
    }

    proptest::proptest! {
        /// A command the model wrote never misaligns its styles or panics
        /// the record, whatever it holds (`[[feedback_fuzz_the_boundary]]`).
        #[test]
        fn any_command_is_recorded_aligned(
            s in "(rg|-n|--x=|'|\"|\\$|\\{|\\}|\\\\|é|\t|\n|\u{1b}|\\||&&|;|>|<<E|E| |do|done|for){0,30}",
            pretty in proptest::bool::ANY,
        ) {
            aligned(&Record::command("ran", &s, pretty.then(Breaks::default)));
            aligned(&Record::started("running", "bash", &json!({"command": s})));
        }
    }
}
