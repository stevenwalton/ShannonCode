//! **A new memory or skill, from a template** — `:memories`' `n` (Steven,
//! 10-06: create memories and skills, "to work *with* the agent"; `N` asks
//! the model instead, and lives in the app).
//!
//! The template is written, `$EDITOR` opens it, and [`Draft::settle`] reads
//! what came back: left as it was, it is taken away again — save-to-apply,
//! as everywhere Shannon opens `$EDITOR` for you — and a memory saved gets
//! its line in the index beside it, as the model is told to keep one.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::read;

/// **A file made from a template, waiting on your edit.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The file `$EDITOR` opens.
    pub path: PathBuf,
    /// What was written, to tell an edit from none.
    pub template: String,
    /// What to take away when nothing was written: the file, or a skill's
    /// directory, made for it.
    made: PathBuf,
    /// A memory's index, for its line.
    index: Option<PathBuf>,
}

/// **A name as a file's** — lower case, a word a `-`, nothing a path could
/// read as more than one name; `None` when nothing is left.
pub fn slug(name: &str) -> Option<String> {
    let mut out = String::new();
    for c in name.trim().chars() {
        match c {
            c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            '-' | '_' | ' ' | '.' if !out.is_empty() && !out.ends_with('-') => out.push('-'),
            _ => {}
        }
    }
    let out = out.trim_end_matches('-').to_string();
    (!out.is_empty()).then_some(out)
}

/// **A new memory `name` in `dir`** — the format the model is told to keep
/// (`context::guidance`, Claude Code's own): frontmatter, then the fact.
pub fn draft_memory(dir: &Path, name: &str) -> Result<Draft, String> {
    let slug = slug(name).ok_or("a memory needs a name of letters or digits")?;
    // `memory.md` IS `MEMORY.md` where case is not told apart (macOS):
    // its index line went into itself, and `find` listed it as the index
    // (review 29, Reviewer B). A slug is lower case already.
    if slug == "memory" {
        return Err("`memory` is its index's name (MEMORY.md) — choose another".into());
    }
    let path = dir.join(format!("{slug}.md"));
    let template = format!(
        "---\nname: {}\ndescription: \ntype: user\n---\n\n",
        name.trim()
    );
    Ok(Draft {
        made: path.clone(),
        index: Some(dir.join("MEMORY.md")),
        path,
        template,
    })
}

/// **A new skill `name` under `place`** — `<name>/SKILL.md`, its
/// `description` what the model reads to decide when to use it.
pub fn draft_skill(place: &Path, name: &str) -> Result<Draft, String> {
    let slug = slug(name).ok_or("a skill needs a name of letters or digits")?;
    let dir = place.join(&slug);
    let template = format!(
        "---\nname: {slug}\ndescription: \n---\n\n# {}\n\n\
         The steps; $ARGUMENTS are the words after its name.\n",
        name.trim()
    );
    Ok(Draft {
        path: dir.join("SKILL.md"),
        made: dir,
        index: None,
        template,
    })
}

impl Draft {
    /// **Write the template** — refused where something is already there
    /// (Enter edits that), the directories made `0700`, the file `0600` and
    /// new (never through a link).
    pub fn write(&self) -> Result<(), String> {
        if std::fs::symlink_metadata(&self.made).is_ok() {
            return Err(format!(
                "{} is there already — Enter on it edits it",
                self.made.display()
            ));
        }
        let parent = self.path.parent().ok_or("no directory to make it in")?;
        make_dirs(parent)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&self.path)
            .map_err(|e| format!("{} NOT made: {e}", self.path.display()))?;
        file.write_all(self.template.as_bytes())
            .map_err(|e| format!("{} NOT written: {e}", self.path.display()))
    }

    /// **After the edit — what became of it, a sentence**: left as written,
    /// or deleted in the editor, nothing is made (what was made for it is
    /// taken away); saved, it is kept, and a memory's line added to its
    /// index (made if missing) when it has a `name` and a `description`.
    /// A file that cannot be read back is left as it is, its index not
    /// changed: "made" was said of a file the editor had deleted, and its
    /// empty line `- [](gone.md) — ` told every turn (review 29, M5).
    pub fn settle(&self) -> Result<String, String> {
        let now = match std::fs::read_to_string(&self.path) {
            Ok(now) => now,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.discard()?;
                return Ok(format!(
                    "nothing made — {} was deleted in the editor",
                    self.path.display()
                ));
            }
            Err(e) => {
                return Err(format!(
                    "{} could not be read back: {e} — left as it is, no index line",
                    self.path.display()
                ));
            }
        };
        if now == self.template {
            self.discard()?;
            return Ok(format!(
                "nothing made — {} was left as written",
                self.path.display()
            ));
        }
        let mut said = format!("made {}", self.path.display());
        if let Some(index) = &self.index {
            let (keys, _) = read::frontmatter(&now);
            let key = |k: &str| read::key(&keys, k).unwrap_or_default().trim().to_string();
            let (name, description) = (key("name"), key("description"));
            let file = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.is_empty() || description.is_empty() {
                said.push_str(&format!(
                    " — no line in {}: it needs a `name` and a `description`",
                    index.display()
                ));
                return Ok(said);
            }
            match append_line(index, &format!("- [{name}]({file}) — {description}")) {
                Ok(()) => said.push_str(&format!(", and its line in {}", index.display())),
                Err(why) => said.push_str(&format!(" — its index NOT changed: {why}")),
            }
        }
        Ok(said)
    }

    /// **Take away what was made for it** — the file, or a skill's
    /// directory; nothing there is nothing to do.
    fn discard(&self) -> Result<(), String> {
        let gone = match std::fs::symlink_metadata(&self.made) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&self.made),
            _ => std::fs::remove_file(&self.made),
        };
        gone.map_err(|e| format!("{} left, unwritten: {e}", self.made.display()))
    }
}

/// Make `dir` and those above it, each made `0700`.
fn make_dirs(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|e| format!("{} NOT made: {e}", dir.display()))
}

/// Add `line` to the index at `index` — a regular file of a readable size
/// (`remove::read_index`, the door both rewriters of an index read it by:
/// one unbounded was read whole), or made new, `0600`; never through a
/// link. One there keeps its mode.
fn append_line(index: &Path, line: &str) -> Result<(), String> {
    let text = match std::fs::symlink_metadata(index) {
        Ok(_) => Some(super::remove::read_index(index)?),
        Err(_) => None,
    };
    let mut out = text.clone().unwrap_or_default();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    if text.is_some() {
        return std::fs::write(index, out).map_err(|e| e.to_string());
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(index).map_err(|e| e.to_string())?;
    file.write_all(out.as_bytes()).map_err(|e| e.to_string())
}
