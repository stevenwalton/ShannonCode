//! **The model file**: TOML, one program per file, read into a [`Model`].
//!
//! The shape, chosen so a `--help` page transcribes into it line by line
//! (design §8.3; the vocabulary follows usage's `cmd`/`flag`/`arg`, so a
//! `clap_usage` export imports near-mechanically):
//!
//! ```toml
//! name = "gh"
//! effect = "read"                  # read | write | delete | exec; the root's
//! trust = "verified"               # completion | generated | author | verified
//! hosts = ["api.github.com"]       # reached without being named
//! external = true                  # an unknown first word runs gh-<word>
//! family = { bundling = true, interspersed = true, double_dash = "terminator" }
//! examples.match = ["gh pr list --limit 5"]
//! flag = [                         # the root's flags
//!   { names = ["-R", "--repo"], arity = 1 },
//! ]
//!
//! [command.api]                    # a subcommand; a chain is "pr view"
//! effect = "read"
//! endpoints = { user = "the authenticated user's profile" }
//! positional = [{ at = 0, role = "endpoint" }]
//!
//! [[command.api.flag]]
//! names = ["-X", "--method"]
//! arity = 1
//! role = "enum"
//!
//! [[command.api.flag]]
//! names = ["-f", "--raw-field"]
//! arity = 1
//! role = "kv"
//! implies = { method = "POST" }
//! ```
//!
//! **Every key is checked.** An unknown key is refused (`deny_unknown_
//! fields`): a misspelt `effcet` would otherwise be a model that silently
//! says nothing. A flag's `canon` defaults to its first long spelling
//! without dashes (`--line-number` → `line-number`), else its first
//! spelling (`-n` → `n`, `-delete` → `delete`), which is what a rule names.
//! Cross-references (`unless`, `read_only_with`, `editor_unless`,
//! `implies`, a switch's `when`) must name a flag the node or an ancestor
//! has: a model cannot refer to a flag it did not describe.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{
    Arity, At, DoubleDash, Effect, Examples, Family, Flag, Model, Node, Positional, Role, Source,
    Switch, Trust,
};

/// Read one model from its file's text. `source` says where it came from,
/// for the pane and `:rules`.
pub fn parse(text: &str, source: Source) -> Result<Model, String> {
    let file: File = toml::from_str(text).map_err(|e| describe(&e))?;
    build(file, source)
}

/// `toml`'s error, one line, without the source excerpt (a user's model
/// file is theirs; the message names the key and the line).
fn describe(e: &toml::de::Error) -> String {
    let msg = e.message().to_string();
    match e.span() {
        Some(span) => format!("{msg} (at byte {})", span.start),
        None => msg,
    }
}

// ---- the file's shape --------------------------------------------------------

/// The file: the program's facts and its root's flags and operands as
/// top-level keys, then one `[command.<chain>]` table per subcommand. (A
/// TOML key after a table header belongs to that table, so the root's
/// `flag = [...]` must come before the first `[command.…]`; the program's
/// facts sit beside it rather than in a `[program]` table of their own.)
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    name: String,
    effect: EffectName,
    trust: TrustName,
    implementation: Option<String>,
    hosts: Vec<String>,
    reads_here: ReadsHereSpec,
    argv_from_stdin: bool,
    operators: Vec<String>,
    placeholder: Option<String>,
    privilege: bool,
    external: bool,
    unknown_flag_why: Option<String>,
    kv_programs: Vec<String>,
    kv_safe: Vec<String>,
    aliases: Vec<String>,
    language: Option<String>,
    family: FamilySpec,
    switch: Vec<SwitchSpec>,
    examples: ExamplesSpec,
    flag: Vec<FlagSpec>,
    positional: Vec<PositionalSpec>,
    command: BTreeMap<String, CommandSpec>,
}

impl Default for File {
    fn default() -> Self {
        File {
            name: String::new(),
            effect: EffectName::Read,
            trust: TrustName::Author,
            implementation: None,
            hosts: Vec::new(),
            reads_here: ReadsHereSpec::Flag(false),
            argv_from_stdin: false,
            operators: Vec::new(),
            placeholder: None,
            privilege: false,
            external: false,
            unknown_flag_why: None,
            kv_programs: Vec::new(),
            kv_safe: Vec::new(),
            aliases: Vec::new(),
            language: None,
            family: FamilySpec::default(),
            switch: Vec::new(),
            examples: ExamplesSpec::default(),
            flag: Vec::new(),
            positional: Vec::new(),
            command: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FamilySpec {
    bundling: bool,
    single_dash_long: bool,
    long_abbrev: bool,
    interspersed: bool,
    double_dash: DoubleDashName,
    numeric_option: Option<String>,
    dash_data: bool,
}

impl Default for FamilySpec {
    fn default() -> Self {
        let d = Family::default();
        FamilySpec {
            bundling: d.bundling,
            single_dash_long: d.single_dash_long,
            long_abbrev: d.long_abbrev,
            interspersed: d.interspersed,
            double_dash: DoubleDashName::Terminator,
            numeric_option: d.numeric_option,
            dash_data: d.dash_data,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchSpec {
    when: String,
    from: RoleName,
    to: RoleName,
    /// The flag whose values switch; absent, the positionals.
    on: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ExamplesSpec {
    #[serde(rename = "match")]
    matches: Vec<String>,
    not_match: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct CommandSpec {
    effect: Option<EffectName>,
    reads_here: Option<ReadsHereSpec>,
    aliases: Vec<String>,
    external: bool,
    read_only_with: Vec<String>,
    editor_unless: Option<Vec<String>>,
    prints_secret: bool,
    writes: Vec<String>,
    endpoints: BTreeMap<String, String>,
    flag: Vec<FlagSpec>,
    positional: Vec<PositionalSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlagSpec {
    names: Vec<String>,
    canon: Option<String>,
    #[serde(default)]
    arity: AritySpec,
    #[serde(default)]
    role: RoleName,
    effect: Option<EffectName>,
    implies: Option<BTreeMap<String, String>>,
    #[serde(default = "yes")]
    benign: bool,
    why: Option<String>,
    #[serde(default)]
    or_attached: bool,
    #[serde(default)]
    chdir: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PositionalSpec {
    at: AtSpec,
    #[serde(default)]
    role: RoleName,
    #[serde(default)]
    unless: Vec<String>,
}

/// `arity = 0 | 1 | ["write_file", "data"] | { until = [";", "+"] } |
/// "optional_attached"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum AritySpec {
    Count(u8),
    Roles(Vec<RoleName>),
    Until { until: Vec<String> },
    Word(String),
}

impl Default for AritySpec {
    fn default() -> Self {
        AritySpec::Count(0)
    }
}

/// `at = 0 | "rest" | "last"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum AtSpec {
    Index(usize),
    Word(String),
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
enum EffectName {
    #[default]
    Read,
    Write,
    Delete,
    Exec,
}

impl From<EffectName> for Effect {
    fn from(e: EffectName) -> Effect {
        match e {
            EffectName::Read => Effect::Read,
            EffectName::Write => Effect::Write,
            EffectName::Delete => Effect::Delete,
            EffectName::Exec => Effect::Exec,
        }
    }
}

/// `reads_here`: `true` (searched, as `dir`), `false`, or the role — `"dir"`
/// or `"list"` (Steven, 10-11: `reads_here = "list"`).
#[derive(Deserialize, Clone, Copy)]
#[serde(untagged)]
enum ReadsHereSpec {
    Flag(bool),
    Role(RoleName),
}

impl Default for ReadsHereSpec {
    fn default() -> Self {
        ReadsHereSpec::Flag(false)
    }
}

impl ReadsHereSpec {
    fn role(self, where_: &str) -> Result<Option<Role>, String> {
        match self {
            ReadsHereSpec::Flag(false) => Ok(None),
            ReadsHereSpec::Flag(true) => Ok(Some(Role::Dir)),
            ReadsHereSpec::Role(r) => match Role::from(r) {
                role @ (Role::Dir | Role::List) => Ok(Some(role)),
                other => Err(format!(
                    "{where_}: `reads_here = \"{}\"` — where it runs is read as `dir` \
                     (searched) or `list` (listed)",
                    other.name()
                )),
            },
        }
    }
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
enum RoleName {
    #[default]
    Data,
    Pattern,
    Enum,
    Number,
    ReadFile,
    WriteFile,
    DeleteFile,
    Dir,
    List,
    ExecFile,
    Program,
    Code,
    Url,
    Host,
    Endpoint,
    Kv,
    Argv,
    IncludeGlob,
}

impl From<RoleName> for Role {
    fn from(r: RoleName) -> Role {
        match r {
            RoleName::Data => Role::Data,
            RoleName::Pattern => Role::Pattern,
            RoleName::Enum => Role::Enum,
            RoleName::Number => Role::Number,
            RoleName::ReadFile => Role::ReadFile,
            RoleName::WriteFile => Role::WriteFile,
            RoleName::DeleteFile => Role::DeleteFile,
            RoleName::Dir => Role::Dir,
            RoleName::List => Role::List,
            RoleName::ExecFile => Role::ExecFile,
            RoleName::Program => Role::Program,
            RoleName::Code => Role::Code,
            RoleName::Url => Role::Url,
            RoleName::Host => Role::Host,
            RoleName::Endpoint => Role::Endpoint,
            RoleName::Kv => Role::Kv,
            RoleName::Argv => Role::Argv,
            RoleName::IncludeGlob => Role::IncludeGlob,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum TrustName {
    Completion,
    Generated,
    Author,
    Verified,
}

impl From<TrustName> for Trust {
    fn from(t: TrustName) -> Trust {
        match t {
            TrustName::Completion => Trust::Completion,
            TrustName::Generated => Trust::Generated,
            TrustName::Author => Trust::Author,
            TrustName::Verified => Trust::Verified,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum DoubleDashName {
    Terminator,
    Ignored,
    InnerArgv,
}

impl From<DoubleDashName> for DoubleDash {
    fn from(d: DoubleDashName) -> DoubleDash {
        match d {
            DoubleDashName::Terminator => DoubleDash::Terminator,
            DoubleDashName::Ignored => DoubleDash::Ignored,
            DoubleDashName::InnerArgv => DoubleDash::InnerArgv,
        }
    }
}

// ---- file → model -------------------------------------------------------------

fn build(file: File, source: Source) -> Result<Model, String> {
    let p = file;
    if p.name.trim().is_empty() {
        return Err("`name` is required".into());
    }
    if p.name.contains(['/', ' ']) {
        return Err(format!(
            "`name = \"{}\"`: a basename, no `/` and no space",
            super::clip(&p.name)
        ));
    }
    // Bounded before anything is built from them: every walker of the tree
    // recurses by its depth, and every example is read at every load.
    if let Some(key) = p
        .command
        .keys()
        .find(|k| k.split_whitespace().count() > super::MOST_CHAIN_WORDS)
    {
        return Err(format!(
            "[command.\"{}\"]: a chain of {} words, and a model nests at most {}",
            super::clip(key),
            key.split_whitespace().count(),
            super::MOST_CHAIN_WORDS
        ));
    }
    let examples = p.examples.matches.len() + p.examples.not_match.len();
    if examples > super::MOST_EXAMPLES {
        return Err(format!(
            "{examples} examples, and a model carries at most {}",
            super::MOST_EXAMPLES
        ));
    }
    if let Some(ex) = p
        .examples
        .matches
        .iter()
        .chain(&p.examples.not_match)
        .find(|ex| ex.len() > super::MOST_EXAMPLE_BYTES)
    {
        return Err(format!(
            "example `{}`: {} bytes, and an example is at most {}",
            super::clip(ex),
            ex.len(),
            super::MOST_EXAMPLE_BYTES
        ));
    }
    let mut root = Node {
        name: p.name.clone(),
        aliases: p.aliases,
        flags: flags(&p.flag, "the program")?,
        positionals: positionals(&p.positional, "the program")?,
        subs: Vec::new(),
        effect: p.effect.into(),
        external: p.external,
        read_only_with: Vec::new(),
        editor_unless: None,
        prints_secret: false,
        writes: Vec::new(),
        endpoints: BTreeMap::new(),
        reads_here: p.reads_here.role("the program")?,
    };
    // Subcommands: a key is the chain below the program (`pr view`); the
    // nodes along it that no table declares are made as the parent's shadow
    // (its effect), so `[command."pr view"]` alone still reads `gh pr view`.
    // Shorter chains first, so a declared parent is in place before its
    // children.
    let mut chains: Vec<(Vec<String>, CommandSpec)> = p
        .command
        .into_iter()
        .map(|(key, spec)| (key.split_whitespace().map(String::from).collect(), spec))
        .collect();
    chains.sort_by_key(|(chain, _)| chain.len());
    for (chain, spec) in chains {
        if chain.is_empty() {
            return Err("[command.\"\"]: an empty chain names no subcommand".into());
        }
        let where_ = format!("command `{}`", chain.join(" "));
        let node = node_at(&mut root, &chain);
        node.effect = spec.effect.map_or(node.effect, Effect::from);
        if let Some(here) = spec.reads_here {
            node.reads_here = here.role(&where_)?;
        }
        node.aliases = spec.aliases;
        node.external = spec.external;
        node.read_only_with = spec.read_only_with;
        node.editor_unless = spec.editor_unless;
        node.prints_secret = spec.prints_secret;
        node.writes = spec.writes;
        node.endpoints = spec.endpoints;
        node.flags = flags(&spec.flag, &where_)?;
        node.positionals = positionals(&spec.positional, &where_)?;
    }
    let switches = p
        .switch
        .into_iter()
        .map(|s| Switch {
            when: s.when,
            from: s.from.into(),
            to: s.to.into(),
            on: s.on,
        })
        .collect::<Vec<_>>();
    // A flag whose presence turns a value into a write, a delete or a run
    // raises the program as that value would: `tar -c` makes the archive
    // `-f` names a file written. Derived here, not left to the model's
    // author, so the flag never rides unnamed on an allow taught without
    // it (Steven, 10-10: `allow tar a` from `tar -xf a` covered `tar -cf
    // a`), and every reader of a flag's effect sees it.
    for s in &switches {
        raise_flag(&mut root, &s.when, s.to.effect());
    }
    let model = Model {
        name: p.name,
        root,
        family: Family {
            bundling: p.family.bundling,
            single_dash_long: p.family.single_dash_long,
            long_abbrev: p.family.long_abbrev,
            interspersed: p.family.interspersed,
            double_dash: p.family.double_dash.into(),
            numeric_option: p.family.numeric_option,
            dash_data: p.family.dash_data,
        },
        implementation: p.implementation,
        trust: p.trust.into(),
        hosts: p.hosts,
        argv_from_stdin: p.argv_from_stdin,
        operators: p.operators,
        placeholder: p.placeholder,
        privilege: p.privilege,
        switches,
        kv_programs: p.kv_programs,
        kv_safe: p.kv_safe,
        unknown_flag_why: p.unknown_flag_why,
        language: p.language,
        examples: Examples {
            matches: p.examples.matches,
            not_matches: p.examples.not_match,
        },
        source,
    };
    check(&model)?;
    Ok(model)
}

/// Raise every flag named `canon` in the tree to at least `effect`; a read
/// raises nothing.
fn raise_flag(node: &mut Node, canon: &str, effect: Effect) {
    if effect <= Effect::Read {
        return;
    }
    for f in node.flags.iter_mut().filter(|f| f.canon == canon) {
        f.effect = Some(f.effect.map_or(effect, |e| e.max(effect)));
    }
    for s in &mut node.subs {
        raise_flag(s, canon, effect);
    }
}

/// The node at `chain` under `root`, made along the way as the parent's
/// shadow where none was declared.
fn node_at<'a>(root: &'a mut Node, chain: &[String]) -> &'a mut Node {
    let mut node = root;
    for word in chain {
        let at = match node.subs.iter().position(|s| &s.name == word) {
            Some(at) => at,
            None => {
                node.subs.push(Node {
                    name: word.clone(),
                    aliases: Vec::new(),
                    flags: Vec::new(),
                    positionals: Vec::new(),
                    subs: Vec::new(),
                    effect: node.effect,
                    external: false,
                    read_only_with: Vec::new(),
                    editor_unless: None,
                    prints_secret: false,
                    writes: Vec::new(),
                    endpoints: BTreeMap::new(),
                    reads_here: node.reads_here,
                });
                node.subs.len() - 1
            }
        };
        node = &mut node.subs[at];
    }
    node
}

fn flags(specs: &[FlagSpec], where_: &str) -> Result<Vec<Flag>, String> {
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        if spec.names.is_empty() {
            return Err(format!("{where_}: a flag with no names"));
        }
        for name in &spec.names {
            if !name.starts_with('-') || name == "-" || name == "--" {
                return Err(format!(
                    "{where_}: flag `{name}` must begin with a dash and be more than one"
                ));
            }
            if name.contains(['=', ' ']) {
                return Err(format!(
                    "{where_}: flag `{name}` holds `=` or a space; a value is not part of a name"
                ));
            }
        }
        let canon = match &spec.canon {
            Some(c) if !c.is_empty() => c.clone(),
            _ => canon_of(&spec.names),
        };
        let arity = match &spec.arity {
            AritySpec::Count(0) => Arity::Zero,
            AritySpec::Count(1) => Arity::One,
            AritySpec::Count(n) => {
                return Err(format!(
                    "{where_}: flag `{}`: arity {n} — write the roles of each word instead",
                    spec.names[0]
                ));
            }
            AritySpec::Roles(roles) if roles.is_empty() => {
                return Err(format!(
                    "{where_}: flag `{}`: an empty arity list",
                    spec.names[0]
                ));
            }
            AritySpec::Roles(roles) => Arity::N(roles.iter().map(|r| Role::from(*r)).collect()),
            AritySpec::Until { until } if until.is_empty() => {
                return Err(format!(
                    "{where_}: flag `{}`: `until` names no terminator",
                    spec.names[0]
                ));
            }
            AritySpec::Until { until } => Arity::Until(until.clone()),
            AritySpec::Word(w) if w == "optional_attached" => Arity::OptionalAttached,
            AritySpec::Word(w) => {
                return Err(format!(
                    "{where_}: flag `{}`: arity `{w}` is not 0, 1, a list of roles, {{ until = [...] }} or \"optional_attached\"",
                    spec.names[0]
                ));
            }
        };
        let implies = match &spec.implies {
            None => None,
            Some(map) if map.len() == 1 => map.iter().next().map(|(k, v)| (k.clone(), v.clone())),
            Some(_) => {
                return Err(format!(
                    "{where_}: flag `{}`: `implies` names one fact, as {{ method = \"POST\" }}",
                    spec.names[0]
                ));
            }
        };
        if !spec.benign && spec.why.is_none() {
            return Err(format!(
                "{where_}: flag `{}`: a guarded flag (`benign = false`) says `why`",
                spec.names[0]
            ));
        }
        out.push(Flag {
            names: spec.names.clone(),
            canon,
            arity,
            role: spec.role.into(),
            effect: spec.effect.map(Effect::from),
            implies,
            benign: spec.benign,
            why: spec.why.clone(),
            or_attached: spec.or_attached,
            chdir: spec.chdir,
        });
    }
    Ok(out)
}

/// The canonical name a flag gets when its file gives none: the first long
/// spelling without its dashes, else the first spelling without its dash.
pub fn canon_of(names: &[String]) -> String {
    names
        .iter()
        .find(|n| n.starts_with("--"))
        .or_else(|| names.first())
        .map(|n| n.trim_start_matches('-').to_string())
        .unwrap_or_default()
}

fn positionals(specs: &[PositionalSpec], where_: &str) -> Result<Vec<Positional>, String> {
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let at = match &spec.at {
            AtSpec::Index(i) => At::Index(*i),
            AtSpec::Word(w) if w == "rest" => At::Rest,
            AtSpec::Word(w) if w == "last" => At::Last,
            AtSpec::Word(w) => {
                return Err(format!(
                    "{where_}: positional `at = \"{w}\"` is not an index, \"rest\" or \"last\""
                ));
            }
        };
        if out.iter().any(|p: &Positional| p.at == at) {
            return Err(format!("{where_}: two positionals at the same place"));
        }
        out.push(Positional {
            at,
            role: spec.role.into(),
            unless: spec.unless.clone(),
        });
    }
    Ok(out)
}

// ---- what a model may not say ---------------------------------------------------

/// The cross-references, and the uniqueness a rule relies on.
fn check(model: &Model) -> Result<(), String> {
    fn walk(node: &Node, ancestors: &[&Node], chain: &str) -> Result<(), String> {
        let visible: Vec<&Flag> = ancestors
            .iter()
            .chain(std::iter::once(&node))
            .flat_map(|n| n.flags.iter())
            .collect();
        let knows = |canon: &str| visible.iter().any(|f| f.canon == canon);
        let here = if chain.is_empty() {
            "the program".to_string()
        } else {
            format!("command `{chain}`")
        };
        // One spelling, one flag — within the node (a sub may legitimately
        // reuse a short its parent uses for something else).
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for f in &node.flags {
            for n in &f.names {
                if let Some(other) = seen.insert(n, &f.canon)
                    && other != f.canon
                {
                    return Err(format!(
                        "{here}: `{n}` spells both `{other}` and `{}`",
                        f.canon
                    ));
                }
            }
        }
        let mut canons: BTreeMap<&str, usize> = BTreeMap::new();
        for f in &node.flags {
            *canons.entry(&f.canon).or_default() += 1;
        }
        if let Some((c, _)) = canons.iter().find(|(_, n)| **n > 1) {
            return Err(format!("{here}: two flags named `{c}`"));
        }
        for f in &node.flags {
            if let Some((fact, _)) = &f.implies
                && !knows(fact)
            {
                return Err(format!(
                    "{here}: flag `{}` implies `{fact}`, a flag nothing here has",
                    f.canon
                ));
            }
        }
        for p in &node.positionals {
            for u in &p.unless {
                if !knows(u) {
                    return Err(format!(
                        "{here}: a positional's `unless` names `{u}`, a flag nothing here has"
                    ));
                }
            }
        }
        for r in &node.read_only_with {
            if !knows(r) {
                return Err(format!(
                    "{here}: `read_only_with` names `{r}`, a flag nothing here has"
                ));
            }
        }
        if let Some(eu) = &node.editor_unless {
            for e in eu {
                if !knows(e) {
                    return Err(format!(
                        "{here}: `editor_unless` names `{e}`, a flag nothing here has"
                    ));
                }
            }
        }
        let mut names: BTreeMap<&str, ()> = BTreeMap::new();
        for s in &node.subs {
            for n in std::iter::once(&s.name).chain(s.aliases.iter()) {
                if names.insert(n, ()).is_some() {
                    return Err(format!("{here}: two subcommands answer to `{n}`"));
                }
            }
        }
        let mut below: Vec<&Node> = ancestors.to_vec();
        below.push(node);
        for s in &node.subs {
            let sub_chain = if chain.is_empty() {
                s.name.clone()
            } else {
                format!("{chain} {}", s.name)
            };
            walk(s, &below, &sub_chain)?;
        }
        Ok(())
    }
    walk(&model.root, &[], "")?;
    for s in &model.switches {
        if model.canonical_of_canon(&s.when).is_none() {
            return Err(format!(
                "a switch names `{}`, a flag the model does not have",
                s.when
            ));
        }
    }
    if let Some(n) = &model.family.numeric_option
        && model.canonical_of_canon(n).is_none()
    {
        return Err(format!(
            "`numeric_option = \"{n}\"` names a flag the model does not have"
        ));
    }
    for ex in model
        .examples
        .matches
        .iter()
        .chain(&model.examples.not_matches)
    {
        let first = ex.split_whitespace().next().unwrap_or("");
        let base = first.rsplit('/').next().unwrap_or(first);
        if base != model.name && !model.root.aliases.iter().any(|a| a == base) {
            return Err(format!(
                "example `{}` does not begin with `{}`",
                super::clip(ex),
                model.name
            ));
        }
    }
    Ok(())
}

impl Model {
    /// Whether any flag in the tree has this canonical name.
    fn canonical_of_canon(&self, canon: &str) -> Option<()> {
        self.root
            .flags_below()
            .iter()
            .any(|(_, f)| f.canon == canon)
            .then_some(())
    }
}
