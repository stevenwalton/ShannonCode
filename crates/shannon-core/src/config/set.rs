//! **`:set`, vim's option command, over the config's dotted keys** — the
//! grammar and the typing. What it becomes is one layer on the stack
//! ([`ConfigStack::set`](super::ConfigStack::set)), so a `:set` is undone by
//! the next file that sets the same key, as vim's is.
//!
//! `:set tui.prompt.height=15` is a key and a value. The value is typed by
//! what the option already HOLDS — a number where it holds a number, a list
//! where it holds a list — so nothing is quoted, as nothing is in vim. Every
//! form vim has for an option is here (`:h :set`): `x?` shows it; a bare `x`
//! turns a boolean on and shows anything else; `nox` and `invx` / `x!` turn
//! one off and flip it; `x&` goes back to the default; `x=v`, `x+=v`, `x-=v`,
//! `x^=v` set, add, remove and prepend (for a number: add, subtract,
//! multiply; for a comma list, `tui.guicursor`, whole parts and their
//! commas, as vim's on `'guicursor'`).
//!
//! Whether a value FITS is not decided here: the stack deserializes the
//! result through the real schema, so a typo'd key or a word that is not one
//! of an option's values is refused with serde's own sentence, and this file
//! holds no second copy of what the options are. It is ASKED here once, of
//! that same schema (`fits`): where the type an option holds refuses a
//! value, which a key of two types must take (`notify`, a switch or words).

use super::{CREDENTIAL_KEYS, Config, key_is_under};

/// One `:set` argument, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `x?` — show the value.
    Query(String),
    /// A bare `x`: on, for a boolean; its value, for anything else. `nox` and
    /// `invx` arrive as this too — which they are depends on what `x` is.
    Bare(String),
    /// `x!` — flip a boolean.
    Toggle(String),
    /// `x&` — back to the default.
    Reset(String),
    /// `x=v` and its `+=`, `-=`, `^=`.
    Assign { key: String, op: Op, value: String },
}

/// Which assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `=`
    Set,
    /// `+=` — append to a list or a string; add to a number.
    Add,
    /// `-=` — remove from a list or a string; subtract from a number.
    Remove,
    /// `^=` — prepend to a list or a string; multiply a number.
    Prepend,
}

/// What a `:set` asks for: the options to show, and the settings to make —
/// the latter a sparse table, which is exactly what one layer is.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Keys to show, in the order given.
    pub show: Vec<String>,
    /// What to set; empty when the line only asked.
    pub table: toml::Table,
}

impl Plan {
    /// The dotted keys [`table`](Self::table) sets.
    pub fn set_keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        leaves(&self.table, "", &mut out);
        out
    }
}

/// Split a `:set` line into its arguments. Whitespace separates them; a
/// backslash keeps the next character (`\ ` is a space in a value, `\\` a
/// backslash), as in vim.
pub fn parse(args: &str) -> Result<Vec<Item>, String> {
    words(args).into_iter().map(|w| item(&w)).collect()
}

/// The words of a `:set` line, backslash escapes resolved. A backslash with
/// nothing after it is kept as itself rather than silently dropped.
fn words(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = args.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => word.push(chars.next().unwrap_or('\\')),
            c if c.is_whitespace() => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// One word as an [`Item`].
fn item(word: &str) -> Result<Item, String> {
    if let Some((left, value)) = word.split_once('=') {
        let (key, op, sign) = match left.char_indices().last() {
            Some((i, '+')) => (&left[..i], Op::Add, "+="),
            Some((i, '-')) => (&left[..i], Op::Remove, "-="),
            Some((i, '^')) => (&left[..i], Op::Prepend, "^="),
            _ => (left, Op::Set, "="),
        };
        return Ok(Item::Assign {
            key: named(key, sign)?,
            op,
            value: value.to_string(),
        });
    }
    Ok(match word.char_indices().last() {
        Some((i, '?')) => Item::Query(named(&word[..i], "?")?),
        Some((i, '!')) => Item::Toggle(named(&word[..i], "!")?),
        Some((i, '&')) => Item::Reset(named(&word[..i], "&")?),
        _ => Item::Bare(long_name(word)),
    })
}

/// A key as written, its short name spelled out ([`SHORT_NAMES`]), or why
/// there is none before `sign`.
///
/// **Names the sign, never the word**: the word is what was typed after it,
/// and `:set secrets.extra =pw` — one stray space — makes the password that
/// word, reaching the notice and the log before any credential check could
/// run (Reviewer B).
fn named(key: &str, sign: &str) -> Result<String, String> {
    if key.is_empty() {
        Err(format!(
            "no option name before `{sign}` — is there a space before it?"
        ))
    } else {
        Ok(long_name(key))
    }
}

/// **Short names**, as vim's `ts` is `tabstop`: an option reached for often
/// enough to deserve a word of its own. Spelled out as the line is read, so
/// everything after — the refusal, the typing, what is shown and set — sees
/// only the long name, and `:set effort?` answers `turn.effort=…` as `:set
/// ts?` answers `tabstop=8`.
const SHORT_NAMES: &[(&str, &str)] = &[("effort", "turn.effort"), ("memory", "memory.mode")];

/// `key`, or the long name it is short for.
fn long_name(key: &str) -> String {
    SHORT_NAMES
        .iter()
        .find(|(short, _)| *short == key)
        .map_or(key, |(_, long)| long)
        .to_string()
}

/// **Resolve `items` against the running config**: type each value by what
/// its option holds, and refuse what `:set` is not for. Nothing is applied.
///
/// **Left to right**, as vim applies a line: each setting is typed against
/// the one before it, so `:set x=10 x+=1` is 11 and `x! x!` leaves it as it
/// was (Reviewer A: it was typed against the line's start).
///
/// **One door for what `:set` may touch** (`refuse`): the key as written,
/// before anything is typed or quoted (typing `secrets.extra=[…` would quote
/// the value in its error), and the key it resolved to (`nox` resolves to
/// `x`), before it is shown or set. The second is a backstop no input
/// reaches today — no switch lives under a refused key — so a mutation of it
/// alone survives; it is there for the switch that someday does.
pub fn plan(items: Vec<Item>, current: &Config) -> Result<Plan, String> {
    let mut now = current.as_table();
    let defaults = Config::default().as_table();
    let mut plan = Plan::default();
    for item in items {
        refuse(item.key())?;
        let (key, value) = resolve(item, &now, &defaults)?;
        refuse(&key)?;
        match value {
            None => plan.show.push(key),
            Some(value) => {
                taught_not_set(&key)?;
                stored_not_set(&key)?;
                put(&mut now, &key, value.clone());
                put(&mut plan.table, &key, value);
            }
        }
    }
    Ok(plan)
}

/// **Rules are taught, not set.** The `[permissions]` lists STACK across
/// files (`stack.rs` `STACKED`), so a `:set` layer holding one would ADD to
/// them where `=` promises to replace — and rules have verbs of their own.
/// Shown (`:verbose set permissions.rules?` lists each file's), never set.
fn taught_not_set(key: &str) -> Result<(), String> {
    if key_is_under(key, "permissions") {
        return Err(
            "rules are taught with `:allow`, `:deny` and `:ask` (this session) or in \
                    your config's [permissions] — `:set` shows them, it does not set them"
                .into(),
        );
    }
    Ok(())
}

impl Item {
    /// The key as written — the word, for a bare one.
    fn key(&self) -> &str {
        match self {
            Item::Query(key)
            | Item::Bare(key)
            | Item::Toggle(key)
            | Item::Reset(key)
            | Item::Assign { key, .. } => key,
        }
    }
}

/// **One item's key and what it does**: `None` to show it, `Some` to set it
/// to that value. A SECTION is neither shown nor set whole — vim has none,
/// and a section's value is every key in it, credentials included (Reviewers
/// A and B: `:set secrets?` printed `extra`, and `secrets&` emptied it).
fn resolve(
    item: Item,
    now: &toml::Table,
    defaults: &toml::Table,
) -> Result<(String, Option<toml::Value>), String> {
    Ok(match item {
        Item::Query(key) => {
            setting(now, &key)?;
            (key, None)
        }
        Item::Bare(word) => match bare(now, &word)? {
            Bare::Show(key) => (key, None),
            Bare::To(key, on) => (key, Some(toml::Value::Boolean(on))),
        },
        Item::Toggle(key) => {
            let on = boolean(now, &key)?;
            (key, Some(toml::Value::Boolean(!on)))
        }
        Item::Reset(key) => {
            setting(now, &key)?;
            let Some(default) = get(defaults, &key) else {
                return Err(format!(
                    "`{key}` has no default to go back to — `:unsource` the file that set it"
                ));
            };
            let default = default.clone();
            (key, Some(default))
        }
        Item::Assign { key, op, value } => {
            let new = assigned(now, &key, op, &value)?;
            (key, Some(new))
        }
    })
}

/// The value at `key` if it is a setting — known, and not a section.
fn setting<'a>(now: &'a toml::Table, key: &str) -> Result<&'a toml::Value, String> {
    held(now, key)?;
    match get(now, key) {
        Some(toml::Value::Table(_)) if is_section(key) => Err(section(key)),
        Some(value) => Ok(value),
        None => Err(unknown(key)),
    }
}

/// **The options `:set` offers** — every setting, dotted, that is not a
/// section and not one `:set` refuses. What Tab completes.
pub fn option_names() -> Vec<String> {
    options(&Config::default())
}

/// **The setting one `:set` word names, read as `:set` reads it** — `x?`,
/// `x!`, `x&`, `x=v` and its `+=`/`-=`/`^=`, a short name spelled out, and
/// `nox`/`invx` as `x` where `x` is a setting or a section — so `:help`
/// lands on what `:set` would change (Steven, 10-19: `:help nosecrets` found
/// nothing). Lenient where `:set` is strict: a `no` before a section or a
/// number still names it, and a key the defaults do not hold (a
/// `[keymap.*]` entry) is returned as read, for the caller to find its
/// nearest section. `None` for a word that names no key at all.
pub fn option_of(word: &str) -> Option<String> {
    let word = word.trim().trim_matches('\'');
    let key = match item(word).ok()? {
        Item::Bare(word) => {
            let known = |key: &str| get(&Config::default().as_table(), key).is_some();
            let off = [word.strip_prefix("no"), word.strip_prefix("inv")];
            match known(&word) {
                true => word,
                false => off
                    .into_iter()
                    .flatten()
                    .find(|key| known(key))
                    .map_or(word.clone(), str::to_string),
            }
        }
        item => item.key().to_string(),
    };
    (!key.is_empty()).then_some(key)
}

/// Every option `config` holds that `:set` would show — what `:set all`
/// lists (a role named in a free-form table included).
pub fn options(config: &Config) -> Vec<String> {
    let mut out = Vec::new();
    leaves(&config.as_table(), "", &mut out);
    out.retain(|key| refuse(key).is_ok());
    out
}

/// The options that differ from their defaults — what a bare `:set` lists,
/// as vim's does.
pub fn changed_options(config: &Config) -> Vec<String> {
    Config::default()
        .changed_keys(config)
        .into_iter()
        .filter(|key| refuse(key).is_ok())
        .collect()
}

/// `key=value`, as `:set x?` shows it ([`spell`]).
pub fn shown(current: &Config, key: &str) -> String {
    match get(&current.as_table(), key) {
        Some(value) => format!("{key}={}", spell(value)),
        None => format!("{key} is not set"),
    }
}

/// **A value as `:set` writes it** — the spelling it would accept back: a
/// string bare, as vim shows one (an empty one as `""`, which would otherwise
/// read as nothing at all); anything else in TOML's spelling.
pub fn spell(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) if s.is_empty() => "\"\"".to_string(),
        toml::Value::String(s) => s.clone(),
        value => value.to_string(),
    }
}

/// The built-in default of `key`, if it has one.
pub fn default_of(key: &str) -> Option<toml::Value> {
    get(&Config::default().as_table(), key).cloned()
}

/// What a bare word turned out to be.
enum Bare {
    /// A non-boolean: show it.
    Show(String),
    /// A boolean, set on or off.
    To(String, bool),
}

/// A bare `x`, `nox` or `invx`, resolved the way vim does: by whether `x` is
/// a boolean option.
fn bare(now: &toml::Table, word: &str) -> Result<Bare, String> {
    match get(now, word) {
        Some(toml::Value::Boolean(_)) => return Ok(Bare::To(word.to_string(), true)),
        Some(toml::Value::Table(_)) if is_section(word) => return Err(section(word)),
        Some(_) => return Ok(Bare::Show(word.to_string())),
        None => {}
    }
    if let Some(key) = word.strip_prefix("no")
        && let Some(toml::Value::Boolean(_)) = get(now, key)
    {
        return Ok(Bare::To(key.to_string(), false));
    }
    if let Some(key) = word.strip_prefix("inv")
        && let Some(toml::Value::Boolean(on)) = get(now, key)
    {
        return Ok(Bare::To(key.to_string(), !on));
    }
    Err(unknown(word))
}

/// The boolean `key` holds, or why it is not one.
fn boolean(now: &toml::Table, key: &str) -> Result<bool, String> {
    match get(now, key) {
        Some(toml::Value::Boolean(on)) => Ok(*on),
        _ => {
            held(now, key)?;
            Err(format!("`{key}` is not on/off — `:set {key}=…`"))
        }
    }
}

/// The sentence for a value given to an on/off option.
fn on_off(key: &str) -> String {
    format!("`{key}` is on or off — `:set {key}` or `:set no{key}`")
}

/// **The value `key op raw` makes**: typed by what `key` holds now
/// ([`as_held`]) — and where that type refuses it, as written, if the
/// schema takes it there ([`fits`]). A key the schema reads as either of two
/// types holds one of them: `notify` is a switch or words, and typed by what
/// it held, `notify=hi` over `true` was refused as on-or-off (review stop 9,
/// A F3 and D4). Asked only where the held type refuses, so every other
/// option types, and refuses, as it did: `entropy_bits=4` is still `4.0`.
///
/// **A table written inline is asked first** (`{ shut_over = 3 }`): no
/// held type makes one, so typed as the string `tui.fold.thinking` held it
/// was the string "{shut_over=3}", refused in the spelling its refusal
/// offers (review stop 15, B M2). Asked only for a table, so a number over
/// a float (`entropy_bits=4`) still types as the float it holds; and only
/// over a setting that holds a value — a new key goes the free-form way, a
/// new alert made whole there (`alert_made_whole`).
fn assigned(now: &toml::Table, key: &str, op: Op, raw: &str) -> Result<toml::Value, String> {
    let written = literal(raw);
    let over_a_setting = get(now, key).is_some() && !is_section(key);
    if op == Op::Set && written.is_table() && over_a_setting && fits(now, key, &written) {
        return Ok(written);
    }
    as_held(now, key, op, raw).or_else(|why| match (op, get(now, key)) {
        (Op::Set, Some(_)) if !is_section(key) && fits(now, key, &written) => Ok(written),
        _ => Err(why),
    })
}

/// **Would the config take `value` at `key`?** — the schema's own answer,
/// asked of a copy of the config as it is: not a second list of what the
/// options are (the module's rule), the deserializer itself.
fn fits(now: &toml::Table, key: &str, value: &toml::Value) -> bool {
    let mut copy = now.clone();
    put(&mut copy, key, value.clone());
    toml::Value::Table(copy).try_into::<Config>().is_ok()
}

/// **The value `key op raw` makes, typed by what `key` holds now.**
fn as_held(now: &toml::Table, key: &str, op: Op, raw: &str) -> Result<toml::Value, String> {
    use toml::Value as V;
    let number = || format!("`{key}` takes a number, not `{raw}`");
    Ok(match (get(now, key), op) {
        (Some(V::Table(_)), _) if is_section(key) => return Err(section(key)),
        // A setting that holds a table (`{ shut_over = 40 }`): `=` sets what
        // is written, the schema its judge as for every value (the stack
        // deserializes it); there is no table to add to or take from.
        (Some(V::Table(_)), Op::Set) => literal(raw),
        (Some(V::Table(_)), _) => return Err(format!("`{key}` is set with `=`, as written")),
        (Some(V::Boolean(_)), Op::Set) => match raw {
            "true" => V::Boolean(true),
            "false" => V::Boolean(false),
            _ => return Err(on_off(key)),
        },
        (Some(V::Boolean(_)), _) => return Err(on_off(key)),
        (Some(V::Integer(n)), op) => {
            let v: i64 = raw.parse().map_err(|_| number())?;
            let out = match op {
                Op::Set => Some(v),
                Op::Add => n.checked_add(v),
                Op::Remove => n.checked_sub(v),
                Op::Prepend => n.checked_mul(v),
            };
            V::Integer(out.ok_or_else(|| format!("`{key}` would be out of range"))?)
        }
        (Some(V::Float(x)), op) => {
            let v: f64 = raw.parse().map_err(|_| number())?;
            V::Float(match op {
                Op::Set => v,
                Op::Add => x + v,
                Op::Remove => x - v,
                Op::Prepend => x * v,
            })
        }
        (Some(V::String(s)), op) => V::String(match op {
            // `""` is how an empty string is SHOWN (`spell`), so it must read
            // back as empty — else `:set tui.model=""` stored two quote marks
            // as a model id (Reviewer A).
            Op::Set if raw == "\"\"" => String::new(),
            Op::Set => raw.to_string(),
            _ if COMMA_LISTS.contains(&key) => comma_list(s, op, raw),
            Op::Add => format!("{s}{raw}"),
            Op::Remove => s.replacen(raw, "", 1),
            Op::Prepend => format!("{raw}{s}"),
        }),
        (Some(V::Array(have)), op) => {
            let items = list(raw)?;
            V::Array(match op {
                Op::Set => items,
                Op::Add => {
                    let mut out = have.clone();
                    out.extend(items.into_iter().filter(|i| !have.contains(i)));
                    out
                }
                Op::Remove => have
                    .iter()
                    .filter(|i| !items.contains(i))
                    .cloned()
                    .collect(),
                Op::Prepend => {
                    let mut out: Vec<V> = items.into_iter().filter(|i| !have.contains(i)).collect();
                    out.extend(have.iter().cloned());
                    out
                }
            })
        }
        (Some(V::Datetime(_)), _) => return Err(format!("`{key}` cannot be set here")),
        // Not in the running config: a free-form table's key (`[tui.graph_glyphs]`
        // is empty until you name a role). Allowed only inside a section — the
        // schema then decides — and only as a plain `=`.
        (None, Op::Set) => {
            let written = literal(raw);
            alert_made_whole(now, key, Some(&written))?;
            room_for(now, key)?;
            written
        }
        (None, _) => {
            alert_made_whole(now, key, None)?;
            room_for(now, key)?;
            return Err(format!("`{key}` is not set — `:set {key}=…` first"));
        }
    })
}

/// **The strings that are comma lists** — written as vim writes the option
/// they are (`'guicursor'` is one), so vim's `:set x+=part` must do what it
/// does there (`comma_list`). Every entry a string option (test).
const COMMA_LISTS: [&str; 1] = ["tui.guicursor"];

/// **`op` on a comma list, by whole parts, as vim's** (`:h :set+=`; nvim
/// 0.12.6, measured 11-02): `+=` and `^=` add `raw`'s parts with a comma —
/// none onto an empty value, nothing when they are there already; `-=` takes
/// them out with their comma, and leaves a value without them as it is.
/// Joined by bytes, `+=a:blinkon500` would end the last part's argument
/// (`hor20a:blinkon500`), refused for a reason the user never wrote.
fn comma_list(s: &str, op: Op, raw: &str) -> String {
    let have: Vec<&str> = match s {
        "" => Vec::new(),
        s => s.split(',').collect(),
    };
    let parts: Vec<&str> = raw.split(',').collect();
    let at = have.windows(parts.len()).position(|run| run == parts);
    match (op, at) {
        (Op::Set, _) => raw.to_string(),
        (Op::Remove, Some(i)) => {
            let mut kept = have;
            kept.drain(i..i + parts.len());
            kept.join(",")
        }
        (Op::Add, None) if !raw.is_empty() => [have, parts].concat().join(","),
        (Op::Prepend, None) if !raw.is_empty() => [parts, have].concat().join(","),
        _ => s.to_string(),
    }
}

/// **A new alert is made whole, or not at all** (review stop 9, A F5 and
/// F4; Steven, 10-24, of this middle way: "build it out and we'll test") —
/// `:set tui.alert.<name>={when=…, …}` makes one, as `[tui.alert.<name>]`
/// in a file does. A name with no alert under it takes nothing else: not a
/// dotted key, not a table with no `when`, because that is what a misspelt
/// name looks like — `unshown_sesion={bell=false}` made a dead alert and
/// said it was set, while the real bell rang on. `Ok` for a key outside
/// `[tui.alert]`, or under an alert that exists: the rest of `:set`'s rules
/// decide it. `written` is the value, `None` for an op that is not `=`.
fn alert_made_whole(
    now: &toml::Table,
    key: &str,
    written: Option<&toml::Value>,
) -> Result<(), String> {
    let Some(rest) = key.strip_prefix("tui.alert.") else {
        return Ok(());
    };
    let (name, inner) = match rest.split_once('.') {
        Some((name, inner)) => (name, Some(inner)),
        None => (rest, None),
    };
    if get(now, &format!("tui.alert.{name}")).is_some() {
        return Ok(());
    }
    let whole = format!("`:set tui.alert.{name}={{when=\"…\", …}}`");
    match (inner, written) {
        (None, Some(toml::Value::Table(alert))) if alert.contains_key("when") => Ok(()),
        (None, _) => Err(format!(
            "a new alert needs its `when` — {whole}: when it holds"
        )),
        // Not "`<Tab>` lists them": completion offers the shipped alerts
        // alone (`option_names` reads the defaults).
        (Some(_), _) => Err(format!(
            "no alert `{name}` — a new one is made whole: {whole}"
        )),
    }
}

/// A list value: TOML's own `[…]`, or vim's comma list of words.
fn list(raw: &str) -> Result<Vec<toml::Value>, String> {
    if raw.trim_start().starts_with('[') {
        return match literal(raw) {
            toml::Value::Array(items) => Ok(items),
            _ => Err(format!("`{raw}` is not a list")),
        };
    }
    Ok(raw
        .split(',')
        .filter(|w| !w.is_empty())
        .map(|w| toml::Value::String(w.to_string()))
        .collect())
}

/// `raw` as a TOML value if it is one (`42`, `true`, `["a"]`), else as the
/// string it spells — vim does not quote strings, and neither does this.
fn literal(raw: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("v = {raw}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(raw.to_string()))
}

/// **What `:set` will not touch, and where to go instead.**
///
/// - A credential key ([`CREDENTIAL_KEYS`]): typed here, the secret lands in
///   command history, the notice and the log. Your config file is its place.
/// - `[keymap]`: a chord can hold a dot (`.` is a key), which a dotted name
///   cannot carry.
///
/// **In both directions**: a key under one of them, and a section holding
/// one. Checking only the first let `:set secrets?` print `extra`, and
/// `:set secrets&` empty it — silently ending the literal-secret scrub, and
/// with `:verbose`, logging the value the scrub had just stopped covering
/// (Reviewers A and B, 2026-09-29).
fn refuse(key: &str) -> Result<(), String> {
    if let Some(credential) = CREDENTIAL_KEYS
        .iter()
        .find(|p| key_is_under(key, p) || key_is_under(p, key))
    {
        return Err(format!(
            "`{credential}` holds credentials — set it in your config file (`:config edit`), \
             where it is not echoed, logged or kept in history"
        ));
    }
    if key_is_under(key, "keymap") || key_is_under("keymap", key) {
        return Err("keys are mapped in your config file for now (`:config edit`)".into());
    }
    Ok(())
}

/// **Settings that re-store data on disk when they change** — so a change
/// must last, and `:set`'s, gone when you quit, must not be one. Shown, as
/// any setting is (`:set secrets.password_protect?`, `:set all`); never set.
/// `secrets.password_protect` seals or unseals every kept secret (Steven,
/// 10-23: "Yes" to config-file only): `069099a` let `:set` move them, and a
/// relaunch looked where they no longer were.
const STORAGE_KEYS: &[&str] = &["secrets.password_protect"];

/// `key` is not one of [`STORAGE_KEYS`] — checked on every SET, as
/// [`taught_not_set`] is (a section is never set whole, so the key itself is
/// the one to ask about).
fn stored_not_set(key: &str) -> Result<(), String> {
    match STORAGE_KEYS.iter().find(|p| key_is_under(key, p)) {
        Some(stored) => Err(format!(
            "`{stored}` decides how your kept secrets are stored on disk — set it in your \
             config file (`:config edit`), where the change lasts"
        )),
        None => Ok(()),
    }
}

/// `Ok` when `key` names a setting; otherwise vim's "unknown option".
///
/// Complete because the config serializes every setting `:set` handles, an
/// empty list or map included (`default_toml_is_complete` holds
/// `DEFAULT_TOML` to it) — a setting skipped when empty would be unknown
/// here until first set. The two still skipped, `[[alias]]` and `[keymap]`,
/// are refused before this is asked.
fn held(now: &toml::Table, key: &str) -> Result<(), String> {
    get(now, key).map(|_| ()).ok_or_else(|| unknown(key))
}

/// Vim's E518, in words.
fn unknown(key: &str) -> String {
    format!("unknown option: `{key}` — `:set all` lists them")
}

/// `Ok` when `key` could be ADDED: its direct parent is a section. For a
/// free-form one (`[tui.graph_glyphs]`, empty until you name a role) that
/// is the whole answer; for any other, the schema then refuses a name it
/// does not have, with serde's list of the ones it does.
fn room_for(now: &toml::Table, key: &str) -> Result<(), String> {
    match key.rsplit_once('.').map(|(parent, _)| get(now, parent)) {
        Some(Some(toml::Value::Table(_))) => Ok(()),
        _ => held(now, key),
    }
}

/// The sentence for a key that names a section.
fn section(key: &str) -> String {
    format!("`{key}` is a section — set one of its keys (`:set {key}.<Tab>`)")
}

/// **Whether `key` names a section** — a table in the DEFAULT config, not
/// in what it holds now: a setting whose value may be a table (`[tui.fold]
/// thinking = { shut_over = 30 }`) is a setting whatever it holds, and read
/// by what it held, a file's `{ shut_over = 40 }` made `?`, `=` and `&` on it
/// say "is a section" (review stop 15, B M2).
fn is_section(key: &str) -> bool {
    matches!(default_of(key), Some(toml::Value::Table(_)))
}

/// The value at a dotted `key`, if there is one.
pub(super) fn get<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let (head, rest) = match key.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (key, None),
    };
    match (table.get(head)?, rest) {
        (value, None) => Some(value),
        (toml::Value::Table(inner), Some(rest)) => get(inner, rest),
        _ => None,
    }
}

/// Set a dotted `key` in `table`, making the sections on the way.
pub(super) fn put(table: &mut toml::Table, key: &str, value: toml::Value) {
    match key.split_once('.') {
        None => {
            table.insert(key.to_string(), value);
        }
        Some((head, rest)) => {
            let inner = table
                .entry(head.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !inner.is_table() {
                *inner = toml::Value::Table(toml::Table::new());
            }
            if let toml::Value::Table(inner) = inner {
                put(inner, rest, value);
            }
        }
    }
}

/// **Every non-table value in `table`, as a dotted key** — what a table of
/// settings names (`:config save`'s report).
pub fn leaf_paths(table: &toml::Table) -> Vec<String> {
    let mut out = Vec::new();
    leaves(table, "", &mut out);
    out
}

/// Every non-table value in `table`, as dotted keys.
fn leaves(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) => leaves(inner, &path, out),
            _ => out.push(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value as V;

    /// Plan one line against the defaults.
    fn planned(line: &str) -> Result<Plan, String> {
        plan(parse(line)?, &Config::default())
    }

    /// The value one line would set at `key`.
    fn value(line: &str, key: &str) -> V {
        let plan = planned(line).unwrap_or_else(|e| panic!("`{line}` refused: {e}"));
        get(&plan.table, key)
            .unwrap_or_else(|| panic!("`{line}` set no `{key}`: {plan:?}"))
            .clone()
    }

    /// **A setting that re-stores secrets on disk is shown, never set** —
    /// every form that sets (Steven, 10-23: config file only), with the way
    /// to change it; `?` and `:set all` still show it.
    #[test]
    fn password_protect_is_shown_but_never_set() {
        for line in [
            "secrets.password_protect",
            "nosecrets.password_protect",
            "invsecrets.password_protect",
            "secrets.password_protect!",
            "secrets.password_protect&",
            "secrets.password_protect=true",
            "secrets.password_protect=false",
        ] {
            let why = planned(line).expect_err(line);
            assert!(why.contains(":config edit"), "{line}: {why}");
        }
        let shown = planned("secrets.password_protect?").expect("shown");
        assert_eq!(shown.show, ["secrets.password_protect"]);
        assert!(
            option_names()
                .iter()
                .any(|n| n == "secrets.password_protect")
        );
    }

    /// Every form `:h :set` has, and vim's backslash for a space in a value.
    #[test]
    fn every_form_vim_has_parses() {
        let items = parse("a? b c! d& e=1 f+=2 g-=x h^=y i=two\\ words").unwrap();
        let assign = |key: &str, op, value: &str| Item::Assign {
            key: key.into(),
            op,
            value: value.into(),
        };
        assert_eq!(
            items,
            [
                Item::Query("a".into()),
                Item::Bare("b".into()),
                Item::Toggle("c".into()),
                Item::Reset("d".into()),
                assign("e", Op::Set, "1"),
                assign("f", Op::Add, "2"),
                assign("g", Op::Remove, "x"),
                assign("h", Op::Prepend, "y"),
                assign("i", Op::Set, "two words"),
            ]
        );
        assert!(parse("=3").is_err(), "a value with no option");
        assert!(parse("?").is_err());
    }

    /// **The value is typed by what the option holds** — nothing is quoted,
    /// as nothing is in vim.
    #[test]
    fn a_value_takes_the_type_its_option_holds() {
        assert_eq!(
            value("tui.prompt.height=15", "tui.prompt.height"),
            V::Integer(15)
        );
        assert_eq!(
            value("tui.model=opus", "tui.model"),
            V::String("opus".into())
        );
        assert_eq!(
            value("secrets.entropy_bits=4.25", "secrets.entropy_bits"),
            V::Float(4.25)
        );
        assert_eq!(
            value("secrets.names=a,b", "secrets.names"),
            V::Array(vec![V::String("a".into()), V::String("b".into())])
        );
        assert_eq!(
            value("secrets.names=[\"a\"]", "secrets.names"),
            V::Array(vec![V::String("a".into())]),
            "TOML's own list spelling works too"
        );
        assert_eq!(
            value("tui.graph_glyphs.child=◇", "tui.graph_glyphs.child"),
            V::String("◇".into()),
            "a key of a free-form table that is empty until named"
        );
    }

    /// Booleans: a bare name turns one on, `no` off, `inv` and `!` flip it —
    /// and a bare name that is NOT a boolean shows it instead of setting it.
    #[test]
    fn booleans_go_on_off_and_flip() {
        let mouse = Config::default().tui.mouse;
        assert_eq!(value("tui.mouse", "tui.mouse"), V::Boolean(true));
        assert_eq!(value("notui.mouse", "tui.mouse"), V::Boolean(false));
        assert_eq!(value("invtui.mouse", "tui.mouse"), V::Boolean(!mouse));
        assert_eq!(value("tui.mouse!", "tui.mouse"), V::Boolean(!mouse));
        assert_eq!(value("tui.mouse=false", "tui.mouse"), V::Boolean(false));

        let plan = planned("tui.scrolloff").unwrap();
        assert_eq!(plan.show, ["tui.scrolloff"], "shown");
        assert!(plan.table.is_empty(), "and not set: {plan:?}");

        // From OFF: `no` keeps it off (it is not a flip), `inv` turns it on.
        let mut off = Config::default();
        off.tui.mouse = false;
        let at = |line: &str| get(&plan_on(line, &off).table, "tui.mouse").cloned();
        assert_eq!(at("notui.mouse"), Some(V::Boolean(false)));
        assert_eq!(at("invtui.mouse"), Some(V::Boolean(true)));
        assert_eq!(at("tui.mouse!"), Some(V::Boolean(true)));
    }

    /// Plan one line against `now`.
    fn plan_on(line: &str, now: &Config) -> Plan {
        plan(parse(line).unwrap(), now).unwrap()
    }

    /// **A setting whose value may be a table is set as written, and read
    /// as a setting while it holds one** — `[tui.fold] thinking = {
    /// shut_over = 30 }` (review stop 15, B M2): typed as the string it held,
    /// `{shut_over=3}` became the string "{shut_over=3}" and was refused in
    /// the very spelling the refusal offers; holding a table, it was called a
    /// section, so `?`, `=` and `&` all refused it. A section is still one.
    #[test]
    fn a_setting_whose_value_is_a_table_is_set_and_read_as_one() {
        let table = |n| {
            let mut t = toml::Table::new();
            t.insert("shut_over".into(), V::Integer(n));
            V::Table(t)
        };
        let key = "tui.fold.thinking";
        assert_eq!(value(&format!("{key}={{shut_over=3}}"), key), table(3));
        let now = Config::parse("[tui.fold]\ntool = { shut_over = 40 }\n").unwrap();
        let at = |line: &str| get(&plan_on(line, &now).table, "tui.fold.tool").cloned();
        assert_eq!(at("tui.fold.tool=open"), Some(V::String("open".into())));
        assert_eq!(at("tui.fold.tool={shut_over=5}"), Some(table(5)));
        assert_eq!(at("tui.fold.tool&"), Some(V::String("shut".into())));
        assert_eq!(plan_on("tui.fold.tool?", &now).show, ["tui.fold.tool"]);
        assert!(shown(&now, "tui.fold.tool").contains("shut_over = 40"));
        let why = planned("tui.fold=open").expect_err("a section");
        assert!(why.contains("is a section"), "{why}");
    }

    /// **A key of two types takes either** — `notify` is a switch or words:
    /// typed by what it held, words over `true` were refused as on-or-off
    /// (review stop 9, A F3 and D4). The schema is asked only where the held
    /// type refuses, so an option of one type types and refuses as it did.
    #[test]
    fn a_key_of_two_types_takes_either() {
        let now = Config::parse("[tui.alert.x]\nwhen = \"queued\"\nnotify = true\n").unwrap();
        let at = |line: &str| get(&plan_on(line, &now).table, "tui.alert.x.notify").cloned();
        assert_eq!(at("tui.alert.x.notify=hi"), Some(V::String("hi".into())));
        assert_eq!(at("tui.alert.x.notify=false"), Some(V::Boolean(false)));
        let why = planned("tui.mouse=yes").expect_err("a switch takes no word");
        assert!(why.contains("is on or off"), "{why}");
        // Only `=` asks, and never of a section — each would fit the schema,
        // and is refused as it was: `+=` on a switch, a section set whole.
        let why = planned("tui.mouse+=true").expect_err("`+=` on a switch");
        assert!(why.contains("is on or off"), "{why}");
        let why = planned("tui.alerts={bell=false}").expect_err("a section");
        assert!(why.contains("is a section"), "{why}");
        assert_eq!(
            value("secrets.entropy_bits=4", "secrets.entropy_bits"),
            V::Float(4.0),
            "taken as held, where it can be"
        );
    }

    /// **A new alert is made whole, or not at all** (review stop 9, A F5
    /// and F4) — `{when=…}` makes one; under a name with no alert, a dotted
    /// key or a table with no `when` is refused with how, the misspelt-name
    /// shape that made a dead alert and said it was set; an alert that
    /// exists, shipped or yours, takes a key of its own either way.
    #[test]
    fn a_new_alert_is_made_whole_or_not_at_all() {
        assert_eq!(
            value(
                "tui.alert.zz={when=\"queued\",say=\"ZZQ\"}",
                "tui.alert.zz.when"
            ),
            V::String("queued".into()),
            "made whole"
        );
        let why = planned("tui.alert.unshown_sesion={bell=false}").expect_err("no `when`");
        assert!(why.starts_with("a new alert needs its `when`"), "{why}");
        let why = planned("tui.alert.yy.when=queued").expect_err("dotted, no alert");
        assert!(
            why.starts_with("no alert `yy`") && why.contains("tui.alert.yy={when="),
            "{why}"
        );
        let why = planned("tui.alert.yy.bell+=true").expect_err("not `=`, no alert");
        assert!(why.starts_with("no alert `yy`"), "{why}");
        // Shipped, and yours.
        assert_eq!(
            value(
                "tui.alert.unshown_session.bell=false",
                "tui.alert.unshown_session.bell"
            ),
            V::Boolean(false)
        );
        let now = Config::parse("[tui.alert.zz]\nwhen = \"queued\"\nsay = \"ZZQ\"\n").unwrap();
        let bell = plan_on("tui.alert.zz.bell=true", &now);
        assert_eq!(
            get(&bell.table, "tui.alert.zz.bell"),
            Some(&V::Boolean(true))
        );
    }

    /// `+=`, `-=`, `^=` are vim's: add, subtract, multiply a number; append,
    /// remove, prepend on a list (never a duplicate) and on a string.
    #[test]
    fn add_remove_and_prepend_are_vims() {
        let s = Config::default().tui.scrolloff as i64;
        assert_eq!(
            value("tui.scrolloff+=2", "tui.scrolloff"),
            V::Integer(s + 2)
        );
        assert_eq!(
            value("tui.scrolloff-=2", "tui.scrolloff"),
            V::Integer(s - 2)
        );
        assert_eq!(
            value("tui.scrolloff^=2", "tui.scrolloff"),
            V::Integer(s * 2)
        );

        let mut c = Config::default();
        c.secrets.names = vec!["alpha".into(), "beta".into()];
        let list = |line: &str| -> Vec<String> {
            let plan = plan(parse(line).unwrap(), &c).unwrap();
            let Some(V::Array(items)) = get(&plan.table, "secrets.names") else {
                panic!("`{line}` set no list: {plan:?}");
            };
            items
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(list("secrets.names+=zzz"), ["alpha", "beta", "zzz"]);
        assert_eq!(
            list("secrets.names+=alpha"),
            ["alpha", "beta"],
            "never twice"
        );
        assert_eq!(list("secrets.names-=alpha"), ["beta"]);
        assert_eq!(list("secrets.names^=zzz"), ["zzz", "alpha", "beta"]);
        assert_eq!(
            list("secrets.names+=x,y"),
            ["alpha", "beta", "x", "y"],
            "a comma list adds each"
        );

        assert_eq!(
            value("tui.model+=x", "tui.model"),
            V::String(format!("{}x", Config::default().tui.model))
        );
    }

    /// **A comma list takes whole parts, as vim's `'guicursor'` does** —
    /// each row nvim 0.12.6's answer to the same `:set` (11-02). Joined by
    /// bytes, the first was `n:blocka:blinkon500`, which `tui.guicursor`
    /// refuses: vim's `:set guicursor+=a:blinkon0` idiom could not be typed.
    #[test]
    fn a_comma_list_takes_whole_parts_as_vims_does() {
        let rows = [
            ("n:block", "+=a:blinkon500", "n:block,a:blinkon500"),
            (
                "n:block,a:blinkon500",
                "+=a:blinkon500",
                "n:block,a:blinkon500",
            ),
            ("n:block,a:blinkon500", "-=a:blinkon500", "n:block"),
            ("n:block", "^=v:ver25", "v:ver25,n:block"),
            ("", "+=i:ver25", "i:ver25"),
            ("n:block,i:ver25", "-=n:block", "i:ver25"),
            (
                "n:block",
                "+=a:blinkon0,i:ver25",
                "n:block,a:blinkon0,i:ver25",
            ),
            ("n:block,i:ver25", "^=i:ver25", "n:block,i:ver25"),
            ("n:block,i:ver25", "-=v:ver25", "n:block,i:ver25"),
            ("n:block,i:ver25,r:hor20", "-=i:ver25", "n:block,r:hor20"),
            ("n:block,i:ver25,r:hor20", "-=i:ver25,r:hor20", "n:block"),
            (
                "n:block,i:ver25,r:hor20",
                "+=i:ver25,r:hor20",
                "n:block,i:ver25,r:hor20",
            ),
            ("n:block,i:ver25", "-=n:block,i:ver25", ""),
            ("n:block", "+=", "n:block"),
            ("n:block", "^=", "n:block"),
            ("n:block", "-=", "n:block"),
        ];
        for (now, op, want) in rows {
            let mut c = Config::default();
            c.tui.guicursor = now.into();
            let line = format!("tui.guicursor{op}");
            let got = get(&plan_on(&line, &c).table, "tui.guicursor").cloned();
            assert_eq!(got, Some(V::String(want.into())), "{now:?} then {op}");
        }
        for key in COMMA_LISTS {
            assert!(
                matches!(default_of(key), Some(V::String(_))),
                "`{key}` is no string option"
            );
        }
    }

    /// The operator rows the first test leaves out: a string's `-=` and `^=`,
    /// a float's three, a list's `^=` (never a duplicate either way), the
    /// empty words a comma list drops, and a number pushed past `i64`.
    #[test]
    fn every_operator_on_every_type() {
        let mut c = Config::default();
        c.tui.model = "claude-opus".into();
        c.secrets.names = vec!["alpha".into(), "beta".into()];
        let on = |line: &str, key: &str| get(&plan_on(line, &c).table, key).cloned().unwrap();
        let strings = |v: V| -> Vec<String> {
            let V::Array(items) = v else {
                panic!("not a list: {v:?}")
            };
            items
                .iter()
                .map(|i| i.as_str().unwrap().to_string())
                .collect()
        };

        assert_eq!(
            on("tui.model-=opus", "tui.model"),
            V::String("claude-".into())
        );
        assert_eq!(
            on("tui.model-=zzz", "tui.model"),
            V::String("claude-opus".into())
        );
        assert_eq!(
            on("tui.model^=x-", "tui.model"),
            V::String("x-claude-opus".into())
        );

        let bits = c.secrets.entropy_bits;
        assert_eq!(
            on("secrets.entropy_bits+=0.5", "secrets.entropy_bits"),
            V::Float(bits + 0.5)
        );
        assert_eq!(
            on("secrets.entropy_bits-=0.5", "secrets.entropy_bits"),
            V::Float(bits - 0.5)
        );
        assert_eq!(
            on("secrets.entropy_bits^=2", "secrets.entropy_bits"),
            V::Float(bits * 2.0)
        );

        assert_eq!(
            strings(on("secrets.names^=beta,zzz", "secrets.names")),
            ["zzz", "alpha", "beta"],
            "`^=` adds only what is missing, in front"
        );
        assert_eq!(
            strings(on("secrets.names=a,,b", "secrets.names")),
            ["a", "b"]
        );
        assert!(
            strings(on("secrets.names=", "secrets.names")).is_empty(),
            "`x=` empties a list"
        );

        let max = i64::MAX.to_string();
        let min = i64::MIN.to_string();
        for line in [
            format!("tui.scrolloff+={max}"),
            format!("tui.scrolloff-={min}"),
            format!("tui.scrolloff^={max}"),
        ] {
            let why = plan(parse(&line).unwrap(), &c).expect_err(&line);
            assert!(why.contains("out of range"), "`{line}`: {why}");
        }
    }

    /// `x&` is the default — whatever the running config says.
    #[test]
    fn reset_is_the_default() {
        let mut changed = Config::default();
        changed.tui.scrolloff = 40;
        let plan = plan(parse("tui.scrolloff&").unwrap(), &changed).unwrap();
        assert_eq!(
            get(&plan.table, "tui.scrolloff"),
            Some(&V::Integer(Config::default().tui.scrolloff as i64))
        );
    }

    /// **What `:set` will not do**, each with a sentence that says where to
    /// go instead: credentials (typed here they would be echoed, logged and
    /// kept in history), keymaps, whole sections, names nothing has, and a
    /// value its option cannot hold.
    #[test]
    fn what_set_refuses_it_says_why() {
        for (line, why) in [
            ("secrets.extra=hunter2-long", "credentials"),
            // Refused BEFORE typing: typing it would quote it ("`[…` is not
            // a list").
            ("secrets.extra=[c4n4ry7", "credentials"),
            ("secrets.extra?", "credentials"),
            ("secrets.extra", "credentials"),
            ("secrets.extra&", "credentials"),
            // The section that HOLDS one, in every form (Reviewers A and B).
            ("secrets?", "credentials"),
            ("secrets", "credentials"),
            ("secrets&", "credentials"),
            ("keymap?", "config file"),
            // No section is shown or reset whole.
            ("tui?", "is a section"),
            ("tui", "is a section"),
            ("tui&", "is a section"),
            ("tui.prompt?", "is a section"),
            ("tui.graph_glyphs&", "is a section"),
            ("bogus&", "unknown option"),
            // Rules stack across files, so `=` would add: taught, not set.
            ("permissions.rules=x", "taught"),
            ("permissions.remove+=x", "taught"),
            ("permissions.rules&", "taught"),
            ("alias=x", "credentials"),
            ("keymap.normal.Q=submit", "config file"),
            ("tui=1", "is a section"),
            ("scrolloff=1", "unknown option"),
            ("bogus+=1", "unknown option"),
            ("tui.scrolloff+", "unknown option"),
            ("tui.mouse=yes", "on or off"),
            ("tui.mouse+=1", "on or off"),
            ("tui.scrolloff=many", "takes a number"),
            ("tui.scrolloff!", "not on/off"),
            ("nosecrets.names", "unknown option"),
        ] {
            match planned(line) {
                Err(e) => assert!(e.contains(why), "`{line}`: {e}"),
                Ok(plan) => panic!("`{line}` was accepted: {plan:?}"),
            }
        }
    }

    /// **Left to right**, as vim applies a line: each item is typed against
    /// the one before it (Reviewer A: `x=10 x+=1` gave the default plus one).
    #[test]
    fn a_line_applies_left_to_right() {
        assert_eq!(
            value("tui.scrolloff=10 tui.scrolloff+=1", "tui.scrolloff"),
            V::Integer(11)
        );
        let mouse = Config::default().tui.mouse;
        assert_eq!(
            value("tui.mouse! tui.mouse!", "tui.mouse"),
            V::Boolean(mouse),
            "flipped twice is as it was"
        );
    }

    /// What `x?` shows is what `x=` takes back: `""` is an empty string, not
    /// two quote marks (Reviewer A: they became a model id). A trailing
    /// backslash is kept, not dropped.
    #[test]
    fn what_is_shown_reads_back() {
        let mut c = Config::default();
        c.tui.model = "opus".into();
        let plan = plan(parse("tui.model=\"\"").unwrap(), &c).unwrap();
        assert_eq!(
            get(&plan.table, "tui.model"),
            Some(&V::String(String::new()))
        );
        assert_eq!(value("tui.model=a\\", "tui.model"), V::String("a\\".into()));
    }

    /// **A malformed word never quotes what was typed** — after a stray space,
    /// that is the value: `:set secrets.extra =pw` (Reviewer B).
    #[test]
    fn a_word_with_no_option_is_not_quoted() {
        for line in ["secrets.extra =c4n4ry7", "+=c4n4ry7", "?", "c4n4ry7 ^=x"] {
            let Err(why) = parse(line) else {
                panic!("`{line}` parsed");
            };
            assert!(!why.contains("c4n4ry7"), "`{line}`: {why}");
            assert!(why.contains("no option name"), "`{line}`: {why}");
        }
    }

    /// What Tab offers: settings, not the sections that hold them, and none
    /// that `:set` would refuse.
    #[test]
    fn the_options_offered_are_the_settable_ones() {
        // Rules are shown, never set: a query plans, an assignment does not.
        let plan = planned("permissions.rules?").expect("a rule list can be shown");
        assert_eq!(plan.show, ["permissions.rules"]);
        let names = option_names();
        assert!(names.iter().any(|n| n == "tui.prompt.height"), "{names:?}");
        assert!(names.iter().any(|n| n == "secrets.names"));
        assert!(!names.iter().any(|n| n == "tui" || n == "tui.prompt"));
        assert!(!names.iter().any(|n| key_is_under(n, "secrets.extra")));
        for name in &names {
            assert!(
                planned(&format!("{name}?")).is_ok(),
                "offered but not queryable: {name}"
            );
        }
    }

    /// `x?` shows a string bare, as vim shows one, and anything else in TOML.
    #[test]
    fn shown_is_key_equals_value() {
        let c = Config::default();
        assert_eq!(
            shown(&c, "tui.scrolloff"),
            format!("tui.scrolloff={}", c.tui.scrolloff)
        );
        assert_eq!(
            shown(&c, "tui.model"),
            "tui.model=\"\"",
            "an empty string is visible"
        );
        let mut c = c;
        c.tui.model = "opus".into();
        assert_eq!(shown(&c, "tui.model"), "tui.model=opus", "no quotes");
    }

    /// **A short name is its long one in every form** — set, shown, reset —
    /// and each names an option `:set` offers, so a renamed key cannot leave
    /// a short name pointing at nothing.
    #[test]
    fn a_short_name_is_its_long_one() {
        for (short, long) in SHORT_NAMES {
            assert!(option_names().iter().any(|n| n == long), "{short} → {long}");
            assert_eq!(planned(&format!("{short}?")).unwrap().show, [*long]);
            assert!(get(&planned(&format!("{short}&")).unwrap().table, long).is_some());
        }
        assert_eq!(value("effort=max", "turn.effort"), V::String("max".into()));
        assert_eq!(
            planned("effort").unwrap().show,
            ["turn.effort"],
            "bare shows"
        );
        // A word that only STARTS like a short name is not one.
        assert!(planned("efforts=max").is_err());

        // `:set memory=` is off and `on` is auto (TODO (40), DECIDED): each
        // spelling plans, and lands as the mode it names.
        for (line, mode) in [
            ("memory=", crate::MemoryMode::Off),
            ("memory=off", crate::MemoryMode::Off),
            ("memory=on", crate::MemoryMode::Auto),
            ("memory=auto", crate::MemoryMode::Auto),
            ("memory=ask", crate::MemoryMode::Ask),
        ] {
            let table = planned(line)
                .unwrap_or_else(|e| panic!("{line}: {e}"))
                .table;
            let config: Config = toml::Value::Table(table).try_into().expect(line);
            assert_eq!(config.memory.mode, mode, "{line}");
        }
        // Any other word is refused where `:set` refuses a value: read alone
        // as a config (`ConfigStack::set`).
        let table = planned("memory=sometimes").unwrap().table;
        let alone: Result<Config, _> = toml::Value::Table(table).try_into();
        let why = alone.expect_err("not a mode").to_string();
        assert!(why.contains("not a memory mode"), "{why}");
    }
}
