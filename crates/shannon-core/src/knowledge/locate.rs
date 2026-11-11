//! **Where a location names** — the one grammar `[memory]` and `[skills]`
//! share (`docs/DesignDocs/memory-and-skills.md` §7).
//!
//! A location beginning with `~`, `/`, `{config}`, `{start}` or
//! `{claude-memory}` is PLACED: that one place, its placeholders filled.
//! Anything else is RELATIVE: looked
//! for in where Shannon started and each directory above it up to the
//! project's root (the nearest holding one of `[memory] project_root`'s
//! markers, `.git` among them), the root's first — the order a more specific
//! file is read after a more general one. With no root above, only where
//! Shannon started.

use std::path::{Path, PathBuf};

use super::Roots;

/// The places `entry` names, in the order they are read. Empty when a
/// placeholder it needs has no value (no config directory, no home) — said by
/// the caller, who knows which list it was in.
pub(crate) fn places(entry: &str, roots: &Roots) -> Vec<PathBuf> {
    let entry = entry.trim();
    if entry.is_empty() {
        return Vec::new();
    }
    match placed(entry, roots) {
        Some(Ok(path)) => vec![path],
        Some(Err(())) => Vec::new(),
        None => walk(roots)
            .into_iter()
            .map(|dir| dir.join(fill(entry, roots)))
            .collect(),
    }
}

/// The one place a PLACED location names, `Err` when a placeholder it leads
/// with has no value; `None` for a relative one.
fn placed(entry: &str, roots: &Roots) -> Option<Result<PathBuf, ()>> {
    if is_relative(entry) {
        return None;
    }
    let filled = |lead: Option<&Path>, rest: &str| {
        lead.map(|lead| lead.join(fill(rest.trim_start_matches('/'), roots)))
            .ok_or(())
    };
    Some(if let Some(rest) = entry.strip_prefix("{config}") {
        filled(roots.config_dir.as_deref(), rest)
    } else if let Some(rest) = entry.strip_prefix(super::CLAUDE_MEMORY) {
        filled(roots.claude_memory.as_deref(), rest)
    } else if let Some(rest) = entry.strip_prefix(crate::secrets::CLAUDE_HOME) {
        filled(roots.claude_home.as_deref(), rest)
    } else if let Some(rest) = entry.strip_prefix("{start}") {
        filled(Some(&roots.start), rest)
    } else if let Some(rest) = entry.strip_prefix('~') {
        filled(roots.home.as_deref(), rest)
    } else {
        Ok(PathBuf::from(fill(entry, roots)))
    })
}

/// Is `entry` a RELATIVE location — looked for up a walk, not one place? The
/// one answer [`places`] and [`write_place`] read too.
pub(crate) fn is_relative(entry: &str) -> bool {
    let entry = entry.trim();
    !(entry.starts_with('/')
        || entry == "~"
        || entry.starts_with("~/")
        || entry.starts_with("{config}")
        || entry.starts_with(super::CLAUDE_MEMORY)
        || entry.starts_with(crate::secrets::CLAUDE_HOME)
        || entry.starts_with("{start}"))
}

/// `{project}` filled; the other placeholders lead a location or mean nothing.
fn fill(text: &str, roots: &Roots) -> String {
    text.replace("{project}", &roots.project)
}

/// Where Shannon started and each directory above it up to the project's
/// root — the nearest holding a `roots.project_root` marker, a file or a
/// directory (`.git` is either) — the root's first. Only `start` when none
/// does, or the list is empty. A marker can only choose among `start`'s
/// ancestors, so no value walks further than `/`.
pub(crate) fn walk(roots: &Roots) -> Vec<PathBuf> {
    let start = &roots.start;
    let above: Vec<&Path> = start.ancestors().collect();
    let marked = |dir: &Path| roots.project_root.iter().any(|m| dir.join(m).exists());
    match above.iter().position(|dir| marked(dir)) {
        Some(root) => above[..=root]
            .iter()
            .rev()
            .map(|d| d.to_path_buf())
            .collect(),
        None => vec![start.to_path_buf()],
    }
}

/// **The top of the walk, as it resolves** — what a relative location's
/// files and places must stay inside (`leaves_the_repository`).
pub(crate) fn repository(roots: &Roots) -> PathBuf {
    let top = walk(roots)
        .into_iter()
        .next()
        .unwrap_or_else(|| roots.start.clone());
    super::resolved(&top)
}

/// **Does a file or place a RELATIVE location found lead out of the
/// repository?** Such a location is the repository's content, and so is a
/// link in it: `AGENTS.md -> ~/.bash_history` was read and told every
/// session, unasked — no credential, so the credential judge let it pass
/// (Reviewer E1, round 27; reads stay inside approved places, Steven
/// 10-05). A placed location (`~`, `/`, `{config}`) is yours, and follows
/// its links. `real` is the resolved path ([`super::resolved`]).
pub(crate) fn leaves_the_repository(entry: &str, real: &Path, repository: &Path) -> bool {
    is_relative(entry) && !real.starts_with(repository)
}

/// The place a WRITE location names: placed, or a relative one where Shannon
/// started — one directory, never a walk.
pub(crate) fn write_place(entry: &str, roots: &Roots) -> Option<PathBuf> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    match placed(entry, roots) {
        Some(found) => found.ok(),
        None => Some(roots.start.join(fill(entry, roots))),
    }
}
