//! **What an alert sends out of the terminal** (`decision-panes-and-alerts.md`
//! §5.3, B2.3): the bell, and a desktop notification — its words
//! ([`Template`]), the one door they pass through ([`door`]), and the bytes
//! written after a frame is drawn ([`encode`]). Pure, so each is tested
//! without a terminal; the write itself is the event loop's
//! (`App::event_loop`), which no test reaches — the smoke drive does.
//!
//! **A notification leaves the terminal** — for the OS's notification
//! centre, and from there wherever it syncs — so it carries Shannon's own
//! words and the names you gave (`$alert`, `$session`), never a `say`, whose
//! values hold what the model wrote (Steven, 10-23: "For now, we keep it
//! simple"). The door still treats them as untrusted: a session's name is
//! read back from the store (`Label::read_back`), whose files a hand or an
//! agent can edit, and the words themselves from a trusted project's config.
//! (This said a name "can be imported from Claude Code", which no path does
//! — review stop 9, E7, counted: an import writes no label.)

use shannon_core::config::NotifyVia;

/// **A notification's words, read** — text with `$alert` and `$session` in
/// it (`${alert}`, `${session}`), named as the status rows name a value: a
/// `$` and a name. `\$` writes the character and `\\` a backslash; nothing
/// else is escaped, since nothing else means anything here — `{`, `[` and
/// `(` are text, and `\{` is the two characters it reads as (review stop 9,
/// C 7: this said "the status rows' spelling", whose `\{` is `{`). Steven, 10-23: it "may want to accept
/// messages from Shannon itself. Just like `<alert name>` and `<session
/// name>`. We can start there and improve later."
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Template(Vec<Part>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    /// The alert's name.
    Alert,
    /// The session it holds for: `b3` and its name, or nothing for the
    /// app's fact.
    Session,
}

impl Template {
    /// Shannon's own words (`notify = true`): the alert, and the session it
    /// holds for when it is a session's — an app's fact has none to name.
    pub(crate) fn shipped(for_sessions: bool) -> Template {
        let mut parts = vec![Part::Text("Shannon: ".into()), Part::Alert];
        if for_sessions {
            parts.extend([Part::Text(" · ".into()), Part::Session]);
        }
        Template(parts)
    }

    /// **Read `text`**, saying under `key` what in it is no value — kept as
    /// written, so the notification shows it too, as a status row does.
    pub(crate) fn read(key: &str, text: &str, notes: &mut Vec<String>) -> Template {
        let chars: Vec<char> = text.chars().collect();
        let mut parts = Vec::new();
        let mut plain = String::new();
        let mut at = 0;
        while let Some(&c) = chars.get(at) {
            at += 1;
            match c {
                '\\' if matches!(chars.get(at), Some('$' | '\\')) => {
                    plain.push(chars[at]);
                    at += 1;
                }
                '$' => {
                    let dollar = at - 1;
                    let Some(named) = crate::status_line::name_after_dollar(&chars, at, false)
                    else {
                        plain.push('$');
                        continue;
                    };
                    at = named.next;
                    let part = match named.name.as_str() {
                        "alert" => Part::Alert,
                        "session" => Part::Session,
                        _ => {
                            let written: String = chars[dollar..at].iter().collect();
                            notes.push(format!(
                                "{key}: `{written}` is not a notification's value — \
                                 `$alert` or `$session`"
                            ));
                            plain.push_str(&written);
                            continue;
                        }
                    };
                    if !plain.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut plain)));
                    }
                    parts.push(part);
                }
                c => plain.push(c),
            }
        }
        if !plain.is_empty() {
            parts.push(Part::Text(plain));
        }
        Template(parts)
    }

    /// Whether the words name the session (`$session`) — which an alert on
    /// the app's fact has none of, so its reader says so.
    pub(crate) fn names_session(&self) -> bool {
        self.0.contains(&Part::Session)
    }

    /// The words for `alert`, holding for `session` (`None`: the app).
    pub(crate) fn say(&self, alert: &str, session: Option<&str>) -> String {
        (self.0.iter())
            .map(|part| match part {
                Part::Text(text) => text.as_str(),
                Part::Alert => alert,
                Part::Session => session.unwrap_or_default(),
            })
            .collect()
    }
}

/// The most characters a notification carries.
pub(crate) const NOTIFY_CAP: usize = 200;

/// **The one door a notification's words pass** on their way out of the
/// terminal (DR-B #3) — in this order, each for a reason:
/// 1. **filtered**: every control character gone, C1 included (`ESC`, `BEL`
///    and `ST` would end or rewrite the sequence they travel in — an
///    `ESC ] 52` in a name is a clipboard write), and every character that
///    [`misleads`](shannon_core::text::misleads): the OS's notification
///    centre runs the bidi algorithm and breaks lines, so an override
///    reverses the words and a line separator starts a second line there,
///    though a terminal cell shows neither (review stop 9, E2). Every
///    language's letters, marks and joiners pass as written;
/// 2. **scrubbed** (`scrub`, the app's): a held secret's value said as its
///    handle — AFTER the filter, or a secret split by a filtered character
///    (`hun\x07ter2`) slips past the matcher and is joined again; and
///    BEFORE it as well, or a value that itself holds one no longer
///    matches once it is gone;
/// 3. **cut** to [`NOTIFY_CAP`] characters, marked `…` — after the scrub, or
///    a value cut short no longer matches and its prefix leaves.
///
/// Private, and asked only by [`Outbox::send`], the one way words reach an
/// outbox: the door was held by convention while the outbox's list was
/// anyone's to push (review stop 9, D10 and E4).
fn door(words: &str, scrub: impl Fn(&str) -> String) -> String {
    let filtered: String = (scrub(words).chars())
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() || shannon_core::text::misleads(c) => None,
            c => Some(c),
        })
        .collect();
    let words = scrub(&filtered);
    match words.char_indices().nth(NOTIFY_CAP) {
        Some((at, _)) => format!("{}…", &words[..at]),
        None => words,
    }
}

/// **What a frame's alerts send**, gathered between frames and written after
/// the draw ([`encode`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Outbox {
    /// Whether the bell rings.
    bell: bool,
    /// Each notification's words, through the [`door`] already — which
    /// [`send`](Self::send) is the only writer of.
    notify: Vec<String>,
}

impl Outbox {
    /// The bell rings, once however many ask.
    pub(crate) fn ring(&mut self) {
        self.bell = true;
    }

    /// **A notification's `words`, through the [`door`]** (`scrub`, the
    /// app's) — and words that come to nothing (`notify = ""`) send none:
    /// an empty notification tells you nothing.
    pub(crate) fn send(&mut self, words: &str, scrub: impl Fn(&str) -> String) {
        let said = door(words, scrub);
        if !said.trim().is_empty() {
            self.notify.push(said);
        }
    }

    /// What `other` holds, after what this one does.
    pub(crate) fn take_from(&mut self, other: Outbox) {
        self.bell |= other.bell;
        self.notify.extend(other.notify);
    }
}

/// **The bytes for `outbox`** — one bell, however many alerts rang (a burst
/// after `$EDITOR` returns is one event to you), and one notification, the
/// first's words with how many more: a stack of them at once is noise that
/// teaches you to turn them off.
pub(crate) fn encode(outbox: &Outbox, via: NotifyVia) -> Vec<u8> {
    let mut out = Vec::new();
    if outbox.bell {
        out.push(0x07);
    }
    let Some(first) = outbox.notify.first() else {
        return out;
    };
    let words = match outbox.notify.len() {
        1 => first.clone(),
        n => format!("{first} (+{} more)", n - 1),
    };
    out.extend(osc9(&words, via));
    out
}

/// **`OSC 9 ; words BEL`, as `via` says it travels.** `words` are through
/// the [`door`]: no control character is in them.
///
/// An `OSC 9` whose words begin with a number and `;` is a different command
/// in ghostty and Windows Terminal (`9;4;…` is a progress bar, ConEmu's
/// `9;1`…`9;12` more), so words that do not begin with a letter are led with
/// `Shannon: ` — nothing you write is read as one. A letter, not "not a
/// digit": a space, a `+`, a full-width `７` or a zero-width space before
/// `4;1;50` went out unled, and whether a terminal skips them before its
/// number is not ours to know (review stop 9, B F5 and E3).
///
/// For tmux the sequence is wrapped in its passthrough, `ESC P tmux; … ESC
/// \`, every `ESC` inside doubled; tmux passes it on only with `set -g
/// allow-passthrough on`.
fn osc9(words: &str, via: NotifyVia) -> Vec<u8> {
    let lead = match words.starts_with(|c: char| c.is_ascii_alphabetic()) {
        true => "",
        false => "Shannon: ",
    };
    let sequence = format!("\x1b]9;{lead}{words}\x07");
    match via {
        NotifyVia::Osc9 => sequence.into_bytes(),
        NotifyVia::Tmux => {
            let inner = sequence.replace('\x1b', "\x1b\x1b");
            format!("\x1bPtmux;{inner}\x1b\\").into_bytes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> (Template, Vec<String>) {
        let mut notes = Vec::new();
        let template = Template::read("t", text, &mut notes);
        (template, notes)
    }

    /// **A notification says the alert and the session**, in either spelling,
    /// a `\$` the character, a lone `$` itself; the app's fact has no session.
    #[test]
    fn a_template_says_the_alert_and_the_session() {
        let (template, notes) = read("$session: ${alert}! \\$5 or $ alone");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            template.say("hot", Some("b3 work")),
            "b3 work: hot! $5 or $ alone"
        );
        assert_eq!(template.say("hot", None), ": hot! $5 or $ alone");
        assert_eq!(
            Template::shipped(true).say("hot", Some("b3 work")),
            "Shannon: hot · b3 work"
        );
        assert_eq!(Template::shipped(false).say("busy", None), "Shannon: busy");
        // `\\` is a backslash, before a `$` too; `\{` is the two it reads as
        // (review stop 9, C 7: the backslash's escape was unpinned).
        let (template, notes) = read(r"a\\b \\$alert \{x\}");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(template.say("hot", None), r"a\b \hot \{x\}");
    }

    /// **What is no value is said, and kept as written** — never a blank in
    /// the notification.
    #[test]
    fn a_misspelt_value_is_said_and_kept() {
        let (template, notes) = read("$sesion and ${model}");
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("`$sesion`"), "{notes:?}");
        assert!(notes[1].contains("`${model}`"), "{notes:?}");
        assert_eq!(template.say("a", Some("s")), "$sesion and ${model}");
    }

    /// **Reading words never panics**, whatever they are — they are yours,
    /// and a project's config can write them.
    #[test]
    fn any_words_read() {
        let alphabet: Vec<char> = "$\\{}a_ .漢".chars().collect();
        let mut stack = vec![String::new()];
        while let Some(text) = stack.pop() {
            let mut notes = Vec::new();
            let _ = Template::read("t", &text, &mut notes).say("a", Some("s"));
            if text.chars().count() < 5 {
                for c in &alphabet {
                    stack.push(format!("{text}{c}"));
                }
            }
        }
    }

    /// **The door takes out every control character, scrubs, then cuts** —
    /// an `ESC ] 52` clipboard write and a C1 `ST` in a name come out as
    /// text; a secret split by a control character is still scrubbed; and a
    /// cut leaves no prefix of one.
    #[test]
    fn the_door_sanitises_then_scrubs_then_cuts() {
        let scrub = |text: &str| text.replace("hunter2", "{{secret}}");
        let out = door("b3 \x1b]52;c;aGk=\x07 done\u{9c}", scrub);
        assert!(out.chars().all(|c| !c.is_control()), "{out:?}");
        assert_eq!(out, "b3 ]52;c;aGk= done");
        assert_eq!(
            door("pw hun\x07ter2", scrub),
            "pw {{secret}}",
            "split, still found"
        );
        let long = format!("{}hunter2", "x".repeat(NOTIFY_CAP - 3));
        let out = door(&long, scrub);
        assert!(!out.contains("hun"), "no prefix of a secret: {out}");
        assert_eq!(out.chars().count(), NOTIFY_CAP + 1, "cut, and marked");
        assert!(out.ends_with('…'));
        assert_eq!(door("short", scrub), "short", "nothing to cut");
    }

    /// **The door takes out what misleads, and no language** (review stop
    /// 9, E2) — a bidi override reverses a notification's words where the
    /// OS draws them, a line separator starts a second line, a tag carries
    /// text unseen: each is gone, and a secret split by one still scrubbed;
    /// a secret whose own value holds one is scrubbed whole (scrubbed before
    /// the filter too). Korean, Chinese, an accent, Persian's non-joiner and
    /// an emoji sequence leave as written.
    #[test]
    fn the_door_takes_out_what_misleads_and_no_language() {
        let scrub = |text: &str| {
            (text.replace("hunter2", "{{secret}}")).replace("ab\u{2028}cd", "{{other}}")
        };
        assert_eq!(
            door("news in b1 \u{202e}txt.exe\u{2028}run me\u{e0041}", scrub),
            "news in b1 txt.exerun me"
        );
        assert_eq!(door("pw hun\u{2066}ter2", scrub), "pw {{secret}}", "split");
        assert_eq!(door("pw ab\u{2028}cd", scrub), "pw {{other}}", "its own");
        for words in [
            "인증 수정",
            "修复认证",
            "cafe\u{301}",
            "می\u{200C}خواهم",
            "\u{1F468}\u{200D}\u{1F4BB}",
        ] {
            assert_eq!(door(words, scrub), words);
        }
    }

    /// **One bell, one notification**, however many rang, in either
    /// encoding — tmux's with every `ESC` inside doubled.
    #[test]
    fn a_frames_rings_are_one_bell_and_one_notification() {
        let outbox = Outbox {
            bell: true,
            notify: vec!["news in b3".into(), "news in b4".into()],
        };
        assert_eq!(
            encode(&outbox, NotifyVia::Osc9),
            b"\x07\x1b]9;news in b3 (+1 more)\x07".to_vec()
        );
        assert_eq!(
            encode(&outbox, NotifyVia::Tmux),
            b"\x07\x1bPtmux;\x1b\x1b]9;news in b3 (+1 more)\x07\x1b\\".to_vec()
        );
        let quiet = Outbox {
            bell: false,
            notify: vec!["one".into()],
        };
        assert_eq!(encode(&quiet, NotifyVia::Osc9), b"\x1b]9;one\x07".to_vec());
        assert!(encode(&Outbox::default(), NotifyVia::Osc9).is_empty());
    }

    /// **Words not beginning with a letter are no `OSC 9` sub-command** —
    /// `9;4;…` is a progress bar in ghostty and Windows Terminal; a space, a
    /// sign, a full-width digit or a zero-width space before the number are
    /// led too, since whether a terminal skips them is not ours to know
    /// (review stop 9, B F5 and E3). A letter goes out as written.
    #[test]
    fn words_beginning_with_a_number_are_led() {
        let sent = |words: &str| {
            let outbox = Outbox {
                bell: false,
                notify: vec![words.into()],
            };
            String::from_utf8(encode(&outbox, NotifyVia::Osc9)).unwrap()
        };
        for words in [
            "4;1;50",
            " 7;hi",
            "+7;hi",
            "\u{a0}7;hi",
            "７;hi",
            "\u{200b}4;1;50",
        ] {
            assert_eq!(
                sent(words),
                format!("\x1b]9;Shannon: {words}\x07"),
                "{words:?}"
            );
        }
        assert_eq!(sent("b3 news"), "\x1b]9;b3 news\x07");
    }

    /// **The outbox's words are the door's** — `send` passes them through it,
    /// and words that come to nothing send nothing (review stop 9, D10 and
    /// E4: the door was held by convention while the list was public).
    #[test]
    fn an_outbox_sends_through_the_door() {
        let mut outbox = Outbox::default();
        outbox.send("\x1b]52;c;aGk=\x07 hi", |text| text.to_string());
        outbox.send("   ", |text| text.to_string());
        outbox.ring();
        assert_eq!(
            outbox,
            Outbox {
                bell: true,
                notify: vec!["]52;c;aGk= hi".into()],
            }
        );
    }
}
