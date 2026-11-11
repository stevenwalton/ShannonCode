//! **The program model: what a command's words ARE.**
//!
//! A rule names a node of a program's tree — the program, a subcommand, a
//! flag, a flag's value — and a path or a host at an operation. Nothing in a
//! line of shell says which words are which: `-name x` is a test to `find`
//! and a file to `cat`; `-exec` runs a program; `--method POST` turns a read
//! into a write. The model is where those facts live, one per program, so
//! the reader (`permission::acts`) can turn an argv into acts and the rules
//! can be matched against nodes rather than text (design
//! `docs/DesignDocs/permission-grammar.md` §8.3; the format decided by probe
//! P2, §7.5).
//!
//! **One format, shipped and written.** The models Shannon ships are TOML
//! files compiled in (`crates/shannon-core/models/*.toml`,
//! [`Models::shipped`]); a user's live in [`USER_DIR`] beside the config
//! file ([`Models::with_user_dir`]), and a project's will under
//! `.shannon/` (P6d), read by the same loader ([`load`]). A model carries
//! its [`Trust`]: a `completion` model (imported from a shell-completion
//! spec) backs only denies and asks — a candidate reading is enough to
//! refuse, not to grant: with no rule its call asks, no flag rides, and
//! its places are at no operation (`acts`).
//!
//! **The model is the bottleneck, measured** (§8.7): twenty one-line models
//! moved 49 of 175 real prompts. So the types here are deliberately plain —
//! a flag is its spellings, its arity, the role of its value and what it
//! raises the command to — and a program is a tree of such nodes, because
//! that is what an author can write from a `--help` page.

pub mod lang;
pub mod load;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

/// What a node does, in the order a rule's strictness follows: a read may
/// be allowed by default; a write asks until placed; a delete and an exec
/// ask until a rule names them (Q7, §10).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Effect {
    Read,
    Write,
    Delete,
    /// Runs a program, or code.
    Exec,
}

impl Effect {
    /// The spelling a model file uses.
    pub fn name(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Write => "write",
            Effect::Delete => "delete",
            Effect::Exec => "exec",
        }
    }

    /// The display lattice the TUI paints with (`shell::Effect`), from the
    /// model's: a read is read-only, a write mutating, a delete destructive,
    /// an exec an executor.
    pub fn shown(self) -> crate::shell::Effect {
        match self {
            Effect::Read => crate::shell::Effect::ReadOnly,
            Effect::Write => crate::shell::Effect::Mutating,
            Effect::Delete => crate::shell::Effect::Destructive,
            Effect::Exec => crate::shell::Effect::Executor,
        }
    }
}

/// What a flag's value, or an operand, IS — the fact that turns a word into
/// an act: a file at an operation, a program run, code, a host.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Text the program prints, compares or names something by.
    Data,
    /// A pattern (a regex, a glob) the program matches with.
    Pattern,
    /// One of a fixed set of words (`--type f`, `--method GET`).
    Enum,
    Number,
    ReadFile,
    WriteFile,
    DeleteFile,
    /// A directory read: entered, searched — what is in its files reaches
    /// the output (`rg`'s paths).
    Dir,
    /// A place listed: its names and metadata reach the output, never what
    /// its files hold (`ls`, `find`'s starting points, `git status`). While
    /// the call stays a read, a guard below it is not reached from above —
    /// a listing of `.` with Shannon's state, or a key, beneath it does not
    /// read them (Steven, 10-11: "names are fine"). Named outright, the
    /// place meets its guard as any read does.
    List,
    /// A file executed.
    ExecFile,
    /// A program name, run.
    Program,
    /// Code in a language the model may scan ([`lang`]) or not (`sh -c`).
    Code,
    Url,
    Host,
    /// A path on the program's implicit host (`gh api repos/o/r`).
    Endpoint,
    /// `key=value` data.
    Kv,
    /// The rest of the argv is another command line.
    Argv,
    /// A glob that INCLUDES what the program otherwise skips (`rg -g`):
    /// benign only when it begins with `!`, an exclude.
    IncludeGlob,
}

impl Role {
    /// Does a value of this role ride on a program allow without being
    /// named? A file read, written or deleted is judged as its own path act,
    /// so it does not disqualify the flag; a program, code or an argv does.
    pub fn benign(self) -> bool {
        !matches!(
            self,
            Role::Program | Role::Code | Role::Argv | Role::ExecFile
        )
    }

    /// What a value of this role raises its node to: a file written,
    /// deleted or run, a program run; every other role is a read. The one
    /// mapping the reader (a role's word) and the loader (a switch's
    /// flag) both raise by.
    pub fn effect(self) -> Effect {
        match self {
            Role::WriteFile => Effect::Write,
            Role::DeleteFile => Effect::Delete,
            Role::ExecFile | Role::Program => Effect::Exec,
            _ => Effect::Read,
        }
    }

    /// Is a value of this role a place in the file tree? What makes a bare
    /// word on a rule for the program a grant (`allow rm build`).
    pub fn is_file(self) -> bool {
        matches!(
            self,
            Role::ReadFile
                | Role::WriteFile
                | Role::DeleteFile
                | Role::Dir
                | Role::List
                | Role::ExecFile
        )
    }

    /// The spelling a model file uses.
    pub fn name(self) -> &'static str {
        match self {
            Role::Data => "data",
            Role::Pattern => "pattern",
            Role::Enum => "enum",
            Role::Number => "number",
            Role::ReadFile => "read_file",
            Role::WriteFile => "write_file",
            Role::DeleteFile => "delete_file",
            Role::Dir => "dir",
            Role::List => "list",
            Role::ExecFile => "exec_file",
            Role::Program => "program",
            Role::Code => "code",
            Role::Url => "url",
            Role::Host => "host",
            Role::Endpoint => "endpoint",
            Role::Kv => "kv",
            Role::Argv => "argv",
            Role::IncludeGlob => "include_glob",
        }
    }
}

/// How many words a flag takes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Arity {
    Zero,
    One,
    /// A fixed several, each with its role (`find -fprintf FILE FORMAT`).
    N(Vec<Role>),
    /// Words until a standalone terminator (`find -exec … ;`).
    Until(Vec<String>),
    /// A value only when attached (`-I[fmt]`, `--color[=when]`).
    OptionalAttached,
}

/// One flag a node understands.
#[derive(Clone, Debug, PartialEq)]
pub struct Flag {
    /// Every spelling, with dashes (`-r`, `-R`, `--recursive`).
    pub names: Vec<String>,
    /// The dashless identity a rule names (`recursive`); every spelling
    /// collapses to it, so a rule holds however the flag was written.
    pub canon: String,
    pub arity: Arity,
    /// The role of the value (or of each, for [`Arity::N`]).
    pub role: Role,
    /// What the flag raises the node to.
    pub effect: Option<Effect>,
    /// A fact the flag's presence implies when the fact's flag is absent
    /// (`gh api -f` ⇒ `method = POST`).
    pub implies: Option<(String, String)>,
    /// Whether the flag rides on a program allow unnamed. `false` marks a
    /// mode left guarded on purpose, with `why`; containment then asks for
    /// it unless a rule names it.
    pub benign: bool,
    /// Why a guarded flag asks — shown beside the verdict.
    pub why: Option<String>,
    /// Implementations differ in whether the value attaches only (GNU sed's
    /// `-i[SUFFIX]`) or is the next word (BSD's `-i SUFFIX`): the argv is
    /// read both ways, and what either reading places or runs is the
    /// call's (review 2026-10-11, E2: BSD's reading of `sed -i s/x/y/ f`
    /// took `f` for the script, and GNU sed edits it).
    pub or_attached: bool,
    /// The value is a directory the program moves to before acting (`git
    /// -C`, `uv --directory`): the rest of the argv is placed under it, as
    /// after a `cd`.
    pub chdir: bool,
}

/// Where an operand sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum At {
    Index(usize),
    Rest,
    Last,
}

/// What an operand at a position is.
#[derive(Clone, Debug, PartialEq)]
pub struct Positional {
    pub at: At,
    pub role: Role,
    /// The role applies only when none of these flags (canonical names) is
    /// given: sed's script operand unless `-e`/`-f`.
    pub unless: Vec<String>,
}

/// A node of a program's tree: the program itself, or a subcommand.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub aliases: Vec<String>,
    pub flags: Vec<Flag>,
    pub positionals: Vec<Positional>,
    pub subs: Vec<Node>,
    pub effect: Effect,
    /// An unknown first word at this node runs a program named by it
    /// (`git-<x>`, a `gh` extension).
    pub external: bool,
    /// The node's effect falls to a read when one of these flags is present
    /// (`uv tree --frozen`).
    pub read_only_with: Vec<String>,
    /// The node runs the editor unless one of these flags is present
    /// (`git commit` without `-m`).
    pub editor_unless: Option<Vec<String>>,
    /// The node prints a credential (`gh auth token`).
    pub prints_secret: bool,
    /// Files the node writes that no operand names, relative to where it
    /// runs (`git add` → `.git`).
    pub writes: Vec<String>,
    /// Known endpoints (`gh api user`) with a one-line gloss for the pane.
    pub endpoints: BTreeMap<String, String>,
    /// What it reads where it runs when no operand names a place: `Dir`
    /// (searched: `rg`, `git diff`), `List` (listed: `ls`, `git status`),
    /// or nothing. A subcommand takes its parent's unless its table says.
    pub reads_here: Option<Role>,
}

impl Node {
    /// The subcommand `word` names at this node, by name or alias.
    pub fn sub(&self, word: &str) -> Option<&Node> {
        self.subs
            .iter()
            .find(|s| s.name == word || s.aliases.iter().any(|a| a == word))
    }

    /// Every flag in the subtree, with the chain of node names it hangs
    /// under (the root's name excluded).
    pub fn flags_below(&self) -> Vec<(Vec<&str>, &Flag)> {
        fn walk<'a>(
            node: &'a Node,
            chain: &mut Vec<&'a str>,
            out: &mut Vec<(Vec<&'a str>, &'a Flag)>,
        ) {
            for f in &node.flags {
                out.push((chain.clone(), f));
            }
            for s in &node.subs {
                chain.push(&s.name);
                walk(s, chain, out);
                chain.pop();
            }
        }
        let mut out = Vec::new();
        walk(self, &mut Vec::new(), &mut out);
        out
    }
}

/// What `--` means to a program.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DoubleDash {
    /// Ends the flags: every later word is an operand (POSIX).
    Terminator,
    /// Ends only the options before the operands; dashed words after it are
    /// still read (`find -- . -delete` deletes).
    Ignored,
    /// Begins another command line (`uv run -- CMD`).
    InnerArgv,
}

/// The argv family: how the program reads its own words.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    /// `-rf` is `-r -f`.
    pub bundling: bool,
    /// Single-dash WORDS that never bundle (`find -name`).
    pub single_dash_long: bool,
    /// An unambiguous prefix of a long option is that option (GNU).
    pub long_abbrev: bool,
    /// Flags may follow operands.
    pub interspersed: bool,
    pub double_dash: DoubleDash,
    /// `-NUM` is this flag with NUM as its value (`head -20`: `lines`).
    pub numeric_option: Option<String>,
    /// A dashed word the model does not name is an operand, not an unknown
    /// flag (`echo -----` prints five dashes).
    pub dash_data: bool,
}

impl Default for Family {
    fn default() -> Self {
        Family {
            bundling: true,
            single_dash_long: false,
            long_abbrev: false,
            interspersed: true,
            double_dash: DoubleDash::Terminator,
            numeric_option: None,
            dash_data: false,
        }
    }
}

/// What a model may back.
///
/// **No order.** Only `Completion` changes a decision; the other three
/// decide alike and differ in what the pane says. A derived `Ord` ranked
/// the 65 `Generated` shipped models below a user's unlabelled file, ready
/// for a `trust >= …` to mean something nobody decided (review stop 13, B
/// 3) — a comparison has to be a written `match`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trust {
    /// Imported from a completion spec: grammar, not danger. Backs denies
    /// and asks; never an allow.
    Completion,
    /// Carried over by a program from the table this format replaced
    /// (`shell::program::PROGRAMS`, P1) and not yet checked flag by flag:
    /// relied on as the engine always relied on that table, and SAID as
    /// what it is beside a verdict no rule made — it said `verified` until
    /// review stop 12 (T8, Q13).
    Generated,
    /// Written by a tool's author or a user: their statement, honoured.
    Author,
    /// Verified by us, or marked so by the user.
    Verified,
}

impl Trust {
    pub fn name(self) -> &'static str {
        match self {
            Trust::Completion => "completion",
            Trust::Generated => "generated",
            Trust::Author => "author",
            Trust::Verified => "verified",
        }
    }
}

/// A role rewritten by a flag's presence: `sed -i` turns the files it reads
/// into files it writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Switch {
    /// The flag (canonical) whose presence switches.
    pub when: String,
    pub from: Role,
    pub to: Role,
    /// Which values switch: those of this flag (canonical), or — `None` —
    /// the positionals.
    pub on: Option<String>,
}

/// Where a model came from, for the pane and `:rules`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Shipped,
    /// A file under the user's config directory.
    User(String),
    /// A file under the project's `.shannon/models/`.
    Project(String),
}

impl Source {
    /// Where the model is written, as the user would find it: `shipped`,
    /// or its file (`programs/lsd.toml`).
    pub fn shown(&self) -> String {
        match self {
            Source::Shipped => "shipped".into(),
            Source::User(file) => format!("{USER_DIR}/{file}"),
            Source::Project(file) => format!(".shannon/models/{file}"),
        }
    }
}

/// The model that read an act, for the pane: whose statement the verdict
/// rests on, and what that statement may back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRef {
    pub name: String,
    pub trust: Trust,
    pub source: Source,
}

impl Model {
    pub fn reference(&self) -> ModelRef {
        ModelRef {
            name: self.name.clone(),
            trust: self.trust,
            source: self.source.clone(),
        }
    }
}

/// One program's model.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    pub root: Node,
    pub family: Family,
    /// Which implementation this model describes (`gnu`, `bsd`, `bfs`,
    /// `bwk`, `gawk`), when it matters; `None` when the model holds for all.
    pub implementation: Option<String>,
    pub trust: Trust,
    /// Hosts every call reaches without naming them (`gh`:
    /// `api.github.com`). Not acts (the concession, §10 10-09); the pane's
    /// gloss.
    pub hosts: Vec<String>,
    /// Builds the argument list of what it runs from its input (`xargs`):
    /// words the gate never sees, wherever it appears — the stage's own
    /// peel or an inner argv (review 2026-10-10, E1).
    pub argv_from_stdin: bool,
    /// Words that are expression operators, not operands (`find`'s `(`, `!`).
    pub operators: Vec<String>,
    /// What `{}` stands for (`find`: the starting points).
    pub placeholder: Option<String>,
    /// A privilege wrapper (`sudo`): a hard shipped ask, whatever a rule
    /// says.
    pub privilege: bool,
    pub switches: Vec<Switch>,
    /// Config keys (`git -c KEY=VAL`) whose value names a program: exact
    /// keys, `prefix.` and `.suffix` forms.
    pub kv_programs: Vec<String>,
    /// **Config keys known to run nothing**, in the same forms: given, every
    /// OTHER key is taken to name a program — the list a program's config
    /// has too many such keys for (`git -c include.path=…`, `…uploadpack`,
    /// `…program` were missing from git's `kv_programs`; review 2026-10-11,
    /// E1). A list of the dangerous is a proxy for a property; a list of the
    /// harmless fails closed.
    pub kv_safe: Vec<String>,
    /// Why this program's unmodelled flags ask (`grep`: among them `-r`).
    pub unknown_flag_why: Option<String>,
    /// The language a [`Role::Code`] operand or value is in, when a reader
    /// exists for it ([`lang`]: `sed`, `awk`); `None` leaves such code
    /// unread, an act that asks.
    pub language: Option<String>,
    /// Lines the model must read in full (`match`) and lines it must find
    /// an unknown word in (`not_match`); held to the reader
    /// (`acts::example_misreads`) by `every_shipped_model_reads_its_examples`
    /// for a shipped model and, for a user's file, at load (said, not
    /// refused: a misread example asks more, never less).
    pub examples: Examples,
    pub source: Source,
}

/// Example lines a model carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Examples {
    pub matches: Vec<String>,
    pub not_matches: Vec<String>,
}

impl Model {
    /// The flag spelled `spelling`, anywhere in the tree — the resolver a
    /// rule's flag goes through when it is read (its canonical name, and
    /// its arity: `--method GET` carries `GET` because the model says so).
    pub fn flag_named(&self, spelling: &str) -> Option<&Flag> {
        self.root
            .flags_below()
            .into_iter()
            .find(|(_, f)| f.names.iter().any(|n| n == spelling))
            .map(|(_, f)| f)
    }

    /// The canonical name of the flag spelled `spelling`, anywhere in the
    /// tree.
    pub fn canonical_of(&self, spelling: &str) -> Option<&str> {
        self.flag_named(spelling).map(|f| f.canon.as_str())
    }

    /// Could a flag the gate cannot see change what this program does? Not
    /// for a program that only reads and whose every flag rides (`cat`,
    /// `ls`): a word that may become a flag is then only a file to place.
    /// Any node past a read, a guarded or escalating flag, or a value that
    /// names a program or code says yes.
    pub fn flags_matter(&self) -> bool {
        fn walk(node: &Node) -> bool {
            node.effect > Effect::Read
                || node.flags.iter().any(|f| {
                    !f.benign || !f.role.benign() || f.effect.is_some_and(|e| e > Effect::Read)
                })
                || node.subs.iter().any(walk)
        }
        // A flag left unmodelled on purpose (`grep -r`) matters too.
        self.unknown_flag_why.is_some() || walk(&self.root)
    }

    /// The flags that raise the root above its own effect — what a hidden
    /// flag could be, and what the shipped carvings must name beneath a
    /// broad allow (`permission::defaults`).
    pub fn escalations(&self) -> Vec<&Flag> {
        self.root
            .flags
            .iter()
            .filter(|f| f.effect.is_some_and(|e| e > self.root.effect))
            .collect()
    }

    /// Could a flag make this program more dangerous than its root effect?
    /// (A mode left guarded on purpose counts: a word that may expand into
    /// a flag could be it.)
    pub fn can_escalate(&self) -> bool {
        !self.escalations().is_empty()
            || self.unknown_flag_why.is_some()
            || self.root.flags.iter().any(|f| !f.benign)
    }
}

/// **The directory a user's own models live in**, beside the config file:
/// one name for the loader, the notes and the manual. A stand-in (Steven,
/// 10-10: `models` reads as AI models in an agent's config; the name is
/// his to settle).
pub const USER_DIR: &str = "programs";

/// At most this many files of the user's directory are read: each is read
/// and parsed at every config change.
pub(crate) const MOST_USER_FILES: usize = 256;

/// At most this many bytes of the user's directory are read, all files
/// together: the load is paid at launch and every config change, on the
/// event loop, so a count of files is no bound on the work (review
/// 2026-10-11, B: 256 files of 1 MiB of examples froze it for tens of
/// seconds). Past it, the rest is said and not read.
pub(crate) const MOST_USER_BYTES: usize = 1 << 20;

/// At most this many entries of the directory are looked at: a `programs`
/// that is a link to a large directory costs a listing at every change.
pub(crate) const MOST_ENTRIES: usize = 4096;

/// At most this many notes from one load: each is a notice.
pub(crate) const MOST_NOTES: usize = 32;

/// The longest subcommand chain a model may declare (`[command."a b c"]`):
/// real programs nest three or four deep, and every walker of the tree
/// recurses by its depth — 20,000 words overflowed the stack at launch and
/// left the terminal raw (review 2026-10-11, B).
pub(crate) const MOST_CHAIN_WORDS: usize = 8;

/// At most this many example lines a model carries, each at most
/// [`MOST_EXAMPLE_BYTES`]: each is read by the command reader at every load.
pub(crate) const MOST_EXAMPLES: usize = 64;

/// The longest example line a model may carry.
pub(crate) const MOST_EXAMPLE_BYTES: usize = 1024;

/// A user's text quoted in a note, cut to what a status row shows: a
/// 110 KB example once made a 110 KB notice, wrapped on every frame (review
/// 2026-10-11, B). Cut on a character boundary.
pub(crate) fn clip(text: &str) -> std::borrow::Cow<'_, str> {
    const MOST: usize = 80;
    match text.char_indices().nth(MOST) {
        None => text.into(),
        Some((at, _)) => format!("{}…", &text[..at]).into(),
    }
}

/// The models in force: shipped, then a user's, then a project's — later
/// replaces earlier by name.
#[derive(Clone, Debug, Default)]
pub struct Models {
    by_name: BTreeMap<String, Arc<Model>>,
}

impl Models {
    /// No models at all: every program unmodelled.
    pub fn empty() -> Models {
        Models::default()
    }

    /// The shipped set, parsed once. A shipped file that does not parse is
    /// a programming error caught by the module's tests, never something a
    /// user can trigger.
    pub fn shipped() -> &'static Models {
        static SHIPPED: OnceLock<Models> = OnceLock::new();
        SHIPPED.get_or_init(|| {
            let mut models = Models::empty();
            for (name, text) in SHIPPED_FILES {
                let model = load::parse(text, Source::Shipped)
                    .unwrap_or_else(|why| panic!("shipped model `{name}` must parse: {why}"));
                models.insert(model);
            }
            models
        })
    }

    /// **The shipped set with a user's directory over it**: each `*.toml`
    /// in `dir`, in name order, read by the one loader, replacing a shipped
    /// model of the same name whole (a model is one author's statement; a
    /// merge would be nobody's). A missing directory is no models and
    /// nothing to say.
    ///
    /// Returns what the user must be told, as sentences: a file that could
    /// not be read or parsed, or that names another program than its file
    /// (skipped, so `ls.toml` cannot quietly replace `rm`); and every
    /// example line the model misreads (loaded all the same — a misread
    /// example asks more, never less, and the sentence is the author's
    /// test).
    pub fn with_user_dir(dir: &std::path::Path) -> (Models, Vec<String>) {
        Models::shipped().clone().load_dir(dir, false)
    }

    /// **A trusted project's models** (`<start>/.shannon/models/`), over
    /// these: each at completion trust whatever it says (Steven, 10-13) — a
    /// repository's model says what its words are, and allows nothing by
    /// itself. One may replace a model Shannon ships, as yours may (Steven,
    /// 10-13: barred "No"); Shannon's facts stand under it, as under yours
    /// (`acts_of_argv`). Read, bounded and said as
    /// [`with_user_dir`](Self::with_user_dir)'s.
    pub fn with_project_dir(self, dir: &std::path::Path) -> (Models, Vec<String>) {
        self.load_dir(dir, true)
    }

    /// The one reader of a directory of models, onto `self`: the user's
    /// (`project` false) or a project's, whose models are `Source::Project`
    /// at completion trust.
    fn load_dir(self, dir: &std::path::Path, project: bool) -> (Models, Vec<String>) {
        let mut models = self;
        let mut notes = Vec::new();
        let label = match project {
            true => format!("{}/models", crate::config::PROJECT_DIR),
            false => USER_DIR.to_string(),
        };
        let shown = |file: &str| format!("{label}/{file}");
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (models, notes),
            Err(e) => {
                notes.push(format!("{label}: not read ({e}) — the other models stand"));
                return (models, notes);
            }
        };
        // A bounded listing: past it the rest is not looked at, and said.
        let (mut files, mut unlisted, mut looked) = (Vec::new(), 0usize, 0usize);
        for entry in entries {
            looked += 1;
            if looked > MOST_ENTRIES {
                notes.push(format!(
                    "{label}: more than {MOST_ENTRIES} entries, and only the first are looked at"
                ));
                break;
            }
            match entry {
                Ok(e) => {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "toml") {
                        files.push(p);
                    }
                }
                Err(_) => unlisted += 1,
            }
        }
        if unlisted > 0 {
            notes.push(format!(
                "{label}: {unlisted} entries could not be listed, and were not read"
            ));
        }
        files.sort();
        if files.len() > MOST_USER_FILES {
            notes.push(format!(
                "{label}: {} files, and only the first {MOST_USER_FILES} by name are read",
                files.len()
            ));
            files.truncate(MOST_USER_FILES);
        }
        let mut read = 0usize;
        for path in files {
            let file = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = match crate::config::read_file(&path) {
                Ok(Some(text)) => text,
                Ok(None) => continue,
                Err(why) => {
                    notes.push(format!("{}: not read ({why})", shown(&file)));
                    continue;
                }
            };
            read += text.len();
            if read > MOST_USER_BYTES {
                notes.push(format!(
                    "{}: not read, nor any after it — the directory holds more than {} KiB, \
                     read at every config change",
                    shown(&file),
                    MOST_USER_BYTES / 1024
                ));
                break;
            }
            let source = match project {
                true => Source::Project(file.clone()),
                false => Source::User(file.clone()),
            };
            match load::parse(&text, source) {
                Err(why) => notes.push(format!("{}: not loaded — {why}", shown(&file))),
                Ok(model) if format!("{}.toml", model.name) != file => notes.push(format!(
                    "{}: not loaded — it models `{}`, and a file is named after its program \
                     (`{}.toml`)",
                    shown(&file),
                    model.name,
                    model.name
                )),
                Ok(mut model) => {
                    if project {
                        model.trust = Trust::Completion;
                    }
                    models.insert(model)
                }
            }
        }
        // Each against the set it is in force with, so an example naming
        // another of these programs is read by that model too.
        for model in models.iter() {
            let file = match (&model.source, project) {
                (Source::User(file), false) | (Source::Project(file), true) => file,
                _ => continue,
            };
            for why in super::acts::example_misreads(model, &models) {
                notes.push(format!("{}: {why}", shown(file)));
            }
        }
        // A bound: each note is a notice at every config change.
        if notes.len() > MOST_NOTES {
            let more = notes.len() - (MOST_NOTES - 1);
            notes.truncate(MOST_NOTES - 1);
            notes.push(format!("{label}: and {more} more like these"));
        }
        (models, notes)
    }

    /// Add or replace a model by its name.
    pub fn insert(&mut self, model: Model) {
        self.by_name.insert(model.name.clone(), Arc::new(model));
    }

    /// The model for `program`, matched on its **basename** so `/usr/bin/rm`
    /// and `rm` resolve alike. `None` for a program no model describes — the
    /// caller reads it conservatively, never as safe.
    pub fn get(&self, program: &str) -> Option<&Model> {
        let base = program.rsplit('/').next().unwrap_or(program);
        self.by_name.get(base).map(Arc::as_ref)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Every model, by name.
    pub fn iter(&self) -> impl Iterator<Item = &Model> {
        self.by_name.values().map(Arc::as_ref)
    }

    /// **The user's and the project's files that read differently from
    /// `before`** — added, edited or taken away, as `programs/<file>` or
    /// `.shannon/models/<file>` — for a reload or a trust to say what it
    /// changed: a model file is no setting, and "nothing changed" over an
    /// edit that flips verdicts misleads (review 2026-10-11, A and E2). A
    /// project's were left out, so `:trust .` on a repository holding only
    /// models said nothing of them (review 2026-10-13, A).
    pub fn user_changes(&self, before: &Models) -> Vec<String> {
        let users = |m: &Models| -> BTreeMap<String, Model> {
            m.iter()
                .filter_map(|m| match &m.source {
                    Source::User(file) => Some((format!("{USER_DIR}/{file}"), m.clone())),
                    Source::Project(file) => Some((
                        format!("{}/models/{file}", crate::config::PROJECT_DIR),
                        m.clone(),
                    )),
                    Source::Shipped => None,
                })
                .collect()
        };
        let (now, then) = (users(self), users(before));
        let mut files: Vec<&String> = now.keys().chain(then.keys()).collect();
        files.sort();
        files.dedup();
        files
            .into_iter()
            .filter(|f| now.get(*f) != then.get(*f))
            .cloned()
            .collect()
    }

    /// Could a flag make `program` more dangerous than its root effect? An
    /// unmodelled program is assumed escalatable (fail closed).
    pub fn can_escalate(&self, program: &str) -> bool {
        self.get(program).is_none_or(Model::can_escalate)
    }

    /// The canonical name `program`'s model gives the flag spelled
    /// `spelling`, or `None` when it models no such flag.
    pub fn canonical_of(&self, program: &str, spelling: &str) -> Option<&str> {
        self.get(program)?.canonical_of(spelling)
    }
}

/// The shipped models, by program name. **Every file under
/// `crates/shannon-core/models/` is listed here** — `tests::every_model_
/// file_is_shipped` holds the list to the directory, so a file added
/// without a row is a failing test, not a silently missing model. A file
/// is named after its program but for two a file name cannot wear well:
/// `.` lives in `dot.toml`, `[` in `bracket.toml`.
const SHIPPED_FILES: &[(&str, &str)] = &[
    (".", include_str!("../../../models/dot.toml")),
    ("[", include_str!("../../../models/bracket.toml")),
    ("alias", include_str!("../../../models/alias.toml")),
    ("awk", include_str!("../../../models/awk.toml")),
    ("basename", include_str!("../../../models/basename.toml")),
    ("bash", include_str!("../../../models/bash.toml")),
    ("break", include_str!("../../../models/break.toml")),
    ("builtin", include_str!("../../../models/builtin.toml")),
    ("cargo", include_str!("../../../models/cargo.toml")),
    ("cat", include_str!("../../../models/cat.toml")),
    ("cd", include_str!("../../../models/cd.toml")),
    ("chrt", include_str!("../../../models/chrt.toml")),
    ("command", include_str!("../../../models/command.toml")),
    ("continue", include_str!("../../../models/continue.toml")),
    ("curl", include_str!("../../../models/curl.toml")),
    ("cut", include_str!("../../../models/cut.toml")),
    ("dirname", include_str!("../../../models/dirname.toml")),
    ("dirs", include_str!("../../../models/dirs.toml")),
    ("doas", include_str!("../../../models/doas.toml")),
    ("echo", include_str!("../../../models/echo.toml")),
    ("env", include_str!("../../../models/env.toml")),
    ("eval", include_str!("../../../models/eval.toml")),
    ("exec", include_str!("../../../models/exec.toml")),
    ("export", include_str!("../../../models/export.toml")),
    ("false", include_str!("../../../models/false.toml")),
    ("fd", include_str!("../../../models/fd.toml")),
    ("find", include_str!("../../../models/find.toml")),
    ("flock", include_str!("../../../models/flock.toml")),
    ("getopts", include_str!("../../../models/getopts.toml")),
    ("gh", include_str!("../../../models/gh.toml")),
    ("git", include_str!("../../../models/git.toml")),
    ("grep", include_str!("../../../models/grep.toml")),
    ("hash", include_str!("../../../models/hash.toml")),
    ("head", include_str!("../../../models/head.toml")),
    ("ionice", include_str!("../../../models/ionice.toml")),
    ("let", include_str!("../../../models/let.toml")),
    ("ls", include_str!("../../../models/ls.toml")),
    ("nice", include_str!("../../../models/nice.toml")),
    ("nohup", include_str!("../../../models/nohup.toml")),
    ("perl", include_str!("../../../models/perl.toml")),
    ("pkexec", include_str!("../../../models/pkexec.toml")),
    ("popd", include_str!("../../../models/popd.toml")),
    ("printf", include_str!("../../../models/printf.toml")),
    ("pushd", include_str!("../../../models/pushd.toml")),
    ("pwd", include_str!("../../../models/pwd.toml")),
    ("python", include_str!("../../../models/python.toml")),
    ("python3", include_str!("../../../models/python3.toml")),
    ("read", include_str!("../../../models/read.toml")),
    ("return", include_str!("../../../models/return.toml")),
    ("rg", include_str!("../../../models/rg.toml")),
    ("rm", include_str!("../../../models/rm.toml")),
    ("run0", include_str!("../../../models/run0.toml")),
    ("sed", include_str!("../../../models/sed.toml")),
    ("seq", include_str!("../../../models/seq.toml")),
    ("setsid", include_str!("../../../models/setsid.toml")),
    ("sh", include_str!("../../../models/sh.toml")),
    ("shift", include_str!("../../../models/shift.toml")),
    ("shred", include_str!("../../../models/shred.toml")),
    ("sort", include_str!("../../../models/sort.toml")),
    ("source", include_str!("../../../models/source.toml")),
    ("stdbuf", include_str!("../../../models/stdbuf.toml")),
    ("su", include_str!("../../../models/su.toml")),
    ("sudo", include_str!("../../../models/sudo.toml")),
    ("tail", include_str!("../../../models/tail.toml")),
    ("tar", include_str!("../../../models/tar.toml")),
    ("test", include_str!("../../../models/test.toml")),
    ("time", include_str!("../../../models/time.toml")),
    ("timeout", include_str!("../../../models/timeout.toml")),
    ("tr", include_str!("../../../models/tr.toml")),
    ("true", include_str!("../../../models/true.toml")),
    ("type", include_str!("../../../models/type.toml")),
    ("unalias", include_str!("../../../models/unalias.toml")),
    ("uniq", include_str!("../../../models/uniq.toml")),
    ("uv", include_str!("../../../models/uv.toml")),
    ("wait", include_str!("../../../models/wait.toml")),
    ("watch", include_str!("../../../models/watch.toml")),
    ("wc", include_str!("../../../models/wc.toml")),
    ("xargs", include_str!("../../../models/xargs.toml")),
    ("zsh", include_str!("../../../models/zsh.toml")),
];

#[cfg(test)]
mod tests;
