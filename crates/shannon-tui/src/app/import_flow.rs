//! **Claude Code conversations, brought in** — `:import` (and `:graph`'s
//! listing) turns one into an ordinary Shannon session
//! (`docs/DesignDocs/claude-code-backend.md`, "Claude Code conversations in
//! `:graph`"; Steven, 10-04: continued "as a Shannon session", the resume path
//! by default and the older history on ask).
//!
//! The conversation is replayed through the calls a live turn makes —
//! `begin_turn_under`, `push_chunk`, `push_tool_use`, `finish_tool_round`,
//! `commit_turn_holding` — so the session it makes is shaped as one Shannon
//! made, and lands through the door every composed session does
//! (`adopt_session`, lazy: written on its first turn or `:write`). Every text,
//! call input and result is SCRUBBED on the way: Claude Code's transcripts
//! hold them as its model saw them.
//!
//! Each imported turn is sealed under `claude-code/<model>` — what produced
//! it, as provenance should say — and the session's own model stays the one
//! you were using, which is what continues it. Turns carry the time of the
//! import, not of the conversation (the replay has no clock of its own).
//!
//! A child module of `app`, as `setup_flow` is.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use shannon_core::claude_import::{self, Earlier, Imported, Scope};
use shannon_core::{Conversation, Message, Role, TurnConfig};
use shannon_core::{SHORT_ID_LEN, short_hex};

use super::claude_reads::Reading;
use super::config_file::{config_path_arg, tilde};
use super::*;
use crate::claude_sessions::{Consent, normal};
use crate::graph_view::{ClaudeSection, Through};

/// How much of a Claude Code conversation's title a row shows.
const TITLE_CHARS: usize = 80;

/// **Why a door refused to read** — the listing's and a reading's, said
/// alike (review stop 5, C 6: each wrote it).
pub(crate) const NOT_ALLOWED: &str =
    "not read — Shannon may not read Claude Code's conversations there yet";

/// **An import waiting on the user's leave to read** — what `y` or `a` to
/// `ConfirmAction::ReadClaude` brings in, as
/// [`App::import_conversation`] was asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Import {
    path: PathBuf,
    scope: Scope,
    through: Option<Through>,
}

impl Import {
    /// The conversation, as the question names it: its id's start.
    pub(crate) fn name(&self) -> String {
        let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        short_hex(stem).to_string()
    }

    /// The file, as the gate names and reads it (folded: `normal`).
    pub(crate) fn file(&self) -> &Path {
        &self.path
    }
}

impl App {
    /// **Where Claude Code keeps its conversations** — launch's (`main`),
    /// folded as leave is keyed (`claude_sessions::normal`): a relative
    /// `CLAUDE_CONFIG_DIR` was kept relative, recorded as a name, and asked
    /// about again under its absolute spelling (review stop 5, E1 F2).
    /// Every directory Shannon asks about is joined from this one.
    pub fn set_claude_projects(&mut self, projects: Option<PathBuf>) {
        self.claude_projects = projects.map(|p| normal(&p));
    }

    /// **The directory of Claude Code's the section reads** — this one's;
    /// `None` when Claude Code's directory is unknown, or holds nothing for
    /// this one. What the user's leave to read is given for
    /// (`ClaudeSessions::may_read`).
    ///
    /// **This one's only** (Steven, 10-21: "I don't think we need to show
    /// _all_ claude code conversations. Only what's also in the same path.
    /// Let's keep it simple"). `a` listed every directory's too, a scope the
    /// two design reviews of the graph buffer could not agree where to keep.
    pub(crate) fn claude_dir(&self) -> Option<PathBuf> {
        let projects = self.claude_projects.clone()?;
        std::env::current_dir()
            .ok()
            .and_then(|cwd| claude_import::project_dir(&projects, &cwd))
    }

    /// **May Shannon read there** — asked by the three doors every reading
    /// of a Claude Code conversation goes through: the listing
    /// ([`claude_list`](Self::claude_list): each file's tail, for its
    /// title), a reading (`claude_reading`: the whole lineage), and an
    /// import ([`import_conversation`](Self::import_conversation), which asks
    /// and imports on the answer).
    pub(crate) fn may_read_claude(&self, dir: &Path) -> bool {
        self.claude_consent(dir) == Consent::Given
    }

    /// **What the user said about reading `dir`** — leave for it, or for
    /// Claude Code's projects directory above it (`ClaudeSessions::may_read`,
    /// the one rule). Asked by [`may_read_claude`](Self::may_read_claude)
    /// and by the first `:graph`'s "asked yet?".
    pub(crate) fn claude_consent(&self, dir: &Path) -> Consent {
        self.claude_sessions
            .may_read(dir, self.claude_projects.as_deref())
    }

    /// **May a section list this directory's conversations?** — where the
    /// user let Shannon read, or where there is nothing there to read
    /// (nothing to ask about). The one answer `claude_section` lists by and
    /// `relist_claude_sections` compares a section with (review stop 5, C 6:
    /// each spelled it).
    fn may_list_claude(&self) -> bool {
        self.claude_dir()
            .is_none_or(|dir| self.may_read_claude(&dir))
    }

    /// **`dir`'s conversations, listed** — the one door to
    /// `claude_import::list` (the section, the match for older imports),
    /// and only where the user let Shannon read.
    pub(crate) fn claude_list(&self, dir: &Path) -> Result<Vec<claude_import::Found>, String> {
        if !self.may_read_claude(dir) {
            return Err(NOT_ALLOWED.into());
        }
        claude_import::list(dir)
    }

    /// **Ask whether Shannon may read Claude Code's conversations** — this
    /// directory's — unless that is allowed, there is nothing there, or the
    /// question is up already.
    pub(crate) fn ask_to_read_claude(&mut self) {
        let Some(dir) = self.claude_dir() else {
            return;
        };
        let asked = self
            .pending_confirm
            .iter()
            .any(|q| matches!(q, ConfirmAction::ReadClaude { dir: d, .. } if *d == dir));
        if !asked && !self.may_read_claude(&dir) {
            self.ask(ConfirmAction::ReadClaude { dir, then: None });
        }
    }

    /// **`y` or `a` to reading `dir`** — allowed (`always`: kept), and every
    /// `:graph` showing a section not read there lists it now, the match for
    /// older imports first, as opening `:graph` does it; then the import
    /// that waited on it, if one did.
    pub(crate) fn let_read_claude(&mut self, dir: &Path, always: bool, then: Option<Box<Import>>) {
        if let Err(why) = self.claude_sessions.allow(dir, always) {
            self.notify(why);
        } else if always {
            // Where it was given, by name unless it is here: an import's
            // directory may be any (review stop 5, A 1 — "for this
            // directory" was said of the home directory).
            let here = self.claude_dir().map(|d| normal(&d));
            let whose = if here == Some(normal(dir)) {
                "for this directory".to_string()
            } else {
                format!("in {}", tilde(dir))
            };
            self.notify(format!(
                "Claude Code's conversations {whose} will be read without asking"
            ));
        }
        // As opening `:graph` does it, whichever was allowed: leave for every
        // directory's holds this one's (review stop 5, A 6). It returns at
        // once where this directory still may not be read.
        self.match_old_imports();
        self.relist_claude_sections();
        // The tree too: the match may have found where an old import hangs.
        self.refresh_graph_views();
        if let Some(import) = then {
            self.import_conversation(&import.path, import.scope, import.through.as_ref(), None);
        }
    }

    /// **Every `:graph` whose Claude Code section is not as it would be
    /// listed now, listed again** ([`current_section`](Self::current_section))
    /// — `not read` where leave was taken back, listed where it was given
    /// (leave for every directory's, still on record from before 10-21,
    /// gives this one's too), and scrubbed again where the secrets changed.
    /// What `y`, `:unread` and every change of the secrets do
    /// (`App::rebuild_rules`, the door a mark, an unmark, `:recall` and a
    /// config read again all take), so they cannot disagree.
    ///
    /// **Set aside too** (the window's history, `Window::history`): `:help`
    /// opened in the graph's pane, then `:unread`, and `:q` gave back a tree
    /// still showing the conversations and turns just taken back (design
    /// review A, 10-21, finding 1). One set aside under leave since taken
    /// back lets its section go now — what was read under the old leave is
    /// not kept meanwhile — and is listed again as `:graph` lists one when it
    /// comes back (`App::view_back`); one under other secrets is listed again
    /// as it comes back, keeping what was open (review stop 10, E 2: a value
    /// just marked showed again, unstyled, after `:q`).
    pub(super) fn relist_claude_sections(&mut self) {
        let may = self.may_list_claude();
        for (_, view) in self.windows_mut() {
            for place in view.tree_places_mut() {
                if place.claude().is_some_and(|s| s.listed != may) {
                    place.forget_claude();
                }
            }
        }
        let stale: Vec<crate::wm::PaneId> = (self.windows())
            .filter(|(_, v)| {
                (v.tree())
                    .and_then(GraphView::claude)
                    .is_some_and(|s| !self.listed_as_now(s))
            })
            .map(|(id, _)| id)
            .collect();
        for pane in stale {
            let Some((section, continued)) = (self.pane_graph(pane))
                .and_then(|v| Some((v.claude()?.clone(), v.continued().clone())))
            else {
                continue;
            };
            let section = self.current_section(section, &continued);
            if let Some(view) = self.window_of_mut(pane).and_then(|v| v.tree_mut()) {
                view.set_claude(section);
            }
        }
    }

    /// **`:unread [all]`** — take back Shannon's leave to read Claude Code's
    /// conversations (Steven, 10-18: "I think we should add a command"):
    /// this directory's, or with `all` every one's. As `n` for this run —
    /// `l` on the heading asks again — and an `always` taken back for good.
    /// What was read from there is let go, and every `:graph` says `not
    /// read`. Leave for every directory's still holds this one's: said, and
    /// `all` takes it back.
    pub(crate) fn unread_command(&mut self, args: &str) {
        let all = match args.trim() {
            "" => false,
            "all" => true,
            other => return self.refuse(format!("`:unread` or `:unread all` — not `{other}`")),
        };
        // What the record holds now, another Shannon's `always` included.
        self.claude_sessions.reread_always();
        let here = self.claude_dir();
        // Leave for every directory — Claude Code's projects directory —
        // can still be on record: given before the section listed this
        // directory's only (10-21), or by another Shannon. It still holds
        // this one's, so `all` takes it back too.
        let every_dir = self.claude_projects.clone();
        let dirs: Vec<PathBuf> = match all {
            false => here.clone().into_iter().collect(),
            true => {
                let mut dirs = self.claude_sessions.allowed();
                dirs.extend(here.clone());
                dirs.extend(every_dir.clone());
                dirs.sort();
                dirs.dedup();
                dirs
            }
        };
        if dirs.is_empty() {
            return self.refuse(
                "Claude Code keeps no conversations for this directory — `:unread all` takes \
                 back every directory's",
            );
        }
        let (mut taken, mut every, mut unkept) = (0, false, None);
        for dir in &dirs {
            let held = match self.claude_sessions.take_back(dir) {
                Ok(held) => held,
                Err(why) => {
                    unkept = Some(why);
                    true
                }
            };
            taken += usize::from(held);
            every |= held && every_dir.as_ref() == Some(dir);
            if !self.may_read_claude(dir) {
                self.claude_reads.forget_under(dir);
            }
        }
        // Nor kept on an inspector's page, unshown (review stop 5, E1 F5).
        for inspector in (self.windows_mut()).filter_map(|(_, v)| v.inspector_mut()) {
            inspector.painted.forget_claude();
        }
        self.relist_claude_sections();
        let still = here.as_deref().is_some_and(|dir| self.may_read_claude(dir));
        self.notify(match (all, taken, still) {
            (false, _, true) => "Claude Code's conversations here are still read: leave for every \
                                 directory's holds them — `:unread all` takes it back"
                .to_string(),
            (_, 0, _) => "Shannon had no leave to read Claude Code's conversations there — \
                          nothing taken back"
                .to_string(),
            (false, _, false) => "Claude Code's conversations for this directory are no longer \
                                  read — `l` on their heading in `:graph` asks again"
                .to_string(),
            // Every directory's leave said as that: "for 1 directory" undersold
            // it (review stop 5, C 6).
            (true, n, _) => format!(
                "Claude Code's conversations are no longer read — leave taken back for {} — `l` \
                 on their heading in `:graph` asks again",
                match every {
                    true => "every directory".to_string(),
                    false => crate::view::counted(n, "directory", "directories"),
                }
            ),
        });
        // Last, so it is the one on screen: the next run would still read.
        if let Some(why) = unkept {
            self.notify(why);
        }
    }

    /// **Claude Code's conversations, for `:graph`** — this directory's
    /// ([`claude_dir`](Self::claude_dir)), newest first. `None` when Claude
    /// Code's directory is unknown (and in tests). Listed when asked — on
    /// opening `:graph`, on leave given — and on a rebuild only when the
    /// leave or the secrets changed since ([`current_section`](Self::current_section)):
    /// a listing reads each file's tail for its title.
    ///
    /// The `continued` ones — a session of the tree continues each — are
    /// listed whoever started them (Steven, 10-16, decision 4: one is that
    /// session's root) and READ now, through the one door: the session
    /// hangs at the turn it parted at, which only the reading knows. One
    /// that cannot be read is said, and its session hangs from its row.
    pub(crate) fn claude_section(&mut self, continued: &BTreeSet<String>) -> Option<ClaudeSection> {
        self.claude_projects.as_ref()?;
        let scrubbed = self.scrubber();
        // Only where the user let Shannon read — the listing reads each
        // file's tail for its title.
        if !self.may_list_claude() {
            return Some(ClaudeSection::not_read(scrubbed));
        }
        let mut found = Vec::new();
        if let Some(dir) = self.claude_dir() {
            match self.claude_list(&dir) {
                Ok(listed) => found = listed,
                Err(why) => self.refuse(format!("Claude Code's conversations: {why}")),
            }
        }
        // A conversation Shannon itself ran in Claude Code is a Shannon
        // session's already, in the tree above (Steven, 10-04) — unless a
        // session continues it.
        found.retain(|f| continued.contains(&f.id) || !self.claude_sessions.started(&f.id));
        found.sort_by_key(|f| std::cmp::Reverse(f.modified));
        // Every title scrubbed as the listing enters: Claude Code's title can
        // be the user's last prompt, word for word (`lastPrompt`), and it is
        // drawn on the row, both inspector pages and in what `/` searches
        // (review stop 3, B 1 and C 1). Scrubbed whole, THEN cut: cut first,
        // a credential across the cut kept its start (review stop 4, E 4).
        // By the scrubber the section records, so it knows what scrubbed it.
        for f in &mut found {
            f.title = f
                .title
                .take()
                .map(|title| claude_import::one_line(&scrubbed.text(&title), TITLE_CHARS));
        }
        let mut section = ClaudeSection {
            found,
            read: HashMap::new(),
            awaiting: BTreeSet::new(),
            listed: true,
            scrubbed,
        };
        // Only a continued one is asked for now — its session hangs where it
        // parted. Any other is read when opened (`l`): scrubbing each one
        // read before, at every `:graph`, cost seconds (B 4, measured).
        for (id, why) in self.read_listed(&mut section, continued) {
            self.notify(format!(
                "Claude Code's conversation {} could not be read ({why}) — the session \
                 continuing it hangs from its row",
                short_hex(&id)
            ));
        }
        Some(section)
    }

    /// **A Claude Code section as it would be listed now** — listed again if
    /// what it was listed under changed since: the leave to read here
    /// (`ClaudeSection::listed`) or the secrets that scrubbed its titles and
    /// every reading it holds (`ClaudeSection::scrubbed`), what was open in
    /// it (read, or being read) asked for again — a mark leaves what you
    /// were reading open, read again under it. Else carried, never listed
    /// again: a tree is rebuilt at every background event, and a listing
    /// reads each file's tail. Either way, what a session has come to
    /// continue since is read, for it to hang where it parted.
    ///
    /// Every tree rebuilt comes through here ([`restore_tree`](Self::restore_tree)): a
    /// refresh, and one set aside coming back (`:q`, CTRL-^, `:b N`); the
    /// shown ones at once when the leave or the secrets change
    /// ([`relist_claude_sections`](Self::relist_claude_sections)). A
    /// conversation that was open and can no longer be read is said.
    pub(crate) fn current_section(
        &mut self,
        mut section: ClaudeSection,
        continued: &BTreeSet<String>,
    ) -> Option<ClaudeSection> {
        if self.listed_as_now(&section) {
            self.read_listed(&mut section, continued);
            return Some(section);
        }
        let open: BTreeSet<String> = (section.read.into_keys()).chain(section.awaiting).collect();
        let mut section = self.claude_section(continued)?;
        for (id, why) in self.read_listed(&mut section, &open) {
            self.notify(format!(
                "Claude Code's conversation {} could not be read again ({why})",
                short_hex(&id)
            ));
        }
        Some(section)
    }

    /// Whether `section` was listed under the leave and the secrets as they
    /// are now — [`current_section`](Self::current_section)'s question, and
    /// [`relist_claude_sections`](Self::relist_claude_sections)'s.
    pub(super) fn listed_as_now(&self, section: &ClaudeSection) -> bool {
        section.listed == self.may_list_claude()
            && (section.scrubbed).scrubs_as(&self.config.secrets, &self.aliases)
    }

    /// **Ask for every listed conversation named in `ids` that `section` has
    /// not**, through the one door ([`claude_reading`](Self::claude_reading))
    /// — what a session of the tree continues, for it to hang at the turn it
    /// parted at, or what was open in a section listed again. One kept as
    /// its file stands is taken now; any other is read off the UI thread,
    /// its row saying so, what was kept of it before shown meanwhile if
    /// scrubbed as the secrets now are. What could not be read comes back,
    /// by id, for the caller to say or not: `:graph` opening says it; a
    /// refresh, run on every background event, logs it.
    pub(crate) fn read_listed(
        &mut self,
        section: &mut ClaudeSection,
        ids: &BTreeSet<String>,
    ) -> Vec<(String, String)> {
        let unread: Vec<claude_import::Found> = section
            .found
            .iter()
            .filter(|f| {
                ids.contains(&f.id)
                    && !section.read.contains_key(&f.id)
                    && !section.awaiting.contains(&f.id)
            })
            .cloned()
            .collect();
        let mut failed = Vec::new();
        for f in unread {
            match self.claude_reading(&f).offered() {
                Ok((shown, awaiting)) => section.offer(&f.id, shown, awaiting),
                Err(why) => {
                    warn!(conversation = %f.id, error = %why, "could not read a conversation");
                    failed.push((f.id, why));
                }
            }
        }
        failed
    }

    /// **`l` in `:graph`**: open the row under the cursor — a Claude Code
    /// conversation not yet read is asked for first, through
    /// [`claude_reading`](Self::claude_reading), its turns being what
    /// opening it shows: now if it is kept, else when it is read, the row
    /// open and saying so meanwhile. A reading that cannot be had says why
    /// and opens nothing. On the section's heading, not read yet, it asks
    /// whether Shannon may read there.
    pub(crate) fn expand_graph_row(&mut self) {
        if self.graph_view().is_some_and(GraphView::unlisted_selected) {
            return self.ask_to_read_claude();
        }
        if let Some(found) = self
            .graph_view()
            .and_then(GraphView::unread_selected)
            .cloned()
        {
            match self.claude_reading(&found).offered() {
                Ok((shown, awaiting)) => {
                    if let Some(view) = self.graph_view_mut() {
                        view.offer(&found.id, shown, awaiting);
                    }
                }
                Err(why) => {
                    return self.refuse(format!(
                        "Claude Code's conversation {} could not be read: {why}",
                        short_hex(&found.id)
                    ));
                }
            }
        }
        if let Some(view) = self.graph_view_mut() {
            view.expand();
        }
    }

    /// **Imports made before imports recorded their source, found once by
    /// what they hold** (Steven, 10-16: "Match once by content") — so
    /// `:graph` hangs them under the conversation they continue, as it does
    /// every import since.
    ///
    /// A candidate is a ROOT session (an import is one) with no record of
    /// where it parted, never checked before, whose first turn reads as
    /// imported: sealed under `claude-code/…` with NO usage. An import records
    /// none; a turn Shannon ran through Claude Code records what it cost —
    /// which is what keeps a session born here on the Claude Code backend
    /// from being "found" in the transcript its own first turn wrote. Each is
    /// matched against this directory's conversations (bounded: one
    /// directory, never every directory's), read once with `Scope::Lineage`
    /// off the UI thread, the match going on when the last has come
    /// (`claude_reads`) — where an old import's first turn, a summary of the
    /// compaction latest then, still is — less those Shannon started; the
    /// longest run of its
    /// turns wins (`claude_import::find_run`). Found, its header carries the
    /// record; found or not, it is checked (`ClaudeSessions::check`) and
    /// never read for again.
    ///
    /// And from the same reading, a transcript EVERY turn of which is one
    /// Shannon ran, or a distillation it asked for after a history it gave —
    /// and that no session continues — is noted as Shannon's
    /// (`ClaudeSessions::note`), as one it starts today is (Steven, 10-16,
    /// decision 4). One a session continues stays listed: it is that
    /// session's root.
    pub(crate) fn match_old_imports(&mut self) {
        use shannon_core::backend::claude_code::MODEL_PREFIX;
        let Some(dir) = self.claude_dir() else {
            return;
        };
        let Ok(graph) = self.store.load_structure() else {
            return;
        };
        let by_claude = |p: &shannon_core::Provenance| p.config.model.starts_with(MODEL_PREFIX);
        let (mut candidates, mut ran_here) = (Vec::new(), Vec::new());
        for id in graph.episode_ids() {
            let Some(episode) = graph.episode(&id) else {
                continue;
            };
            let placed = graph.placed(&id);
            if placed
                .iter()
                .any(|p| by_claude(p.provenance) && p.provenance.usage.is_some())
            {
                ran_here.push(id);
            }
            if reads_imported(&graph, &id)
                && episode.origin.is_none()
                && episode.inherited.as_ref().is_none_or(|i| i.at.is_none())
                && !self.claude_sessions.checked(&id.to_hex())
            {
                candidates.push(id);
            }
        }
        // A conversation is examined once too: whether Shannon ran it is a
        // question about what it holds, which does not change once written.
        // Through the door that asks whether Shannon may read here — and
        // not listed is not matched: read as "none there", a listing
        // refused had every candidate unfound and checked for good, before
        // the first `:graph`'s question was answered.
        let Ok(listed) = self.claude_list(&dir) else {
            return;
        };
        let listed: Vec<claude_import::Found> = listed
            .into_iter()
            .filter(|found| !self.claude_sessions.started(&found.id))
            .collect();
        let unexamined = |found: &claude_import::Found| !self.claude_sessions.checked(&found.id);
        if candidates.is_empty() && !listed.iter().any(unexamined) {
            return;
        }
        // Every conversation when an old import is looked for (it may
        // continue one examined before); else only those never examined.
        let to_read: Vec<claude_import::Found> = listed
            .iter()
            .filter(|found| !candidates.is_empty() || unexamined(found))
            .cloned()
            .collect();
        // **What each reading found, in the log** — ids, counts and yes/no
        // only, never words: two of Steven's four old imports went unfound
        // (10-16) with nothing to say why, and a conversation that failed to
        // read was dropped without a word. Through the one door, so `:graph`
        // opens what was read here without reading it again.
        //
        // Each reading scrubbed ONCE, as it was read, as an import keeps its
        // words: compared scrubbed on every comparison, a long conversation
        // was scrubbed once per turn of every candidate (review stop 3, B
        // 4). Kept beside its listing, for a tie's time. One kept from
        // before the file last grew will do — an old import is in what was
        // read then — or the live session's file, which grows as it is
        // read, would keep the match waiting for good.
        let mut read: Vec<(claude_import::Found, std::sync::Arc<Imported>)> = Vec::new();
        let mut unread = Vec::new();
        for found in &to_read {
            match self.claude_reading(found) {
                Reading::Fresh(conversation) | Reading::Pending(Some(conversation)) => {
                    info!(
                        conversation = %found.id,
                        turns = conversation.turns.len(),
                        compactions = conversation.turns.iter().filter(|t| t.summary).count(),
                        cut = ?conversation.earlier,
                        "read for older imports"
                    );
                    read.push((found.clone(), conversation));
                }
                Reading::Pending(None) => unread.push(found.path.clone()),
                Reading::Failed(why) => {
                    warn!(conversation = %found.id, error = %why, "could not read for older imports");
                }
            }
        }
        // **Not all read: the match goes on when they are**
        // ([`go_on_matching`](Self::go_on_matching)). Read here, on the UI
        // thread, they froze the first `:graph` for as long as every
        // conversation in the directory took to read and scrub.
        if !unread.is_empty() {
            return self.claude_reads.wait_for(unread);
        }
        // A turn as its text: the prompt, and the reply it closed on.
        let said = |turn: &Turn| -> (String, String) {
            let text = |m: Option<&Message>| m.map(|m| m.content.clone()).unwrap_or_default();
            let reply = turn.messages.last().filter(|m| m.role == Role::Assistant);
            (text(turn.messages.first()), text(reply))
        };
        let turns_of = |id: &EpisodeId| -> Vec<Turn> {
            self.store
                .load_episode(id)
                .map(|loaded| loaded.turns)
                .unwrap_or_default()
        };
        let found: Vec<(EpisodeId, Option<shannon_core::Inherited>)> = candidates
            .iter()
            .map(|id| {
                let session: Vec<(String, String)> = turns_of(id).iter().map(said).collect();
                // A record that names its conversation is looked for there
                // alone: another holding the same turns — a fork, a copy
                // `--resume` wrote — must not take its place (review stop 3,
                // C 4c). The longest run wins, the newest conversation on a
                // tie, as `find_run` takes the later run (B).
                let named = graph
                    .episode(id)
                    .and_then(|e| e.inherited.as_ref())
                    .map(|i| i.session.clone());
                let best = read
                    .iter()
                    .filter(|(found, _)| named.as_ref().is_none_or(|n| *n == found.id))
                    .filter_map(|(found, conversation)| {
                        claude_import::find_run(&conversation.turns, &session, |a, b| a == b)
                            .map(|(at, run)| (run, found.modified, at, conversation))
                    })
                    .max_by_key(|&(run, modified, ..)| (run, modified));
                if best.is_none() {
                    self.log_unfound(*id, &session, &read);
                }
                let inherited = best.map(|(run, _, at, conversation)| {
                    let matched = &conversation.turns[at..at + run];
                    shannon_core::Inherited {
                        session: conversation.id.clone(),
                        turns: run,
                        at: matched.last().and_then(|t| t.uuid.clone()),
                        opened: matched.iter().map(|t| t.uuid.clone()).collect(),
                    }
                });
                (*id, inherited)
            })
            .collect();
        // Every turn Shannon ran through Claude Code, as text.
        let ran: HashSet<(String, String)> = ran_here
            .iter()
            .flat_map(turns_of)
            .filter(|t| by_claude(&t.provenance) && t.provenance.usage.is_some())
            .map(|t| said(&t))
            .collect();
        let continued: HashSet<String> = graph
            .episode_ids()
            .iter()
            .filter_map(|id| {
                graph
                    .episode(id)?
                    .inherited
                    .as_ref()
                    .map(|i| i.session.clone())
            })
            .chain(
                found
                    .iter()
                    .filter_map(|(_, i)| Some(i.as_ref()?.session.clone())),
            )
            .collect();
        // A distillation Shannon asked for (`S`, `:compact`) is no turn it
        // keeps: the summary sealed is the draft as reviewed, not the
        // instruction and the draft (Steven's `6a7b8c9d`, 10-16). Known by the
        // instruction AND by a history before it that only Shannon gives —
        // its own turns, or its handoff line — so a conversation typed in
        // Claude Code that asks to "Summarize" is not taken for one.
        let instructions = [
            shannon_core::DEFAULT_SQUASH_INSTRUCTION,
            self.config.merge.squash_instruction.as_str(),
        ];
        let distills = |prompt: &str| {
            instructions
                .iter()
                .any(|i| !i.is_empty() && prompt.contains(i))
        };
        let shannons: Vec<String> = read
            .iter()
            .map(|(_, c)| c)
            .filter(|c| !c.turns.is_empty() && !continued.contains(&c.id))
            .filter(|c| {
                c.turns.iter().enumerate().all(|(at, t)| {
                    ran.contains(&(t.prompt.clone(), t.reply.clone()))
                        || (distills(&t.prompt) && (at > 0 || c.skipped.handoffs > 0))
                })
            })
            .map(|c| c.id.clone())
            .collect();

        let (total, mut linked, mut unsaid) = (found.len(), Vec::new(), None);
        for (id, inherited) in found {
            if let Some(inherited) = inherited {
                let pair = (id.short_hex(), short_hex(&inherited.session).to_string());
                let written = self.store.load_episode(&id).and_then(|loaded| {
                    let mut episode = loaded.episode;
                    episode.inherited = Some(inherited.clone());
                    self.store.put_episode(&episode)
                });
                if let Err(e) = written {
                    // Not checked: it is matched again next time — and said,
                    // or every `:graph` read again for a reason nobody saw
                    // (review stop 3, B).
                    warn!(episode = %id.to_hex(), error = %e, "could not record an old import's source");
                    unsaid = Some(format!(
                        "could not record which Claude Code conversation {} continues: {e}",
                        id.short_hex()
                    ));
                    continue;
                }
                for b in self.buffers.sessions_mut() {
                    if b.conversation.episode().id == id {
                        b.conversation.set_inherited(Some(inherited.clone()));
                    }
                }
                linked.push(pair);
            }
            if let Err(why) = self.claude_sessions.check(&id.to_hex()) {
                unsaid = Some(why);
            }
        }
        for id in &shannons {
            if let Err(why) = self.claude_sessions.note(id) {
                unsaid = Some(why);
            }
        }
        for (_, conversation) in &read {
            if let Err(why) = self.claude_sessions.check(&conversation.id) {
                unsaid = Some(why);
            }
        }
        info!(
            total,
            matched = linked.len(),
            shannons = shannons.len(),
            "matched older imports"
        );
        // **What changed, to what, by name** — each half only when it has
        // something to say. Counts alone did not: "1 conversation Shannon ran
        // is its own now" read, beside sessions Shannon ran and sessions
        // imported, as a count of one of those (Steven, 10-16: "I don't know
        // how to interpret that message").
        let found_said = match linked.as_slice() {
            [] => None,
            [(id, session)] => Some(format!(
                "older import {id} continues Claude Code's {session}"
            )),
            many => {
                let pairs: Vec<String> = many.iter().map(|(i, s)| format!("{i} in {s}")).collect();
                Some(format!(
                    "older imports found in Claude Code: {}",
                    crate::view::first_and_more(&pairs, 3)
                ))
            }
        };
        let ids: Vec<String> = shannons.iter().map(|s| short_hex(s).to_string()).collect();
        let runs_said = match ids.as_slice() {
            [] => None,
            [id] => Some(format!("{id} left the Claude Code list: Shannon wrote it")),
            many => Some(format!(
                "{} left the Claude Code list: Shannon wrote them",
                crate::view::first_and_more(many, 3)
            )),
        };
        let said: Vec<String> = found_said.into_iter().chain(runs_said).collect();
        if !said.is_empty() {
            self.notify(said.join("; "));
        }
        if let Some(why) = unsaid {
            self.notify(why);
        }
    }

    /// **Why an older import was not found, in the log** — for each
    /// conversation read: whether the session's first prompt is one of its
    /// turns' exactly; whether one holds the other (an importer that joined a
    /// summary and the question after it differently); whether the reply
    /// matches where the prompt does. Facts, never the words: the log is for
    /// telling a moved file from a changed importer from a conversation that
    /// is not here at all. `read` is scrubbed already, as the session is.
    fn log_unfound(
        &self,
        id: EpisodeId,
        session: &[(String, String)],
        read: &[(claude_import::Found, std::sync::Arc<Imported>)],
    ) {
        let candidate = id.to_hex();
        let Some((prompt, reply)) = session.first().filter(|(p, _)| !p.is_empty()) else {
            return info!(%candidate, "an older import with no prompt to look for");
        };
        let mut near = 0;
        for (_, conversation) in read {
            let prompts: Vec<&str> = conversation
                .turns
                .iter()
                .map(|t| t.prompt.as_str())
                .collect();
            let equal = prompts.iter().position(|p| p == prompt);
            let within = prompts.iter().any(|p| {
                !p.is_empty() && (p.starts_with(prompt.as_str()) || prompt.starts_with(p))
            });
            if equal.is_none() && !within {
                continue;
            }
            near += 1;
            let reply_equal = equal.is_some_and(|at| conversation.turns[at].reply == *reply);
            info!(
                %candidate,
                conversation = %conversation.id,
                prompt_equal = equal.is_some(),
                prompt_within = within,
                reply_equal,
                "an older import, not found"
            );
        }
        if near == 0 {
            info!(
                %candidate,
                turns = session.len(),
                // A compacted conversation's summary opens so (observed 11-08).
                at_a_compaction = prompt.starts_with("This session is being continued"),
                conversations = read.len(),
                "an older import, its first turn in no conversation read"
            );
        }
    }

    /// **`:import [--all] <id|path>`** — bring a Claude Code conversation in
    /// as a session in this pane: what `claude --resume` holds, or with
    /// `--all` everything back to its first prompt.
    pub(crate) fn import_command(&mut self, args: &str) {
        let mut scope = Scope::Resume;
        let mut target = None;
        for word in args.split_whitespace() {
            match word {
                "--all" | "-a" => scope = Scope::Everything,
                other => target = Some(other),
            }
        }
        let Some(target) = target else {
            return self.refuse(
                "`:import <id>` or `:import <file>` — Tab lists this directory's Claude Code \
                 conversations, and so does `:graph`",
            );
        };
        let here = self.claude_dir();
        match find_conversation(target, self.claude_projects.as_deref(), here.as_deref()) {
            Ok(path) => self.import_conversation(&path, scope, None, None),
            Err(why) => self.refuse(why),
        }
    }

    /// **Import the conversation in `path`** into the pane the prompt aims at
    /// — to its end, or `through` one of its turns (`<CR>` on it in `:graph`)
    /// — or `into` one window (`<CR>`'s, the tree's own), beside the session
    /// beneath it, else beside the aim. **Into a window only when it lands
    /// now**: asked first, it lands where an `:import` does. `<CR>` never
    /// asks — its row is listed because the directory may be read — and a
    /// window held across the question could be gone by the answer (V4b
    /// verify, M2 1).
    ///
    /// Only where the user let Shannon read: the one door every import reads
    /// through, `:import` and `<CR>` alike. Elsewhere it asks: of this
    /// directory's Claude Code folder, leave for it (`y`, `a`, `n`); of any
    /// other file — a path, another project's id — that file alone, nothing
    /// recorded (Steven, 10-19: `:import ~/test.jsonl` and `a` read what else
    /// was there: "Let's only import named imports when they are outside the
    /// working directory").
    pub(crate) fn import_conversation(
        &mut self,
        path: &Path,
        scope: Scope,
        through: Option<&Through>,
        into: Option<crate::wm::PaneId>,
    ) {
        // The file as the gate names it — `..` folded by name — and READ as
        // that: checking one spelling and reading another is how
        // `-here/../-there` read where only `-here` was allowed (review stop
        // 5, A 2, B 6, C 1).
        let path = &normal(path);
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return self.refuse(format!("not imported — {} is in no directory", tilde(path)));
        };
        if self.may_read_claude(&dir) {
            return self.import_read(path, scope, through, into);
        }
        let import = Box::new(Import {
            path: path.to_path_buf(),
            scope,
            through: through.cloned(),
        });
        let here = self.claude_dir().map(|d| normal(&d));
        match here == Some(dir.clone()) {
            true => self.ask(ConfirmAction::ReadClaude {
                dir,
                then: Some(import),
            }),
            false => self.ask(ConfirmAction::ImportOne { import }),
        }
    }

    /// **`y` to `ConfirmAction::ImportOne`**: the one file the user named,
    /// read now — asked about already, and no leave kept.
    pub(crate) fn import_one(&mut self, import: Import) {
        self.import_read(&import.path, import.scope, import.through.as_ref(), None);
    }

    /// The import itself, past the gate: read, replayed, adopted, said.
    fn import_read(
        &mut self,
        path: &Path,
        scope: Scope,
        through: Option<&Through>,
        into: Option<crate::wm::PaneId>,
    ) {
        let until = through.map(|t| &t.until);
        let imported = match claude_import::read_until(path, scope, until) {
            Ok(imported) => imported,
            Err(why) => return self.refuse(format!("not imported — {why}")),
        };
        if imported.turns.is_empty() {
            return self.refuse(format!(
                "not imported — {} holds no turn Shannon can read",
                tilde(path)
            ));
        }
        let config = (self.target_conversation())
            .map_or_else(|| self.fresh_config(), |c| c.episode().config.clone());
        let conversation = self.replay(&imported, config);
        // Into the window, which `<CR>` named this event: beside the session
        // beneath it, else the aim (a tree in a pane of its own) — the session `<CR>`'s
        // close re-aimed at.
        let beside = match into {
            Some(pane) => Lands::Into(
                self.live_session(pane).unwrap_or(self.prompt_target()),
                pane,
            ),
            None => Lands::Beside(self.prompt_target()),
        };
        let mut episode = conversation.episode().clone();
        // **Where it came from** — what `:graph` hangs it under, and what tells
        // a conversation continued from Claude Code from one born here
        // (Steven, 10-16). It was recorded nowhere: an import was a root like
        // any other, its source only guessable from its turns' model.
        //
        // `at` names the turn it parted at in the terms `:graph` reads the
        // conversation in (`Scope::Lineage`, every compaction its turn). The
        // resume path's turns are a run of that reading's but at its start:
        // the compaction's kept messages answer its summary there, so the
        // question after it opens a turn the lineage's summary turn took in
        // — found by `ImportedTurn::holds`. `--all`'s are not
        // once it has dropped a compaction's summary — and joined the reply
        // after it into the turn before — so placing it by them hung the
        // session before a compaction it holds (review stop 3, A F3, C 3).
        // Then there is no turn-exact place to say: none is recorded, and
        // the session hangs from the conversation's row, every turn its own.
        //
        // `opened`: each turn's own place, by the entry that opened it — the
        // resume path holds the kept messages after the summary, the lineage
        // before, so a count from `at` put a branch of the summary's turn,
        // and a session ending on a kept question, before the compaction
        // they follow (review stop 4, A 1).
        let placed = scope != Scope::Everything || imported.skipped.summaries == 0;
        episode.inherited = Some(shannon_core::Inherited {
            session: imported.id.clone(),
            turns: imported.turns.len(),
            at: imported
                .turns
                .last()
                .and_then(|t| t.uuid.clone())
                .filter(|_| placed),
            opened: match placed {
                true => imported.turns.iter().map(|t| t.uuid.clone()).collect(),
                false => Vec::new(),
            },
        });
        // **No place, recorded as none, not as unknown** — an import that
        // reached past a compaction it dropped hangs from the conversation's
        // row on purpose, and the match for older imports read the same
        // missing `at` as "an import from before the record" and placed it
        // before the compaction it holds (review stop 4, A 2). Examined now,
        // so it is never a candidate.
        if !placed && let Err(why) = self.claude_sessions.check(&episode.id.to_hex()) {
            let session = episode.id.to_hex();
            warn!(%session, error = %why, "could not record an import as placed nowhere");
        }
        // Its turns are new to Shannon's store: written by the landing, not
        // on the first continue (`write_if_it_owns`).
        let turns = conversation.own_turns();
        // Its own turns, read from Claude Code: no secret Shannon kept, so
        // nothing a password guards — `None` only should that change.
        if self
            .land(beside, Landing::Asked, episode, turns, OnDisk::Absent)
            .is_none()
        {
            return;
        }
        let said = imported_note(&imported, path, through);
        info!(id = %imported.id, turns = imported.turns.len(), "imported a Claude Code conversation");
        self.notify(said);
    }

    /// **`imported` as a conversation**, each turn built as a live one is,
    /// everything scrubbed on the way in.
    fn replay(&self, imported: &Imported, config: TurnConfig) -> Conversation {
        let mut conversation =
            Conversation::with_episode(super::fresh_episode_id(), config.clone());
        let scrubber = self.scrubber();
        for turn in &imported.turns {
            // Every word scrubbed by the one walk a view's reading takes too
            // (`Scrubber::turn`): the match for older imports compares them.
            let turn = scrubber.turn(turn);
            // **Only what the transcript says** (`TurnConfig::authored`):
            // Shannon sent these turns no ceiling and no effort, and the
            // transcript records neither; taken from the session they land
            // in, each would claim the session's effort as what produced it.
            let sealed = TurnConfig {
                model: format!(
                    "{}{}",
                    shannon_core::backend::claude_code::MODEL_PREFIX,
                    turn.model.as_deref().unwrap_or("imported")
                ),
                ..config.authored()
            };
            // Told nothing: this is Claude Code's history, which its own
            // session told whatever it told.
            conversation.begin_turn_under(turn.prompt, Vec::new(), sealed);
            for round in turn.rounds {
                conversation.push_chunk(&round.text);
                for call in round.calls {
                    conversation.push_tool_use(call);
                }
                conversation.finish_tool_round(Message {
                    role: Role::User,
                    content: String::new(),
                    blocks: round.results,
                });
            }
            conversation.push_chunk(&turn.reply);
            conversation.commit_turn_holding(&self.aliases);
        }
        conversation
    }
}

/// **Whether session `id` reads as imported** — its first turn sealed under
/// `claude-code/…` with NO usage: an import records none, and a turn Shannon
/// ran through Claude Code records what it cost. The one test of it, for the
/// match for older imports and for `:delete all`'s gate, which keeps the
/// ones no match found ("possibly inherited", Steven, 10-16).
pub(super) fn reads_imported(graph: &shannon_core::Graph, id: &shannon_core::EpisodeId) -> bool {
    graph
        .placed(id)
        .first()
        .is_some_and(|p| came_in(p.provenance))
}

/// **A turn that came in from Claude Code** — sealed `claude-code/…`, nothing
/// billed: what every import seals, and no turn Shannon ran.
fn came_in(provenance: &shannon_core::Provenance) -> bool {
    use shannon_core::backend::claude_code::MODEL_PREFIX;
    provenance.config.model.starts_with(MODEL_PREFIX) && provenance.usage.is_none()
}

/// **How a session goes on from Claude Code**, if it does
/// ([`from_claude`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FromClaude<'a> {
    /// An import, by its record: the conversation, the turns that came in.
    Continues(&'a shannon_core::Inherited),
    /// An import from before imports recorded their source, no match found
    /// yet: its first turn reads as imported ([`reads_imported`]).
    Unrecorded,
    /// **Born in Shannon** — a clone, a branch, a fork — holding `imported`
    /// turns that came in, `through` the nearest session it was cut from
    /// that goes on from Claude Code, while that one is still here.
    /// `:delete all shannon-only` takes it: it was born here (Steven, 10-18,
    /// asked whether it should count so: "Can we at least record the
    /// information into the inspector?").
    Holds {
        imported: usize,
        through: Option<shannon_core::EpisodeId>,
    },
}

impl FromClaude<'_> {
    /// **Does the session go on from a Claude Code conversation itself** —
    /// what `:delete all`'s second question names and `shannon-only` keeps.
    pub(crate) fn continues(self) -> bool {
        matches!(self, FromClaude::Continues(_) | FromClaude::Unrecorded)
    }
}

/// **How session `id` goes on from Claude Code** — the one answer for the
/// inspector and `:delete all` (`App::continued_sessions`), so the page
/// never says one thing and the delete do another. Its origins walked by
/// the core's one cycle-guarded chain (`Graph::origin_chain`): a corrupt
/// cycle answers "through none" rather than hang the frame or `:delete all`.
pub(crate) fn from_claude<'g>(
    graph: &'g shannon_core::Graph,
    id: &shannon_core::EpisodeId,
) -> Option<FromClaude<'g>> {
    if let Some(itself) = continues_itself(graph, id) {
        return Some(itself);
    }
    let imported = graph
        .placed(id)
        .iter()
        .filter(|p| came_in(p.provenance))
        .count();
    if imported == 0 {
        return None;
    }
    let through = graph
        .origin_chain(id)
        .into_iter()
        .map(|origin| origin.episode)
        .find(|at| continues_itself(graph, at).is_some());
    Some(FromClaude::Holds { imported, through })
}

/// [`from_claude`]'s answer for a session going on from a conversation
/// ITSELF — by its record, or read as an import — with no walk.
fn continues_itself<'g>(
    graph: &'g shannon_core::Graph,
    id: &shannon_core::EpisodeId,
) -> Option<FromClaude<'g>> {
    let episode = graph.episode(id)?;
    match &episode.inherited {
        Some(record) => Some(FromClaude::Continues(record)),
        None => (episode.origin.is_none() && reads_imported(graph, id))
            .then_some(FromClaude::Unrecorded),
    }
}

/// **The conversation `target` names**: a file that exists; else a session
/// id among this directory's conversations (`here`); else among every
/// directory's — by file NAME only, never reading another's contents —
/// because a conversation from elsewhere (the parent directory, say) is
/// exactly what a path is for, and an id is what Claude Code shows.
///
/// **An id is its whole name, or its first [`SHORT_ID_LEN`] characters or
/// more** — as Shannon's own ids are shown and typed (`id_prefix`, at 8
/// since 11-10) — never fewer (Steven,
/// 11-09: "Since we have tab completion let's require the 8"), and Tab lists
/// this directory's ([`conversation_ids`]). Fewer took whatever began so,
/// in any directory, and named each it could not tell apart, another
/// project's among them (review stop 17, E2 LOW 5): `:import abc` read as
/// the glob it was not written as.
fn find_conversation(
    target: &str,
    projects: Option<&Path>,
    here: Option<&Path>,
) -> Result<PathBuf, String> {
    let as_path = config_path_arg(target);
    if as_path.is_file() {
        return Ok(as_path);
    }
    let projects =
        projects.ok_or("Claude Code's directory is unknown — set HOME or CLAUDE_CONFIG_DIR")?;
    let abbreviates = target.chars().count() >= SHORT_ID_LEN;
    let named = |dir: &Path| -> Vec<PathBuf> {
        (conversations_in(dir).into_iter())
            .filter(|(_, id)| id == target || (abbreviates && id.starts_with(target)))
            .map(|(path, _)| path)
            .collect()
    };
    let mut matches = here.map(named).unwrap_or_default();
    let elsewhere = matches.is_empty();
    if elsewhere {
        matches = std::fs::read_dir(projects)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .flat_map(|dir| named(&dir))
            .collect();
    }
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] if !abbreviates => Err(format!(
            "`{target}` is no file, and an id is its first {SHORT_ID_LEN} characters or more — \
             Tab lists this directory's"
        )),
        [] => Err(format!(
            "no Claude Code conversation is named `{target}`, and no file is there"
        )),
        // Other projects' conversations, counted and never named: their
        // ids are theirs, as Tab offers this directory's alone (stop 17,
        // V2a LOW 4; E2 LOW 5's rule).
        many if elsewhere => Err(format!(
            "`{target}` names {} conversations in other projects — give more of the id",
            many.len()
        )),
        many => {
            let names: Vec<String> = many
                .iter()
                .filter_map(|p| p.file_stem().and_then(|s| s.to_str()))
                .map(str::to_string)
                .collect();
            Err(format!(
                "`{target}` names {} conversations — give more of the id: {}",
                many.len(),
                crate::view::first_and_more(&names, 3)
            ))
        }
    }
}

/// The conversations a Claude Code project folder `dir` holds, each its
/// file and its id (the file's name) — read by name, never opened.
fn conversations_in(dir: &Path) -> Vec<(PathBuf, String)> {
    (std::fs::read_dir(dir).into_iter().flatten())
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?.to_string();
            Some((path, id))
        })
        .collect()
}

/// **What Tab offers `:import`**: this directory's conversations (`here`)
/// whose id starts with `typed`, each cut to [`SHORT_ID_LEN`] characters or
/// to what was typed, if more — as Shannon's own ids complete
/// (`CommandLine::matching_short_ids`) — sorted, one each. This directory's
/// only: another's are not listed, as `:graph` lists none (review stop 17,
/// E2 LOW 5); a path, or its id typed whole, reaches one.
pub(crate) fn conversation_ids(here: Option<&Path>, typed: &str) -> Vec<String> {
    let mut ids: Vec<String> = (here.map(conversations_in).unwrap_or_default().into_iter())
        .map(|(_, id)| id)
        .filter(|id| id.starts_with(typed))
        .map(|id| {
            let keep = typed.chars().count().max(SHORT_ID_LEN);
            id.chars().take(keep).collect()
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// What an import says: what came in, from where, what did not, and how to
/// get the rest.
fn imported_note(imported: &Imported, path: &Path, through: Option<&Through>) -> String {
    let calls: usize = imported
        .turns
        .iter()
        .flat_map(|t| &t.rounds)
        .map(|r| r.calls.len())
        .sum();
    let short = short_hex(&imported.id);
    use crate::view::counted;
    let mut said = format!(
        "imported Claude Code conversation {short} ({}): {}, {}",
        imported
            .cwd
            .as_deref()
            .map_or_else(|| tilde(path), |d| tilde(Path::new(d))),
        counted(imported.turns.len(), "turn", "turns"),
        counted(calls, "tool call", "tool calls"),
    );
    match (through, imported.earlier) {
        // Short of its end, where `<CR>` on a turn asked — no command
        // brings that span with more of what came before.
        (Some(through), earlier) => {
            said.push_str(&format!(
                " — up to its turn {} of {}",
                through.turn + 1,
                through.of
            ));
            if earlier == Some(Earlier::Compacted) {
                said.push_str(", from the compaction before it");
            }
        }
        (None, Some(Earlier::Compacted)) => said.push_str(&format!(
            " — from its latest compaction; `:import --all {short}` brings the rest"
        )),
        // `--all` cannot bring it: the file holds none of it.
        (None, Some(Earlier::Elsewhere)) => said.push_str(
            " — from where `claude --resume` began it; what came before is in the \
             conversation it was resumed from",
        ),
        (None, None) => {}
    }
    let s = imported.skipped;
    let left_out: Vec<String> = [
        (s.thinking, "thinking block", "thinking blocks"),
        (s.images, "image", "images"),
        (s.attachments, "attachment", "attachments"),
        (
            s.meta + s.system,
            "Claude Code's own entry",
            "Claude Code's own entries",
        ),
        (
            s.handoffs,
            "copy of Shannon's own history",
            "copies of Shannon's own history",
        ),
        (s.cut, "result cut", "results cut"),
        (
            s.unanswered,
            "call without a result",
            "calls without results",
        ),
        (s.unknown + s.unparsable, "unread entry", "unread entries"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| counted(n, one, many))
    .collect();
    if !left_out.is_empty() {
        said.push_str(&format!("; not brought in: {}", left_out.join(", ")));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use shannon_core::claude_import::Skipped;

    /// **Each count in the number it is** — Steven's `:messages` (10-16)
    /// read "1 Claude Code's own entries": every noun was a fixed plural, and
    /// two handoffs would have read "2 copy of Shannon's own history".
    #[test]
    fn an_import_note_counts_in_the_number_it_is() {
        let note = |skipped: Skipped| {
            let imported = Imported {
                id: "3c4d5e6f-0000-0000-0000-000000000000".into(),
                cwd: None,
                turns: Vec::new(),
                skipped,
                earlier: None,
                leaf: None,
            };
            imported_note(&imported, Path::new("/p/3c4d5e6f.jsonl"), None)
        };
        let one = note(Skipped {
            meta: 1,
            handoffs: 1,
            unanswered: 1,
            ..Default::default()
        });
        assert!(
            one.ends_with(
                "0 turns, 0 tool calls; not brought in: 1 Claude Code's own entry, \
                 1 copy of Shannon's own history, 1 call without a result"
            ),
            "{one}"
        );
        let two = note(Skipped {
            meta: 1,
            system: 1,
            handoffs: 2,
            unanswered: 2,
            ..Default::default()
        });
        assert!(
            two.ends_with(
                "not brought in: 2 Claude Code's own entries, \
                 2 copies of Shannon's own history, 2 calls without results"
            ),
            "{two}"
        );
    }

    /// **What an import began after says how to get it, or that it can't** —
    /// a compaction `--all` reads past; a resume's beginning `--all` cannot,
    /// the file holding none of what came before; a copy up to a turn says
    /// which, and offers no `--all`, which would not stop there (10-17).
    #[test]
    fn an_import_note_says_where_the_rest_is() {
        let note = |earlier, through: Option<&Through>| {
            let imported = Imported {
                id: "1a2b3c4d-0000-0000-0000-000000000000".into(),
                cwd: None,
                turns: Vec::new(),
                skipped: Skipped::default(),
                earlier,
                leaf: None,
            };
            imported_note(&imported, Path::new("/p/1a2b3c4d.jsonl"), through)
        };
        let compacted = note(Some(Earlier::Compacted), None);
        assert!(
            compacted.contains("`:import --all 1a2b3c4d`"),
            "{compacted}"
        );
        let elsewhere = note(Some(Earlier::Elsewhere), None);
        assert!(
            elsewhere.contains("resumed from") && !elsewhere.contains("--all"),
            "{elsewhere}"
        );
        assert!(!note(None, None).contains(" — "), "nothing to say");
        let through = Through {
            turn: 4,
            of: 12,
            until: claude_import::Until::Before("u6".into()),
        };
        let cut = note(Some(Earlier::Compacted), Some(&through));
        assert!(
            cut.contains(" — up to its turn 5 of 12, from the compaction before it")
                && !cut.contains("--all"),
            "{cut}"
        );
    }
}
