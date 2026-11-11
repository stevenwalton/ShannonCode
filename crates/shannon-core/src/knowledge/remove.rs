//! **Removing a memory or a skill** — what goes, said before it goes
//! (Steven, 10-06: "remove a memory or skill", on `:memories`' `d`).
//!
//! A memory is its file and its lines in the index beside it; a skill, its
//! directory. Never a location you configured, nor anything holding one —
//! that is taken out of the config, not deleted — nor an instruction file or
//! an index, which have their own door (`$EDITOR`, `x`), nor a file not
//! read, nor what a repository's location leads to outside it.

use std::path::{Path, PathBuf};

use super::{Item, Role, Roots, State, locate};
use crate::config::{MemoryConfig, SkillsConfig};

/// **What removing one found file takes with it** — worked out from the
/// file as it is, before the question and again when it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// Removed whole: a memory's file, or a skill's directory (a link is
    /// unlinked, never followed).
    pub target: PathBuf,
    /// Where it is, links in the path above it followed: what is deleted.
    /// Named in the question when it is not the target's own spelling.
    pub real: PathBuf,
    /// A memory's index, and how many of its lines name the memory alone:
    /// taken out with it, so no line points at nothing.
    pub index: Option<(PathBuf, usize)>,
    /// An index there that could not be read, and why: its lines stay.
    pub index_unread: Option<(PathBuf, String)>,
    /// How many files and directories go — said in the question; `capped`
    /// when the count stopped at `COUNT_CAP`.
    pub files: usize,
    pub capped: bool,
}

/// The most entries of a skill's directory counted, and of an index read.
const COUNT_CAP: usize = 1000;
const INDEX_CAP: u64 = 1 << 20;

/// **What removing `item` takes** — or why it is not removed here, a
/// sentence for the screen. `memory` and `skills` are the config: a removal
/// that would take ANY location it names is refused, not only the one the
/// file was found by (review 29, Reviewers A and B: a skill configured as
/// its `SKILL.md`, and a memory a second location names, were removable).
pub fn removal(
    item: &Item,
    roots: &Roots,
    memory: &MemoryConfig,
    skills: &SkillsConfig,
) -> Result<Removal, String> {
    if let State::Refused(why) = &item.state {
        return Err(format!(
            "not read ({why}) — Shannon removes only what it reads"
        ));
    }
    let target = match (item.role, item.path.file_name()) {
        (Role::Skill, Some(name)) if name == "SKILL.md" => {
            item.path.parent().unwrap_or(&item.path).to_path_buf()
        }
        _ => item.path.clone(),
    };
    match item.role {
        Role::Instructions => {
            return Err(
                "an instruction file — yours to delete by hand; `x` stops it being read".into(),
            );
        }
        Role::MemoryIndex => {
            return Err(
                "a memory location's index — Enter edits it; `d` on a memory removes it \
                        and its line here"
                    .into(),
            );
        }
        Role::Memory | Role::Skill => {}
    }
    // Taking `target` takes a location when the location is it or is IN
    // it — any location the config names.
    if let Some(entry) = holds_a_location(&target, roots, memory, skills) {
        return Err(format!(
            "it holds a location you configured (`{entry}`) — take that out of the config \
             instead"
        ));
    }
    // What is deleted is where the path leads: a repository's location that
    // links out of it would delete there (review 29, Reviewer B: a repo's
    // `.claude/skills` linked to a shared directory, in a Claude Code
    // session, where such a file is `claude's`, not refused).
    let real = real_path(&target);
    let repository = locate::repository(roots);
    if locate::leaves_the_repository(&item.location, &real, &repository) {
        return Err(format!(
            "it leads out of the repository, to {} — remove it there by hand",
            real.display()
        ));
    }
    let (index, index_unread) = match item.role {
        Role::Memory => index_lines(&item.path),
        _ => (None, None),
    };
    let (files, capped) = count(&target);
    Ok(Removal {
        target,
        real,
        index,
        index_unread,
        files,
        capped,
    })
}

/// The first location the config names that `target` is or holds.
fn holds_a_location(
    target: &Path,
    roots: &Roots,
    memory: &MemoryConfig,
    skills: &SkillsConfig,
) -> Option<String> {
    let claude = [super::CLAUDE_MEMORY]
        .into_iter()
        .chain(super::CLAUDE_INSTRUCTIONS)
        .chain(super::CLAUDE_SKILLS)
        .map(str::to_string);
    memory
        .instructions
        .iter()
        .chain(&memory.paths)
        .chain([&memory.write])
        .chain(&skills.paths)
        .cloned()
        .chain(claude)
        .find(|entry| {
            locate::places(entry, roots)
                .iter()
                .any(|place| place.starts_with(target))
        })
}

/// `path` with every link ABOVE it followed — what a removal of it deletes:
/// its last part is unlinked as itself, never followed.
fn real_path(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => super::resolved(parent).join(name),
        _ => path.to_path_buf(),
    }
}

impl Removal {
    /// **The question**, naming what goes — where it really is, when a link
    /// above it leads elsewhere.
    pub fn question(&self, name: &str) -> String {
        let at = match self.real == self.target {
            true => self.target.display().to_string(),
            false => format!("{} (→ {})", self.target.display(), self.real.display()),
        };
        let mut what = match (self.index.as_ref(), self.files, self.capped) {
            (Some((index, n)), ..) => {
                format!("{at} and its {} in {}", lines(*n), index.display())
            }
            (None, 1, false) => at,
            (None, n, false) => format!("{at} — {n} files"),
            (None, n, true) => format!("{at} — {n}+ files"),
        };
        if let Some((index, why)) = &self.index_unread {
            what.push_str(&format!(
                " ({} is not read: {why} — its lines stay)",
                index.display()
            ));
        }
        format!("remove `{name}`: {what}? It is deleted, not hidden — `x` disables instead.")
    }

    /// **Remove it** — and what was done, a sentence; or why not.
    pub fn apply(&self) -> Result<String, String> {
        // What is THERE, a link as a link. Not a guard: `remove_file` and
        // `remove_dir_all` both remove a link itself, never what it names
        // (std's documented rule; a link to a memory outside is tested) —
        // this only says so where the choice is made.
        let meta = std::fs::symlink_metadata(&self.target)
            .map_err(|e| format!("{} could not be read: {e}", self.target.display()))?;
        let removed = match meta.is_dir() {
            true => std::fs::remove_dir_all(&self.target),
            false => std::fs::remove_file(&self.target),
        };
        removed.map_err(|e| format!("{} NOT removed: {e}", self.target.display()))?;
        let mut said = format!("removed {}", self.real.display());
        if let Some((index, _)) = &self.index {
            match drop_lines(index, &self.target) {
                Ok(0) => {}
                Ok(n) => said.push_str(&format!(", and its {} in {}", lines(n), index.display())),
                Err(why) => said.push_str(&format!(" — its index NOT changed: {why}")),
            }
        }
        Ok(said)
    }
}

fn lines(n: usize) -> String {
    match n {
        1 => "line".into(),
        n => format!("{n} lines"),
    }
}

/// The index beside a memory and how many of its lines name it alone —
/// or, when an index is there but cannot be read, why.
#[allow(clippy::type_complexity)]
fn index_lines(memory: &Path) -> (Option<(PathBuf, usize)>, Option<(PathBuf, String)>) {
    let Some(index) = memory.parent().map(|dir| dir.join("MEMORY.md")) else {
        return (None, None);
    };
    if std::fs::symlink_metadata(&index).is_err() {
        return (None, None);
    }
    match read_index(&index) {
        Ok(text) => {
            let n = text.lines().filter(|l| names_alone(l, memory)).count();
            ((n > 0).then_some((index, n)), None)
        }
        Err(why) => (None, Some((index, why))),
    }
}

/// Does an index line link `memory` — `](old.md)` or `](./old.md)` — and
/// nothing else? A line linking another memory too stays: taking it would
/// take that one out of the index (review 29, Reviewer B).
fn names_alone(line: &str, memory: &Path) -> bool {
    names(line, memory) && line.matches("](").count() == 1
}

/// Does an index line link `memory` — `](old.md)` or `](./old.md)`?
fn names(line: &str, memory: &Path) -> bool {
    let Some(file) = memory.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    line.contains(&format!("]({file})")) || line.contains(&format!("](./{file})"))
}

/// Take `memory`'s lines out of `index`, every other line kept as it was
/// (its line ending too); how many went.
fn drop_lines(index: &Path, memory: &Path) -> Result<usize, String> {
    let text = read_index(index)?;
    let mut gone = 0;
    let kept: String = text
        .split_inclusive('\n')
        .filter(|line| {
            let drop = names_alone(line.trim_end_matches(['\r', '\n']), memory);
            gone += usize::from(drop);
            !drop
        })
        .collect();
    if gone > 0 {
        std::fs::write(index, kept).map_err(|e| e.to_string())?;
    }
    Ok(gone)
}

/// An index as text: a regular file, not past [`INDEX_CAP`] — read so to
/// be rewritten, here and by `create`'s `append_line`.
pub(super) fn read_index(index: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(index).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > INDEX_CAP {
        return Err("not a regular file of a readable size".into());
    }
    std::fs::read_to_string(index).map_err(|e| e.to_string())
}

/// The files under `dir` (itself, if a file), and whether the count stopped
/// at [`COUNT_CAP`] — EVERY entry visited counts against it, a directory
/// too, and a listing is read no further than the cap: 65k empty
/// directories took a second on the screen's thread (review 29, Reviewer B).
/// Links are not followed.
fn count(dir: &Path) -> (usize, bool) {
    let mut stack = vec![dir.to_path_buf()];
    let (mut files, mut visited) = (0, 0);
    while let Some(here) = stack.pop() {
        if visited >= COUNT_CAP {
            return (files, true);
        }
        visited += 1;
        let Ok(meta) = std::fs::symlink_metadata(&here) else {
            continue;
        };
        if !meta.is_dir() {
            files += 1;
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&here) {
            let room = COUNT_CAP.saturating_sub(visited + stack.len()) + 1;
            stack.extend(entries.filter_map(Result::ok).take(room).map(|e| e.path()));
        }
    }
    (files, false)
}
