//! **The status row, as its user writes it** (Steven, 10-14: "We'll also want
//! to make this configurable for people. So there should be macros associated
//! with these things. I think starship has a helpful syntax").
//!
//! A format per kind of pane (`[tui.status]`), read here once per config
//! change and drawn every frame. Starship's shape, with two changes, both
//! because starship reserves exactly the characters a status row is made of —
//! its `[NORMAL]` is a parse error that blanks the whole prompt, and its
//! `(read-only)` a group that is never shown (probed, starship 1.26):
//!
//! - `$name` / `${name}` is a value ([`Var`]); `[text](style)` paints `text`.
//!   A `[` not closed by `](` is text, as in markdown — `[$mode]` is the
//!   mode in brackets.
//! - `{ … }` is a **group**, shown when a value in it is (starship spells it
//!   `( … )`, which made every parenthesis an escape). A group with no value
//!   in it is always shown — vim's `%( %)` rule, not starship's.
//! - **Position is priority.** When the row is too narrow, the rightmost
//!   group goes first, then the next, and what is left is cut with `…`. No
//!   number to rank by: the tools that rank disagree on which way the number
//!   runs (Powerline drops the larger first, feline keeps it), and the order
//!   on screen is already a ranking the user can see. The one exception is
//!   what waits on you — an [`Alert`]: a group showing one is cut with the
//!   rest, never dropped.
//! - `\` before `$ [ ] { } ( ) \` writes the character itself.
//! - `$fill` is the room the rest leaves, as spaces — starship's `$fill`,
//!   vim's `%=`: what follows it sits at the right. Several share it
//!   evenly. Not a value: it is how wide the rest is, so it is grammar.
//! - `$alerts` is every alert that holds (`[tui.alert.<name>]`: a `when` of
//!   [facts](crate::facts), and a `say`), each in its own format, and
//!   `${alert.<name>}` one of them, placed. Grammar too, not values: an
//!   alert's text is drawn by the pieces it is written in, so a style in it
//!   survives. `$alerts` leaves out the alerts the
//!   format places ([`Format::places_alert`]), as a value that joins others
//!   (`$activity`) leaves out what the format writes itself
//!   ([`Format::places`]): placed, a thing is said where it is placed, and
//!   once.
//!
//! **Nothing is dropped in silence**: a value the row does not have is said at
//! load and drawn as written (`$modle`), never as nothing; so is a style word
//! it cannot read, and a `{` never closed.

use ratatui::style::{Color, Modifier, Style};

use crate::facts::{Fact, Trigger};
use crate::style::Ink;

/// Declares [`Var`] once — each value with the name a format writes — so
/// [`Var::ALL`] lists every variant by construction, and the manual's list
/// can be checked against it rather than against itself.
macro_rules! vars {
    ($($(#[$doc:meta])* $variant:ident => $name:literal,)*) => {
        /// **A value the status row can show** — `$name` in a format.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum Var {
            $($(#[$doc])* $variant,)*
        }

        impl Var {
            /// Every value, in declaration order.
            pub(crate) const ALL: &'static [Var] = &[$(Var::$variant,)*];

            /// The name a format writes after `$`.
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Var::$variant => $name,)*
                }
            }
        }
    };
}

vars! {
    /// The focused pane's mode: `NORMAL`, `INSERT`, `VISUAL` …
    Mode => "mode",
    /// The focused pane's name: `prompt`, `output`, `graph`, `rules` …
    Pane => "pane",
    /// `read-only` in a pane typing cannot change: every pane but the prompt
    /// (`PaneKind::is_read_only`).
    Readonly => "readonly",
    /// The manual's section for the focused pane, as `:help` takes it.
    Help => "help",
    /// Where the cursor is, of how many rows: `3/5`.
    Position => "position",
    /// The model answering this session, and the effort it is sent.
    Model => "model",
    /// How full the context window is: `43%` (`*` when the ceiling is
    /// yours). Coloured by `tui.ctx_colors`. Empty before the first answer.
    Ctx => "ctx",
    /// Every token this session has sent, cached or not.
    TokensIn => "tokens_in",
    /// Every token this session has been sent back.
    TokensOut => "tokens_out",
    /// What the prompt cache did: `85% cached`, `6147 stored`, or nothing.
    Cache => "cache",
    /// The terminal's size: `183x59`.
    Size => "size",
    /// How many lines the transcript has.
    Lines => "lines",
    /// `↧` while the transcript follows its tail, `‖` once you scroll away.
    Follow => "follow",
    /// A decision whose pane cannot show its keys: `DECIDE: d deny · a allow`.
    Decide => "decide",
    /// Prompts waiting to be sent: `2 queued`.
    Queued => "queued",
    /// An allowed command, running: `running: cargo test`.
    Running => "running",
    /// `^C:stop` while anything runs that `CTRL-C` would stop.
    Interrupt => "interrupt",
    /// Decisions waiting where no pane shows them — hidden by `:hide`, or
    /// in a session no window shows: `decision hidden · :decide 3`.
    Hidden => "hidden",
    /// Sessions with news you have not seen (`App::unshown`, which says
    /// what that is), and what shows the first: `news in session 3 · :b 3`.
    Unshown => "unshown",
    /// What is running: `$answering`, `$distilling`, `$continuing`,
    /// `$sessions`, `$children` and `$sends`, each that has something to say
    /// and the format does not write itself.
    Activity => "activity",
    /// `answering…` while this session's reply streams.
    Answering => "answering",
    /// `distilling <session>…` while the model drafts this session's summary.
    Distilling => "distilling",
    /// `continuing…` while this session's cut-off reply is continued.
    Continuing => "continuing",
    /// `2 sessions running`: sessions with a turn, a distillation or a
    /// continuation, when one of them is not this one.
    Sessions => "sessions",
    /// Child sessions running or held: `2 children running`.
    Children => "children",
    /// `:Send` sessions answering or held: `1 session answering`.
    Sends => "sends",
}

impl Var {
    /// The value `name` writes, if it is one.
    pub(crate) fn from_name(name: &str) -> Option<Var> {
        Var::ALL.iter().copied().find(|v| v.name() == name)
    }
}

/// **Something waiting on you**, by name: when it holds (`when`), and what
/// the row says of it — a format of its own (`say`), drawn where `$alerts` or
/// `${alert.<name>}` is written (`[tui.alert.<name>]`).
///
/// A group showing one is never dropped for width — the row is cut instead:
/// a row that clips is honest, and one that silently omits a pending
/// decision is not. The one rule that is not position, because it was the
/// row's whole reason to rank at all (`DECIDE:` exists for a terminal too
/// small to draw the decision pane — exactly when the row is too narrow).
/// Asked of the PIECE, not the value: `$hidden` written bare is a value like
/// any other, dropped with its group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Alert {
    /// Shared, not copied: what its edges are kept by holds it too
    /// ([`RuleKey`]), every frame, for each session.
    name: std::rc::Rc<str>,
    when: When,
    say: Format,
    /// The border of each window showing a session it holds for (B2.1) —
    /// only an alert whose `when` names a session's fact has one.
    border: Option<Look>,
    /// Whether `:ls` marks each session it holds for (B2.2) — as `border`,
    /// only for a session's fact.
    mark: bool,
    /// Whether the bell rings when it begins to hold (B2.3) — only with a
    /// `when`.
    bell: bool,
    /// The words of the notification it sends when it begins to hold, if it
    /// sends one (B2.3) — only with a `when`.
    notify: Option<crate::ring::Template>,
}

/// **What an alert's edges are kept by** — its name and its trigger. A
/// rule rewritten in place, or a new one, is a new key, so what it holds is
/// taken without a word; one moved among the alerts, or any other setting
/// of the alerts changed, keeps what it held. Kept by its place, every
/// change to the alerts re-seeded every edge, so `:set tui.alerts.glyph=⚑`
/// swallowed a rise in the same frame (review stop 9, D5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RuleKey {
    name: std::rc::Rc<str>,
    trigger: Trigger,
}

/// **An alert that sends something when it begins to hold** — what its
/// edges are kept by, and what it sends ([`Alerts::ringing`]).
pub(crate) struct Ringing<'a> {
    pub(crate) key: RuleKey,
    pub(crate) bell: bool,
    pub(crate) notify: Option<&'a crate::ring::Template>,
    alert: &'a Alert,
}

impl<'a> Ringing<'a> {
    /// Its name, as a notification says it (`$alert`).
    pub(crate) fn name(&self) -> &'a str {
        &self.alert.name
    }

    /// The fact its trigger asks — a session's, asked of each session, or
    /// the app's, once.
    pub(crate) fn fact(&self) -> Fact {
        self.key.trigger.fact()
    }

    /// **Does it hold for the subject `source` answers for?**
    /// ([`Alert::holding`], the one answer).
    pub(crate) fn holds(&self, source: &mut dyn Source) -> bool {
        self.alert.holding(source) == Holding::Holds
    }
}

/// **Whether an alert holds now** ([`Alert::holding`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holding {
    /// Its trigger holds — or it has none, and holds whenever its `say`
    /// has something to say.
    Holds,
    /// Its trigger does not hold, or it is disabled.
    Not,
    /// Its `when` could not be read: said in its place (`[name: when?]`).
    Broken,
}

impl Alert {
    /// **A disabled alert** (`[tui.alerts] disabled`) — read as any other,
    /// so what it could not read is still said, then given no outcome: it
    /// holds nowhere, says nothing, and marks and rings for no one. Applied
    /// here, once, at read: its four consumers — the border, the mark, the
    /// rings and the row — each asked a `drawn` field, and a fifth would
    /// have had to remember to (review stop 9, D7).
    fn off(name: std::rc::Rc<str>) -> Alert {
        Alert {
            name,
            when: When::Off,
            say: Format::default(),
            border: None,
            mark: false,
            bell: false,
            notify: None,
        }
    }

    /// **Does it hold for the subject `source` answers for?** — the one
    /// answer the row (`eval`'s `say`), a border, a mark and the rings each
    /// ask: three spelled it, one leaning on `ringing` to have left out what
    /// was disabled (review stop 9, C 2). A border and a mark are given only
    /// to a session's trigger, so they ask it of a session.
    fn holding(&self, source: &mut dyn Source) -> Holding {
        match self.when {
            When::On(trigger) if trigger.holds(source.fact(trigger.fact())) => Holding::Holds,
            When::On(_) | When::Off => Holding::Not,
            When::Says => Holding::Holds,
            When::Broken => Holding::Broken,
        }
    }
}

/// **When an alert holds.**
#[derive(Debug, Clone, PartialEq)]
enum When {
    /// No `when`: whenever its `say` has something to say — each shipped
    /// alert, whose `say` is its value.
    Says,
    /// Its trigger, asked of the session the row is about, or of the app.
    On(Trigger),
    /// A `when` that could not be read, said at load: drawn as
    /// `[name: when?]`, never silently dead (§5.2).
    Broken,
    /// In `[tui.alerts] disabled`: holds nowhere, and has no outcome to
    /// give — so no consumer asks whether it is disabled ([`Alert::off`]).
    Off,
}

/// **The alerts the row knows**, in the order `$alerts` says them, and the
/// glyph a `mark` draws.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Alerts {
    list: Vec<Alert>,
    glyph: String,
}

impl Alerts {
    /// **`[tui.alert.*]` and `[tui.alerts]`, read** — every alert, the
    /// shipped ones first in their order (`SHIPPED_ALERTS`), then yours by
    /// name — and what any could not say.
    ///
    /// A `say` is read as no format is: `$alerts` and `${alert.<name>}` in
    /// it are said, and drawn as written — an alert that drew alerts would
    /// draw itself.
    fn read(tui: &shannon_core::config::TuiConfig) -> (Alerts, Vec<String>) {
        let shipped = || (shannon_core::config::SHIPPED_ALERTS.iter()).map(|alert| alert.name);
        let yours = (tui.alert.keys().map(String::as_str)).filter(|n| !shipped().any(|s| s == *n));
        let mut notes = Vec::new();
        let mut alerts = Vec::new();
        for name in shipped().chain(yours) {
            let Some(written) = tui.alert.get(name) else {
                continue;
            };
            let key = format!("tui.alert.{name}");
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                notes.push(format!(
                    "{key}: an alert's name is a-z, 0-9 and _, so `${{alert.<name>}}` can \
                     write it — not drawn"
                ));
                continue;
            }
            let when = match written.when.as_deref().map(Trigger::read) {
                None => When::Says,
                Some(Ok(trigger)) => When::On(trigger),
                Some(Err(why)) => {
                    notes.push(format!("{key}.when: {why}"));
                    When::Broken
                }
            };
            let say = written.say.as_deref().unwrap_or_default();
            let (say, said) = parse(&format!("{key}.say"), say, None);
            notes.extend(said);
            let border = (written.border.as_deref())
                .and_then(|text| read_border(&format!("{key}.border"), text, &when, &mut notes));
            let mark = written.mark == Some(true)
                && for_sessions(
                    &format!("{key}.mark"),
                    "a mark is drawn in `:ls` beside",
                    &when,
                    &mut notes,
                );
            let (bell, notify) = read_rings(name, &key, written, &when, &mut notes);
            let disabled = tui.alerts.disabled.iter().any(|off| off == name);
            // **An alert that does nothing says so** (review stop 9, F6): a
            // `when` alone was read, and nothing said. One whose outcomes were
            // refused above has been told why already.
            let wrote = written.say.as_deref().is_some_and(|say| !say.is_empty())
                || written.border.is_some()
                || written.mark == Some(true)
                || written.bell == Some(true)
                || !matches!(
                    written.notify,
                    None | Some(shannon_core::config::Notify::On(false))
                );
            if !disabled && !wrote {
                notes.push(format!(
                    "{key}: nothing says it — give it a `say`, a `border`, a `mark`, a `bell` \
                     or a `notify`"
                ));
            }
            alerts.push(match disabled {
                true => Alert::off(name.into()),
                false => Alert {
                    name: name.into(),
                    when,
                    say,
                    border,
                    mark,
                    bell,
                    notify,
                },
            });
        }
        for off in &tui.alerts.disabled {
            if !alerts.iter().any(|alert| *alert.name == **off) {
                notes.push(format!("tui.alerts.disabled: `{off}` is not an alert"));
            }
        }
        let glyph = read_glyph(&tui.alerts.glyph, &mut notes);
        (
            Alerts {
                list: alerts,
                glyph,
            },
            notes,
        )
    }

    /// **The border of a window showing the session `source` answers
    /// for** — the first alert holding there that has one, as the theme
    /// says it now; `None` when none does (B2.1).
    pub(crate) fn border(&self, source: &mut dyn Source) -> Option<Style> {
        self.list.iter().find_map(|alert| {
            let look = alert.border?;
            (alert.holding(source) == Holding::Holds).then(|| look.over(Style::default(), source))
        })
    }

    /// **What `:ls` marks the session `source` answers for with** — the
    /// glyph, when an alert with a `mark` holds there (B2.2).
    pub(crate) fn mark(&self, source: &mut dyn Source) -> Option<&str> {
        (self.list.iter())
            .any(|alert| alert.mark && alert.holding(source) == Holding::Holds)
            .then_some(self.glyph.as_str())
    }

    /// **The cells `:ls`' mark column takes** — the glyph's, when an alert
    /// could mark at all, else none: a listing with nothing to mark keeps
    /// the shape it had.
    pub(crate) fn mark_cells(&self) -> usize {
        match self.list.iter().any(|alert| alert.mark) {
            true => crate::wrap::str_cells(&self.glyph),
            false => 0,
        }
    }

    /// **The alerts that send something when they begin to hold** — with a
    /// bell or a notification, and so a trigger (B2.3).
    pub(crate) fn ringing(&self) -> impl Iterator<Item = Ringing<'_>> {
        self.list.iter().filter_map(|alert| {
            let When::On(trigger) = alert.when else {
                return None;
            };
            (alert.bell || alert.notify.is_some()).then(|| Ringing {
                key: RuleKey {
                    name: alert.name.clone(),
                    trigger,
                },
                bell: alert.bell,
                notify: alert.notify.as_ref(),
                alert,
            })
        })
    }

    /// Where the alert `name` is, if the row has one.
    fn index(&self, name: &str) -> Option<usize> {
        self.list.iter().position(|alert| *alert.name == *name)
    }
}

/// **An alert's `bell` and `notify`, read** — each needs a `when`: they go
/// when the alert BEGINS to hold, and one with no `when` holds whenever its
/// value says something, of the row's session alone, with no edge to send on
/// for each session (Steven, 10-23: "Let's go with this but I have a
/// suspicion we will modify this later"). Asked for without one, said, and
/// sent nowhere. `notify = true` is Shannon's own words — a shipped alert's
/// own, when it ships some, else `ring::Template::shipped`'s; a string,
/// yours.
fn read_rings(
    name: &str,
    key: &str,
    written: &shannon_core::config::AlertConfig,
    when: &When,
    notes: &mut Vec<String>,
) -> (bool, Option<crate::ring::Template>) {
    use shannon_core::config::Notify;
    let bell = written.bell == Some(true);
    let notify = !matches!(written.notify, None | Some(Notify::On(false)));
    let trigger = match when {
        _ if !bell && !notify => return (false, None),
        When::On(trigger) => trigger,
        // Said already, as the `when` it could not read; and no `when` a
        // file writes is `Off` (a disabled alert is made after, `Alert::off`).
        When::Broken | When::Off => return (false, None),
        When::Says => {
            notes.push(format!(
                "{key}: a bell or a notification goes when the alert begins to hold, so \
                 it needs a `when` (`decision`, `unshown`) — none sent"
            ));
            return (false, None);
        }
    };
    let for_sessions = matches!(trigger.fact(), Fact::Session(_));
    let words = match &written.notify {
        Some(Notify::Says(text)) => {
            let key = format!("{key}.notify");
            let words = crate::ring::Template::read(&key, text, notes);
            // An outcome its fact's scope cannot carry is said (DR-B #5),
            // words as a border or a mark: `$session` of the app's fact said
            // nothing, and nothing said so (review stop 9, D6).
            if words.names_session() && !for_sessions {
                notes.push(format!(
                    "{key}: `$session` says nothing here — the alert's `when` is the app's \
                     fact, which holds for no one session"
                ));
            }
            Some(words)
        }
        // A shipped alert's words, where it ships some: `notify=false`, then
        // `=true`, sent "Shannon: unshown_session · b1 …" — its name, not
        // its "news in" (review stop 9, F7).
        Some(Notify::On(true)) => Some(match shipped_words(name) {
            Some(words) => crate::ring::Template::read(&format!("{key}.notify"), words, notes),
            None => crate::ring::Template::shipped(for_sessions),
        }),
        Some(Notify::On(false)) | None => None,
    };
    (bell, words)
}

/// The words the shipped alert `name` ships for its notification, if any.
fn shipped_words(name: &str) -> Option<&'static str> {
    (shannon_core::config::SHIPPED_ALERTS.iter())
        .find(|alert| alert.name == name)
        .and_then(|alert| alert.notify)
}

/// **Can an outcome of an alert holding on `when` go to sessions?** — a
/// border, a mark: each goes to every session the alert holds for, so its
/// `when` names a session's fact. One that cannot is said, and not drawn
/// (DR-B #5: an outcome its fact's scope cannot carry is said at load).
fn for_sessions(key: &str, outcome: &str, when: &When, notes: &mut Vec<String>) -> bool {
    let session = match when {
        When::On(trigger) => matches!(trigger.fact(), Fact::Session(_)),
        // Said already, as the `when` it could not read — as `read_rings`
        // leaves it. A second note here told `when = "ctx >= 80"` that its
        // `when` must name a session's fact, which it meant to (review stop
        // 9, A F7 and C 6). `Off` is made after reading (`Alert::off`).
        When::Broken | When::Off => return false,
        When::Says => false,
    };
    if !session {
        notes.push(format!(
            "{key}: {outcome} each session the alert holds for, so its `when` must \
             name a session's fact (`ctx_percent >= 80`, `decision`) — not drawn"
        ));
    }
    session
}

/// **`[tui.alerts] glyph`, read** — drawn into `:ls`' cells, from any
/// config file a project's included, so it is text a cell can hold: no
/// control character (an escape would reach the terminal), one to four
/// cells. Anything else is said, shown escaped, and the shipped glyph drawn.
fn read_glyph(written: &str, notes: &mut Vec<String>) -> String {
    // What misleads too, not only the controls: an override in a glyph
    // reverses the rest of `:ls`'s row where a terminal runs the bidi
    // algorithm (review stop 9, B F6 and E2).
    let misleading = |c: char| c.is_control() || shannon_core::text::misleads(c);
    let cells = crate::wrap::str_cells(written);
    if !written.chars().any(misleading) && (1..=4).contains(&cells) {
        return written.to_string();
    }
    let shipped = shannon_core::config::DEFAULT_ALERT_GLYPH;
    // Quoted as written, escaped once with every note (`legible`): Rust's
    // `Debug` escaped a combining accent and Devanagari's virama too
    // (review stop 9's verification, G 8).
    notes.push(format!(
        "tui.alerts.glyph: `{written}` is not one to four cells of text — `{shipped}` is drawn"
    ));
    shipped.to_string()
}

/// **An alert's `border`, read** — style words, as a format's `(style)`
/// writes them — or `None`, said, for an alert that holds for no session
/// ([`for_sessions`]).
fn read_border(key: &str, text: &str, when: &When, notes: &mut Vec<String>) -> Option<Look> {
    let outcome = "a border is drawn on the windows of";
    if !for_sessions(key, outcome, when, notes) {
        return None;
    }
    // Drawn only where its fact is 0: a border on news is never seen.
    if let When::On(trigger) = when
        && let Fact::Session(fact) = trigger.fact()
        && fact.zero_where_drawn()
        && !trigger.holds(0)
    {
        notes.push(format!(
            "{key}: a border is drawn on a window showing the session, and `{}` never holds \
             while one does — `mark` says it beside the session in `:ls`",
            fact.name()
        ));
        return None;
    }
    Some(read_look(key, text, notes))
}

/// **A name read after a `$`** ([`name_after_dollar`]).
pub(crate) struct Named {
    pub(crate) name: String,
    /// Written `${…}`.
    pub(crate) braced: bool,
    /// Where reading goes on: past the name, and its `}`.
    pub(crate) next: usize,
}

/// **The name after a `$`** at `at` in `chars` — `$name` or `${name}`, of
/// ASCII letters, digits and `_`, and between braces a `.` too where `dots`
/// allows (`${alert.<name>}`) — or `None` when no name follows, and the `$`
/// is text. The one spelling of a `$value` for the status rows
/// (`Parser::variable`) and a notification's words (`ring::Template::read`),
/// which each lexed it (review stop 9, C 3).
pub(crate) fn name_after_dollar(chars: &[char], at: usize, dots: bool) -> Option<Named> {
    let braced = chars.get(at) == Some(&'{');
    let start = at + usize::from(braced);
    let named = |c: &char| c.is_ascii_alphanumeric() || *c == '_' || (dots && braced && *c == '.');
    let end = start
        + chars[start.min(chars.len())..]
            .iter()
            .take_while(|c| named(c))
            .count();
    if end == start || (braced && chars.get(end) != Some(&'}')) {
        return None;
    }
    Some(Named {
        name: chars[start..end].iter().collect(),
        braced,
        next: end + usize::from(braced),
    })
}

/// **Style words, read** — a format's `(…)`, an alert's `border` — each
/// word one [`read_style_word`] takes, and one it cannot said under `key`.
/// Both wrote this loop and its note (review stop 9, C 3).
fn read_look(key: &str, words: &str, notes: &mut Vec<String>) -> Look {
    let mut look = Look::default();
    for word in words.split_whitespace() {
        if let Err(why) = read_style_word(word, &mut look) {
            notes.push(format!("{key}: `{word}` — {why}"));
        }
    }
    look
}

/// **What a value is right now**: its text, and the style it brings — `ctx`'s
/// gauge colour — which an enclosing `[…](style)` overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Value {
    pub(crate) text: String,
    pub(crate) style: Option<Style>,
    /// **Where a row of keys starts in `text`**, running to its end:
    /// `$decide`'s `d deny · a allow`, drawn as every row of keys is
    /// (`paint::hint`). Yields, as `style` does, to a `[…](style)`.
    pub(crate) keys_from: Option<usize>,
}

impl From<String> for Value {
    fn from(text: String) -> Value {
        Value {
            text,
            ..Value::default()
        }
    }
}

/// **What a row is drawn from**: each value's text, each fact an alert's
/// `when` asks — a session's of the session the row is about — and what
/// each theme role looks like now.
pub(crate) trait Source {
    /// What `$var` says now.
    fn value(&mut self, var: Var) -> Value;
    /// What `fact` is now.
    fn fact(&mut self, fact: Fact) -> u64;
    /// What the theme makes of `ink` now — asked at draw, so a theme
    /// changed shows on the next frame, with no format read again.
    fn ink(&mut self, ink: Ink) -> Style;
}

/// A test's row: its values from a closure, every fact 0, and no theme.
#[cfg(test)]
impl<F: FnMut(Var) -> Value> Source for F {
    fn value(&mut self, var: Var) -> Value {
        self(var)
    }

    fn fact(&mut self, _: Fact) -> u64 {
        0
    }

    fn ink(&mut self, _: Ink) -> Style {
        Style::default()
    }
}

/// **A value holding a row of keys, drawn** — what precedes `at` in its own
/// look, the row after it as every row of keys is (`paint::hint`): each key
/// in the manual's `ui.help.code`, over that look.
fn keys_drawn(text: &str, at: usize, own: Style, source: &mut dyn Source) -> Vec<Run> {
    use crate::paint::Part;
    let key = own.patch(source.ink(Ink::HelpCode));
    let (lead, keys) = text.split_at(at);
    let mut runs = vec![Run::Text(lead.to_string(), own)];
    runs.extend(
        (crate::paint::key_row(keys).into_iter()).map(|(part, text)| {
            let look = match part {
                Part::Key => key,
                Part::Words => own,
            };
            Run::Text(text, look)
        }),
    );
    runs
}

/// A colour as a style writes one: the colour itself, or a theme role's
/// (`fg:ui.caution`), kept as the role until the row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hue {
    Colour(Color),
    Role(Ink),
}

impl Hue {
    /// The colour now: a role's is the theme's foreground for it — none
    /// where the theme gives none, which leaves the text its own (a colour
    /// chosen here would be one no theme reaches).
    fn now(self, source: &mut dyn Source) -> Option<Color> {
        match self {
            Hue::Colour(colour) => Some(colour),
            Hue::Role(ink) => source.ink(ink).fg,
        }
    }
}

/// A style as a format writes one (`bold fg:#ff8800 bg:blue`, `ui.caution`),
/// laid over the text it encloses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Look {
    /// A role written alone: all of its style, under what else is written.
    role: Option<Ink>,
    fg: Option<Hue>,
    bg: Option<Hue>,
    add: Modifier,
}

impl Look {
    /// `style` with this laid over it, each role as the theme says it now.
    fn over(self, style: Style, source: &mut dyn Source) -> Style {
        let mut style = match self.role {
            Some(ink) => style.patch(source.ink(ink)),
            None => style,
        };
        style = style.add_modifier(self.add);
        if let Some(fg) = self.fg.and_then(|fg| fg.now(source)) {
            style = style.fg(fg);
        }
        if let Some(bg) = self.bg.and_then(|bg| bg.now(source)) {
            style = style.bg(bg);
        }
        style
    }
}

/// One piece of a parsed format.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Text(String),
    Var(Var),
    /// `$alerts`: every alert with something to say that the format does
    /// not place, joined with ` · `.
    Alerts,
    /// `${alert.<name>}`: the alert at this index of the row's [`Alerts`].
    Alert(usize),
    /// `[…](style)`.
    Styled(Vec<Piece>, Look),
    /// `{…}`: shown when a value in it is, and dropped first for width.
    Group(Vec<Piece>),
    /// `$fill`: the room the rest of the row leaves.
    Fill,
}

/// **A status format, read** — what `[tui.status]` says for one kind of pane.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Format {
    pieces: Vec<Piece>,
    /// The alerts it places itself (`${alert.<name>}`), found once, as it
    /// is read: `$alerts` asks of each alert on every frame, and a walk of
    /// the whole format for each made a long one quadratic (review stop 9,
    /// B F2: 2.1 s a frame for 8k `$alerts`).
    placed: Vec<usize>,
}

impl Format {
    /// **Does the format write `var` itself** — anywhere, in a group or a
    /// style? Then a value that joins it (`$activity`) leaves it out, rather
    /// than say it a second time.
    pub(crate) fn places(&self, var: Var) -> bool {
        self.holds(&|piece| *piece == Piece::Var(var))
    }

    /// **Does the format place the alert at `index` itself**, with
    /// `${alert.<name>}`? Then `$alerts` leaves it out: the shipped rows put
    /// `${alert.hidden}` at the right, and `$alerts` beside the pane's name
    /// said it a second time.
    fn places_alert(&self, index: usize) -> bool {
        self.placed.contains(&index)
    }

    /// Is a piece `is` anywhere in the format, in a group or a style?
    fn holds(&self, is: &dyn Fn(&Piece) -> bool) -> bool {
        fn walk(pieces: &[Piece], is: &dyn Fn(&Piece) -> bool) -> bool {
            pieces.iter().any(|piece| match piece {
                Piece::Styled(inner, _) | Piece::Group(inner) => walk(inner, is),
                piece => is(piece),
            })
        }
        walk(&self.pieces, is)
    }
}

/// **`[tui.status]`, read** — a format per kind of pane, and the alerts they
/// draw.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Formats {
    prompt: Format,
    output: Format,
    decision: Format,
    graph: Format,
    page: Format,
    alerts: Alerts,
}

impl Formats {
    /// Each format `[tui.status]` holds, the alerts they draw
    /// (`[tui.alert.*]`, `[tui.alerts]`), and what any of them could not say.
    pub(crate) fn read(tui: &shannon_core::config::TuiConfig) -> (Formats, Vec<String>) {
        let (alerts, mut notes) = Alerts::read(tui);
        let config = &tui.status;
        let mut one = |key: &str, text: &str| {
            let (format, said) = parse(&format!("tui.status.{key}"), text, Some(&alerts));
            notes.extend(said);
            format
        };
        let (prompt, output, decision, graph, page) = (
            one("prompt", &config.prompt),
            one("output", &config.output),
            one("decision", &config.decision),
            one("graph", &config.graph),
            one("page", &config.page),
        );
        let formats = Formats {
            prompt,
            output,
            decision,
            graph,
            page,
            alerts,
        };
        // **A `say` no row writes is never said** (review stop 9, F6): rows
        // of yours without `$alerts`, and without the alert by name, drew
        // nothing of it — a decision waiting among them — and said nothing.
        let rows = [
            &formats.prompt,
            &formats.output,
            &formats.decision,
            &formats.graph,
            &formats.page,
        ];
        if !rows
            .iter()
            .any(|row| row.holds(&|piece| *piece == Piece::Alerts))
        {
            let unsaid: Vec<&str> = (formats.alerts.list.iter().enumerate())
                .filter(|(_, alert)| !alert.say.pieces.is_empty())
                .filter(|&(index, _)| !rows.iter().any(|row| row.places_alert(index)))
                .map(|(_, alert)| &*alert.name)
                .collect();
            if !unsaid.is_empty() {
                notes.push(format!(
                    "tui.status: no row writes `$alerts`, nor `${{alert.<name>}}` for {} — \
                     not said",
                    unsaid.join(", ")
                ));
            }
        }
        (formats, notes.iter().map(|note| legible(note)).collect())
    }

    /// The alerts every format draws (`$alerts`, `${alert.<name>}`).
    pub(crate) fn alerts(&self) -> &Alerts {
        &self.alerts
    }

    /// The format for a pane of `kind`. Exhaustive, so a new kind says
    /// which row it has.
    pub(crate) fn of(&self, kind: crate::workspace::PaneKind) -> &Format {
        use crate::workspace::PaneKind;
        match kind {
            PaneKind::Prompt => &self.prompt,
            PaneKind::Output => &self.output,
            PaneKind::Tool => &self.decision,
            PaneKind::Graph => &self.graph,
            PaneKind::Inspector | PaneKind::Page(_) | PaneKind::ThemePreview => &self.page,
        }
    }
}

/// The characters `\` writes as themselves.
const ESCAPED: &[char] = &['$', '[', ']', '{', '}', '(', ')', '\\'];

/// **A note as it can be read** — every control (`\t`, `\u{1b}`), every
/// character that [`misleads`](shannon_core::text::misleads) (`\u{202e}`)
/// and every one that [draws as
/// nothing](shannon_core::text::draws_as_nothing) (`\u{200b}`) written as
/// its escape, and the rest as written. So the key a note names is the key
/// the file holds: an alert named `ab` and U+202E was refused with a note
/// that read `tui.alert.ab` (review stop 9, E6). Not Rust's `escape_debug`
/// for every character, as first written: that escapes every combining
/// mark and variation selector too, so `cafe\u{301}` and a Devanagari word
/// came out as escapes (probed; E2). Asked once, of every note
/// `Formats::read` returns — what the formats, the alerts and their `when`s
/// could not read — rather than at each of the places one quotes what a
/// config wrote.
fn legible(note: &str) -> String {
    use shannon_core::text::{draws_as_nothing, misleads};
    let mut out = String::with_capacity(note.len());
    for c in note.chars() {
        if c.is_control() {
            out.extend(c.escape_debug());
        } else if misleads(c) || draws_as_nothing(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

/// **How deep `{…}` and `[…](…)` nest** before a `{` or `[` is text, said.
/// Reading a format, drawing it and dropping it each walk its nesting by
/// recursion, and some 20 000 levels — 20 KB of a config file — overflowed
/// the stack: an abort, which runs no `Drop`, so the terminal was left raw,
/// on every launch until the file changed (review stop 9, B F1). No row
/// needs a tenth of this.
const DEEPEST: usize = 32;

/// Read a format, and what in it could not be read — `key` names it in what
/// is said (`tui.status.prompt`). Never fails: what it cannot read is drawn
/// as written, and said. `alerts` is what `${alert.<name>}` can name, and
/// `None` for an alert's own `say`, which cannot draw one.
fn parse(key: &str, text: &str, alerts: Option<&Alerts>) -> (Format, Vec<String>) {
    let mut parser = Parser {
        key,
        alerts,
        chars: text.chars().collect(),
        at: 0,
        depth: 0,
        too_deep: false,
        notes: Vec::new(),
    };
    let pieces = parser.until(None, &[]).0;
    fn placed(pieces: &[Piece], out: &mut Vec<usize>) {
        for piece in pieces {
            match piece {
                Piece::Alert(index) => out.push(*index),
                Piece::Styled(inner, _) | Piece::Group(inner) => placed(inner, out),
                _ => {}
            }
        }
    }
    let mut placed_alerts = Vec::new();
    placed(&pieces, &mut placed_alerts);
    let format = Format {
        pieces,
        placed: placed_alerts,
    };
    (format, parser.notes)
}

struct Parser<'a> {
    key: &'a str,
    alerts: Option<&'a Alerts>,
    chars: Vec<char>,
    at: usize,
    /// The pieces open around `at`, at most [`DEEPEST`].
    depth: usize,
    /// Whether the format has said it nests past [`DEEPEST`]: once, not
    /// for every `{` past it.
    too_deep: bool,
    notes: Vec<String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    /// Pieces up to `close` (consumed), or up to a closer of an enclosing
    /// piece in `outer` (left for it) or the end. `true` when `close` was
    /// found — an unclosed `[` is text, an unclosed `{` a note.
    fn until(&mut self, close: Option<char>, outer: &[char]) -> (Vec<Piece>, bool) {
        let mut out = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, out: &mut Vec<Piece>| {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(text)));
            }
        };
        while let Some(c) = self.peek() {
            if Some(c) == close {
                self.at += 1;
                flush(&mut text, &mut out);
                return (out, true);
            }
            if outer.contains(&c) {
                break;
            }
            self.at += 1;
            match c {
                '\\' => match self.peek() {
                    Some(next) if ESCAPED.contains(&next) => {
                        text.push(next);
                        self.at += 1;
                    }
                    _ => text.push('\\'),
                },
                '$' => match self.variable() {
                    Some(piece) => {
                        flush(&mut text, &mut out);
                        out.push(piece);
                    }
                    None => text.push('$'),
                },
                '{' | '[' if self.depth == DEEPEST => {
                    if !self.too_deep {
                        self.too_deep = true;
                        self.notes.push(format!(
                            "{}: groups and styles nest at most {DEEPEST} deep — a `{c}` \
                             past that is drawn as written",
                            self.key
                        ));
                    }
                    text.push(c);
                }
                '{' => {
                    flush(&mut text, &mut out);
                    let (inner, closed) = self.nested('}', outer, close);
                    if closed {
                        out.push(Piece::Group(inner));
                    } else {
                        self.notes.push(format!(
                            "{}: a `{{` with no `}}` — `\\{{` writes the character",
                            self.key
                        ));
                        out.push(Piece::Text("{".into()));
                        out.extend(inner);
                    }
                }
                '[' => {
                    flush(&mut text, &mut out);
                    let (inner, closed) = self.nested(']', outer, close);
                    match (closed, self.style()) {
                        (true, Some(look)) => out.push(Piece::Styled(inner, look)),
                        _ => {
                            out.push(Piece::Text("[".into()));
                            out.extend(inner);
                            if closed {
                                out.push(Piece::Text("]".into()));
                            }
                        }
                    }
                }
                '}' => {
                    self.notes.push(format!(
                        "{}: a `}}` with no `{{` — `\\}}` writes the character",
                        self.key
                    ));
                    text.push('}');
                }
                _ => text.push(c),
            }
        }
        flush(&mut text, &mut out);
        (out, false)
    }

    /// **The pieces inside a `{` or `[` just read**, up to its `close` — one
    /// level deeper ([`DEEPEST`]), and ended early by whatever ends a piece
    /// around it: the closers it was given (`outer`) and the close of the
    /// piece it is in (`around`). Both openers ask here, so they cannot
    /// disagree on that list — which they did: the `{` arm's copy dropped
    /// `outer`'s first closer instead of its own `}`, and `[{{a](bold)` lost
    /// its style (review stop 9, B for A).
    fn nested(&mut self, close: char, outer: &[char], around: Option<char>) -> (Vec<Piece>, bool) {
        let enclosing = [outer, around.as_slice()].concat();
        self.depth += 1;
        let inside = self.until(Some(close), &enclosing);
        self.depth -= 1;
        inside
    }

    /// After a `$`: a value by name, `$fill`, `$alerts` or `${alert.<name>}`
    /// — a `.` only between braces, so `$model.` ends a sentence — or `None`
    /// when no name follows (the `$` is then text). A name the row does not
    /// have is said, and kept as written so the screen shows it too.
    fn variable(&mut self) -> Option<Piece> {
        let Named { name, braced, next } = name_after_dollar(&self.chars, self.at, true)?;
        self.at = next;
        let written = match braced {
            true => format!("${{{name}}}"),
            false => format!("${name}"),
        };
        let why = match (name.as_str(), name.strip_prefix("alert."), self.alerts) {
            ("fill", ..) => return Some(Piece::Fill),
            ("alerts", _, Some(_)) => return Some(Piece::Alerts),
            (_, Some(alert), Some(alerts)) => match alerts.index(alert) {
                Some(index) => return Some(Piece::Alert(index)),
                None => "is not an alert the row has (`:help statusline` lists them)",
            },
            ("alerts", ..) | (_, Some(_), None) => {
                "is an alert, and an alert's `say` cannot draw one: it would draw itself"
            }
            ("alert", ..) if self.peek() == Some('.') => {
                "is not a value the row has — one alert is written `${alert.<name>}`"
            }
            _ => match Var::from_name(&name) {
                Some(var) => return Some(Piece::Var(var)),
                None => "is not a value the row has (`:help statusline` lists them)",
            },
        };
        self.notes.push(format!("{}: `{written}` {why}", self.key));
        Some(Piece::Text(written))
    }

    /// After a `]`: `(style)` read, or `None` — no `(` follows, or no `)`
    /// closes it — and nothing consumed, so the bracket is text.
    fn style(&mut self) -> Option<Look> {
        if self.peek() != Some('(') {
            return None;
        }
        let close = self.chars[self.at..].iter().position(|c| *c == ')')?;
        let words: String = self.chars[self.at + 1..self.at + close].iter().collect();
        self.at += close + 1;
        Some(read_look(self.key, &words, &mut self.notes))
    }
}

/// One word of a style: an emphasis, `fg:<colour>`, `bg:<colour>`, a colour
/// alone (the text's), or a theme role (`ui.caution`) — alone, all of its
/// style; after `fg:` or `bg:`, its colour. A colour as a theme writes one,
/// a role as a theme names it, kept as the role until the row is drawn.
fn read_style_word(word: &str, look: &mut Look) -> Result<(), String> {
    let role = |name: &str| Ink::ALL.iter().copied().find(|ink| ink.name() == name);
    let emphasis = match word {
        "bold" => Some(Modifier::BOLD),
        "italic" => Some(Modifier::ITALIC),
        "dim" | "dimmed" => Some(Modifier::DIM),
        "underline" | "underlined" => Some(Modifier::UNDERLINED),
        "reverse" | "reversed" | "inverted" => Some(Modifier::REVERSED),
        "strikethrough" => Some(Modifier::CROSSED_OUT),
        _ => None,
    };
    if let Some(modifier) = emphasis {
        look.add |= modifier;
        return Ok(());
    }
    let (slot, colour) = match word.split_once(':') {
        Some(("fg", colour)) => (&mut look.fg, colour),
        Some(("bg", colour)) => (&mut look.bg, colour),
        Some(_) => {
            return Err(
                "a style is bold, italic, dim, underline, reverse, strikethrough, \
                 fg:<colour>, bg:<colour>, a colour or a theme role (`ui.caution`)"
                    .into(),
            );
        }
        None => match role(word) {
            Some(ink) => {
                look.role = Some(ink);
                return Ok(());
            }
            None => (&mut look.fg, word),
        },
    };
    *slot = match role(colour) {
        Some(ink) => Some(Hue::Role(ink)),
        None if colour.contains('.') => {
            return Err(format!(
                "`{colour}` is not a theme role (`:help themes` lists them)"
            ));
        }
        None => crate::theme::colour_word(colour)?.map(Hue::Colour),
    };
    Ok(())
}

/// The row for `format` in `width` cells: each value and fact asked of
/// `source`, each alert of `alerts`, the text in `base`, groups whose values
/// are all empty left out, and — when it is still too wide — the rightmost
/// group dropped, then the next (never one showing an alert), then the rest
/// cut with `…`. A `$fill` takes what room is left, and none when there is
/// none.
pub(crate) fn render(
    format: &Format,
    alerts: &Alerts,
    source: &mut dyn Source,
    base: Style,
    width: u16,
) -> Vec<(String, Style)> {
    let alerting = Some(Alerting { alerts, format });
    let mut items: Vec<(Vec<Run>, bool)> = Vec::new();
    for piece in &format.pieces {
        let shown = eval(std::slice::from_ref(piece), base, false, alerting, source);
        let droppable = matches!(piece, Piece::Group(_)) && !shown.alert;
        if !shown.runs.is_empty() {
            items.push((shown.runs, droppable));
        }
    }
    let cells = |items: &[(Vec<Run>, bool)]| -> usize {
        items
            .iter()
            .flat_map(|(runs, _)| runs)
            .map(Run::cells)
            .sum()
    };
    // The rightmost droppable group first, then the next, until it fits —
    // in one pass from the right, the width kept as it goes: re-summing the
    // row and removing from the vector at every drop made a long format
    // quadratic, every frame (review stop 9, B F2: 222 ms a frame for 16k
    // groups).
    let width = usize::from(width);
    let mut total = cells(&items);
    let mut kept = vec![true; items.len()];
    for (at, (runs, droppable)) in items.iter().enumerate().rev() {
        if total <= width {
            break;
        }
        if *droppable {
            kept[at] = false;
            total -= runs.iter().map(Run::cells).sum::<usize>();
        }
    }
    let runs: Vec<Run> = (items.into_iter().zip(kept))
        .filter(|(_, kept)| *kept)
        .flat_map(|((runs, _), _)| runs)
        .collect();
    clip(filled(runs, width), width)
}

/// **One run of a row**: text in a style, or a `$fill` in the style around
/// it, its width not known until the rest of the row is.
#[derive(Debug, Clone)]
enum Run {
    Text(String, Style),
    Fill(Style),
}

impl Run {
    /// The cells it takes before any room is shared out: a fill takes none.
    fn cells(&self) -> usize {
        match self {
            Run::Text(text, _) => crate::wrap::str_cells(text),
            Run::Fill(_) => 0,
        }
    }
}

/// `runs` with each `$fill` given its share of the room the text leaves in
/// `width` — evenly, the first ones a cell more where it does not divide —
/// as spaces in its style.
fn filled(runs: Vec<Run>, width: usize) -> Vec<(String, Style)> {
    let room = width.saturating_sub(runs.iter().map(Run::cells).sum());
    let fills = runs.iter().filter(|r| matches!(r, Run::Fill(_))).count();
    let mut nth = 0;
    (runs.into_iter())
        .filter_map(|run| match run {
            Run::Text(text, style) => Some((text, style)),
            Run::Fill(style) => {
                let share = room / fills + usize::from(nth < room % fills);
                nth += 1;
                (share > 0).then(|| (" ".repeat(share), style))
            }
        })
        .collect()
}

/// `runs` cut to `width` cells, the cut marked with `…` — on a character
/// boundary, a wide character counted as the two cells it takes.
fn clip(runs: Vec<(String, Style)>, width: usize) -> Vec<(String, Style)> {
    let total: usize = runs.iter().map(|(t, _)| crate::wrap::str_cells(t)).sum();
    if total <= width {
        return runs;
    }
    let Some(budget) = width.checked_sub(1) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut used = 0;
    let mut last = Style::default();
    for (text, style) in runs {
        last = style;
        let cells = crate::wrap::str_cells(&text);
        if used + cells <= budget {
            used += cells;
            out.push((text, style));
            continue;
        }
        // Within the room, never past it: `wrap_cells` gives a wide character
        // a row of its own where one cell is left, and `漢` drew two cells
        // into a one-cell gap — the row a cell wider than the screen
        // (review stop 9, B F7).
        let head = crate::wrap::leading(&text, budget - used);
        if !head.is_empty() {
            out.push((head.to_string(), style));
        }
        break;
    }
    out.push(("…".to_string(), last));
    out
}

/// What some pieces drew, how many values they held and filled, and whether
/// an alert was among what they drew ([`Alert`]).
struct Shown {
    runs: Vec<Run>,
    values: usize,
    filled: usize,
    alert: bool,
}

impl Shown {
    /// Would a group of these show — a value in it filled, or none in it?
    fn shows(&self) -> bool {
        self.values == 0 || self.filled > 0
    }
}

/// What an alert piece draws from: the row's alerts, and the format being
/// drawn, whose own `${alert.<name>}` `$alerts` leaves out.
#[derive(Clone, Copy)]
struct Alerting<'a> {
    alerts: &'a Alerts,
    format: &'a Format,
}

/// Draw `pieces` in `style` — `styled` when a `[…](style)` encloses them, so
/// a value's own style (the gauge's) yields to the one written. `alerting`
/// is `None` in an alert's `say`, which holds no alert piece (`parse`
/// refuses one there).
fn eval(
    pieces: &[Piece],
    style: Style,
    styled: bool,
    alerting: Option<Alerting>,
    source: &mut dyn Source,
) -> Shown {
    let mut shown = Shown {
        runs: Vec::new(),
        values: 0,
        filled: 0,
        alert: false,
    };
    // An alert, drawn where it is written: its `say`'s runs, when it holds
    // and its `say` has something to say — as a group would show. One whose
    // `when` could not be read says so in its place.
    let say = |alert: &Alert, source: &mut dyn Source| {
        match alert.holding(source) {
            Holding::Not => return None,
            Holding::Broken => {
                return Some(vec![Run::Text(format!("[{}: when?]", alert.name), style)]);
            }
            Holding::Holds => {}
        }
        let said = eval(&alert.say.pieces, style, styled, None, source);
        (said.shows() && !said.runs.is_empty()).then_some(said.runs)
    };
    for piece in pieces {
        match piece {
            Piece::Text(text) => shown.runs.push(Run::Text(text.clone(), style)),
            Piece::Fill => shown.runs.push(Run::Fill(style)),
            Piece::Var(var) => {
                let got = source.value(*var);
                shown.values += 1;
                if got.text.is_empty() {
                    continue;
                }
                shown.filled += 1;
                let own = match got.style {
                    Some(own) if !styled => style.patch(own),
                    _ => style,
                };
                match got.keys_from.filter(|_| !styled) {
                    Some(at) => shown.runs.extend(keys_drawn(&got.text, at, own, source)),
                    None => shown.runs.push(Run::Text(got.text, own)),
                }
            }
            Piece::Alerts => {
                shown.values += 1;
                let Some(Alerting { alerts, format }) = alerting else {
                    continue;
                };
                let mut said = (alerts.list.iter().enumerate())
                    .filter(|&(index, _)| !format.places_alert(index))
                    .filter_map(|(_, alert)| say(alert, source))
                    .peekable();
                if said.peek().is_none() {
                    continue;
                }
                shown.filled += 1;
                shown.alert = true;
                let mut first = true;
                for runs in said {
                    if !std::mem::take(&mut first) {
                        shown.runs.push(Run::Text(" · ".to_string(), style));
                    }
                    shown.runs.extend(runs);
                }
            }
            Piece::Alert(index) => {
                shown.values += 1;
                let alert = alerting.and_then(|at| at.alerts.list.get(*index));
                if let Some(runs) = alert.and_then(|alert| say(alert, source)) {
                    shown.filled += 1;
                    shown.alert = true;
                    shown.runs.extend(runs);
                }
            }
            Piece::Styled(inner, look) => {
                let painted = look.over(style, source);
                let inner = eval(inner, painted, true, alerting, source);
                shown.values += inner.values;
                shown.filled += inner.filled;
                shown.alert |= inner.alert;
                shown.runs.extend(inner.runs);
            }
            Piece::Group(inner) => {
                let inner = eval(inner, style, styled, alerting, source);
                shown.values += inner.values;
                shown.filled += inner.filled;
                if inner.shows() {
                    shown.alert |= inner.alert;
                    shown.runs.extend(inner.runs);
                }
            }
        }
    }
    shown
        .runs
        .retain(|run| !matches!(run, Run::Text(text, _) if text.is_empty()));
    shown
}

/// **A token count as the row writes it** (`[tui.status] tokens`): `999`,
/// `1.2k`, `40k`, `10.4M` — or every digit, for [`TokenCounts::Exact`].
///
/// One decimal at every size, to the NEAREST, a trailing `.0` dropped, and
/// SI's lowercase `k`. Not `du -h`'s rule: that rounds up and keeps a
/// decimal only below ten, right for a disk (never promise room you lack)
/// and wrong for a bill — `10.4M` read as `11M` overstates it by 6%. A
/// count that rounds to `1000.0` of a unit is written in the next one
/// (`999950` is `1M`, never `1000k`).
///
/// Consulted by the row's three token values (`view::token_values`). Not
/// yet wired: the model picker's limits (`model_picker::tokens`, which
/// truncates — `4096` is its `4k`); the inspector's `usage`/`tokens`
/// (`view::usage_str`, exact by design — it is where the exact count
/// lives), its `max tok` and per-message `~N tok`; and the "billed N output
/// tokens" notice (`app::unaccounted_output`).
///
/// [`TokenCounts::Exact`]: shannon_core::TokenCounts::Exact
pub(crate) fn token_count(n: u64, style: shannon_core::TokenCounts) -> String {
    const UNITS: [&str; 6] = ["k", "M", "G", "T", "P", "E"];
    if style == shannon_core::TokenCounts::Exact || n < 1_000 {
        return n.to_string();
    }
    // u128: `n * 10` overflows a u64 near its top, and an exa-scale unit
    // must still hold `u64::MAX` (18.4E) — so the search below always ends.
    let n = u128::from(n);
    UNITS
        .iter()
        .scan(1u128, |unit, suffix| {
            *unit *= 1_000;
            Some(((n * 10 + *unit / 2) / *unit, suffix))
        })
        .find(|&(tenths, _)| tenths < 10_000)
        .map_or_else(
            || n.to_string(),
            |(tenths, suffix)| match tenths % 10 {
                0 => format!("{}{suffix}", tenths / 10),
                d => format!("{}.{d}{suffix}", tenths / 10),
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{AppFact, SessionFact};

    impl Alerts {
        /// The shipped alerts, as a config that writes none reads them.
        fn shipped() -> (Alerts, Vec<String>) {
            Alerts::read(&shannon_core::config::TuiConfig::default())
        }

        /// Only the alerts `written` (`name`, `when`, `say`), as a config
        /// writing them over no shipped ones would read them.
        fn written(written: &[(&str, Option<&str>, &str)]) -> (Alerts, Vec<String>) {
            let bordered: Vec<_> = (written.iter())
                .map(|&(name, when, say)| (name, when, say, None))
                .collect();
            Alerts::bordered(&bordered)
        }

        /// [`written`](Self::written), each with a `border` too.
        fn bordered(written: &[(&str, Option<&str>, &str, Option<&str>)]) -> (Alerts, Vec<String>) {
            let alert = (written.iter())
                .map(|&(name, when, say, border)| {
                    let alert = shannon_core::config::AlertConfig {
                        when: when.map(str::to_string),
                        say: Some(say.to_string()),
                        border: border.map(str::to_string),
                        ..Default::default()
                    };
                    (name.to_string(), alert)
                })
                .collect();
            Alerts::read(&shannon_core::config::TuiConfig {
                alert,
                ..Default::default()
            })
        }
    }

    /// **A row's values and facts, given** — every other value empty, every
    /// other fact 0 — and a theme painting each role in a colour of its own
    /// ([`painted`]).
    struct Given<'a> {
        values: &'a [(&'a str, &'a str)],
        facts: &'a [(Fact, u64)],
    }

    /// The colour [`Given`]'s theme paints `ink`, its background the next —
    /// but `ui.text`, which it gives nothing, as a theme may.
    fn painted(ink: Ink) -> Style {
        if ink == Ink::Text {
            return Style::default();
        }
        let at = ink as u8;
        (Style::default().fg(Color::Indexed(at)))
            .bg(Color::Indexed(at.wrapping_add(1)))
            .add_modifier(Modifier::ITALIC)
    }

    impl Source for Given<'_> {
        fn value(&mut self, var: Var) -> Value {
            (self.values.iter())
                .find(|(name, _)| *name == var.name())
                .map(|(_, v)| Value::from(v.to_string()))
                .unwrap_or_default()
        }

        fn fact(&mut self, fact: Fact) -> u64 {
            (self.facts.iter())
                .find(|(f, _)| *f == fact)
                .map_or(0, |&(_, n)| n)
        }

        fn ink(&mut self, ink: Ink) -> Style {
            painted(ink)
        }
    }

    /// The text of `format` (read knowing `alerts`) drawn from `given`.
    fn said(alerts: &Alerts, format: &str, given: &mut Given) -> String {
        let (format, notes) = parse("t", format, Some(alerts));
        assert!(notes.is_empty(), "{notes:?}");
        (render(&format, alerts, given, Style::default(), 200).into_iter())
            .map(|(t, _)| t)
            .collect()
    }

    /// **A value's row of keys is drawn as keys**, past its lead-in, in the
    /// row's own theme — and yields to a style written around it, as a
    /// value's own colour does.
    #[test]
    fn a_values_row_of_keys_is_drawn_as_keys() {
        struct Decide;
        impl Source for Decide {
            fn value(&mut self, var: Var) -> Value {
                match var {
                    Var::Decide => Value {
                        text: "DECIDE: d deny · a allow".into(),
                        style: Some(Style::default().bg(Color::Blue)),
                        keys_from: Some("DECIDE: ".len()),
                    },
                    _ => Value::default(),
                }
            }
            fn fact(&mut self, _: Fact) -> u64 {
                0
            }
            fn ink(&mut self, ink: Ink) -> Style {
                // A role's colour alone, as a key's is: its ground is the
                // value's.
                Style::default().fg(painted(ink).fg.unwrap_or(Color::Reset))
            }
        }
        let keys = |format: &str| -> (String, Vec<(String, Style)>) {
            let (format, notes) = read("t", format);
            assert!(notes.is_empty(), "{notes:?}");
            let runs = draw(&format, &mut Decide, 80);
            let text = runs.iter().map(|(t, _)| t.as_str()).collect();
            let key = painted(Ink::HelpCode);
            let keys = (runs.into_iter())
                .filter(|(_, style)| style.fg == key.fg)
                .collect();
            (text, keys)
        };
        let (text, drawn): (String, Vec<(String, Style)>) = keys("$decide");
        assert_eq!(text, "DECIDE: d deny · a allow", "the words are unchanged");
        let names: Vec<&str> = drawn.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["d", "a"], "the keys, past the lead-in");
        assert!(
            drawn.iter().all(|(_, style)| style.bg == Some(Color::Blue)),
            "over the value's own ground: {drawn:?}"
        );
        let (_, drawn) = keys("[$decide](fg:ui.danger)");
        assert!(drawn.is_empty(), "a style around it wins: {drawn:?}");
    }

    /// A format read as `[tui.status]` reads one, knowing the shipped
    /// alerts.
    fn read(key: &str, text: &str) -> (Format, Vec<String>) {
        parse(key, text, Some(&Alerts::shipped().0))
    }

    /// `format` drawn in `width`, its alerts the shipped ones.
    fn draw(format: &Format, source: &mut dyn Source, width: u16) -> Vec<(String, Style)> {
        render(
            format,
            &Alerts::shipped().0,
            source,
            Style::default(),
            width,
        )
    }

    /// The row's text for `format` in `width`, each value from `values`
    /// (`name=value`), every other value empty.
    fn row(format: &str, values: &[(&str, &str)], width: u16) -> String {
        let (format, notes) = read("t", format);
        assert!(notes.is_empty(), "{notes:?}");
        text(&format, values, width)
    }

    fn text(format: &Format, values: &[(&str, &str)], width: u16) -> String {
        let mut value = |var: Var| -> Value {
            values
                .iter()
                .find(|(name, _)| *name == var.name())
                .map(|(_, v)| Value::from(v.to_string()))
                .unwrap_or_default()
        };
        draw(format, &mut value, width)
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    /// **Every shipped row says each alert that holds, and once** — in
    /// every state of the shipped alerts that say something, wide enough
    /// for all of them: `$alerts` beside the pane's name, `${alert.hidden}`
    /// at the right, and nothing said twice or not at all. (B1.1 was
    /// measured byte for byte against the row before alerts were rules, over
    /// these states at thirteen widths; this keeps the half that can
    /// regress.)
    ///
    /// **Built from the shipped list**: the texts are asserted to be one
    /// for each shipped alert that says something, and an alert with a
    /// `when` is given a fact that holds it. Hand-listed, this was five of
    /// seven, its facts all 0 — and a second `$unshown` in the shipped row
    /// passed (review stop 9, C 4).
    #[test]
    fn every_shipped_row_says_each_alert_once() {
        let texts = [
            ("decide", "DECIDE: d deny · a allow · rm /tmp/a"),
            ("queued", "2 queued"),
            ("running", "running: cargo test"),
            ("interrupt", "^C:stop"),
            ("hidden", "decision hidden · `:decide 3`"),
            ("unshown", "news in session 3 · `:b 3`"),
        ];
        let (alerts, _) = Alerts::shipped();
        let saying: Vec<&Alert> = (alerts.list.iter())
            .filter(|alert| !alert.say.pieces.is_empty())
            .collect();
        let names: Vec<&str> = saying.iter().map(|alert| &*alert.name).collect();
        assert_eq!(names, texts.map(|(name, _)| name), "a text for each");
        let holding = |trigger: &Trigger| (0..=100).find(|&n| trigger.holds(n)).expect("held");
        let shipped = shannon_core::StatusConfig::default();
        for (key, format) in [
            ("prompt", &shipped.prompt),
            ("output", &shipped.output),
            ("decision", &shipped.decision),
            ("graph", &shipped.graph),
            ("page", &shipped.page),
        ] {
            let (format, notes) = parse(key, format, Some(&alerts));
            assert!(notes.is_empty(), "{notes:?}");
            for mask in 0u32..(1 << texts.len()) {
                let on: Vec<(&str, &str)> = (texts.iter().enumerate())
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, &pair)| pair)
                    .collect();
                let facts: Vec<(Fact, u64)> = (saying.iter())
                    .filter(|alert| on.iter().any(|(name, _)| *name == &*alert.name))
                    .filter_map(|alert| match &alert.when {
                        When::On(trigger) => Some((trigger.fact(), holding(trigger))),
                        _ => None,
                    })
                    .collect();
                let mut given = Given {
                    values: &on,
                    facts: &facts,
                };
                let row: String = (render(&format, &alerts, &mut given, Style::default(), 300))
                    .into_iter()
                    .map(|(t, _)| t)
                    .collect();
                for (name, text) in texts {
                    let times = row.matches(text).count();
                    let held = on.iter().any(|(n, _)| *n == name);
                    assert_eq!(times, usize::from(held), "{key}, {name}: {row}");
                }
            }
        }
    }

    /// **Steven's rows read as he wrote them** (10-14) — brackets and
    /// parentheses are text, which starship's grammar refuses outright. The
    /// transcript's row leaves `$readonly` out though it holds (10-30,
    /// S14-Q1: typing says so, as vim's E21 does).
    #[test]
    fn the_shipped_rows_read_as_written() {
        let values = [
            ("mode", "NORMAL"),
            ("pane", "output"),
            ("readonly", "read-only"),
            ("model", "sonnet"),
            ("ctx", "40%"),
            ("tokens_in", "1200"),
            ("cache", "85% cached"),
            ("tokens_out", "300"),
        ];
        assert_eq!(
            row(shannon_core::config::DEFAULT_STATUS_OUTPUT, &values, 200),
            "[NORMAL] output · sonnet · ctx 40% · tok ↑1200 (85% cached) ↓300"
        );
        let prompt = [("pane", "prompt")];
        let values: Vec<_> = values
            .iter()
            .filter(|(n, _)| !prompt.iter().any(|(p, _)| p == n))
            .chain(&prompt)
            .copied()
            .collect();
        assert_eq!(
            row(shannon_core::config::DEFAULT_STATUS_PROMPT, &values, 200),
            "[NORMAL] prompt · sonnet · ctx 40% · tok ↑1200 (85% cached) ↓300"
        );
        assert_eq!(
            row(
                shannon_core::config::DEFAULT_STATUS_GRAPH,
                &[
                    ("mode", "NORMAL"),
                    ("pane", "graph"),
                    ("position", "3/5"),
                    ("help", "graph-keys")
                ],
                200
            ),
            "[NORMAL] graph · 3/5 · help `:help graph-keys`"
        );
    }

    /// **Every shipped format and alert reads with nothing to say** — a
    /// typo in one would draw `$modle` on everyone's screen.
    #[test]
    fn every_shipped_format_reads_cleanly() {
        let (formats, notes) = Formats::read(&shannon_core::config::TuiConfig::default());
        assert!(notes.is_empty(), "{notes:?}");
        let list = &formats.alerts().list;
        let names: Vec<&str> = list.iter().map(|alert| &*alert.name).collect();
        assert_eq!(
            names,
            [
                "decide",
                "queued",
                "running",
                "interrupt",
                "hidden",
                "unshown",
                "unshown_session"
            ]
        );
        assert!(
            list.iter()
                .all(|alert| alert.when != When::Off && alert.border.is_none())
        );
        // What waits on you: each said by the value of its name, whenever
        // that has something to say, and marking nothing.
        for alert in &list[..5] {
            let var = Var::from_name(&alert.name).expect("a shipped alert's value");
            assert_eq!(alert.say.pieces, [Piece::Var(var)], "{}", alert.name);
            assert_eq!(alert.when, When::Says, "{}", alert.name);
            assert!(!alert.mark, "{}", alert.name);
        }
        // News you have not seen: counted on the row, marked per session.
        let trigger = |text| When::On(Trigger::read(text).unwrap());
        let (unshown, each) = (&list[5], &list[6]);
        assert_eq!(unshown.when, trigger("unshown_count"));
        assert_eq!(unshown.say.pieces, [Piece::Var(Var::Unshown)]);
        assert!(!unshown.mark);
        assert_eq!(each.when, trigger("unshown"));
        assert!(each.say.pieces.is_empty(), "it says nothing on the row");
        assert!(each.mark);
    }

    /// **A short count rounds to the nearest tenth and never reads `1000k`**
    /// — each edge walked: exact below a thousand, the half-up step, a
    /// trailing `.0` dropped, the roll into the next unit, and the top of a
    /// u64 (which must end the unit search rather than fall back to digits).
    #[test]
    fn a_short_token_count_rounds_to_the_nearest_tenth() {
        use shannon_core::TokenCounts::{Exact, Short};
        for (n, short) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1k"),
            (1_049, "1k"),
            (1_050, "1.1k"),
            (39_996, "40k"),
            (999_949, "999.9k"),
            (999_950, "1M"),
            (10_415_096, "10.4M"),
            (4_294_967_295, "4.3G"),
            (u64::MAX, "18.4E"),
        ] {
            assert_eq!(token_count(n, Short), short, "{n}");
            assert_eq!(token_count(n, Exact), n.to_string(), "{n}, exact");
        }
    }

    /// **A group shows when a value in it does** — and one with no value in
    /// it always shows (vim's rule: starship's hides it, so its
    /// `(read-only)` could never appear).
    #[test]
    fn a_group_shows_when_a_value_in_it_does() {
        assert_eq!(row("a{ · $model}b", &[], 80), "ab");
        assert_eq!(row("a{ · $model}b", &[("model", "m")], 80), "a · mb");
        assert_eq!(row("a{ · $model$ctx}b", &[("ctx", "1%")], 80), "a · 1%b");
        assert_eq!(row("a{ (fixed)}b", &[], 80), "a (fixed)b");
        // Nested: the inner hides alone; the outer asks every value in it.
        assert_eq!(
            row("{↑$tokens_in{ ($cache)}}", &[("tokens_in", "5")], 80),
            "↑5"
        );
        assert_eq!(row("{x{ $cache}}", &[], 80), "", "all empty: hidden");
    }

    /// **The rightmost group goes first** when the row is too narrow, then
    /// the next — and what is outside a group is cut, marked, last.
    #[test]
    fn a_narrow_row_drops_its_rightmost_groups_first() {
        let format = "[$mode]{ · $alerts}{ · $model}{ · ctx $ctx}";
        let values = [
            ("mode", "NORMAL"),
            ("decide", "DECIDE"),
            ("model", "sonnet"),
            ("ctx", "40%"),
        ];
        assert_eq!(
            row(format, &values, 80),
            "[NORMAL] · DECIDE · sonnet · ctx 40%"
        );
        assert_eq!(row(format, &values, 30), "[NORMAL] · DECIDE · sonnet");
        assert_eq!(row(format, &values, 20), "[NORMAL] · DECIDE");
        assert_eq!(row(format, &values, 6), "[NORM…");
        assert_eq!(row(format, &values, 0), "");
        // Cells, not chars: a wide character counts twice, and is never
        // split.
        assert_eq!(row("ab漢字", &[], 4), "ab…");
    }

    /// **`$fill` is the room the rest leaves** — what follows it at the
    /// right edge, several sharing it evenly (the first a cell more), none
    /// in a group that hides, and nothing at all once the row is full: the
    /// rest is dropped and cut by the rules a row without one keeps.
    #[test]
    fn fill_takes_the_room_the_rest_leaves() {
        assert_eq!(row("ab$fill${model}", &[("model", "m")], 6), "ab   m");
        assert_eq!(row("a$fill|$fill|b", &[], 8), "a  |  |b", "shared evenly");
        assert_eq!(
            row("a$fill|$fill|b", &[], 9),
            "a   |  |b",
            "the first a cell more"
        );
        assert_eq!(row("a{ $fill$model}", &[], 9), "a", "hidden with its group");
        assert_eq!(row("a{ $fill$model}", &[("model", "m")], 6), "a    m");
        // Full: no room, and the groups go as they would with no fill.
        let format = "[$mode]{ · $model}{ $fill$alerts}";
        let values = [("mode", "N"), ("model", "sonnet"), ("decide", "DECIDE")];
        assert_eq!(row(format, &values, 20), "[N] · sonnet  DECIDE");
        assert_eq!(
            row(format, &values, 12),
            "[N]   DECIDE",
            "the model goes, not the alert, and the fill takes what it left"
        );
        assert_eq!(row(format, &values, 9), "[N] DECI…", "then the rest is cut");
        // In a style, a fill is the style's: a coloured band to the edge.
        let (format, _) = read("t", "[a$fill](bg:#102030)");
        let runs = draw(&format, &mut |_| Value::default(), 4);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(runs[1].0, "   ");
        assert_eq!(runs[1].1.bg, Some(Color::Rgb(0x10, 0x20, 0x30)));
    }

    /// **A format places a value, or an alert, wherever it writes it** — in
    /// a group, in a style — and `$fill` and `$alerts` are no values: never
    /// "a value the row does not have".
    #[test]
    fn a_format_places_what_it_writes() {
        let (format, notes) = read("t", "$mode{ [x $hidden](bold)} ${fill}$alerts");
        assert!(notes.is_empty(), "{notes:?}");
        assert!(format.places(Var::Mode));
        assert!(format.places(Var::Hidden), "inside a group and a style");
        assert!(!format.places(Var::Decide));
        let (alerts, _) = Alerts::shipped();
        let hidden = alerts.index("hidden").unwrap();
        assert!(!format.places_alert(hidden), "a value is not its alert");
        let (format, notes) = read("t", "{ [x ${alert.hidden}](bold)}");
        assert!(notes.is_empty(), "{notes:?}");
        assert!(format.places_alert(hidden), "inside a group and a style");
        assert!(!format.places_alert(alerts.index("decide").unwrap()));
        assert!(!format.places(Var::Hidden), "an alert is not its value");
    }

    /// **The shipped row sheds numbers before it sheds what waits on you**
    /// — and a decision it cannot fit is cut, never dropped (a row that
    /// clips is honest; one that silently omits a pending decision is not).
    /// `DECIDE:` exists for a terminal too small to draw the decision pane,
    /// exactly when this row overflows; at 80 columns it once sat past column
    /// 150 behind the counts (Reviewer F, measured).
    #[test]
    fn the_shipped_row_sheds_numbers_before_a_decision() {
        let values = [
            ("mode", "NORMAL"),
            ("pane", "prompt"),
            ("decide", "DECIDE: d deny · a allow · rm /tmp/a"),
            ("queued", "2 queued"),
            ("interrupt", "^C:stop"),
            ("activity", "answering…"),
            ("model", "sonnet high"),
            ("ctx", "40%"),
            ("tokens_in", "120000"),
            ("tokens_out", "5000"),
            ("cache", "85% cached"),
        ];
        let format = shannon_core::config::DEFAULT_STATUS_PROMPT;
        let wide = row(format, &values, 200);
        assert!(wide.contains("tok ↑120000"), "{wide}");
        // Middling: the numbers go, rightmost first, the alerts stay whole.
        let middling = row(format, &values, 100);
        assert!(middling.contains("^C:stop"), "{middling}");
        assert!(middling.contains("answering…"), "{middling}");
        assert!(!middling.contains("tok"), "{middling}");
        assert!(crate::wrap::str_cells(&middling) <= 100, "{middling}");
        // Tight: everything droppable gone, and the alerts CUT, not dropped.
        for width in [40, 30] {
            let tight = row(format, &values, width);
            assert!(tight.contains(" · DECIDE"), "{width}: {tight}");
            assert!(tight.ends_with('…'), "{width}: {tight}");
            assert_eq!(crate::wrap::str_cells(&tight), usize::from(width));
        }
    }

    /// **A hidden decision sits at the shipped row's right, and outlasts the
    /// numbers** (Steven, 10-22: "by default it should go in the bottom on
    /// the right side") — past a `$fill`, on the edge; narrowed, the numbers
    /// go and it stays, as what waits on you does.
    #[test]
    fn the_shipped_row_keeps_a_hidden_decision_at_its_right() {
        let hidden = "decision hidden · `:decide 3`";
        let values = [
            ("mode", "NORMAL"),
            ("pane", "prompt"),
            ("model", "sonnet high"),
            ("ctx", "40%"),
            ("tokens_in", "120000"),
            ("tokens_out", "5000"),
            ("hidden", hidden),
        ];
        let format = shannon_core::config::DEFAULT_STATUS_PROMPT;
        let wide = row(format, &values, 120);
        assert!(wide.contains("tok ↑120000"), "{wide}");
        assert!(wide.ends_with(&format!("  {hidden}")), "{wide}");
        assert_eq!(crate::wrap::str_cells(&wide), 120, "on the edge: {wide}");
        let narrow = row(format, &values, 50);
        assert!(!narrow.contains("tok"), "{narrow}");
        assert!(narrow.ends_with(hidden), "{narrow}");
    }

    /// **`[text](style)` paints; a bracket not written that way is text**,
    /// and a value's own style yields to the one written around it.
    #[test]
    fn styles_paint_and_a_bare_bracket_is_text() {
        let (format, notes) = read("t", "[$mode](bold fg:red) [x] (y) [$ctx](bg:#102030)");
        assert!(notes.is_empty(), "{notes:?}");
        let mut value = |var: Var| match var {
            Var::Mode => Value::from("N".to_string()),
            Var::Ctx => Value {
                text: "9%".into(),
                style: Some(Style::default().fg(Color::Yellow)),
                keys_from: None,
            },
            _ => Value::default(),
        };
        let runs = draw(&format, &mut value, 80);
        let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "N [x] (y) 9%");
        assert_eq!(
            runs[0].1,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        );
        let ctx = runs.last().unwrap().1;
        assert_eq!(ctx.bg, Some(Color::Rgb(0x10, 0x20, 0x30)));
        assert_eq!(ctx.fg, None, "the written style, not the gauge's");
        // Unstyled, the value brings its own.
        let (bare, _) = read("t", "$ctx");
        let runs = draw(&bare, &mut value, 80);
        assert_eq!(runs[0].1.fg, Some(Color::Yellow));
    }

    /// **A theme role in a style is the theme's, asked when drawn** — alone,
    /// all of its style; after `fg:` or `bg:`, its colour; under whatever
    /// else the style writes — in a format, and in an alert's `say`. A
    /// dotted word that is no role is said.
    #[test]
    fn a_role_word_paints_as_the_theme_says_when_drawn() {
        let alerts = Alerts::shipped().0;
        let (format, notes) = parse(
            "t",
            "[a](ui.caution)[b](fg:ui.danger bg:ui.safe)[c](ui.caution fg:red bold)",
            Some(&alerts),
        );
        assert!(notes.is_empty(), "{notes:?}");
        let mut given = Given {
            values: &[],
            facts: &[],
        };
        let runs = render(&format, &alerts, &mut given, Style::default(), 80);
        assert_eq!(
            runs[0],
            ("a".to_string(), painted(Ink::Caution)),
            "all of it"
        );
        let b = Style::default()
            .fg(painted(Ink::Danger).fg.unwrap())
            .bg(painted(Ink::Safe).fg.unwrap());
        assert_eq!(runs[1], ("b".to_string(), b), "its colour, and only that");
        let c = painted(Ink::Caution)
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD);
        assert_eq!(runs[2], ("c".to_string(), c), "under what else is written");
        // A role the theme gives no colour leaves the text its own.
        let (format, _) = parse("t", "[d](fg:ui.text bg:ui.text)", Some(&alerts));
        let base = Style::default().fg(Color::Yellow).bg(Color::Blue);
        let runs = render(&format, &alerts, &mut given, base, 80);
        assert_eq!(runs, [("d".to_string(), base)], "no colour chosen for it");

        let (_, notes) = parse("t", "[x](ui.cautoin) [y](bg:ui.nope)", Some(&alerts));
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(
            notes[0].contains("`ui.cautoin` is not a theme role"),
            "{notes:?}"
        );
        assert!(
            notes[1].contains("`ui.nope` is not a theme role"),
            "{notes:?}"
        );

        let (alerts, notes) = Alerts::written(&[("hot", None, "[HOT](ui.danger)")]);
        assert!(notes.is_empty(), "{notes:?}");
        let (format, _) = parse("t", "$alerts", Some(&alerts));
        let runs = render(&format, &alerts, &mut given, Style::default(), 80);
        assert_eq!(
            runs,
            [("HOT".to_string(), painted(Ink::Danger))],
            "in a say"
        );
    }

    /// **An alert's border is the first holding for the session asked
    /// about** — in table order, as the theme says it now, none when none
    /// holds; and a border on an alert that holds for no session (no `when`,
    /// or the app's fact) is said and drawn nowhere.
    #[test]
    fn a_border_is_the_first_alert_holding_for_the_session() {
        let ctx = Fact::Session(SessionFact::CtxPercent);
        // Yours are in name order: `a_full` before `b_filling`.
        let (alerts, notes) = Alerts::bordered(&[
            (
                "b_filling",
                Some("ctx_percent >= 70"),
                "",
                Some("fg:ui.caution"),
            ),
            ("a_full", Some("ctx_percent >= 90"), "", Some("ui.danger")),
            ("c_quiet", Some("ctx_percent >= 10"), "", None),
            ("d_always", None, "x", Some("ui.safe")),
            ("e_busy", Some("sessions"), "", Some("ui.safe")),
        ]);
        // `c_quiet` writes no outcome at all: said (review stop 9, F6).
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[0].starts_with("tui.alert.c_quiet: nothing says it"));
        let notes = &notes[1..];
        assert!(
            notes[0].starts_with("tui.alert.d_always.border: "),
            "{notes:?}"
        );
        assert!(
            notes[1].starts_with("tui.alert.e_busy.border: "),
            "{notes:?}"
        );
        let border = |percent, sessions| {
            let facts = [(ctx, percent), (Fact::App(AppFact::Sessions), sessions)];
            let mut given = Given {
                values: &[],
                facts: &facts,
            };
            alerts.border(&mut given)
        };
        assert_eq!(
            border(95, 1),
            Some(painted(Ink::Danger)),
            "the first that holds"
        );
        let caution = Style::default().fg(painted(Ink::Caution).fg.unwrap());
        assert_eq!(border(75, 1), Some(caution), "the next, its colour alone");
        assert_eq!(border(20, 5), None, "one holding with no border has none");

        // Disabled, it borders nothing.
        let mut tui = shannon_core::config::TuiConfig::default();
        let hot = shannon_core::config::AlertConfig {
            when: Some("ctx_percent >= 1".to_string()),
            border: Some("ui.danger".to_string()),
            ..Default::default()
        };
        tui.alert.insert("hot".to_string(), hot);
        tui.alerts.disabled = vec!["hot".to_string()];
        let (alerts, notes) = Alerts::read(&tui);
        assert!(notes.is_empty(), "{notes:?}");
        let mut given = Given {
            values: &[],
            facts: &[(ctx, 50)],
        };
        assert_eq!(alerts.border(&mut given), None, "disabled: drawn nowhere");
    }

    /// **A mark is the glyph, for a session an alert with `mark` holds
    /// for** — none otherwise, and no column at all while no alert marks; a
    /// `mark` on an alert that holds for no session is said and drawn
    /// nowhere.
    #[test]
    fn a_mark_is_the_glyph_for_a_session_an_alert_marks() {
        assert_eq!(
            Alerts::shipped().0.mark_cells(),
            2,
            "`unshown_session` marks, so the column is there"
        );
        let ctx = Fact::Session(SessionFact::CtxPercent);
        // Only the alerts written here: none of the shipped ones.
        let mut tui = shannon_core::config::TuiConfig {
            alert: Default::default(),
            ..Default::default()
        };
        assert_eq!(
            Alerts::read(&tui).0.mark_cells(),
            0,
            "nothing marks: no column"
        );
        for (name, when, mark) in [
            ("hot", Some("ctx_percent >= 80"), true),
            ("odd", None, true),
            // Asked for no mark: nothing to say of its scope.
            ("quiet", None, false),
        ] {
            let alert = shannon_core::config::AlertConfig {
                when: when.map(str::to_string),
                mark: Some(mark),
                ..Default::default()
            };
            tui.alert.insert(name.to_string(), alert);
        }
        let (alerts, notes) = Alerts::read(&tui);
        // And `quiet`, asked for nothing, is said as nothing (review stop 9,
        // F6).
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].starts_with("tui.alert.odd.mark: "), "{notes:?}");
        assert!(notes[1].starts_with("tui.alert.quiet: nothing says it"));
        assert_eq!(alerts.mark_cells(), 2, "a bell is two cells");
        let mark = |percent| {
            let facts = [(ctx, percent)];
            let mut given = Given {
                values: &[],
                facts: &facts,
            };
            alerts.mark(&mut given).map(str::to_string)
        };
        assert_eq!(mark(90).as_deref(), Some("🔔"));
        assert_eq!(mark(10), None);
        tui.alerts.disabled = vec!["hot".to_string()];
        let (alerts, _) = Alerts::read(&tui);
        assert_eq!(alerts.mark_cells(), 0, "disabled, nothing marks");
    }

    /// **What an alert sends, read** — a bell and a notification need a
    /// `when` (one without is said, and sends nothing); `notify = true` is
    /// Shannon's words, naming the session only for a session's fact; a
    /// string is yours, its misspelt values said; `false` sends nothing; and
    /// a disabled alert is no ringing one. The shipped `unshown_session`
    /// rings and notifies by default.
    #[test]
    fn what_an_alert_sends_is_read_and_said() {
        use shannon_core::config::{AlertConfig, Notify};
        let shipped: Vec<_> = (Alerts::shipped().0.ringing())
            .map(|r| (r.name().to_string(), r.bell, r.notify.cloned()))
            .collect();
        let words = crate::ring::Template::read("t", "Shannon: news in $session", &mut Vec::new());
        assert_eq!(
            shipped,
            [("unshown_session".to_string(), true, Some(words))],
            "the shipped one that sends"
        );
        let mut tui = shannon_core::config::TuiConfig {
            alert: Default::default(),
            ..Default::default()
        };
        for (name, when, bell, notify) in [
            (
                "a_session",
                Some("ctx_percent >= 80"),
                true,
                Some(Notify::On(true)),
            ),
            ("b_app", Some("sessions"), false, Some(Notify::On(true))),
            (
                "c_yours",
                Some("queued"),
                false,
                Some(Notify::Says("$sesion".into())),
            ),
            ("d_none", None, true, None),
            ("e_off", Some("queued"), false, Some(Notify::On(false))),
            ("f_disabled", Some("queued"), true, None),
        ] {
            let alert = AlertConfig {
                when: when.map(str::to_string),
                bell: Some(bell),
                notify,
                ..Default::default()
            };
            tui.alert.insert(name.to_string(), alert);
        }
        tui.alerts.disabled = vec!["f_disabled".to_string()];
        let (alerts, notes) = Alerts::read(&tui);
        // `e_off`, its notification off and no bell, does nothing: said
        // (review stop 9, F6); `f_disabled`, drawn nowhere, is not.
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[0].contains("`$sesion`"), "{notes:?}");
        assert!(notes[1].starts_with("tui.alert.d_none: "), "{notes:?}");
        assert!(notes[2].starts_with("tui.alert.e_off: nothing says it"));
        let ringing: Vec<(&str, bool, Option<String>)> = (alerts.ringing())
            .map(|r| {
                (
                    r.name(),
                    r.bell,
                    r.notify.map(|w| w.say(r.name(), Some("b3 s"))),
                )
            })
            .collect();
        assert_eq!(
            ringing,
            [
                (
                    "a_session",
                    true,
                    Some("Shannon: a_session · b3 s".to_string())
                ),
                ("b_app", false, Some("Shannon: b_app".to_string())),
                ("c_yours", false, Some("$sesion".to_string())),
            ]
        );
    }

    /// **The glyph is text a cell can hold** — from any config file, a
    /// project's included, so a control character (an escape reaching the
    /// terminal) or a width past four cells is said, shown escaped, and the
    /// shipped glyph drawn.
    #[test]
    fn a_glyph_is_text_a_cell_can_hold() {
        for (written, drawn) in [("!", "!"), ("[*]", "[*]"), ("⚑", "⚑")] {
            let mut notes = Vec::new();
            assert_eq!(read_glyph(written, &mut notes), drawn);
            assert!(notes.is_empty(), "{written}: {notes:?}");
        }
        for written in [
            "",
            "\u{1b}]52;c;aGk=\u{7}",
            "a\u{9b}b",
            "12345",
            "🔔🔔🔔",
            "!\u{202e}",
            "!\u{2028}",
        ] {
            let mut notes = Vec::new();
            assert_eq!(read_glyph(written, &mut notes), "🔔", "{written:?}");
            assert_eq!(notes.len(), 1, "{written:?}");
            // As `Formats::read` says every note: escaped once (`legible`).
            let said = legible(&notes[0]);
            assert!(
                !said.contains(['\u{1b}', '\u{202e}']),
                "said escaped: {said}"
            );
        }
        // A glyph too wide, in a script with marks: said as written, the
        // marks too (G 8: `Debug` escaped the virama).
        let mut notes = Vec::new();
        read_glyph("क्षेत्र-क्षेत्र", &mut notes);
        assert!(legible(&notes[0]).contains("`क्षेत्र-क्षेत्र`"), "{notes:?}");
    }

    /// **What cannot be read is said and drawn as written** — never
    /// dropped: a misspelt value, a style word, a `{` left open, a stray `}`.
    #[test]
    fn what_cannot_be_read_is_said_and_shown() {
        let (format, notes) = read("tui.status.prompt", "$modle [x](blod) {open");
        assert_eq!(notes.len(), 3, "{notes:?}");
        for said in ["`$modle`", "`blod`", "`{`"] {
            assert!(notes.iter().any(|n| n.contains(said)), "{said}: {notes:?}");
        }
        assert!(notes.iter().all(|n| n.starts_with("tui.status.prompt:")));
        assert_eq!(text(&format, &[], 80), "$modle x {open");
        let (format, notes) = read("t", "a}b ${nope}");
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(text(&format, &[], 80), "a}b ${nope}");
    }

    /// **`$alerts` says every alert with something to say, in the shipped
    /// order** — not the order the values were asked in — joined with ` · `
    /// in the style around it, and nothing at all when none has: a group
    /// holding it hides.
    #[test]
    fn alerts_says_each_alert_in_the_shipped_order() {
        let values = [("hidden", "H"), ("decide", "D"), ("interrupt", "I")];
        assert_eq!(row("<$alerts>", &values, 80), "<D · I · H>");
        assert_eq!(
            row("<{$alerts}>", &[], 80),
            "<>",
            "a group with nothing hides"
        );
        assert_eq!(row("<{($alerts)}>", &[("queued", "Q")], 80), "<(Q)>");
        // A style written around it paints every alert and every join.
        let (format, notes) = read("t", "[$alerts](bold)");
        assert!(notes.is_empty(), "{notes:?}");
        let mut value = |var: Var| match var {
            Var::Decide | Var::Queued => Value::from("x".to_string()),
            _ => Value::default(),
        };
        let runs = draw(&format, &mut value, 80);
        let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "x · x");
        assert!(
            runs.iter()
                .all(|(_, style)| style.add_modifier.contains(Modifier::BOLD)),
            "{runs:?}"
        );
    }

    /// **An alert placed is said there, and once** — `$alerts` leaves out
    /// what `${alert.<name>}` places. Its value written bare is not the
    /// alert: `$alerts` still says it (the shipped rows' `$hidden` before
    /// B1.1, which a file written then still holds).
    #[test]
    fn an_alert_placed_is_said_there_and_once() {
        let values = [("decide", "D"), ("hidden", "H")];
        assert_eq!(row("$alerts|${alert.hidden}", &values, 80), "D|H");
        assert_eq!(row("${alert.decide}|$alerts", &values, 80), "D|H");
        assert_eq!(
            row("$alerts|$hidden", &values, 80),
            "D · H|H",
            "a value bare"
        );
        assert_eq!(
            row("<{${alert.queued}}>", &values, 80),
            "<>",
            "nothing to say"
        );
    }

    /// **Only an alert keeps its group** when the row narrows — the PIECE,
    /// `$alerts` or `${alert.<name>}`, not the value it says: `$decide`
    /// written bare goes with its group, rightmost first, like any value.
    #[test]
    fn only_an_alert_keeps_its_group() {
        let values = [
            ("mode", "NORMAL"),
            ("decide", "DECIDE"),
            ("model", "sonnet"),
        ];
        for alert in ["$alerts", "${alert.decide}"] {
            let format = format!("[$mode]{{ · $model}}{{ · {alert}}}");
            assert_eq!(
                row(&format, &values, 80),
                "[NORMAL] · sonnet · DECIDE",
                "{alert}"
            );
            assert_eq!(row(&format, &values, 20), "[NORMAL] · DECIDE", "{alert}");
        }
        let bare = "[$mode]{ · $model}{ · $decide}";
        assert_eq!(row(bare, &values, 20), "[NORMAL] · sonnet");
    }

    /// **An alert's `say` shows as a group would** — when a value in it has
    /// something to say, or it holds none — and one that draws nothing adds
    /// no join. Every shipped `say` is one value, where the two agree; yours
    /// are written, and words around a value are the common one.
    #[test]
    fn an_alerts_say_shows_as_a_group_would() {
        let (alerts, notes) = Alerts::written(&[
            ("ctx", None, "ctx $ctx"),
            ("none", None, ""),
            ("z", None, "$decide"),
        ]);
        // An empty `say` is no outcome: said (review stop 9, F6).
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("tui.alert.none: nothing says it"));
        let given = |values| Given { values, facts: &[] };
        assert_eq!(
            said(&alerts, "<$alerts>", &mut given(&[])),
            "<>",
            "its value empty: not `ctx `"
        );
        assert_eq!(
            said(&alerts, "<$alerts>", &mut given(&[("ctx", "85%")])),
            "<ctx 85%>"
        );
        assert_eq!(
            said(
                &alerts,
                "<$alerts>",
                &mut given(&[("ctx", "85%"), ("decide", "D")])
            ),
            "<ctx 85% · D>",
            "the empty say adds no join"
        );
    }

    /// **An alert with a `when` holds when its fact does** — a session's
    /// fact asked of the row's session, the app's of the app — and says its
    /// `say` then, and nothing otherwise, whatever its values say.
    #[test]
    fn an_alert_holds_when_its_fact_does() {
        let (alerts, notes) = Alerts::written(&[
            ("hot", Some("ctx_percent >= 80"), "[ctx $ctx]"),
            ("busy", Some("sessions"), "busy"),
        ]);
        assert!(notes.is_empty(), "{notes:?}");
        let ctx = Fact::Session(SessionFact::CtxPercent);
        let sessions = Fact::App(AppFact::Sessions);
        let values = [("ctx", "85%")];
        for (facts, row) in [
            (&[(ctx, 79)][..], "<>"),
            (&[(ctx, 80)][..], "<[ctx 85%]>"),
            (&[(ctx, 80), (sessions, 2)][..], "<busy · [ctx 85%]>"),
            (&[(sessions, 1)][..], "<busy>"),
        ] {
            let mut given = Given {
                values: &values,
                facts,
            };
            assert_eq!(said(&alerts, "<$alerts>", &mut given), row, "{facts:?}");
        }
        // Placed, the same.
        let mut given = Given {
            values: &values,
            facts: &[(ctx, 90)],
        };
        assert_eq!(said(&alerts, "${alert.hot}", &mut given), "[ctx 85%]");
    }

    /// **A `when` it cannot read is said at load and drawn in its place** —
    /// `[name: when?]`, never silently dead — and keeps its group as any
    /// alert does.
    #[test]
    fn a_broken_when_is_said_and_drawn_in_its_place() {
        let (alerts, notes) = Alerts::written(&[("hot", Some("ctx >= 80"), "x")]);
        assert_eq!(
            notes,
            ["tui.alert.hot.when: `ctx` is not a fact: the fact is `ctx_percent`"]
        );
        let mut given = Given {
            values: &[("model", "sonnet")],
            facts: &[],
        };
        assert_eq!(said(&alerts, "<$alerts>", &mut given), "<[hot: when?]>");
        let (format, _) = parse("t", "N{ · $model}{ · $alerts}", Some(&alerts));
        let narrow: String = (render(&format, &alerts, &mut given, Style::default(), 18))
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(narrow, "N · [hot: when?]", "the model goes, not the alert");
    }

    /// **`[tui.alerts] disabled` draws an alert nowhere** — not in
    /// `$alerts`, not where a format places it, and a format placing it is no
    /// mistake (the shipped rows place `hidden`); a name in it that is no
    /// alert is said.
    #[test]
    fn a_disabled_alert_draws_nowhere() {
        let mut tui = shannon_core::config::TuiConfig::default();
        tui.alerts.disabled = vec!["queued".to_string(), "nope".to_string()];
        let (alerts, notes) = Alerts::read(&tui);
        assert_eq!(notes, ["tui.alerts.disabled: `nope` is not an alert"]);
        let mut given = Given {
            values: &[("queued", "2 queued"), ("decide", "D")],
            facts: &[],
        };
        assert_eq!(
            said(&alerts, "<$alerts|${alert.queued}>", &mut given),
            "<D|>"
        );
    }

    /// **`$alerts` says the shipped alerts first, in their order, then yours
    /// by name** — and an alert named past `[a-z0-9_]` is said and not drawn,
    /// since no `${alert.<name>}` could write it; nor one named nothing
    /// (`[tui.alert.""]`), whose every character passes that test because it
    /// has none (review stop 9, A M1: the guard was unpinned).
    #[test]
    fn yours_follow_the_shipped_alerts_by_name() {
        let mut tui = shannon_core::config::TuiConfig::default();
        for (name, say) in [("zed", "Z"), ("abc", "A"), ("My-Alert", "M"), ("", "E")] {
            let alert = shannon_core::config::AlertConfig {
                say: Some(say.to_string()),
                ..Default::default()
            };
            tui.alert.insert(name.to_string(), alert);
        }
        let (alerts, notes) = Alerts::read(&tui);
        assert_eq!(notes.len(), 2, "{notes:?}");
        for key in ["tui.alert.My-Alert: ", "tui.alert.: "] {
            assert!(notes.iter().any(|n| n.starts_with(key)), "{key}: {notes:?}");
        }
        let mut given = Given {
            values: &[("hidden", "H"), ("decide", "D")],
            facts: &[],
        };
        assert_eq!(said(&alerts, "<$alerts>", &mut given), "<D · H · A · Z>");
    }

    /// **A `when` that cannot be read is said once**, as the `when` — not
    /// again by each outcome that needs a session's fact, whose note told
    /// `ctx >= 80` its `when` must name one, which it meant to (review stop
    /// 9, A F7 and C 6). And `$session` in the words of an alert on the
    /// app's fact, which can say nothing there, is said (D6).
    #[test]
    fn a_broken_when_is_said_once_and_an_empty_session_too() {
        use shannon_core::config::{AlertConfig, Notify, TuiConfig};
        let notes_of = |name: &str, alert: AlertConfig| {
            let mut tui = TuiConfig::default();
            tui.alert.insert(name.to_string(), alert);
            let (_, notes) = Alerts::read(&tui);
            let key = format!("tui.alert.{name}");
            (notes.into_iter())
                .filter(|n| n.starts_with(&key))
                .collect::<Vec<_>>()
        };
        let typo = notes_of(
            "typo",
            AlertConfig {
                when: Some("ctx >= 80".into()),
                border: Some("bold".into()),
                mark: Some(true),
                bell: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(typo.len(), 1, "{typo:?}");
        assert!(typo[0].starts_with("tui.alert.typo.when: "), "{typo:?}");

        let words = |notify: &str| {
            notes_of(
                "sent",
                AlertConfig {
                    when: Some("sends".into()),
                    notify: Some(Notify::Says(notify.into())),
                    ..Default::default()
                },
            )
        };
        let said = words("$session sent");
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].starts_with("tui.alert.sent.notify: `$session` says nothing"),
            "{said:?}"
        );
        assert!(words("$alert ended").is_empty(), "no session named");
    }

    /// **What could never show is said** (review stop 9): a `border` on news,
    /// which a window drawing the session has seen — though `unshown == 0`,
    /// which holds there, is drawn (F5); a `say` no row writes (F6); and
    /// `notify = true` on a shipped alert is its own words, not its name in
    /// Shannon's (F7: "Shannon: unshown_session · b1 …").
    #[test]
    fn what_could_never_show_is_said() {
        use shannon_core::config::{AlertConfig, Notify, TuiConfig};
        let mut tui = TuiConfig::default();
        for (name, when) in [("news", "unshown"), ("calm", "unshown == 0")] {
            let alert = AlertConfig {
                when: Some(when.into()),
                border: Some("fg:red".into()),
                ..Default::default()
            };
            tui.alert.insert(name.into(), alert);
        }
        // Parked by `disabled`: its doing nothing is what you asked for.
        let parked = AlertConfig {
            when: Some("queued".into()),
            ..Default::default()
        };
        tui.alert.insert("parked".into(), parked);
        tui.alerts.disabled = vec!["parked".into()];
        let (alerts, notes) = Alerts::read(&tui);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("tui.alert.news.border: "), "{notes:?}");
        assert!(notes[0].contains("`mark`"), "and says what does: {notes:?}");
        let bordered = |name| alerts.list[alerts.index(name).unwrap()].border.is_some();
        assert!(!bordered("news") && bordered("calm"));

        let mut tui = TuiConfig::default();
        let status = &mut tui.status;
        for row in [&mut status.prompt, &mut status.output, &mut status.decision] {
            *row = "[$mode]".into();
        }
        status.graph = "[$mode]".into();
        status.page = "[$mode] ${alert.hidden}".into();
        let (_, notes) = Formats::read(&tui);
        assert_eq!(
            notes,
            [
                "tui.status: no row writes `$alerts`, nor `${alert.<name>}` for decide, \
                 queued, running, interrupt, unshown — not said"
            ]
        );

        let mut tui = TuiConfig::default();
        tui.alert.get_mut("unshown_session").unwrap().notify = Some(Notify::On(true));
        let (alerts, notes) = Alerts::read(&tui);
        assert!(notes.is_empty(), "{notes:?}");
        let words = (alerts.ringing())
            .find(|ringing| ringing.name() == "unshown_session")
            .and_then(|ringing| ringing.notify)
            .map(|words| words.say("unshown_session", Some("b1 x")));
        assert_eq!(words.as_deref(), Some("Shannon: news in b1 x"));
    }

    /// **What a note quotes of a config is shown as written** — a bidi
    /// override in an alert's name, a zero-width space in a `when`, as their
    /// escapes: drawn, a cell skips them, and the note named a key the file
    /// does not hold (review stop 9, E6). The rest of a note — `—`, `·`,
    /// quotes — as it is.
    #[test]
    fn a_notes_quotes_show_what_a_cell_would_hide() {
        use shannon_core::config::{AlertConfig, TuiConfig};
        let mut tui = TuiConfig::default();
        for (name, when) in [("ab\u{202e}", "queued"), ("zw", "queued\u{200b}")] {
            let alert = AlertConfig {
                when: Some(when.into()),
                ..Default::default()
            };
            tui.alert.insert(name.to_string(), alert);
        }
        let (_, notes) = Formats::read(&tui);
        for shown in [r"tui.alert.ab\u{202e}: ", r"queued\u{200b}"] {
            assert!(
                notes.iter().any(|n| n.contains(shown)),
                "{shown}: {notes:?}"
            );
        }
        assert!(
            notes.iter().all(|n| !n.contains(['\u{202e}', '\u{200b}'])),
            "{notes:?}"
        );
        assert_eq!(legible("`a` — \"b\" · c's"), "`a` — \"b\" · c's");
        // And every language as written (E2): `escape_debug` per character
        // escaped the combining accent and the Devanagari signs.
        for text in ["인증 수정", "修复认证", "cafe\u{301}", "नमस्ते"] {
            assert_eq!(legible(text), text);
        }
    }

    /// **An alert's `say` cannot draw an alert** — it would draw itself:
    /// `$alerts` and `${alert.<name>}` in one are said, and drawn as
    /// written.
    #[test]
    fn an_alert_never_draws_another() {
        let (say, notes) = parse("tui.alert.x.say", "$alerts ${alert.decide}", None);
        assert_eq!(notes.len(), 2, "{notes:?}");
        for said in ["`$alerts`", "`${alert.decide}`"] {
            assert!(notes.iter().any(|n| n.contains(said)), "{said}: {notes:?}");
        }
        assert_eq!(
            text(&say, &[("decide", "D")], 80),
            "$alerts ${alert.decide}"
        );
    }

    /// **A misspelt alert says what to write**, and is drawn as written: a
    /// name the row has no alert for, an alert written without its braces
    /// (a `.` is a name's only between them, so `$model.` ends a sentence).
    #[test]
    fn a_misspelt_alert_says_what_to_write() {
        let (format, notes) = read("t", "${alert.nope} ${alert.} $alert.decide");
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(
            notes[0].contains("`${alert.nope}` is not an alert"),
            "{notes:?}"
        );
        assert!(
            notes[1].contains("`${alert.}` is not an alert"),
            "{notes:?}"
        );
        assert!(notes[2].contains("`${alert.<name>}`"), "{notes:?}");
        assert_eq!(
            text(&format, &[("decide", "D")], 80),
            "${alert.nope} ${alert.} $alert.decide"
        );
        assert_eq!(row("$model.", &[("model", "m")], 80), "m.");
    }

    /// **`\` writes a special character itself**, and a `$` with no name
    /// after it is a dollar sign.
    #[test]
    fn escapes_and_lone_dollars_are_text() {
        assert_eq!(
            row(r"\$mode \{x\} \[y\](z) \\ $ 5 $", &[("mode", "N")], 80),
            r"$mode {x} [y](z) \ $ 5 $"
        );
        assert_eq!(row("${mode}x", &[("mode", "N")], 80), "Nx");
    }

    /// **A bracket inside a group closes with the group**, not past it: an
    /// unclosed `[` must not swallow the `}` that ends its group.
    #[test]
    fn an_open_bracket_ends_at_its_groups_close() {
        assert_eq!(row("{ [$mode }!", &[("mode", "N")], 80), " [N !");
        assert_eq!(
            row("{ [$mode }!", &[], 80),
            "!",
            "and the group still hides"
        );
    }

    /// **A row is never wider than its width** — a wide character where one
    /// cell is left is cut, not drawn into it: `漢a` in two cells drew
    /// `漢…`, three (review stop 9, B F7: 118 of 3240 of its draws overran).
    #[test]
    fn a_row_never_overruns_its_width() {
        for format in [
            "$mode",
            "{$mode} x",
            "[$mode](bold)$fill$mode",
            "a$mode",
            "漢$mode",
        ] {
            for value in ["漢a", "a漢", "漢漢漢", "ab", "🔔x"] {
                for width in 0..8u16 {
                    let drawn = row(format, &[("mode", value)], width);
                    let cells = crate::wrap::str_cells(&drawn);
                    assert!(
                        cells <= usize::from(width),
                        "{format} with {value} in {width}: {drawn:?}, {cells} cells"
                    );
                }
            }
        }
    }

    /// **A long format draws in time linear in its length** — the row is
    /// drawn every frame, and dropping groups for width re-summed the row at
    /// each drop while each `$alerts` walked the whole format once per
    /// alert: in a release build, 16 000 groups took 222 ms a frame and
    /// 8 000 `$alerts` 2.1 s (review stop 9, B F2). The bound is measured,
    /// in a debug build (10-23): both here take 24 ms; with the walk per
    /// alert back, 3.4 s; with the drop re-summing the row, 7.7 s.
    #[test]
    fn a_long_format_draws_in_linear_time() {
        let started = std::time::Instant::now();
        let (groups, _) = read("t", &"{a}".repeat(16_000));
        assert_eq!(
            text(&groups, &[], 80),
            "a".repeat(80),
            "the rightmost dropped"
        );
        let (alerts, _) = read("t", &"$alerts".repeat(8_000));
        assert_eq!(text(&alerts, &[], 80), "", "none holds");
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    }

    /// **And an open group inside a bracket ends at the bracket's close** —
    /// its mirror, two groups deep: both openers ask one list of what ends
    /// them (`Parser::nested`), and the `{` arm's own copy had dropped the
    /// bracket's `]` from it, so `[{{a](bold)` was drawn as written,
    /// unstyled (review stop 9, B for A).
    #[test]
    fn an_open_group_ends_at_its_brackets_close() {
        let (format, notes) = read("t", "[{{a](bold)");
        assert_eq!(notes.len(), 2, "two unclosed groups, said: {notes:?}");
        let drawn = draw(&format, &mut |_| Value::default(), 80);
        let text: String = drawn.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "{{a", "the bracket closed: {drawn:?}");
        assert!(
            (drawn.iter()).all(|(_, style)| style.add_modifier.contains(Modifier::BOLD)),
            "and styled: {drawn:?}"
        );
    }

    /// **Nesting is bounded** — reading a format, drawing it and dropping it
    /// each walk its nesting by recursion, and 20 000 levels overflowed the
    /// stack: an abort, leaving the terminal raw (review stop 9, B F1). Past
    /// [`DEEPEST`] a `{` or `[` is text, said once; up to it, a format.
    #[test]
    fn nesting_past_the_deepest_is_text_and_said() {
        let deep =
            |n: usize, open: &str, close: &str| format!("{}a{}", open.repeat(n), close.repeat(n));
        let (format, notes) = read("t", &deep(DEEPEST, "{", "}"));
        assert!(notes.is_empty(), "{DEEPEST} deep is a format: {notes:?}");
        assert_eq!(text(&format, &[], 80), "a");
        for (open, close) in [("{", "}"), ("[", "](bold)")] {
            let (format, notes) = read("t", &deep(20_000, open, close));
            let deep_notes = notes.iter().filter(|n| n.contains("deep")).count();
            assert_eq!(deep_notes, 1, "{open}: said once");
            let _ = text(&format, &[], 80);
        }
    }

    /// **The parser never panics, whatever it is given** — a format is the
    /// user's text, and it is drawn every frame.
    #[test]
    fn any_text_parses_and_renders() {
        let (alerts, _) = Alerts::shipped();
        let alphabet: Vec<char> = "$[]{}()\\ab: _.漢".chars().collect();
        // Every string of up to four from the alphabet (16^4 ≈ 65k), and
        // each of those after an alert's beginnings.
        let mut stack = vec![String::new()];
        while let Some(text) = stack.pop() {
            for text in [
                &text,
                &format!("${{alert.{text}"),
                &format!("$alerts{text}"),
            ] {
                let (format, _) = parse("t", text, Some(&alerts));
                for width in [0, 1, 3, 80] {
                    let mut value = |_| Value::from("v".to_string());
                    let _ = render(&format, &alerts, &mut value, Style::default(), width);
                }
            }
            if text.chars().count() < 4 {
                for c in &alphabet {
                    stack.push(format!("{text}{c}"));
                }
            }
        }
    }
}
