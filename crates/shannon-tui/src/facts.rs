//! **What an alert's `when` asks** (`decision-panes-and-alerts.md` §5.2):
//! facts — numbers the app knows, each of a session or of the whole app —
//! and the trigger that compares one with a whole number.
//!
//! Facts are not status values ([`Var`](crate::status_line::Var)): a value
//! is text to show (`$model`, `$queued`'s `2 queued`), a fact a number to
//! test (`queued`'s `2`). Each has its own name table, and the manual lists
//! each from its type.
//!
//! **Whose a fact is, is its type** ([`SessionFact`], [`AppFact`]): one of a
//! session cannot be asked without naming the session, and the row asks it
//! of the session it is about — the one your prompt goes to (§5.2: "a
//! per-session alert holds for each session where its fact does").

/// Declares one scope's facts once — each with the name a `when` writes —
/// so `ALL` lists every variant by construction.
macro_rules! facts {
    ($(#[$kind_doc:meta])* $kind:ident { $($(#[$doc:meta])* $variant:ident => $name:literal,)* }) => {
        $(#[$kind_doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub(crate) enum $kind {
            $($(#[$doc])* $variant,)*
        }

        impl $kind {
            /// Every fact of this scope, in declaration order.
            pub(crate) const ALL: &'static [$kind] = &[$($kind::$variant,)*];

            /// The name a `when` writes.
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $($kind::$variant => $name,)*
                }
            }
        }
    };
}

facts! {
    /// **A fact of one session** — asked of each session it could hold for.
    SessionFact {
        /// Decisions waiting on you in the session: its calls, and its
        /// summary's review.
        Decision => "decision",
        /// 1 while the session's summary waits for your review.
        Review => "review",
        /// How full the session's context window is, in percent: 0 before
        /// its first answer.
        CtxPercent => "ctx_percent",
        /// Prompts queued for the session.
        Queued => "queued",
        /// The session's allowed commands, running.
        Running => "running",
        /// 1 while the session's reply streams.
        Answering => "answering",
        /// 1 once something has arrived in its transcript since a pane last
        /// drew it and nothing runs in it — a reply, a turn ending, a
        /// summary landing, where no window shows it — until one does; what
        /// you did there is not news (`App::unshown`).
        Unshown => "unshown",
    }
}

impl SessionFact {
    /// **Is it 0 wherever a window draws the session?** — `unshown` is, a
    /// window drawing the session having seen it that frame; so a `border`,
    /// drawn on those windows, shows only for a `when` that holds at 0
    /// (review stop 9, F5: `border` on `unshown_session` drew nowhere and
    /// said nothing). Each fact answers, so a new one decides.
    pub(crate) const fn zero_where_drawn(self) -> bool {
        match self {
            SessionFact::Unshown => true,
            SessionFact::Decision
            | SessionFact::Review
            | SessionFact::CtxPercent
            | SessionFact::Queued
            | SessionFact::Running
            | SessionFact::Answering => false,
        }
    }
}

facts! {
    /// **A fact of the whole app**.
    AppFact {
        /// Sessions with work of their own running: a turn, a distillation,
        /// a continuation.
        Sessions => "sessions",
        /// Child sessions running or held.
        Children => "children",
        /// `:Send` sessions answering or held.
        Sends => "sends",
        /// Sessions whose transcripts changed where no pane drew them
        /// ([`SessionFact::Unshown`], counted).
        UnshownCount => "unshown_count",
    }
}

/// **A fact, with whose it is.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Fact {
    Session(SessionFact),
    App(AppFact),
}

impl Fact {
    /// Every fact: each session's, then the app's.
    pub(crate) fn all() -> impl Iterator<Item = Fact> {
        let session = SessionFact::ALL.iter().map(|&f| Fact::Session(f));
        session.chain(AppFact::ALL.iter().map(|&f| Fact::App(f)))
    }

    /// The name a `when` writes.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Fact::Session(fact) => fact.name(),
            Fact::App(fact) => fact.name(),
        }
    }

    /// The fact `name` writes, if it is one.
    fn from_name(name: &str) -> Option<Fact> {
        Fact::all().find(|fact| fact.name() == name)
    }
}

/// How a trigger compares its fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Test {
    /// The fact alone: it holds when it is not 0.
    NotZero,
    Above(u64),
    AtLeast(u64),
    Exactly(u64),
    Below(u64),
    AtMost(u64),
}

/// **An alert's `when`, read**: a fact, or a fact compared with a whole
/// number — `queued`, `ctx_percent >= 80`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Trigger {
    fact: Fact,
    test: Test,
}

impl Trigger {
    /// **Read a `when`**, or say what to write instead — the words a
    /// trigger is most likely mistyped as each named (DR-B #18: `ctx >= 80`
    /// gets the fact's name, `and`/`or` gets "two alerts").
    pub(crate) fn read(text: &str) -> Result<Trigger, String> {
        let text = text.trim();
        let comparisons = "compare with `>` `>=` `==` `<` `<=` and a whole number";
        if text.is_empty() {
            return Err(
                "it is empty: a `when` names a fact, `queued`, or compares one, \
                        `ctx_percent >= 80`"
                    .to_string(),
            );
        }
        let joined = (text.split_whitespace())
            .any(|word| matches!(word.to_ascii_lowercase().as_str(), "and" | "or"))
            || text.contains("&&")
            || text.contains("||");
        if joined {
            return Err(format!(
                "`{text}`: an alert holds on one fact — for either of two, write two alerts"
            ));
        }
        let name_end =
            (text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))).unwrap_or(text.len());
        let (name, rest) = text.split_at(name_end);
        if name.is_empty() {
            return Err(format!("`{text}` names no fact: a `when` begins with one"));
        }
        let Some(fact) = Fact::from_name(name) else {
            return Err(match near(name) {
                Some(fact) => format!("`{name}` is not a fact: the fact is `{}`", fact.name()),
                None => format!("`{name}` is not a fact (`:help alerts` lists them)"),
            });
        };
        let rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(Trigger {
                fact,
                test: Test::NotZero,
            });
        }
        let op_end = (rest.find(|c: char| !"<>=!".contains(c))).unwrap_or(rest.len());
        let (op, number) = rest.split_at(op_end);
        let number = number.trim();
        let test: fn(u64) -> Test = match op {
            ">" => Test::Above,
            ">=" => Test::AtLeast,
            "==" => Test::Exactly,
            "<" => Test::Below,
            "<=" => Test::AtMost,
            "" => return Err(format!("`{text}`: {comparisons}, as `ctx_percent >= 80`")),
            "=" => return Err("`=` sets: `==` compares".to_string()),
            "=>" => return Err("`=>` is written `>=`".to_string()),
            "=<" => return Err("`=<` is written `<=`".to_string()),
            "!=" | "<>" => {
                return Err(format!(
                    "`{op}` is not a comparison here: `{name}` alone holds when it is not \
                     0, `{name} == 0` when it is"
                ));
            }
            op => return Err(format!("`{op}` is not a comparison: {comparisons}")),
        };
        if number.is_empty() {
            return Err(format!("`{op}` needs a whole number after it"));
        }
        match number.parse::<u64>() {
            Ok(n) => Ok(Trigger {
                fact,
                test: test(n),
            }),
            // A percent fact is a number of percent: `80%` is `80`.
            Err(_) => Err(match number.strip_suffix('%').map(str::parse::<u64>) {
                Some(Ok(n)) => format!("`{number}` is written `{n}`: the fact is a number"),
                _ => format!("`{number}` is not a whole number"),
            }),
        }
    }

    /// The fact it asks.
    pub(crate) fn fact(self) -> Fact {
        self.fact
    }

    /// **Does it hold** when its fact is `value`?
    pub(crate) fn holds(self, value: u64) -> bool {
        match self.test {
            Test::NotZero => value != 0,
            Test::Above(n) => value > n,
            Test::AtLeast(n) => value >= n,
            Test::Exactly(n) => value == n,
            Test::Below(n) => value < n,
            Test::AtMost(n) => value <= n,
        }
    }
}

/// The fact a mistyped `name` most likely meant: one it begins, or that
/// begins it — `ctx` for `ctx_percent` — or the one sharing the longest
/// beginning, of four letters or more (`decide` for `decision`).
fn near(name: &str) -> Option<Fact> {
    let shared = |fact: &Fact| {
        let common = (fact.name().chars().zip(name.chars()))
            .take_while(|(a, b)| a == b)
            .count();
        let whole = common == name.len().min(fact.name().len());
        (whole, common)
    };
    Fact::all()
        .map(|fact| (shared(&fact), fact))
        .filter(|&((whole, common), _)| (whole && common > 0) || common >= 4)
        .max_by_key(|&(rank, _)| rank)
        .map(|(_, fact)| fact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Trigger {
        Trigger::read(text).unwrap_or_else(|why| panic!("{text}: {why}"))
    }

    /// **A fact alone holds when it is not 0; a comparison as it reads** —
    /// each of the five, at either side of its number.
    #[test]
    fn a_trigger_holds_as_it_reads() {
        let queued = read("queued");
        assert_eq!(queued.fact(), Fact::Session(SessionFact::Queued));
        assert!(!queued.holds(0));
        assert!(queued.holds(1));
        for (text, holds) in [
            ("ctx_percent > 80", [false, false, true]),
            ("ctx_percent >= 80", [false, true, true]),
            ("ctx_percent == 80", [false, true, false]),
            ("ctx_percent < 80", [true, false, false]),
            ("ctx_percent <= 80", [true, true, false]),
            ("  ctx_percent>=80  ", [false, true, true]),
        ] {
            let trigger = read(text);
            assert_eq!(trigger.fact(), Fact::Session(SessionFact::CtxPercent));
            assert_eq!([79, 80, 81].map(|v| trigger.holds(v)), holds, "{text}");
        }
        assert_eq!(read("sessions >= 2").fact(), Fact::App(AppFact::Sessions));
    }

    /// **A `when` it cannot read says what to write instead** — the mistakes
    /// one is likeliest to be: a value's name for a fact's, two conditions,
    /// a sign that is not a comparison, a number that is not whole.
    #[test]
    fn a_broken_when_says_what_to_write() {
        for (text, says) in [
            ("", "names a fact"),
            ("ctx >= 80", "the fact is `ctx_percent`"),
            ("decide", "the fact is `decision`"),
            ("hidden", "`:help alerts` lists them"),
            ("queued and running", "write two alerts"),
            ("queued || running", "write two alerts"),
            ("queued = 2", "`==` compares"),
            ("queued => 2", "written `>=`"),
            ("queued =< 2", "written `<=`"),
            ("queued != 0", "`queued` alone holds when it is not 0"),
            ("queued >", "needs a whole number"),
            ("ctx_percent >= 80%", "written `80`"),
            ("ctx_percent >= 0.8", "`0.8` is not a whole number"),
            ("ctx_percent >= x", "`x` is not a whole number"),
            ("ctx_percent 80", "compare with"),
            (">= 80", "names no fact"),
            ("ctx_percent >>= 80", "`>>=` is not a comparison"),
        ] {
            let why = Trigger::read(text).expect_err(text);
            assert!(why.contains(says), "{text}: {why}");
        }
    }

    /// **Facts are named in `[a-z0-9_]`**, uniquely across both scopes — a
    /// name two facts shared would read as whichever came first.
    #[test]
    fn every_fact_has_its_own_name() {
        let names: Vec<&str> = Fact::all().map(Fact::name).collect();
        for (i, name) in names.iter().enumerate() {
            assert!(!names[..i].contains(name), "{name} twice");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{name}"
            );
            assert_eq!(Fact::from_name(name).map(Fact::name), Some(*name));
        }
    }

    /// **Reading a `when` never panics**, whatever it is given — it is the
    /// user's text.
    #[test]
    fn any_when_reads() {
        let alphabet: Vec<char> = "ab_<>=!& 9.%漢".chars().collect();
        let mut stack = vec![String::new()];
        while let Some(text) = stack.pop() {
            for text in [text.clone(), format!("queued{text}")] {
                let _ = Trigger::read(&text);
            }
            if text.chars().count() < 4 {
                for c in &alphabet {
                    stack.push(format!("{text}{c}"));
                }
            }
        }
    }
}
