//! Replay the diary's recorded gate decisions through the engine.
//!
//! The measurement behind the acts rebuild (design §8.7): the lines of
//! `shannon.log.<date>` that say what the gate did, each decided again by
//! `RuleSet` under the shipped rules, optionally plus rules given on the
//! command line, and the two tabulated. The measure is ALARM PRECISION — of
//! the asks left, how many are decisions — not the count alone.
//!
//! Takes paths, ships no data: the gate lines hold a user's own command
//! history and stay in their scratch directory (`$TMPDIR/probe-p4/`), and
//! where each project started is given, not known: `--start` maps a
//! project's key (the first 8 hex of `<basename>-<hash8>` under
//! `~/.config/shannon/projects/`) to its directory, and a line whose
//! project is not given starts at `--start-default` (else `$HOME`).
//!
//! ```text
//! cargo run -p shannon-core --example replay -- GATE_LINES EPISODE_FILES \
//!     [--start HASH8=DIR]… [--start-default DIR] [--rule 'allow cargo *']… \
//!     [--home DIR] [-v]
//! ```

use std::collections::HashMap;

use shannon_core::permission::{ActKind, ActVerdict, By, Decision, RuleSet, StageVerdict};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Then {
    /// "allowed without prompting; running" — the engine ran it unasked.
    Auto,
    /// "allowed once" — asked; the human said yes.
    Approved,
    /// "denied" / "denied by the user" — asked; the human said no.
    Denied,
    /// "sent to the prompt for editing".
    Edited,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    /// A shell line the agent wrote.
    Shell,
    /// `cat 'path'` — how the Claude Code backend's Read reaches the gate.
    RenderedRead,
    /// `cat > 'path'` — the backend's Write, rendered.
    RenderedWrite,
    /// `Skill {…}`, `WebFetch …`, `WebSearch …`, `ToolSearch {…}`: a provider
    /// tool, not a shell line. Counted, not decided.
    ProviderTool,
}

struct Entry {
    date: String,
    then: Then,
    command: String,
    kind: Kind,
    start: String,
    start_guessed: bool,
}

/// Rust `{:?}` string escaping, undone.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('u') => {
                if it.peek() == Some(&'{') {
                    it.next();
                    let mut hex = String::new();
                    for h in it.by_ref() {
                        if h == '}' {
                            break;
                        }
                        hex.push(h);
                    }
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The `command=` value: a `{:?}`-quoted string, or the raw rest of the line.
fn command_of(line: &str) -> Option<String> {
    let at = line.find("command=")?;
    let rest = &line[at + "command=".len()..];
    let Some(q) = rest.strip_prefix('"') else {
        return Some(rest.trim_end().to_string());
    };
    let mut esc = false;
    for (i, c) in q.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' => esc = true,
            '"' => return Some(unescape(&q[..i])),
            _ => {}
        }
    }
    None
}

fn kind_of(command: &str) -> Kind {
    if ["Skill {", "WebFetch ", "WebSearch ", "ToolSearch {"]
        .iter()
        .any(|t| command.starts_with(t))
    {
        return Kind::ProviderTool;
    }
    let rendered = |prefix: &str| {
        command
            .strip_prefix(prefix)
            .and_then(|r| r.strip_prefix('\''))
            .and_then(|r| r.strip_suffix('\''))
            .is_some_and(|inner| !inner.contains('\''))
    };
    if rendered("cat > ") {
        Kind::RenderedWrite
    } else if rendered("cat ") {
        Kind::RenderedRead
    } else {
        Kind::Shell
    }
}

/// Where Shannon started: the project's key (its name under
/// `~/.config/shannon/projects/`, `<basename>-<hash8>`) looked up by its
/// first 8 characters in what `--start` gave, and `default` for any other.
struct Starts {
    by_key: HashMap<String, String>,
    default: String,
}

impl Starts {
    fn of(&self, key: Option<&String>) -> (String, bool) {
        match key
            .and_then(|k| k.get(..8))
            .and_then(|k| self.by_key.get(k))
        {
            Some(dir) => (dir.clone(), false),
            None => (self.default.clone(), true),
        }
    }
}

fn parse(lines: &str, episode_files: &str, starts: &Starts) -> Vec<Entry> {
    let mut key_of: HashMap<String, String> = HashMap::new();
    for f in episode_files.lines() {
        let mut parts = f.split('/');
        let (Some(key), Some("episodes"), Some(name)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if let Some(short) = name.get(..8) {
            key_of.insert(short.to_string(), key.to_string());
        }
    }
    let mut out = Vec::new();
    for raw in lines.lines() {
        let Some((file, line)) = raw.split_once(':') else {
            continue;
        };
        let date = file.rsplit('.').next().unwrap_or("").to_string();
        let Some(at) = line.find("tool call ") else {
            continue;
        };
        let msg = &line[at + "tool call ".len()..];
        let then = if msg.starts_with("allowed once") {
            Then::Approved
        } else if msg.starts_with("allowed without prompting") {
            Then::Auto
        } else if msg.starts_with("denied") {
            Then::Denied
        } else if msg.starts_with("sent to the prompt for editing") {
            Then::Edited
        } else {
            continue;
        };
        let Some(command) = command_of(line) else {
            continue;
        };
        let head = line[..at].trim_end();
        let episode = head
            .rsplit(' ')
            .next()
            .filter(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_hexdigit()));
        let (start, start_guessed) = starts.of(episode.and_then(|e| key_of.get(e)));
        out.push(Entry {
            date,
            then,
            kind: kind_of(&command),
            command,
            start,
            start_guessed,
        });
    }
    out
}

struct Decided<'a> {
    entry: &'a Entry,
    decision: Option<Decision>,
    stages: Vec<StageVerdict>,
}

fn run<'a>(entries: &'a [Entry], user_rules: &[String], home: &str) -> Vec<Decided<'a>> {
    entries
        .iter()
        .map(|e| {
            if e.kind == Kind::ProviderTool {
                return Decided {
                    entry: e,
                    decision: None,
                    stages: Vec::new(),
                };
            }
            let mut rs = RuleSet::defaults().with_start(&e.start);
            if !user_rules.is_empty()
                && let Err(why) = rs.extend_from_lines(&user_rules.join("\n"))
            {
                panic!("user rules: {why}");
            }
            // Shannon renders a Read/Write path inside single quotes with `~`
            // unexpanded; the gate is deciding the backend's path, so it
            // means home.
            let line = e.command.replace("'~/", &format!("'{home}/"));
            let v = rs.decide_line(&line);
            Decided {
                entry: e,
                decision: Some(v.decision),
                stages: v.stages().to_vec(),
            }
        })
        .collect()
}

/// The act that carries the verdict: the first act not allowed, else none.
fn why<'a>(d: &'a Decided<'a>) -> Option<&'a ActVerdict> {
    d.stages
        .iter()
        .flat_map(|s| s.acts.iter())
        .find(|a| a.decision != Decision::Allow)
}

/// The cause with the command's own words stripped, so causes can be
/// counted: the act's kind and the program, and what decided it.
fn cause_key(a: &ActVerdict) -> String {
    let kind = match &a.act {
        ActKind::Program { name, chain, .. } => format!("program {name} {}", chain.join(" ")),
        ActKind::Path { op, .. } => format!("path {}", op.map_or("*".into(), |o| format!("{o:?}"))),
        ActKind::Host { dir, .. } => format!("host {dir:?}"),
        ActKind::Code { program, .. } => format!("code {program}"),
        ActKind::Privilege(p) => format!("privilege {p}"),
        ActKind::Secret { .. } => "secret".into(),
        ActKind::LentVar { .. } => "lent variable".into(),
        // The message with its quoted word taken out, so the class counts.
        ActKind::Unsound(w) => {
            let mut text = String::new();
            let mut quoted = false;
            for c in w.chars() {
                match c {
                    '`' => quoted = !quoted,
                    _ if quoted => {}
                    c => text.push(c),
                }
            }
            format!(
                "unsound: {}",
                text.split(',').next().unwrap_or(&text).trim()
            )
        }
        ActKind::Unplaced(_) => "unplaced".into(),
    };
    let by = match &a.by {
        By::Rules(cited) => format!(
            "by {}",
            cited
                .iter()
                .map(|c| c.source.as_str())
                .collect::<Vec<_>>()
                .join(" + ")
        ),
        By::Default(why) => format!("default {why:?}"),
        By::Hard(h) => format!("hard {h:?}"),
    };
    let uncovered = if a.uncovered.is_empty() {
        String::new()
    } else {
        format!(" uncovered {}", a.uncovered.join(", "))
    };
    // Whether a guarded place lies below it too (`ActVerdict::reaches`) —
    // not how many, so a class still counts as one.
    let held = match a.reaches.is_empty() {
        true => "",
        false => " + a guarded place below",
    };
    format!("{kind}: {by}{held}{uncovered}")
}

fn short(s: &str, n: usize) -> String {
    let one: String = s.chars().map(|c| if c == '\n' { '⏎' } else { c }).collect();
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

fn report(decided: &[Decided], title: &str, verbose: bool) {
    println!("\n== {title} ==");
    let shell: Vec<&Decided> = decided.iter().filter(|d| d.decision.is_some()).collect();
    let tools = decided.len() - shell.len();
    println!(
        "{} tool calls: {} shell lines decided, {tools} provider tools (not decided)",
        decided.len(),
        shell.len()
    );
    let thens = [Then::Auto, Then::Approved, Then::Denied, Then::Edited];
    let verdicts = [Decision::Allow, Decision::Ask, Decision::Deny];
    println!(
        "\n{:<28}{:>8}{:>8}{:>8}{:>8}",
        "today \\ engine", "allow", "ask", "deny", "total"
    );
    for t in thens {
        let row: Vec<&&Decided> = shell.iter().filter(|d| d.entry.then == t).collect();
        if row.is_empty() {
            continue;
        }
        let n = |v: Decision| row.iter().filter(|d| d.decision == Some(v)).count();
        println!(
            "{:<28}{:>8}{:>8}{:>8}{:>8}",
            format!("{t:?}"),
            n(verdicts[0]),
            n(verdicts[1]),
            n(verdicts[2]),
            row.len()
        );
    }
    let n_all = |t: Then, v: Decision| {
        shell
            .iter()
            .filter(|d| d.entry.then == t && d.decision == Some(v))
            .count()
    };
    let prompts_today = shell
        .iter()
        .filter(|d| matches!(d.entry.then, Then::Approved | Then::Denied | Then::Edited))
        .count();
    let prompts_now = shell
        .iter()
        .filter(|d| d.decision == Some(Decision::Ask))
        .count();
    println!(
        "\nprompts: today {prompts_today} of {}; engine {prompts_now} of {} — approved-then→allow {} \
         (alarms removed), auto→ask {} (new prompts), auto→deny {}, denied-then→allow {} (the \
         human said no; the engine would run it)",
        shell.len(),
        shell.len(),
        n_all(Then::Approved, Decision::Allow),
        n_all(Then::Auto, Decision::Ask),
        n_all(Then::Auto, Decision::Deny),
        n_all(Then::Denied, Decision::Allow),
    );
    println!("\nby kind (today→engine):");
    for k in [Kind::Shell, Kind::RenderedRead, Kind::RenderedWrite] {
        let rows: Vec<&&Decided> = shell.iter().filter(|d| d.entry.kind == k).collect();
        if rows.is_empty() {
            continue;
        }
        let mut s = Vec::new();
        for t in thens {
            for v in verdicts {
                let n = rows
                    .iter()
                    .filter(|d| d.entry.then == t && d.decision == Some(v))
                    .count();
                if n > 0 {
                    s.push(format!("{t:?}→{v:?} {n}"));
                }
            }
        }
        println!("  {k:?} ({}): {}", rows.len(), s.join(", "));
    }
    let tally = |name: &str, pick: &dyn Fn(&Decided) -> bool| {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for d in shell.iter().filter(|d| pick(d)) {
            let k = why(d).map(cause_key).unwrap_or_else(|| {
                if d.stages.is_empty() {
                    "unanalyzable: the line could not be read".into()
                } else {
                    "(allowed)".into()
                }
            });
            match counts.iter_mut().find(|(c, _)| *c == k) {
                Some((_, n)) => *n += 1,
                None => counts.push((k, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let total: usize = counts.iter().map(|(_, n)| n).sum();
        println!("\n{name} ({total}), by cause:");
        for (k, n) in counts {
            println!("  {n:>4}  {k}");
        }
    };
    tally("asks", &|d| d.decision == Some(Decision::Ask));
    tally("asks where today ran unprompted", &|d| {
        d.entry.then == Then::Auto && d.decision == Some(Decision::Ask)
    });
    tally("denies", &|d| d.decision == Some(Decision::Deny));
    if !verbose {
        return;
    }
    let section = |name: &str, pick: &dyn Fn(&Decided) -> bool| {
        let rows: Vec<&&Decided> = shell.iter().filter(|d| pick(d)).collect();
        if rows.is_empty() {
            return;
        }
        println!("\n-- {name} ({}) --", rows.len());
        for d in rows {
            let g = if d.entry.start_guessed {
                " [start guessed]"
            } else {
                ""
            };
            let cause = why(d).map(cause_key).unwrap_or_default();
            println!(
                "  {} {:?}{g}  {}\n      ⇒ {}",
                d.entry.date,
                d.entry.kind,
                short(&d.entry.command, 110),
                short(&cause, 160)
            );
        }
    };
    section("auto then, ASKS now", &|d| {
        d.entry.then == Then::Auto && d.decision == Some(Decision::Ask)
    });
    section("auto then, DENIED now", &|d| {
        d.entry.then == Then::Auto && d.decision == Some(Decision::Deny)
    });
    section("denied by the human then, allowed now", &|d| {
        d.entry.then == Then::Denied && d.decision == Some(Decision::Allow)
    });
    section("approved then, denied now", &|d| {
        d.entry.then == Then::Approved && d.decision == Some(Decision::Deny)
    });
    section("still asks (approved or denied then)", &|d| {
        matches!(d.entry.then, Then::Approved | Then::Denied | Then::Edited)
            && d.decision == Some(Decision::Ask)
    });
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut paths: Vec<String> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    let mut home: Option<String> = None;
    let mut by_key = HashMap::new();
    let mut default_start: Option<String> = None;
    let mut verbose = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rule" => rules.push(args.next().expect("--rule takes a rule")),
            "--home" => home = Some(args.next().expect("--home takes a directory")),
            "--start" => {
                let given = args.next().expect("--start takes HASH8=DIR");
                let (key, dir) = given.split_once('=').expect("--start takes HASH8=DIR");
                by_key.insert(key.to_string(), dir.trim_end_matches('/').to_string());
            }
            "--start-default" => {
                default_start = Some(args.next().expect("--start-default takes a directory"));
            }
            "-v" | "--verbose" => verbose = true,
            _ => paths.push(a),
        }
    }
    let [gate_lines, episode_files] = &paths[..] else {
        eprintln!(
            "usage: replay GATE_LINES EPISODE_FILES [--start HASH8=DIR]… \
             [--start-default DIR] [--rule RULE]… [--home DIR] [-v]"
        );
        std::process::exit(2);
    };
    let home = home
        .or_else(|| std::env::var("HOME").ok())
        .expect("a home directory");
    let home = home.trim_end_matches('/').to_string();
    let starts = Starts {
        by_key,
        default: default_start
            .map_or_else(|| home.clone(), |d| d.trim_end_matches('/').to_string()),
    };
    let lines = std::fs::read_to_string(gate_lines).expect("gate lines");
    let files = std::fs::read_to_string(episode_files).expect("episode files");
    let entries = parse(&lines, &files, &starts);
    let title = if rules.is_empty() {
        "shipped rules only".to_string()
    } else {
        format!("shipped rules + {}", rules.join(" | "))
    };
    report(&run(&entries, &rules, &home), &title, verbose);
}
