//! Lexing — one quote-aware scan shared by the simple-command path
//! ([`super::analyze`]) and the whole-line path ([`super::pipeline`]).
//!
//! It emits words and the shell **operators** that separate them, and refuses
//! (fails closed) on constructs it will not interpret: subshells, arithmetic,
//! backgrounding, heredocs. Having exactly one
//! implementation of quote handling matters: two copies would eventually
//! disagree, and a disagreement between "what we analyzed" and "what the shell
//! runs" is a security bug, not a formatting nit.
//!
//! Each lexeme carries the **byte range** of the source that produced it (a
//! [`Spanned`]). The word's *node* holds the quote-resolved text (`"$i"` → `$i`),
//! but its *span* covers the original slice including the quotes — so a consumer
//! can recover the text as written by re-slicing the input. That split (resolved
//! value vs. source span) is what lets the permission view highlight a command
//! in situ and show it faithfully, while the engine still matches on the
//! canonical form.

use std::ops::Range;

use super::Unanalyzable;

/// A shell operator we recognize and model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `&&` — run next only on success.
    And,
    /// `||` — run next only on failure.
    Or,
    /// `;` — run next unconditionally.
    Seq,
    /// A newline — a command separator exactly like `;`. Distinguished only so
    /// messages can name it. Treating a newline as mere whitespace would merge
    /// two commands into one, hiding the second inside the first's operands.
    Newline,
    /// `|` — pipe stdout into the next command.
    Pipe,
    /// `>` — truncate-write stdout to a file.
    Write,
    /// `>>` — append stdout to a file.
    Append,
    /// `<` — read stdin from a file.
    Read,
    /// `N>&M` / `N<&M` — duplicate a file descriptor (`2>&1`), or close one
    /// (`>&-`). Names no file, so it carries no target: the whole construct,
    /// digits included, is this one lexeme.
    Dup,
    /// `<<DELIM` / `<<-DELIM` — a heredoc: stdin from the lines that follow,
    /// up to `DELIM`. Names no file; the body arrives as its own
    /// [`Lexeme::HereDoc`] after the line ends.
    HereDoc,
}

impl Operator {
    /// The operator as written, for messages.
    pub fn symbol(self) -> &'static str {
        match self {
            Operator::And => "&&",
            Operator::Or => "||",
            Operator::Seq => ";",
            Operator::Newline => "newline",
            Operator::Pipe => "|",
            Operator::Write => ">",
            Operator::Append => ">>",
            Operator::Read => "<",
            Operator::Dup => ">&",
            Operator::HereDoc => "<<",
        }
    }

    /// Does this operator separate one command from the next (as opposed to
    /// attaching a redirection to the current one)?
    pub fn is_connector(self) -> bool {
        matches!(
            self,
            Operator::And | Operator::Or | Operator::Seq | Operator::Pipe | Operator::Newline
        )
    }
}

/// A value paired with the byte range of the source text that produced it.
///
/// The range indexes the *original* input the lexer was given, so
/// `&input[spanned.span]` recovers the exact source slice — quotes, spelling,
/// and all — even when `node` holds a resolved/canonical form. This is the
/// "spans = chars, identity separate from measure" doctrine applied to shell
/// tokens: the node is the value we reason about, the span is where it came
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Range<usize>,
}

/// A lexed unit: a word (quotes resolved), an operator, or a **substitution**
/// whose inner command text we captured to analyze on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lexeme {
    Word(String),
    Op(Operator),
    /// `$(cmd)`, `` `cmd` ``, `<(cmd)`, or `>(cmd)` — a construct that **runs
    /// `cmd`** and splices its output (or a FIFO path) in place. We can't know the
    /// resulting text, but we *can* know what `cmd` does, so we keep its inner
    /// text to analyze as its own pipeline rather than refusing the whole line.
    /// The value it produces is treated as unresolved.
    Subst(String),
    /// A heredoc's BODY — the lines after an [`Operator::HereDoc`]'s line, up
    /// to its delimiter — as DATA: the shell expands nothing in it (its
    /// delimiter was quoted, or it holds no `$` or backtick to expand). Its
    /// span covers the body and the delimiter line, so a view can print it
    /// as written.
    HereDoc(String),
}

/// A heredoc whose body the lexer has yet to read.
struct PendingHereDoc {
    delimiter: String,
    /// `<<-`: leading tabs are stripped from the body and the delimiter line.
    strip_tabs: bool,
    /// Any quoting in the delimiter: the body is taken literally.
    quoted: bool,
    /// Where its operator began, for a refusal's span.
    at: usize,
}

/// Read a heredoc's delimiter word after `<<` / `<<-`: its text with quotes
/// removed, and whether any part was quoted (`'EOF'`, `"EOF"`, `\EOF`,
/// `E'O'F`) — which is what makes the body literal. `None` if there is none,
/// or a quote in it never closes.
fn heredoc_delimiter(cursor: &mut Cursor<'_>) -> Option<(String, bool)> {
    let mut delimiter = String::new();
    let mut quoted = false;
    while let Some(c) = cursor.peek() {
        match c {
            '\'' | '"' => {
                cursor.next();
                quoted = true;
                loop {
                    match cursor.next()? {
                        close if close == c => break,
                        ch => delimiter.push(ch),
                    }
                }
            }
            '\\' => {
                cursor.next();
                quoted = true;
                delimiter.push(cursor.next()?);
            }
            c if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')') => break,
            c => {
                cursor.next();
                delimiter.push(c);
            }
        }
    }
    (!delimiter.is_empty()).then_some((delimiter, quoted))
}

/// A heredoc's body in `input` from byte `from`: the lines up to the one that
/// IS its delimiter (after leading tabs, for `<<-`), and the offset just past
/// that line. `None` when no line closes it.
fn heredoc_body(input: &str, from: usize, doc: &PendingHereDoc) -> Option<(String, usize)> {
    let mut body = String::new();
    let mut at = from;
    loop {
        let rest = input.get(at..).filter(|r| !r.is_empty())?;
        let (line, next) = match rest.find('\n') {
            Some(n) => (&rest[..n], at + n + 1),
            None => (rest, input.len()),
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line = match doc.strip_tabs {
            true => line.trim_start_matches('\t'),
            false => line,
        };
        if line == doc.delimiter {
            return Some((body, next));
        }
        body.push_str(line);
        body.push('\n');
        at = next;
    }
}

/// A char cursor over the input that also tracks byte offsets, so each lexeme
/// can record the source range that produced it.
///
/// `next`/`peek` mirror `Peekable<Chars>` (returning the `char`, dropping the
/// index the callers don't want), while `end` reports the byte offset just past
/// the last char `next` returned. The start of any construct is then
/// `end - c.len_utf8()` for the char `c` that opened it — no separate bookkeeping.
struct Cursor<'a> {
    iter: std::iter::Peekable<std::str::CharIndices<'a>>,
    /// Byte offset just past the last char returned by `next` (0 before any).
    end: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str) -> Cursor<'a> {
        Cursor {
            iter: input.char_indices().peekable(),
            end: 0,
        }
    }

    fn next(&mut self) -> Option<char> {
        match self.iter.next() {
            Some((i, c)) => {
                self.end = i + c.len_utf8();
                Some(c)
            }
            None => None,
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.iter.peek().map(|&(_, c)| c)
    }

    /// Byte offset just past the last char `next` returned.
    fn end(&self) -> usize {
        self.end
    }
}

/// Capture the balanced inner text of a `(…)`-delimited substitution, having
/// already consumed the opening `(`. Returns the inner text (without the
/// parens), or `None` if it never balances — in which case the caller **refuses**
/// rather than analyzing a fragment. Nested `(`/`)` and quoted regions are
/// tracked so a `)` inside a string or a nested `$( )` doesn't close early.
fn capture_parens(cursor: &mut Cursor<'_>) -> Option<String> {
    let mut inner = String::new();
    let mut depth = 1usize;
    while let Some(c) = cursor.next() {
        match c {
            '(' => {
                depth += 1;
                inner.push(c);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(inner);
                }
                inner.push(c);
            }
            // Skip over quoted spans verbatim — a `)` inside them is data.
            '\'' => {
                inner.push(c);
                while let Some(q) = cursor.next() {
                    inner.push(q);
                    if q == '\'' {
                        break;
                    }
                }
            }
            '"' => {
                inner.push(c);
                while let Some(q) = cursor.next() {
                    inner.push(q);
                    if q == '\\' {
                        if let Some(e) = cursor.next() {
                            inner.push(e);
                        }
                    } else if q == '"' {
                        break;
                    }
                }
            }
            '\\' => {
                inner.push(c);
                if let Some(e) = cursor.next() {
                    inner.push(e);
                }
            }
            _ => inner.push(c),
        }
    }
    None // never balanced
}

/// Capture the inner text of a backtick substitution, having consumed the
/// opening `` ` ``. Backticks don't nest; a `\` escapes the next char.
fn capture_backtick(cursor: &mut Cursor<'_>) -> Option<String> {
    let mut inner = String::new();
    while let Some(c) = cursor.next() {
        match c {
            '`' => return Some(inner),
            '\\' => {
                inner.push(c);
                if let Some(e) = cursor.next() {
                    inner.push(e);
                }
            }
            _ => inner.push(c),
        }
    }
    None
}

/// After a `>&` or `<&`: how many bytes of `rest` are the descriptor it
/// duplicates (`2>&1`) or the `-` that closes one (`>&-`) — blanks before it
/// included — when that is what follows, up to a delimiter. `None` means the
/// word after is something else, and bash then reads `>&word` as a redirect of
/// stdout AND stderr into the FILE `word`.
fn dup_target_len(rest: &str) -> Option<usize> {
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let body = &rest[blanks..];
    let len = match body.bytes().take_while(u8::is_ascii_digit).count() {
        0 if body.starts_with('-') => 1,
        0 => return None,
        digits => digits,
    };
    let delimited = body[len..]
        .chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')'));
    delimited.then_some(blanks + len)
}

/// Scan a command line into [`Spanned`] [`Lexeme`]s.
///
/// Quoting: `'single'` is fully literal; `"double"` is literal except that
/// substitution inside it is still refused (because the shell would run it).
/// A backslash escapes the next character into the current word.
///
/// Every emitted lexeme records the byte range of the source that produced it.
/// For a word, `node` is the quote-resolved text and `span` covers the original
/// slice *including* any quotes, so `&input[span]` is the text as written.
pub fn lex(input: &str) -> Result<Vec<Spanned<Lexeme>>, Unanalyzable> {
    lex_spanned(input).map_err(|refused| refused.node)
}

/// **Where the lexer refused `input`**, if it did: the construct it would not
/// read (`$((…))`, a subshell, an unterminated quote to the end). A view marks
/// THAT, rather than painting the whole line as unread — a refusal is about one
/// construct, and the rest of the line is ordinary text (demo 2026-09-25,
/// class F: every refused line was one yellow block).
pub fn refusal_span(input: &str) -> Option<Range<usize>> {
    lex_spanned(input).err().map(|refused| refused.span)
}

/// [`lex`], with the span of what it refused.
fn lex_spanned(input: &str) -> Result<Vec<Spanned<Lexeme>>, Spanned<Unanalyzable>> {
    let mut out: Vec<Spanned<Lexeme>> = Vec::new();
    let mut cur: Option<String> = None;
    // Source bounds of the in-progress word. `word_start` is set when the word
    // begins (cur None → Some); `word_end` advances to `cursor.end()` on every
    // char consumed into the word, so it's the offset past the word's last char
    // whatever token later triggers the flush.
    let mut word_start = 0usize;
    let mut word_end = 0usize;
    let mut cursor = Cursor::new(input);
    // Heredocs on the current line, whose bodies follow its end.
    let mut heredocs: Vec<PendingHereDoc> = Vec::new();

    /// Flush the in-progress word, if any, into the output with its source span.
    macro_rules! flush {
        () => {
            if let Some(w) = cur.take() {
                out.push(Spanned {
                    node: Lexeme::Word(w),
                    span: word_start..word_end,
                });
            }
        };
    }

    /// Begin the word (recording its start `$start`) if it isn't already open,
    /// then push `ch` and advance the word end past it. `$start` is passed in
    /// because it's the current char's start offset, computed inside the loop
    /// (below the macro definition, so out of the macro's hygienic scope) — a
    /// `\`-escape opens the word at the backslash.
    macro_rules! extend_word {
        ($ch:expr, $start:expr) => {{
            if cur.is_none() {
                word_start = $start;
            }
            cur.get_or_insert_with(String::new).push($ch);
            word_end = cursor.end();
        }};
    }

    /// Refuse the line: `$why`, spanning the construct from `$start` (where
    /// it began) to where the scan stopped. `$start` is passed in for the same
    /// hygiene reason as in `extend_word!`.
    macro_rules! refuse {
        ($why:expr, $start:expr) => {
            return Err(Spanned {
                node: $why,
                span: $start..cursor.end(),
            })
        };
    }

    /// Where a redirection operator at `$at` really starts. **Unquoted digits
    /// right against it are its file descriptor** (`2>`, `10>>`, `0<` — POSIX's
    /// IO_NUMBER), not a word: lexed as one, `rm -f x 2>/dev/null` deleted `x`
    /// AND `2` as far as every check could tell. Judged on the source slice,
    /// so a quoted or escaped digit (`"2">`, `\2>`) stays a word, as in bash.
    /// Anything else in progress is flushed as the word it is.
    macro_rules! redirect_start {
        ($at:expr) => {{
            let written = &input[word_start..word_end];
            // Only digits that fit an fd (an `int` to bash): larger ones are a
            // WORD, and dropping it hid an argument (SECURITY-2026-09-30 R2-9).
            let fd = cur.is_some()
                && word_end == $at
                && !written.is_empty()
                && written.bytes().all(|b| b.is_ascii_digit())
                && written.parse::<i32>().is_ok();
            if fd {
                cur = None;
                word_start
            } else {
                flush!();
                $at
            }
        }};
    }

    /// Having consumed a `>&`/`<&` that began at `$start`: a descriptor
    /// duplicated or closed is a [`Operator::Dup`], consuming its digits;
    /// anything else is `$otherwise`, whose target is the word that follows.
    macro_rules! dup_or {
        ($start:expr, $otherwise:expr) => {{
            let op = match dup_target_len(&input[cursor.end()..]) {
                Some(len) => {
                    let until = cursor.end() + len;
                    while cursor.end() < until {
                        cursor.next();
                    }
                    Operator::Dup
                }
                None => $otherwise,
            };
            out.push(Spanned {
                node: Lexeme::Op(op),
                span: $start..cursor.end(),
            });
        }};
    }

    while let Some(c) = cursor.next() {
        // Byte offset of the char `c` just consumed: `end` is now past it.
        let c_start = cursor.end() - c.len_utf8();
        match c {
            // A newline separates commands; it is not whitespace.
            '\n' | '\r' => {
                flush!();
                out.push(Spanned {
                    node: Lexeme::Op(Operator::Newline),
                    span: c_start..cursor.end(),
                });
                // The line ended: the bodies of its heredocs follow, in order
                // (a `\r\n` reads them at its `\n`).
                if c == '\r' && cursor.peek() == Some('\n') {
                    continue;
                }
                for doc in std::mem::take(&mut heredocs) {
                    let from = cursor.end();
                    let Some((body, end)) = heredoc_body(input, from, &doc) else {
                        refuse!(
                            Unanalyzable::ShellStructure {
                                found: "a heredoc never closed"
                            },
                            doc.at
                        );
                    };
                    // An unquoted delimiter expands the body. With nothing in
                    // it to expand it is data all the same; with a `$` or a
                    // backtick it can run a command we would not have seen.
                    if !doc.quoted && body.contains(['$', '`']) {
                        refuse!(
                            Unanalyzable::ShellStructure {
                                found: "a heredoc that expands `$` or backticks — quote its \
                                        delimiter (<<'EOF') for a literal body",
                            },
                            doc.at
                        );
                    }
                    while cursor.end() < end {
                        cursor.next();
                    }
                    out.push(Spanned {
                        node: Lexeme::HereDoc(body),
                        span: from..end,
                    });
                }
            }
            c if c.is_whitespace() => flush!(),

            '\'' => {
                if cur.is_none() {
                    word_start = c_start;
                }
                let buf = cur.get_or_insert_with(String::new);
                loop {
                    match cursor.next() {
                        Some('\'') => break,
                        Some(ch) => buf.push(ch),
                        None => refuse!(Unanalyzable::UnterminatedQuote, c_start),
                    }
                }
                word_end = cursor.end();
            }
            '"' => {
                if cur.is_none() {
                    word_start = c_start;
                }
                let buf = cur.get_or_insert_with(String::new);
                loop {
                    match cursor.next() {
                        Some('"') => break,
                        Some('`') => refuse!(Unanalyzable::Substitution, c_start),
                        Some('$') if cursor.peek() == Some('(') => {
                            refuse!(Unanalyzable::Substitution, c_start);
                        }
                        // `${ ...; }` runs a command even INSIDE double quotes —
                        // the same construct refused unquoted below. Without this
                        // it was kept as literal `${VAR}` text and the command
                        // classified read-only. (Reviewer A, HIGH.)
                        Some('$') if cursor.peek() == Some('{') => {
                            cursor.next();
                            match cursor.peek() {
                                Some(c) if c.is_whitespace() || c == '|' => {
                                    refuse!(
                                        Unanalyzable::ShellStructure {
                                            found: "${ ...; } (runs a command)",
                                        },
                                        c_start
                                    );
                                }
                                _ => {
                                    buf.push('$');
                                    buf.push('{');
                                }
                            }
                        }
                        // Inside double quotes a backslash escapes ONLY $ ` " \ and
                        // a newline (line continuation); before anything else bash
                        // keeps the backslash literal. Dropping it unconditionally
                        // changed the operand text. (Reviewer A.)
                        Some('\\') => match cursor.next() {
                            Some('\n') => {}
                            Some(c @ ('$' | '`' | '"' | '\\')) => buf.push(c),
                            Some(other) => {
                                buf.push('\\');
                                buf.push(other);
                            }
                            None => refuse!(Unanalyzable::UnterminatedQuote, c_start),
                        },
                        Some(ch) => buf.push(ch),
                        None => refuse!(Unanalyzable::UnterminatedQuote, c_start),
                    }
                }
                word_end = cursor.end();
            }

            // Command substitution — runs the inner command, output becomes
            // text. We can't know the text, but we keep the inner command to
            // analyze on its own (a stage), rather than refusing the whole line.
            // Unbalanced ⇒ refuse (fail closed).
            '`' => {
                flush!();
                match capture_backtick(&mut cursor) {
                    Some(inner) => out.push(Spanned {
                        node: Lexeme::Subst(inner),
                        span: c_start..cursor.end(),
                    }),
                    None => refuse!(Unanalyzable::Substitution, c_start),
                }
            }
            '$' if cursor.peek() == Some('(') => {
                cursor.next(); // consume '('
                // `$((…))` is arithmetic — no inner command, but not inert
                // either: bash evaluates a variable named in it AS an
                // expression, and an array subscript there runs `$(…)`, so
                // `X='a[$(rm -rf ~)]'; echo $((X))` runs the `rm`. Refused, as
                // what it is (it was reported as "command substitution"), with
                // the whole construct as the span the view marks.
                if cursor.peek() == Some('(') {
                    cursor.next();
                    if capture_parens(&mut cursor).is_some() && cursor.peek() == Some(')') {
                        cursor.next();
                    }
                    refuse!(Unanalyzable::Arithmetic, c_start);
                }
                flush!();
                match capture_parens(&mut cursor) {
                    Some(inner) => out.push(Spanned {
                        node: Lexeme::Subst(inner),
                        span: c_start..cursor.end(),
                    }),
                    None => refuse!(Unanalyzable::Substitution, c_start),
                }
            }
            // ANSI-C quoting decodes escapes: $'\\x2f' IS `/`. We do not decode
            // it, and showing the user the literal text while the shell runs
            // something else is the exact text-is-not-behavior failure we exist
            // to prevent — so refuse rather than misrepresent.
            '$' if cursor.peek() == Some('\'') => {
                refuse!(
                    Unanalyzable::ShellStructure {
                        found: "$'...' (ANSI-C quoting decodes escapes)",
                    },
                    c_start
                );
            }
            // Bash 5.3's `${ cmd; }` / `${| cmd; }` RUN A COMMAND (without a
            // subshell). Ordinary `${VAR}` does not — the brace is followed by a
            // name, not by a space or a pipe, which is how we tell them apart.
            '$' if cursor.peek() == Some('{') => {
                cursor.next();
                match cursor.peek() {
                    Some(c) if c.is_whitespace() || c == '|' => {
                        refuse!(
                            Unanalyzable::ShellStructure {
                                found: "${ ...; } (runs a command)",
                            },
                            c_start
                        );
                    }
                    _ => {
                        if cur.is_none() {
                            word_start = c_start;
                        }
                        let buf = cur.get_or_insert_with(String::new);
                        buf.push('$');
                        buf.push('{');
                        word_end = cursor.end();
                    }
                }
            }
            '$' if cursor.peek() == Some('"') => {
                refuse!(
                    Unanalyzable::ShellStructure {
                        found: "$\"...\" (locale translation)",
                    },
                    c_start
                );
            }
            // Subshell / grouping: changes scope in ways we don't model.
            '(' | ')' => {
                refuse!(Unanalyzable::ShellStructure { found: "subshell" }, c_start);
            }

            '&' => {
                flush!();
                if cursor.peek() == Some('&') {
                    cursor.next();
                    out.push(Spanned {
                        node: Lexeme::Op(Operator::And),
                        span: c_start..cursor.end(),
                    });
                } else if cursor.peek() == Some('>') {
                    // `&>file` / `&>>file` (bash, zsh): stdout AND stderr into a
                    // file — a write like `>`, recorded as one.
                    cursor.next();
                    let op = if cursor.peek() == Some('>') {
                        cursor.next();
                        Operator::Append
                    } else {
                        Operator::Write
                    };
                    out.push(Spanned {
                        node: Lexeme::Op(op),
                        span: c_start..cursor.end(),
                    });
                } else {
                    // A lone `&` backgrounds — execution we don't model. Fail
                    // closed.
                    refuse!(Unanalyzable::ShellStructure { found: "&" }, c_start);
                }
            }
            '|' => {
                flush!();
                if cursor.peek() == Some('|') {
                    cursor.next();
                    out.push(Spanned {
                        node: Lexeme::Op(Operator::Or),
                        span: c_start..cursor.end(),
                    });
                } else {
                    out.push(Spanned {
                        node: Lexeme::Op(Operator::Pipe),
                        span: c_start..cursor.end(),
                    });
                }
            }
            ';' => {
                flush!();
                out.push(Spanned {
                    node: Lexeme::Op(Operator::Seq),
                    span: c_start..cursor.end(),
                });
            }
            '>' if cursor.peek() == Some('(') => {
                flush!();
                // `>(cmd)` process substitution — cmd runs and reads what the
                // outer command writes. Capture the inner command.
                cursor.next();
                match capture_parens(&mut cursor) {
                    Some(inner) => out.push(Spanned {
                        node: Lexeme::Subst(inner),
                        span: c_start..cursor.end(),
                    }),
                    None => refuse!(Unanalyzable::Substitution, c_start),
                }
            }
            '>' => {
                let start = redirect_start!(c_start);
                if cursor.peek() == Some('>') {
                    cursor.next();
                    out.push(Spanned {
                        node: Lexeme::Op(Operator::Append),
                        span: start..cursor.end(),
                    });
                } else if cursor.peek() == Some('&') {
                    cursor.next();
                    dup_or!(start, Operator::Write);
                } else {
                    out.push(Spanned {
                        node: Lexeme::Op(Operator::Write),
                        span: start..cursor.end(),
                    });
                }
            }
            '<' if cursor.peek() == Some('&') => {
                let start = redirect_start!(c_start);
                cursor.next();
                dup_or!(start, Operator::Read);
            }
            '<' if cursor.peek() == Some('(') => {
                flush!();
                // `<(cmd)` process substitution — cmd runs and the outer
                // command reads its output through a FIFO. Capture the inner
                // command; the FIFO path itself is an unknowable value.
                cursor.next();
                match capture_parens(&mut cursor) {
                    Some(inner) => out.push(Spanned {
                        node: Lexeme::Subst(inner),
                        span: c_start..cursor.end(),
                    }),
                    None => refuse!(Unanalyzable::Substitution, c_start),
                }
            }
            '<' if cursor.peek() == Some('<') => {
                let start = redirect_start!(c_start);
                cursor.next(); // the second '<'
                // `<<<` here-string: its word is expanded like any word, but
                // what reads it is not modelled yet. Still refused.
                if cursor.peek() == Some('<') {
                    cursor.next();
                    refuse!(
                        Unanalyzable::ShellStructure {
                            found: "here-string (<<<)"
                        },
                        start
                    );
                }
                // `<<DELIM` / `<<-DELIM`: stdin from the lines that follow.
                // Its body is read when this line ends (the newline arm).
                let strip_tabs = cursor.peek() == Some('-');
                if strip_tabs {
                    cursor.next();
                }
                while matches!(cursor.peek(), Some(' ' | '\t')) {
                    cursor.next();
                }
                let Some((delimiter, quoted)) = heredoc_delimiter(&mut cursor) else {
                    refuse!(
                        Unanalyzable::ShellStructure {
                            found: "a heredoc with no delimiter"
                        },
                        start
                    );
                };
                out.push(Spanned {
                    node: Lexeme::Op(Operator::HereDoc),
                    span: start..cursor.end(),
                });
                heredocs.push(PendingHereDoc {
                    delimiter,
                    strip_tabs,
                    quoted,
                    at: start,
                });
            }
            '<' => {
                let start = redirect_start!(c_start);
                out.push(Spanned {
                    node: Lexeme::Op(Operator::Read),
                    span: start..cursor.end(),
                });
            }

            // Outside quotes a backslash escapes the next character into the
            // word — EXCEPT before a newline, where `\<newline>` is a line
            // continuation and both characters vanish. We were embedding the
            // newline into the operand, splitting a path. (Reviewer A.)
            '\\' => match cursor.next() {
                Some('\r') if cursor.peek() == Some('\n') => {
                    cursor.next();
                }
                Some('\r' | '\n') => {}
                Some(ch) => extend_word!(ch, c_start),
                None => {}
            },
            // `#` starts a comment, but only at the start of a word (bash: a
            // mid-word `foo#bar` is literal). Discard to end of line; the newline
            // still separates. Without this, an ordinary `ls # note` failed as an
            // unterminated quote — a spurious refusal. (Reviewer E.)
            '#' if cur.is_none() => {
                while let Some(c) = cursor.peek() {
                    if c == '\n' || c == '\r' {
                        break;
                    }
                    cursor.next();
                }
            }
            ch => extend_word!(ch, c_start),
        }
    }
    flush!();
    // A heredoc whose line never ended has no body at all.
    if let Some(doc) = heredocs.first() {
        let at = doc.at;
        refuse!(
            Unanalyzable::ShellStructure {
                found: "a heredoc never closed"
            },
            at
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lexeme nodes, spans stripped — for tests that assert on token content.
    fn nodes(input: &str) -> Vec<Lexeme> {
        lex(input).unwrap().into_iter().map(|s| s.node).collect()
    }

    fn words(input: &str) -> Vec<String> {
        nodes(input)
            .into_iter()
            .filter_map(|l| match l {
                Lexeme::Word(w) => Some(w),
                Lexeme::Op(_) | Lexeme::Subst(_) | Lexeme::HereDoc(_) => None,
            })
            .collect()
    }

    #[test]
    fn splits_words_and_operators() {
        assert_eq!(
            nodes("cd foo && ls -l | wc"),
            vec![
                Lexeme::Word("cd".into()),
                Lexeme::Word("foo".into()),
                Lexeme::Op(Operator::And),
                Lexeme::Word("ls".into()),
                Lexeme::Word("-l".into()),
                Lexeme::Op(Operator::Pipe),
                Lexeme::Word("wc".into()),
            ]
        );
    }

    #[test]
    fn distinguishes_doubled_operators() {
        assert!(nodes("a && b").contains(&Lexeme::Op(Operator::And)));
        assert!(nodes("a || b").contains(&Lexeme::Op(Operator::Or)));
        assert!(nodes("a > f").contains(&Lexeme::Op(Operator::Write)));
        assert!(nodes("a >> f").contains(&Lexeme::Op(Operator::Append)));
    }

    #[test]
    fn quotes_make_operators_literal() {
        assert_eq!(words("echo 'a && b'"), vec!["echo", "a && b"]);
        assert_eq!(words("echo \"x | y\""), vec!["echo", "x | y"]);
    }

    /// The payoff of spans: `node` is the quote-resolved value, but the span
    /// re-slices the ORIGINAL input to the source text with quotes intact — so a
    /// renderer can show the command as written while the engine matches the
    /// canonical form.
    #[test]
    fn spans_reslice_to_original_text_including_quotes() {
        let input = r#"rm -rf "/etc/x""#;
        let lexed = lex(input).unwrap();
        let pairs: Vec<(&str, &str)> = lexed
            .iter()
            .map(|s| {
                let node = match &s.node {
                    Lexeme::Word(w) => w.as_str(),
                    _ => "?",
                };
                (node, &input[s.span.clone()])
            })
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("rm", "rm"),
                ("-rf", "-rf"),
                // resolved value has no quotes; the span keeps them.
                ("/etc/x", "\"/etc/x\""),
            ]
        );
    }

    /// A substitution's span covers the whole `$(…)` construct, so it can be
    /// highlighted as one unit.
    #[test]
    fn subst_span_covers_the_whole_construct() {
        let input = "echo $(id)";
        let lexed = lex(input).unwrap();
        let subst = lexed
            .iter()
            .find(|s| matches!(s.node, Lexeme::Subst(_)))
            .unwrap();
        assert_eq!(&input[subst.span.clone()], "$(id)");
    }

    /// Operator spans cover exactly the operator text (`&&` two chars, `|` one).
    #[test]
    fn operator_spans_cover_the_operator_text() {
        let input = "a && b | c";
        let lexed = lex(input).unwrap();
        let ops: Vec<&str> = lexed
            .iter()
            .filter(|s| matches!(s.node, Lexeme::Op(_)))
            .map(|s| &input[s.span.clone()])
            .collect();
        assert_eq!(ops, vec!["&&", "|"]);
    }

    /// Byte offsets must land on char boundaries — a multibyte char in a word
    /// (é = 2 bytes) must not shift or panic the slice.
    #[test]
    fn spans_are_char_boundary_safe_for_multibyte() {
        let input = "echo café";
        let lexed = lex(input).unwrap();
        let sliced: Vec<&str> = lexed.iter().map(|s| &input[s.span.clone()]).collect();
        assert_eq!(sliced, vec!["echo", "café"]);
    }

    /// A line continuation (`\<newline>`) opens the word's span at the backslash
    /// and both continuation chars vanish from the joined word.
    #[test]
    fn line_continuation_span_starts_at_the_backslash() {
        // `a\<newline>b` is one word `ab`; its span covers the whole `a\\\nb`.
        let input = "a\\\nb";
        let lexed = lex(input).unwrap();
        assert_eq!(lexed.len(), 1);
        assert_eq!(lexed[0].node, Lexeme::Word("ab".into()));
        assert_eq!(&input[lexed[0].span.clone()], "a\\\nb");
    }

    /// Bash 5.3 added `${ cmd; }` and `${| cmd; }`, which RUN A COMMAND without
    /// a subshell — command substitution that does not look like it. Ordinary
    /// `${VAR}` must keep working, so the space/pipe after the brace is the tell.
    #[test]
    fn bash53_brace_command_substitution_is_refused() {
        assert!(lex("x=${ id; }").is_err());
        assert!(lex("x=${| id; }").is_err());
        // Ordinary parameter expansion is untouched.
        assert_eq!(words("echo ${HOME}"), vec!["echo", "${HOME}"]);
        assert_eq!(words("echo ${x:-d}"), vec!["echo", "${x:-d}"]);
    }

    /// Heredocs and here-strings carry a body we never see, and with an
    /// unquoted delimiter that body is expanded — so it can contain command
    /// substitution. Refused, and named plainly rather than reported as a
    /// malformed redirect.
    #[test]
    fn a_heredoc_without_a_body_is_refused_by_name() {
        for src in [
            "cat <<EOF",
            "cat <<-EOF",
            "cat <<'EOF'",
            "cat <<'EOF'\nno end",
        ] {
            match lex(src) {
                Err(Unanalyzable::ShellStructure { found }) => {
                    assert!(found.contains("heredoc"), "{src} -> {found}");
                }
                other => panic!("{src} should be refused as a heredoc, got {other:?}"),
            }
        }
        assert_eq!(
            lex("cat <<< word"),
            Err(Unanalyzable::ShellStructure {
                found: "here-string (<<<)"
            })
        );
        // A single `<` is still an ordinary redirect.
        assert!(lex("sort < file").is_ok());
    }

    /// The bodies of the heredocs, in order.
    fn bodies(input: &str) -> Vec<String> {
        nodes(input)
            .into_iter()
            .filter_map(|l| match l {
                Lexeme::HereDoc(body) => Some(body),
                _ => None,
            })
            .collect()
    }

    /// **A heredoc's body is data the shell does not expand** — with a quoted
    /// delimiter always, with an unquoted one when there is nothing in it to
    /// expand — and the line goes on after it. A body that WOULD expand (`$`,
    /// a backtick) is refused: it can run a command we would not see (demo
    /// 2026-09-25, class F: both demo heredocs were refused outright).
    #[test]
    fn a_heredoc_body_is_data() {
        let script = "cat > s.sh <<'EOF'\nrm -rf \"$HOME\"\n`id`\nEOF\nchmod +x s.sh";
        assert_eq!(bodies(script), ["rm -rf \"$HOME\"\n`id`\n"]);
        assert_eq!(
            words(script),
            ["cat", "s.sh", "chmod", "+x", "s.sh"],
            "none of the body"
        );

        assert_eq!(bodies("cat <<EOF\nplain text\nEOF"), ["plain text\n"]);
        assert_eq!(
            bodies("cat <<-\"END\"\n\tindented\n\tEND\n"),
            ["indented\n"]
        );
        assert_eq!(
            bodies("paste <<'A' <<'B'\none\nA\ntwo\nB\n"),
            ["one\n", "two\n"],
            "two on a line, in order"
        );
        for expands in ["cat <<EOF\n$HOME\nEOF", "cat <<EOF\n`id`\nEOF"] {
            let Err(Unanalyzable::ShellStructure { found }) = lex(expands) else {
                panic!("{expands:?} was read");
            };
            assert!(found.contains("quote its delimiter"), "{found}");
        }
    }

    /// Bash 5.3 `${ cmd; }` runs a command even INSIDE double quotes, so it must
    /// be refused there too — not only unquoted. (Reviewer A, HIGH.)
    #[test]
    fn dollar_brace_command_substitution_refused_inside_double_quotes() {
        assert!(lex("echo \"${ id; }\"").is_err());
        assert!(lex("echo \"${| id; }\"").is_err());
        assert_eq!(words("echo \"${HOME}\""), vec!["echo", "${HOME}"]);
    }

    /// `\<newline>` is a line continuation: both characters vanish. We had been
    /// embedding the newline into the operand. (Reviewer A.)
    #[test]
    fn backslash_newline_is_a_line_continuation() {
        assert_eq!(words("rm -rf \\\n/etc/x"), vec!["rm", "-rf", "/etc/x"]);
        assert_eq!(words("a\\\nb"), vec!["ab"]);
    }

    /// Inside double quotes bash keeps a backslash literal before ordinary
    /// characters, escaping only $ ` " \ and newline. (Reviewer A.)
    #[test]
    fn double_quote_backslash_is_literal_before_ordinary_chars() {
        assert_eq!(words(r#""a\nb""#), vec![r"a\nb"]);
        assert_eq!(words(r#""a\"b""#), vec![r#"a"b"#]);
        assert_eq!(words(r#""a\$b""#), vec!["a$b"]);
    }

    /// `#` begins a comment only at a word boundary; a spurious refusal on
    /// `ls # note` (unterminated quote) is a usability bug. (Reviewer E.)
    #[test]
    fn hash_starts_a_comment_only_at_a_word_boundary() {
        assert_eq!(words("ls # a note"), vec!["ls"]);
        assert!(lex("ls # it's fine").is_ok());
        assert_eq!(words("echo foo#bar"), vec!["echo", "foo#bar"]);
        assert!(nodes("ls #c\nrm x").contains(&Lexeme::Word("rm".into())));
    }

    #[test]
    fn refuses_what_it_will_not_model() {
        assert_eq!(
            lex("(cd /)"),
            Err(Unanalyzable::ShellStructure { found: "subshell" })
        );
        assert_eq!(
            lex("sleep 1 &"),
            Err(Unanalyzable::ShellStructure { found: "&" })
        );
        assert_eq!(lex("echo 'oops"), Err(Unanalyzable::UnterminatedQuote));
        // Arithmetic runs no command itself, but evaluates variables as code:
        // refused, as what it is.
        assert_eq!(lex("echo $((1+1))"), Err(Unanalyzable::Arithmetic));
        // Unbalanced substitution → refuse (fail closed).
        assert_eq!(lex("echo $(id"), Err(Unanalyzable::Substitution));
    }

    #[test]
    fn substitutions_are_captured_not_refused() {
        // We keep the inner command to analyze rather than refusing the line.
        assert_eq!(
            nodes("echo $(id)"),
            vec![Lexeme::Word("echo".into()), Lexeme::Subst("id".into())]
        );
        assert_eq!(
            nodes("echo `id`"),
            vec![Lexeme::Word("echo".into()), Lexeme::Subst("id".into())]
        );
        // Process substitution as a redirect target: `< <(find .)`.
        assert_eq!(
            nodes("sort < <(find .)"),
            vec![
                Lexeme::Word("sort".into()),
                Lexeme::Op(Operator::Read),
                Lexeme::Subst("find .".into()),
            ]
        );
    }

    /// **A refusal names the construct it refused**, so a view can mark that
    /// and not the whole line: the arithmetic whole, a subshell's paren, an
    /// open quote to the end. A line the lexer reads has none.
    #[test]
    fn a_refusal_spans_what_it_refused() {
        fn at(input: &str) -> Option<&str> {
            refusal_span(input).map(|r| &input[r])
        }
        assert_eq!(at("echo $((1+2)) && ls"), Some("$((1+2))"));
        assert_eq!(at("ls && (cd /)"), Some("("));
        assert_eq!(at("echo 'oops && ls"), Some("'oops && ls"));
        assert_eq!(at("ls -la"), None);
    }

    /// The operator and the source it spans, for each lexeme — a redirect's
    /// descriptor lives in its span.
    fn ops(input: &str) -> Vec<(Operator, &str)> {
        lex(input)
            .unwrap()
            .into_iter()
            .filter_map(|s| match s.node {
                Lexeme::Op(op) => Some((op, &input[s.span])),
                _ => None,
            })
            .collect()
    }

    /// **Digits against a redirection are its descriptor, not a word** —
    /// POSIX's IO_NUMBER. `rm -f x 2>/dev/null` read as deleting `x` and `2`.
    /// Only unquoted digits touching the operator: a quoted digit, a digit in
    /// a longer word, and a digit before a blank stay words, as in bash.
    #[test]
    fn a_descriptor_belongs_to_its_redirection() {
        assert_eq!(words("rm -f x 2>/dev/null"), ["rm", "-f", "x", "/dev/null"]);
        assert_eq!(ops("rm -f x 2>/dev/null"), [(Operator::Write, "2>")]);
        assert_eq!(ops("echo x 10>>log"), [(Operator::Append, "10>>")]);
        assert_eq!(ops("cat 0<in"), [(Operator::Read, "0<")]);
        for (line, word) in [
            ("echo \"2\">x", "2"),
            ("echo a2>x", "a2"),
            ("echo 2 >x", "2"),
        ] {
            assert!(
                words(line).contains(&word.to_string()),
                "{line}: {:?}",
                words(line)
            );
        }
    }

    /// `2>&1`, `>&2`, `<&3`, `>&-`: a descriptor copied or closed names no
    /// file — one lexeme, no target. `>&word` with a word that is not one is
    /// bash writing stdout and stderr to the FILE `word`, so it is a write;
    /// and `&>`/`&>>` are that write spelled the other way.
    #[test]
    fn duplication_names_no_file_and_the_rest_are_writes() {
        for line in [
            "ls 2>&1",
            "ls >&2",
            "ls 2>&-",
            "ls >& 2",
            "cat <&3",
            "ls 2>&1|wc",
        ] {
            assert_eq!(ops(line)[0].0, Operator::Dup, "{line}");
        }
        assert_eq!(ops("ls 2>&1")[0].1, "2>&1", "the whole construct");
        assert_eq!(words("ls 2>&1"), ["ls"]);
        assert_eq!(ops("ls >&out.txt"), [(Operator::Write, ">&")]);
        assert_eq!(words("ls >&out.txt"), ["ls", "out.txt"]);
        assert_eq!(ops("ls >&2x"), [(Operator::Write, ">&")], "`2x` is a file");
        assert_eq!(ops("ls &>/dev/null"), [(Operator::Write, "&>")]);
        assert_eq!(ops("ls &>>log"), [(Operator::Append, "&>>")]);
        assert_eq!(
            lex("sleep 1 &"),
            Err(Unanalyzable::ShellStructure { found: "&" })
        );
    }
}
