//! **What the model is told beyond the conversation** — instruction files,
//! memories and skills — found along `[memory]`'s and `[skills]`' locations
//! (`locate`), read bounded (`read`), and made into the context blocks a turn
//! carries ([`context()`], [`attach`]). `docs/DesignDocs/memory-and-skills.md`.
//!
//! Pure but for the files it reads: the app hands it the [`Roots`], the
//! config and the session's kind, and a judge of credential paths, so nothing
//! here reads the environment — a test builds its roots in a temporary
//! directory and never meets the user's own.

mod context;
mod create;
mod locate;
mod places;
pub(crate) mod read;
mod remove;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use context::{Told, attach, context, skill_text};
pub use create::{Draft, draft_memory, draft_skill, slug};
pub use places::rules;
pub use remove::{Removal, removal};

use crate::config::{ClaudeFiles, MemoryConfig, MemoryMode, ProjectName, SkillsConfig};

/// **Where locations are read from**: the placeholders' values and the
/// directory a relative location walks up from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    /// Where Shannon started — `{start}`, and where a relative location is
    /// looked for from.
    pub start: PathBuf,
    /// Shannon's config directory — `{config}`. `None` when there is none
    /// (no `$XDG_CONFIG_HOME` or `$HOME`): a location under it names nothing.
    pub config_dir: Option<PathBuf>,
    /// `{project}` — [`project_name`] of the start, by `[memory] project`.
    pub project: String,
    /// `~`.
    pub home: Option<PathBuf>,
    /// `{claude-memory}` — Claude Code's own memory directory for a session
    /// started here (`claude_import::auto_memory_dir`). `None` when there is
    /// no Claude Code home to put it under.
    pub claude_memory: Option<PathBuf>,
    /// `{claude-home}` — Claude Code's home as launch found it
    /// (`claude_import::claude_home`: `$CLAUDE_CONFIG_DIR`, else
    /// `~/.claude`), where its own `CLAUDE.md` and skills are read from: the
    /// one value the gate's `{claude-home}` guard names too
    /// (`secrets::CLAUDE_HOME`).
    pub claude_home: Option<PathBuf>,
    /// What marks the project's root — `[memory] project_root`: where a
    /// relative location's walk stops (`locate::walk`).
    pub project_root: Vec<String>,
}

impl Roots {
    /// **What `[memory]` decides about the roots** — how `{project}` is
    /// named and what marks the project's root — taken from `memory` again.
    /// Launch and a config change both call it, so neither keeps a copy the
    /// other has moved (`{project}` was derived at both, separately).
    pub fn follow(&mut self, memory: &MemoryConfig) {
        self.project = project_name(&self.start, memory.project);
        self.project_root = memory.project_root.clone();
    }
}

/// **Where the model writes memories this session — and who tells it how.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// `[memory] write`: Shannon tells the model how to keep them there.
    Shannon(PathBuf),
    /// Claude Code's own directory, in its session: its own prompt says how,
    /// so Shannon says nothing — saying it too named two places, and the
    /// model asked which (Steven, 10-06, image 85).
    ClaudeCode(PathBuf),
}

impl Write {
    pub fn dir(&self) -> &Path {
        match self {
            Write::Shannon(dir) | Write::ClaudeCode(dir) => dir,
        }
    }
}

/// **A file's text, for the `:memories` screen to show you** — read as what
/// the model is told is (`read::bounded`: a regular file only, opened never
/// to wait, to `cap` bytes), with whether it was cut. For your screen: a
/// planted link to a FIFO cannot hold the draw, nor `/dev/zero` fill it.
pub fn peek(path: &Path, cap: usize) -> Result<(String, bool), String> {
    read::bounded(path, cap).map(|text| (text.text, text.cut))
}

/// **Where a new skill goes** — the first `[skills]` location that is one
/// place and no single file (`{config}/skills` by default): a place of
/// yours, never one a repository's walk found.
pub fn skill_home(skills: &SkillsConfig, roots: &Roots) -> Option<PathBuf> {
    skills
        .paths
        .iter()
        .filter(|entry| !locate::is_relative(entry))
        .flat_map(|entry| locate::places(entry, roots))
        .find(|place| !place.is_file())
}

/// **What `{project}` stands for**, for a session started at `start` — by
/// `[memory] project`: the directory's name and the history store's hash
/// (`store::project_name`), or Claude Code's name for its repository
/// (`claude_import::project_key`). The one derivation: at launch, and when
/// the setting changes (`App::launch_knowledge`, `App::adopt_config`).
pub fn project_name(start: &Path, named: ProjectName) -> String {
    match named {
        ProjectName::Hash => crate::store::project_name(start),
        ProjectName::ClaudeCode => crate::claude_import::project_key(start),
    }
}

/// Claude Code's own memory location, as `[memory]` locations are written.
pub const CLAUDE_MEMORY: &str = "{claude-memory}";
/// Claude Code's instruction files, read when its files are (`claude`) —
/// its home's where it really is: under `$CLAUDE_CONFIG_DIR`, `~/.claude`'s
/// is not the one Claude Code reads (review stop 5, L1).
const CLAUDE_INSTRUCTIONS: [&str; 2] = ["CLAUDE.md", "{claude-home}/CLAUDE.md"];
/// Claude Code's skill locations, likewise.
const CLAUDE_SKILLS: [&str; 2] = [".claude/skills", "{claude-home}/skills"];

/// The kind of session the model answers in — which decides who reads
/// Claude Code's own files (`[memory] claude`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// Shannon's own conversation with the API.
    Api,
    /// `claude -p`, which reads its own `CLAUDE.md` and skills.
    ClaudeCode,
}

/// What a found file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// An instruction file, read whole into every session.
    Instructions,
    /// A memory location's `MEMORY.md`, read whole.
    MemoryIndex,
    /// A memory: named outright (read whole), or listed in a memory
    /// directory and read by the model when its index line is relevant.
    Memory,
    /// A skill: its `SKILL.md`.
    Skill,
}

/// What becomes of a found file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Sent to the model, whole.
    Loaded,
    /// Named to the model — by an index line, or the skills list — and read
    /// when it is wanted.
    Listed,
    /// Claude Code's own, in a Claude Code session: `claude` reads it, so
    /// Shannon never sends it again.
    ClaudeCode,
    /// Not read, and why: a credential path, a skill another shadows.
    Refused(String),
    /// Disabled by you (`[memory] disabled`, `[skills] disabled`): not told,
    /// not listed, and a read of it refused at the gate (`places::rules`).
    Disabled,
}

/// One file found along a location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub role: Role,
    pub path: PathBuf,
    /// Its name: a skill's (its directory's), otherwise the file's.
    pub name: String,
    /// A skill's frontmatter `name`, where it is not its directory's — a
    /// name Claude Code loads it by too (the skills docs, "How a skill gets
    /// its command name").
    pub alias: Option<String>,
    /// A skill's or a memory's `description`, from its frontmatter.
    pub description: Option<String>,
    /// The location it was found by, as written.
    pub location: String,
    pub state: State,
    /// Under the write location: the model's own.
    pub writable: bool,
}

/// Everything found, in the order it is read, and what to say about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub items: Vec<Item>,
    /// Where the model writes memories — `None` under `mode = off`, or when
    /// `write` names nothing ([`write_place`]).
    pub write: Option<Write>,
    /// Why a location named nothing it could have (a placeholder with no
    /// value).
    pub notes: Vec<String>,
    /// What you disabled, as the finding read it — the predicate the doors
    /// ask ([`skill_disabled`](Self::skill_disabled)).
    pub disabled: DisabledSet,
}

impl Found {
    /// **The skill named `name`** that can be used — not refused, not
    /// shadowed. A Claude Code skill counts: `:skill` reads it for any
    /// session.
    pub fn skill(&self, name: &str) -> Option<&Item> {
        self.skills().find(|item| item.name == name)
    }

    /// **Is the skill CALLED `name` disabled — by which entry?** The one
    /// question every door that loads a skill asks (`:skill`, Shannon's
    /// `skill` tool, Claude Code's `Skill`). By its name, in any spelling
    /// [`DisabledSet::skill_named`] takes; or, when every skill found by
    /// that name is disabled by its place, by that place. A usable one
    /// elsewhere is not refused for it (review 29, Reviewer A: a Shannon
    /// skill disabled by place refused Claude Code's own of that name).
    pub fn skill_disabled(&self, name: &str) -> Option<String> {
        if let Some(entry) = self.disabled.skill_named(name) {
            return Some(entry.to_string());
        }
        let name = bare_skill_name(name);
        let named: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| {
                i.role == Role::Skill && (i.name == name || i.alias.as_deref() == Some(name))
            })
            .collect();
        let usable = named
            .iter()
            .any(|i| !matches!(i.state, State::Refused(_) | State::Disabled));
        if usable {
            return None;
        }
        named
            .iter()
            .find(|i| i.state == State::Disabled)
            .and_then(|i| {
                let names = [i.name.as_str(), i.alias.as_deref().unwrap_or_default()];
                let real = resolved(&i.path);
                self.disabled
                    .why(i.role, &names, &i.path, &real)
                    .map(|why| why.entry)
            })
    }

    /// Every usable skill, in the order found.
    pub fn skills(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|item| {
            item.role == Role::Skill && !matches!(item.state, State::Refused(_) | State::Disabled)
        })
    }
}

/// The most of a file read to find its frontmatter.
const HEAD: usize = 4 << 10;

/// **Who reads Claude Code's own files** for this session.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaudeReads {
    /// Shannon sends them: an API session under `claude = on`.
    Shannon,
    /// `claude` does: a Claude Code session, unless `off`.
    Claude,
    /// Nobody.
    Nobody,
}

fn claude_reads(setting: ClaudeFiles, session: Session) -> ClaudeReads {
    match (setting, session) {
        (ClaudeFiles::Off, _) => ClaudeReads::Nobody,
        (_, Session::ClaudeCode) => ClaudeReads::Claude,
        (ClaudeFiles::On, Session::Api) => ClaudeReads::Shannon,
        (ClaudeFiles::Auto, Session::Api) => ClaudeReads::Nobody,
    }
}

/// **Where the model writes memories in a `session`** — whatever the mode,
/// so what is under it is the model's ([`Item::writable`]); [`find`] drops
/// it under `mode = off`. Claude Code's own directory in its session while
/// its files are read there: its prompt names that one, and Shannon's
/// place would be a second (Steven, 10-06: "When using a Claude Code session
/// we should be able to access the Claude Code memories"). Otherwise
/// `[memory] write`. The one answer `find` and the app read (its two vouches
/// and `memory_write_for`, four sites); [`rules`] reaches the same place by
/// `{claude-memory}`, which it adds in exactly the sessions this says so.
pub fn write_place(memory: &MemoryConfig, roots: &Roots, session: Session) -> Option<Write> {
    match (claude_reads(memory.claude, session), &roots.claude_memory) {
        (ClaudeReads::Claude, Some(dir)) => Some(Write::ClaudeCode(dir.clone())),
        _ => locate::write_place(&memory.write, roots).map(Write::Shannon),
    }
}

/// **Find everything `[memory]` and `[skills]` name**, for a `session` —
/// cheap enough for every turn and every frame a pane shows it: a stat per
/// place, and a skill's or memory's first 4 KB (`HEAD`) for its
/// description. Nothing it finds is sent from here; [`context()`] reads what
/// is.
///
/// `sensitive` says why a path is never read, whole — the app's judge: a
/// credential path as the gate judges one (`[secrets]` and Shannon's own
/// state), a `.shannon/` whose directory is not trusted. A file it names is
/// not read, and is listed with that sentence.
pub fn find(
    memory: &MemoryConfig,
    skills: &SkillsConfig,
    roots: &Roots,
    session: Session,
    sensitive: &dyn Fn(&Path) -> Option<String>,
) -> Found {
    let write = write_place(memory, roots, session);
    let mut found = Found {
        write: match memory.mode {
            MemoryMode::Off => None,
            MemoryMode::Auto | MemoryMode::Ask => write.clone(),
        },
        ..Found::default()
    };
    let mut adder = Adder {
        found: &mut found,
        seen: HashSet::new(),
        skills: HashMap::new(),
        repository: locate::repository(roots),
        sensitive,
        write: write.map(|w| w.dir().to_path_buf()),
        disabled: DisabledSet::of(memory, skills, roots),
    };
    let claude = claude_reads(memory.claude, session);
    let theirs = match claude {
        ClaudeReads::Shannon => Some(State::Loaded),
        ClaudeReads::Claude => Some(State::ClaudeCode),
        ClaudeReads::Nobody => None,
    };

    let mut instructions: Vec<(&str, State)> = memory
        .instructions
        .iter()
        .map(|entry| (entry.as_str(), State::Loaded))
        .collect();
    if let Some(state) = &theirs {
        instructions.extend(CLAUDE_INSTRUCTIONS.map(|entry| (entry, state.clone())));
    }
    for (entry, state) in instructions {
        for place in adder.places(entry, roots) {
            let mut cut = false;
            for path in markdown(&place, None, &mut cut) {
                adder.add(Role::Instructions, path, None, entry, state.clone());
            }
            adder.said_cut(&place, cut);
        }
    }

    // The write location is read too: the model's memories come back. And
    // Claude Code's own when its files are read: by `claude` in its session
    // (its index is in its prompt), by Shannon under `claude = on`.
    let mut memories: Vec<(&str, State, State)> = memory
        .paths
        .iter()
        .map(|entry| (entry.as_str(), State::Loaded, State::Listed))
        .collect();
    if !memory.write.trim().is_empty() {
        memories.push((&memory.write, State::Loaded, State::Listed));
    }
    if let Some(state) = &theirs {
        let (index, file) = match (state, memory.mode) {
            // `memory = off` is off for claude too: it is launched with its
            // auto memory off (`SendRequest::own_memory`), so it reads none
            // of this — and `claude's` said it did (Reviewer A, round 28).
            (State::ClaudeCode, MemoryMode::Off) => {
                let off = State::Refused("memory = off — claude runs with its memory off".into());
                (off.clone(), off)
            }
            (State::Loaded, _) => (State::Loaded, State::Listed),
            (other, _) => (other.clone(), other.clone()),
        };
        memories.push((CLAUDE_MEMORY, index, file));
    }
    for (entry, index_state, file_state) in memories {
        for place in adder.places(entry, roots) {
            if place.is_file() {
                adder.add(Role::Memory, place, None, entry, index_state.clone());
                continue;
            }
            let index = place.join("MEMORY.md");
            if index.is_file() {
                adder.add(Role::MemoryIndex, index, None, entry, index_state.clone());
            }
            let mut cut = false;
            for path in markdown(&place, Some("MEMORY.md"), &mut cut) {
                adder.add(Role::Memory, path, None, entry, file_state.clone());
            }
            adder.said_cut(&place, cut);
        }
    }

    let mut skill_locations: Vec<(&str, State)> = skills
        .paths
        .iter()
        .map(|entry| (entry.as_str(), State::Listed))
        .collect();
    if let Some(state) = theirs {
        let state = match state {
            State::Loaded => State::Listed,
            other => other,
        };
        skill_locations.extend(CLAUDE_SKILLS.map(|entry| (entry, state.clone())));
    }
    for (entry, state) in skill_locations {
        for place in adder.places(entry, roots) {
            let mut cut = false;
            for (name, path) in skill_files(&place, &mut cut) {
                adder.add(Role::Skill, path, Some(name), entry, state.clone());
            }
            adder.said_cut(&place, cut);
        }
    }
    let disabled = std::mem::take(&mut adder.disabled);
    found.disabled = disabled;
    found
}

/// **The most entries one directory is read for** — a location can be a link
/// to a system directory, and finding runs every turn and every key of a
/// `:skill` line (Reviewer E1, round 27). Past it, the place is said to be cut
/// (`Found::notes`).
const SCAN_CAP: usize = 1000;

/// A directory's entries, at most [`SCAN_CAP`], and whether there were more.
fn entries(place: &Path) -> (Vec<PathBuf>, bool) {
    let Ok(entries) = std::fs::read_dir(place) else {
        return (Vec::new(), false);
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .take(SCAN_CAP + 1)
        .collect();
    let cut = paths.len() > SCAN_CAP;
    paths.truncate(SCAN_CAP);
    (paths, cut)
}

/// The `*.md` files a place names: itself when a file, those directly in it
/// when a directory (but `skip`), in name order; none when it is neither.
fn markdown(place: &Path, skip: Option<&str>, cut: &mut bool) -> Vec<PathBuf> {
    if place.is_file() {
        return vec![place.to_path_buf()];
    }
    let (entries, more) = entries(place);
    *cut |= more;
    let mut files: Vec<PathBuf> = entries
        .into_iter()
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|x| x == "md")
                && p.file_name()
                    .is_some_and(|n| Some(n.to_string_lossy().as_ref()) != skip)
        })
        .collect();
    files.sort();
    files
}

/// The skills a place names, `(name, SKILL.md)`, in name order: a file is one
/// skill (named by its directory when it is a `SKILL.md`, else by its stem); a
/// directory holding `SKILL.md` is one; any other directory is each
/// `<name>/SKILL.md` in it.
fn skill_files(place: &Path, cut: &mut bool) -> Vec<(String, PathBuf)> {
    let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    if place.is_file() {
        let name = match place.file_name().is_some_and(|n| n == "SKILL.md") {
            true => place.parent().and_then(name_of),
            false => place.file_stem().map(|s| s.to_string_lossy().into_owned()),
        };
        return name.map(|n| (n, place.to_path_buf())).into_iter().collect();
    }
    let own = place.join("SKILL.md");
    if own.is_file() {
        return name_of(place).map(|n| (n, own)).into_iter().collect();
    }
    let (entries, more) = entries(place);
    *cut |= more;
    let mut skills: Vec<(String, PathBuf)> = entries
        .into_iter()
        .filter_map(|dir| {
            let file = dir.join("SKILL.md");
            file.is_file().then(|| name_of(&dir).map(|n| (n, file)))?
        })
        .collect();
    skills.sort();
    skills
}

/// Adds found files once each, judging every one as it comes.
///
/// Sets, not lists: a repository can hold thousands of skills, and a scan
/// per file made finding quadratic — 1.7 s at 4k, on every turn and every
/// key of a `:skill` line (Reviewer B, round 27).
struct Adder<'a> {
    found: &'a mut Found,
    /// Each file placed, as it resolves: a symlinked twin is one file.
    seen: HashSet<PathBuf>,
    /// The first usable skill of each name — what a later one is shadowed by.
    skills: HashMap<String, PathBuf>,
    /// The repository's top, resolved: what a relative location's files
    /// must resolve inside (`locate::leaves_the_repository`).
    repository: PathBuf,
    sensitive: &'a dyn Fn(&Path) -> Option<String>,
    /// The write location, whatever the mode: what under it is the model's.
    write: Option<PathBuf>,
    /// What you disabled.
    disabled: DisabledSet,
}

impl Adder<'_> {
    /// The places `entry` names, a note when a placeholder it needs has none.
    fn places(&mut self, entry: &str, roots: &Roots) -> Vec<PathBuf> {
        let places = locate::places(entry, roots);
        if places.is_empty() && !entry.trim().is_empty() {
            self.found.notes.push(format!(
                "`{entry}` names nothing here: a placeholder in it has no value"
            ));
        }
        places
    }

    /// Say a place was read only in part ([`SCAN_CAP`]).
    fn said_cut(&mut self, place: &Path, cut: bool) {
        if cut {
            self.found.notes.push(format!(
                "`{}` holds more than {SCAN_CAP} entries: only {SCAN_CAP} were read",
                place.display()
            ));
        }
    }

    fn add(
        &mut self,
        role: Role,
        path: PathBuf,
        name: Option<String>,
        location: &str,
        state: State,
    ) {
        let real = resolved(&path);
        // Why Shannon may not read it: the judge's (a credential path, a
        // `.shannon/` not trusted), as named or as it resolves; or a
        // repository's file that leads out of the repository.
        let refused = never_read(&path, &real, self.sensitive).or_else(|| {
            locate::leaves_the_repository(location, &real, &self.repository).then(|| {
                format!(
                    "leads out of the repository, to {} — never read",
                    real.display()
                )
            })
        });
        if !self.seen.insert(real.clone()) {
            return;
        }
        let name = name.unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        // A skill's or memory's head, read once: its description, and a
        // skill's frontmatter name where it is not its directory's.
        let (description, alias) = match (role, &refused) {
            (Role::Skill | Role::Memory, None) => head(&path),
            _ => (None, None),
        };
        let alias = alias.filter(|a| role == Role::Skill && *a != name);
        let state = match (&refused, state) {
            // Claude Code reads its own; Shannon reads nothing of it.
            (Some(_), State::ClaudeCode) => State::ClaudeCode,
            (Some(why), _) => State::Refused(why.clone()),
            (None, state) => state,
        };
        // Disabled before a name is claimed, so a disabled skill shadows
        // nothing; a credential path stays refused, which says more.
        let names = [name.as_str(), alias.as_deref().unwrap_or_default()];
        let state = match state {
            State::Refused(_) => state,
            _ if self.disabled.why(role, &names, &path, &real).is_some() => State::Disabled,
            state => state,
        };
        let state = match state {
            State::Listed | State::ClaudeCode if role == Role::Skill => {
                match self.skills.get(&name) {
                    Some(first) => State::Refused(format!("shadowed by {}", first.display())),
                    None => {
                        self.skills.insert(name.clone(), path.clone());
                        state
                    }
                }
            }
            state => state,
        };
        let writable = self.write.as_ref().is_some_and(|w| path.starts_with(w));
        self.found.items.push(Item {
            role,
            path,
            name,
            alias,
            description,
            location: location.to_string(),
            state,
            writable,
        });
    }
}

/// Where `path` leads, links followed — itself when it cannot be resolved.
pub(crate) fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// **What you disabled**, its locations placed: `[skills] disabled`'s names
/// and places, and `[memory] disabled`'s places. The ONE predicate: a found
/// file is put to it (`Adder::add`), the gate's refusals are built from
/// what it marked (`places::rules`), every door that loads a skill asks it
/// ([`Found::skill_disabled`]), and `x` asks it which entry to take back
/// ([`why`](Self::why)) — four copies of the rule disagreed (review 29,
/// Reviewer C).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisabledSet {
    /// `[skills] disabled`'s names. Each entry is kept as written, as `-=`
    /// takes back only that spelling; it is compared trimmed.
    skill_names: Vec<String>,
    skill_places: Vec<Placed>,
    memory_places: Vec<Placed>,
}

/// One entry naming a place: as written, the places it names, and where
/// they lead (a link matched either way).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    entry: String,
    places: Vec<PathBuf>,
    real: Vec<PathBuf>,
}

/// **Which entry disables a file** — and whether it names that file alone
/// (`x` can take it back) or a place holding more (`x` says which).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Why {
    pub entry: String,
    pub exact: bool,
}

impl DisabledSet {
    pub(crate) fn of(memory: &MemoryConfig, skills: &SkillsConfig, roots: &Roots) -> DisabledSet {
        let placed = |entry: &String| {
            let places = locate::places(entry, roots);
            Placed {
                entry: entry.clone(),
                real: places.iter().map(|p| resolved(p)).collect(),
                places,
            }
        };
        let (places, names): (Vec<&String>, Vec<&String>) =
            skills.disabled.iter().partition(|e| names_a_place(e));
        DisabledSet {
            skill_names: names.into_iter().cloned().collect(),
            skill_places: places.into_iter().map(placed).collect(),
            memory_places: memory.disabled.iter().map(placed).collect(),
        }
    }

    /// Nothing disabled at all.
    pub fn is_empty(&self) -> bool {
        self.skill_names.is_empty() && self.skill_places.is_empty() && self.memory_places.is_empty()
    }

    /// Every place an entry names, found or not yet.
    pub(crate) fn places(&self) -> impl Iterator<Item = &PathBuf> {
        self.skill_places
            .iter()
            .chain(&self.memory_places)
            .flat_map(|p| &p.places)
    }

    /// **The entry that disables a skill CALLED `name`, by its name** —
    /// `/review` is `review` (the `/` is how a skill is typed, not part of
    /// its name), and a plain name
    /// holds a plugin's `<plugin>:review` too, as `:help disabled`
    /// says; a plugin-qualified entry holds only that one (review 29,
    /// Reviewers A and C: `/review` was asked, not refused).
    pub fn skill_named(&self, name: &str) -> Option<&str> {
        let name = name.trim().trim_start_matches('/');
        let bare = bare_skill_name(name);
        self.skill_names
            .iter()
            // `bare` holds no `:`, so only a plain entry can equal it.
            .find(|d| {
                let d = d.trim();
                d == name || d == bare
            })
            .map(String::as_str)
    }

    /// **Which entry disables the file at `path`** (leading to `real`),
    /// found as `role` and called `names` — a skill by a name, or any file
    /// by a place it is in, as spelled or as it resolves.
    pub fn why(&self, role: Role, names: &[&str], path: &Path, real: &Path) -> Option<Why> {
        if role == Role::Skill
            && let Some(entry) = names
                .iter()
                .filter(|n| !n.is_empty())
                .find_map(|n| self.skill_named(n))
        {
            return Some(Why {
                entry: entry.to_string(),
                exact: true,
            });
        }
        // A skill is its directory: a place naming it, or its `SKILL.md`,
        // names it alone.
        let own = |place: &Path| {
            place == path
                || place == real
                || (role == Role::Skill
                    && (Some(place) == path.parent() || Some(place) == real.parent()))
        };
        let places = match role {
            Role::Skill => &self.skill_places,
            Role::Instructions | Role::MemoryIndex | Role::Memory => &self.memory_places,
        };
        places
            .iter()
            .find(|p| {
                p.places.iter().any(|place| path.starts_with(place))
                    || p.real.iter().any(|place| real.starts_with(place))
            })
            .map(|p| Why {
                entry: p.entry.clone(),
                exact: p.places.iter().chain(&p.real).any(|place| own(place)),
            })
    }
}

/// A skill's name as Claude Code's `Skill` call may spell it, bare: one
/// leading `/` gone, a plugin's `<plugin>:` too.
fn bare_skill_name(name: &str) -> &str {
    let name = name.trim().trim_start_matches('/');
    name.rsplit(':').next().unwrap_or(name)
}

/// A `[skills] disabled` entry naming a location rather than a skill: a
/// path, or one led by `~` or a placeholder. A skill's name is a directory's,
/// and holds no `/`.
fn names_a_place(entry: &str) -> bool {
    let entry = entry.trim();
    entry.contains('/') || entry.starts_with('~') || entry.starts_with('{')
}

/// **Why a file is never read — as named, or as it resolves** (`real`,
/// [`resolved`]): the judge's whole sentence (the app's: a credential path
/// as the gate judges one, a `.shannon/` whose directory is not trusted).
/// The judge reads text and a read follows links: a repository's
/// `AGENTS.md` linked to `~/.ssh/id_rsa` was judged by its own name and the
/// key sent with every turn (Reviewer B, round 27). Both readers of a found
/// file ask this one: `Adder::add` and [`skill_text`].
pub(crate) fn never_read(
    path: &Path,
    real: &Path,
    sensitive: &dyn Fn(&Path) -> Option<String>,
) -> Option<String> {
    sensitive(path).or_else(|| (real != path).then(|| sensitive(real)).flatten())
}

/// A skill's or memory's `description` and frontmatter `name`, from the
/// head of its file.
fn head(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(head) = read::bounded(path, HEAD) else {
        return (None, None);
    };
    let (keys, _) = read::frontmatter(&head.text);
    let key = |k: &str| read::key(&keys, k).map(str::to_string);
    (key("description"), key("name"))
}
