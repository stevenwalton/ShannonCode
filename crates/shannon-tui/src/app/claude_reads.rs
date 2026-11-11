//! **Claude Code's conversations, read off the UI thread** (Steven, 10-17: "I
//! think we can do these in parallel and asynchronously").
//!
//! Read with every compaction kept and scrubbed for a view, one of Steven's
//! took 1.4 s to read and 2.7 s to scrub in release (10-17), and
//! `:graph` did both on the UI thread at every opening, for each conversation
//! a session continues. Now a file is read once and kept ([`Kept`]): its
//! `Transcript`, which reads on from where it stopped, and its reading as
//! scrubbed, whose turns that came through unchanged are not scrubbed again.
//! A file not kept as it now stands goes to a few reading threads; the
//! reading comes back on a channel the event loop selects on
//! ([`App::claude_read_done`]) and every `:graph` that asked for it takes it.
//! Meanwhile a view shows what was kept from before, if anything, and the
//! conversation's row says it is being read.
//!
//! A child module of `app`, as `import_flow` is.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use shannon_core::claude_import::{Found, Imported, Transcript};
use shannon_core::short_hex;
use tokio::sync::mpsc::UnboundedSender;

use super::*;
use crate::scrub::Scrubber;

/// How many threads read at once, at most — each holds a whole file's
/// reading while it reads (~126 MB at its end for a 283 MB transcript,
/// measured at review stop 4, B 5).
const READERS: usize = 4;

/// **The conversation files read, being read, or not readable** — by path.
pub(crate) struct ClaudeReads {
    files: HashMap<PathBuf, Held>,
    /// Work for the reading threads, started at the first.
    jobs: Option<std::sync::mpsc::Sender<Job>>,
    done: UnboundedSender<Done>,
    /// The files the match for older imports waits on, every one.
    awaited: HashSet<PathBuf>,
    /// How many times the match went on after waiting — for the test that
    /// it does so once, when the LAST reading it waited on comes.
    #[cfg(test)]
    pub(crate) matched: usize,
}

/// **A file asked for** — which conversation, where, the file as its listing
/// gave it then (its time and size: a reading is the file's while they
/// are), and what scrubs it. What a job carries, what comes back with it,
/// and what a kept reading or a failure remembers.
#[derive(Clone)]
struct Asked {
    id: String,
    path: PathBuf,
    modified: Option<SystemTime>,
    bytes: u64,
    scrubber: Scrubber,
}

impl Asked {
    fn of(found: &Found, scrubber: &Scrubber) -> Asked {
        Asked {
            id: found.id.clone(),
            path: found.path.clone(),
            modified: found.modified,
            bytes: found.bytes,
            scrubber: scrubber.clone(),
        }
    }

    /// Whether `other` asks for the file as it was when this was asked.
    fn as_listed(&self, other: &Asked) -> bool {
        self.modified == other.modified && self.bytes == other.bytes
    }
}

/// One file, as the app holds it.
enum Held {
    /// Being read; what was kept of it before, and what scrubbed that — to
    /// show meanwhile, while the secrets are still as they were.
    Reading(Option<Box<(Arc<Imported>, Scrubber)>>),
    Kept(Box<Kept>),
    /// Could not be read, as the file was when asked, and why — not asked
    /// again until it changes, or every reading that waits on all of them
    /// (the match for older imports) would ask again, forever.
    Failed(Box<Asked>, String),
}

/// A file read: what reads on from it, and its reading as a view shows it,
/// scrubbed by what `asked` carries.
struct Kept {
    asked: Asked,
    transcript: Transcript,
    shown: Arc<Imported>,
}

/// **What `found` reads as now** ([`App::claude_reading`]).
pub(crate) enum Reading {
    /// Kept as the file stands, scrubbed as the secrets now are.
    Fresh(Arc<Imported>),
    /// Being read; what was kept before, if anything.
    Pending(Option<Arc<Imported>>),
    /// Could not be read, and why.
    Failed(String),
}

impl Reading {
    /// **What a section is offered** (`ClaudeSection::offer`): the reading
    /// to show now, and whether one is awaited — or why none can be had.
    /// The one mapping every asker of a reading for a view takes (C M7) —
    /// three: `:graph` opening on a conversation a session continues, `l`,
    /// and `a`'s every-directory listing.
    pub(crate) fn offered(self) -> Result<(Option<Arc<Imported>>, bool), String> {
        match self {
            Reading::Fresh(shown) => Ok((Some(shown), false)),
            Reading::Pending(meanwhile) => Ok((meanwhile, true)),
            Reading::Failed(why) => Err(why),
        }
    }
}

/// **What came of a reading**, for the views that wait on it
/// ([`ClaudeReads::done`]).
enum Landed {
    /// Read, and scrubbed as the secrets now are.
    Read(Arc<Imported>),
    /// Read — but a secret was marked while it was, which it shows: kept,
    /// and being read again under the mark; what waits on it waits on.
    Again,
    Failed(String),
}

/// One file to read on a reading thread.
struct Job {
    asked: Asked,
    transcript: Transcript,
    /// The reading kept, and what scrubbed it: its unchanged turns are
    /// taken as they are, if this scrubs alike.
    before: Option<(Arc<Imported>, Scrubber)>,
}

/// **A file read** — back from a reading thread, for the event loop.
pub(crate) struct Done {
    asked: Asked,
    /// What reads on from the file, and its reading scrubbed — or why it
    /// could not be read, the transcript dropped: the next read starts over.
    read: Result<(Transcript, Arc<Imported>), String>,
}

impl Job {
    fn run(mut self) -> Done {
        let read = (|| {
            self.transcript.read_on()?;
            let (raw, kept) = self.transcript.lineage()?;
            let scrubber = &self.asked.scrubber;
            let before = self
                .before
                .as_ref()
                .filter(|(_, by)| by.same(scrubber))
                .map(|(shown, _)| shown.as_ref());
            Ok(Arc::new(scrubber.imported(raw, kept, before)))
        })();
        Done {
            read: read.map(|shown| (self.transcript, shown)),
            asked: self.asked,
        }
    }

    /// What became of it, when it could not be read.
    fn failed(self, why: String) -> Done {
        Done {
            asked: self.asked,
            read: Err(why),
        }
    }
}

impl ClaudeReads {
    pub(crate) fn new(done: UnboundedSender<Done>) -> Self {
        ClaudeReads {
            files: HashMap::new(),
            jobs: None,
            done,
            awaited: HashSet::new(),
            #[cfg(test)]
            matched: 0,
        }
    }

    /// **Readings the match for older imports waits on, all of them** —
    /// [`arrived`](Self::arrived) says when the last has come.
    pub(crate) fn wait_for(&mut self, paths: Vec<PathBuf>) {
        self.awaited.extend(paths);
    }

    /// **Nothing kept from under `dir`** — readings held, being read or
    /// failed, and the match's wait for them: the user took back Shannon's
    /// leave to read there (`:unread`). A reading in flight is dropped as it
    /// lands (`App::claude_read_done`); one still waited on would have kept
    /// every later match waiting for good.
    pub(crate) fn forget_under(&mut self, dir: &Path) {
        self.files.retain(|path, _| !path.starts_with(dir));
        self.awaited.retain(|path| !path.starts_with(dir));
    }

    /// Whether `path` was the last reading waited on.
    fn arrived(&mut self, path: &Path) -> bool {
        self.awaited.remove(path) && self.awaited.is_empty()
    }

    /// **What the file `asked` names reads as** — and if it is not kept as
    /// the file stands, scrubbed as `asked` scrubs, read off the UI thread:
    /// unless it is being read already, or could not be read as the file is.
    fn reading(&mut self, asked: Asked) -> Reading {
        // Answered from what is held, while what is held answers it.
        match self.files.get(&asked.path) {
            Some(Held::Kept(kept))
                if kept.asked.as_listed(&asked) && kept.asked.scrubber.same(&asked.scrubber) =>
            {
                return Reading::Fresh(Arc::clone(&kept.shown));
            }
            Some(Held::Reading(before)) => {
                return Reading::Pending(shown_meanwhile(before.as_deref(), &asked.scrubber));
            }
            Some(Held::Failed(was, why)) if was.as_listed(&asked) => {
                return Reading::Failed(why.clone());
            }
            _ => {}
        }
        // Else read — on from what was kept (the file grew, or the secrets
        // changed), or from the start. Shown meanwhile only if scrubbed as
        // the secrets now are: after a secret is marked, the reading from
        // before shows the value just marked.
        let (transcript, before) = match self.files.remove(&asked.path) {
            Some(Held::Kept(kept)) => {
                let Kept {
                    asked: then,
                    transcript,
                    shown,
                } = *kept;
                (transcript, Some((shown, then.scrubber)))
            }
            _ => (Transcript::new(&asked.path), None),
        };
        let meanwhile = shown_meanwhile(before.as_ref(), &asked.scrubber);
        let held = Held::Reading(before.clone().map(Box::new));
        self.files.insert(asked.path.clone(), held);
        self.start(Job {
            asked,
            transcript,
            before,
        });
        Reading::Pending(meanwhile)
    }

    /// Hand a file to the reading threads, starting them if none are.
    fn start(&mut self, job: Job) {
        let unsent = match self.pool() {
            Ok(jobs) => jobs
                .send(job)
                .err()
                .map(|lost| (lost.0, "the reading threads are gone".to_string())),
            Err(why) => Some((job, why)),
        };
        // Said as a reading that failed, never left "being read" for good.
        if let Some((job, why)) = unsent {
            let _ = self.done.send(job.failed(why));
        }
    }

    /// The reading threads' work queue: [`READERS`] threads at most, each
    /// taking the next file as it finishes one.
    fn pool(&mut self) -> Result<&std::sync::mpsc::Sender<Job>, String> {
        if self.jobs.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            let threads = std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(READERS);
            let mut started = 0;
            for n in 0..threads {
                let (rx, done) = (Arc::clone(&rx), self.done.clone());
                let spawned = std::thread::Builder::new()
                    .name(format!("{READER}{n}"))
                    .spawn(move || read_jobs(&rx, &done, Job::run));
                match spawned {
                    Ok(_) => started += 1,
                    Err(e) => warn!(error = %e, "could not start a reading thread"),
                }
            }
            if started == 0 {
                return Err("no reading thread could be started".to_string());
            }
            self.jobs = Some(tx);
        }
        Ok(self.jobs.as_ref().expect("started above"))
    }

    /// **A file read**: kept, or remembered as not readable — and what came
    /// of it for the views that wait on it, the secrets being `now`. Read
    /// under secrets changed since, it is kept and asked for again at once,
    /// under `now`: what it shows is what was just marked.
    fn done(&mut self, done: Done, now: &Scrubber) -> Landed {
        let Done { asked, read } = done;
        match read {
            Ok((transcript, shown)) => {
                let current = asked.scrubber.same(now);
                let again = Asked {
                    scrubber: now.clone(),
                    ..asked.clone()
                };
                let kept = Kept {
                    asked,
                    transcript,
                    shown: Arc::clone(&shown),
                };
                self.files
                    .insert(kept.asked.path.clone(), Held::Kept(Box::new(kept)));
                match current {
                    true => Landed::Read(shown),
                    false => {
                        let _ = self.reading(again);
                        Landed::Again
                    }
                }
            }
            Err(why) => {
                self.files.insert(
                    asked.path.clone(),
                    Held::Failed(Box::new(asked), why.clone()),
                );
                Landed::Failed(why)
            }
        }
    }

    /// The readings kept, as views would show them — for a test to see
    /// what is read once and kept.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> Vec<Arc<Imported>> {
        self.files
            .values()
            .filter_map(|held| match held {
                Held::Kept(kept) => Some(Arc::clone(&kept.shown)),
                _ => None,
            })
            .collect()
    }

    /// How many files are being read.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.files
            .values()
            .filter(|held| matches!(held, Held::Reading(_)))
            .count()
    }
}

/// What a reading thread is named, and its number after.
const READER: &str = "claude-read-";

/// **What a view may show of a file while it is read again** — the reading
/// kept from before, only if scrubbed as the secrets now are: after a secret
/// is marked, that one shows the value just marked.
fn shown_meanwhile(
    before: Option<&(Arc<Imported>, Scrubber)>,
    now: &Scrubber,
) -> Option<Arc<Imported>> {
    before
        .filter(|(_, by)| by.same(now))
        .map(|(shown, _)| Arc::clone(shown))
}

/// **A reading thread**: the next file, `run`, until the app is gone. A
/// reader that panics answers for its file as a failure — a row left
/// "being read" for good, and a match waiting for good, is a failure no one
/// is told of. `run` is `Job::run`; a parameter so a test can hand it one
/// that panics.
fn read_jobs(
    jobs: &Mutex<std::sync::mpsc::Receiver<Job>>,
    done: &UnboundedSender<Done>,
    run: fn(Job) -> Done,
) {
    loop {
        let job = match jobs.lock() {
            Ok(jobs) => jobs.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else {
            return;
        };
        // What answers for the file if its reading panics: the job itself is
        // gone with the panic.
        let asked = job.asked.clone();
        RUNNING.with(|running| *running.borrow_mut() = Some(asked.scrubber.clone()));
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)))
            .unwrap_or_else(|_| Done {
                asked,
                read: Err("the reader failed (a bug: see the log)".into()),
            });
        RUNNING.with(|running| *running.borrow_mut() = None);
        if done.send(finished).is_err() {
            return;
        }
    }
}

/// Whether the current thread is a reading thread — whose panic is caught
/// and answered for, and so is not to be printed over the screen
/// (`main`'s panic hook; review stop 4, B 4).
pub fn on_a_reading_thread() -> bool {
    std::thread::current()
        .name()
        .is_some_and(|name| name.starts_with(READER))
}

std::thread_local! {
    /// The scrubber of the job this reading thread runs, while it runs.
    static RUNNING: std::cell::RefCell<Option<Scrubber>> = const { std::cell::RefCell::new(None) };
}

/// **`text` scrubbed by the rule this thread's reading is scrubbed by** —
/// for a panic's message (`main`'s hook): scrubbed with the defaults, a
/// value the user marked went to the log as itself (review stop 4, E 5).
/// `None` off a reading thread or between its jobs, for the defaults.
/// Never waits: a borrow already held is `None` too.
pub fn scrubbed_by_this_threads_reading(text: &str) -> Option<String> {
    RUNNING
        .try_with(|running| {
            let running = running.try_borrow().ok()?;
            running.as_ref().map(|scrubber| scrubber.text(text))
        })
        .ok()
        .flatten()
}

impl App {
    /// **What `found` reads as now**, for a view ([`Reading`]) — asking for
    /// it off the UI thread when it is not kept as the file stands. Only
    /// where the user let Shannon read (`App::may_read_claude`): every
    /// reading FOR A VIEW comes through here, so none goes past it. An import
    /// reads through its own door, which asks (`App::import_conversation`).
    pub(crate) fn claude_reading(&mut self, found: &Found) -> Reading {
        if !found
            .path
            .parent()
            .is_some_and(|dir| self.may_read_claude(dir))
        {
            return Reading::Failed(super::import_flow::NOT_ALLOWED.into());
        }
        let scrubber = self.scrubber();
        self.claude_reads.reading(Asked::of(found, &scrubber))
    }

    /// **A conversation read, back from a reading thread** — kept, and given
    /// to every `:graph` that asked for it or shows it from before; one that
    /// could not be read is said there, if one asked. Then what waited on
    /// all of them, the match for older imports, goes on if this was the
    /// last.
    pub(crate) fn claude_read_done(&mut self, done: Done) {
        let (id, path) = (done.asked.id.clone(), done.asked.path.clone());
        // Taken back while it was read (`:unread`): neither kept nor shown.
        if !path.parent().is_some_and(|dir| self.may_read_claude(dir)) {
            return;
        }
        let now = self.scrubber();
        let read = match self.claude_reads.done(done, &now) {
            Landed::Read(shown) => Ok(shown),
            Landed::Failed(why) => Err(why),
            Landed::Again => return,
        };
        // Every tree, a set-aside one too (`Window::tree_places_mut`) — each
        // section listed under these secrets: one listed under others asks
        // again once it is listed again (`ClaudeSection::scrubbed`).
        let mut asked = false;
        for (_, view) in self.windows_mut() {
            if let Some(graph) = view.tree_mut() {
                asked |= graph.take_reading(&path, &id, read.as_ref().ok(), &now);
            }
            for place in view.tree_places_mut() {
                asked |= place.take_reading(&path, &id, read.as_ref().ok(), &now);
            }
        }
        match &read {
            Ok(shown) => info!(conversation = %id, turns = shown.turns.len(), "read"),
            Err(why) => {
                warn!(conversation = %id, error = %why, "could not read a conversation");
                if asked {
                    self.refuse(format!(
                        "Claude Code's conversation {} could not be read: {why}",
                        short_hex(&id)
                    ));
                }
            }
        }
        self.go_on_matching(&path);
    }

    /// **A reading the match for older imports waited on, come** — the
    /// last of them, and the match goes on, and the tree shows what it
    /// found: a session found to continue a conversation hangs under it.
    fn go_on_matching(&mut self, path: &Path) {
        if self.claude_reads.arrived(path) {
            #[cfg(test)]
            {
                self.claude_reads.matched += 1;
            }
            self.match_old_imports();
            self.refresh_graph_views();
        }
    }

    /// **Wait for every reading in flight, and take each** — the event
    /// loop's arm, in test form. A file that comes back again and again is
    /// a loop, and said as one rather than as a reading that never came.
    #[cfg(test)]
    pub(crate) fn await_claude_reads(&mut self) {
        let mut landed: HashMap<PathBuf, usize> = HashMap::new();
        while self.claude_reads.in_flight() > 0 {
            let done = self.next_claude_landing();
            let times = landed.entry(done.asked.path.clone()).or_default();
            *times += 1;
            assert!(
                *times <= 20,
                "{} asked again {times} times: a loop",
                done.asked.path.display()
            );
            self.claude_read_done(done);
        }
    }

    /// **The next reading to come back**, waited for and NOT taken — what
    /// the event loop's arm would be handed next, for a test that changes
    /// something first (`:unread` while it reads). One wait, where three
    /// were written out (review stop 5, C 6).
    #[cfg(test)]
    pub(crate) fn next_claude_landing(&mut self) -> Done {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match self.claude_rx.try_recv() {
                Ok(done) => return done,
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "a reading never came back"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A reading thread is told apart by its name** — what `main`'s panic
    /// hook asks before printing a panic over the screen (review stop 4,
    /// B 4); the threads are named so where they are started.
    #[test]
    fn a_reading_thread_is_known_by_its_name() {
        assert!(!on_a_reading_thread(), "the test's own thread");
        let reader = std::thread::Builder::new()
            .name(format!("{READER}0"))
            .spawn(on_a_reading_thread)
            .unwrap();
        assert!(reader.join().unwrap(), "a reader");
    }

    /// **A reader that panics answers for its file** — as a failure, the
    /// file no longer "being read" and the match not waiting for it for good
    /// (review stop 4, B 2: taking the guard away passed every test).
    #[test]
    fn a_reader_that_panics_answers_for_its_file() {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
        let scrubber = Scrubber::new(
            &shannon_core::SecretsConfig::default(),
            &Arc::new(shannon_core::Aliases::default()),
        );
        let path = PathBuf::from("/nowhere/cc-panics.jsonl");
        let asked = Asked {
            id: "cc-panics".into(),
            path: path.clone(),
            modified: None,
            bytes: 0,
            scrubber,
        };
        tx.send(Job {
            transcript: Transcript::new(&path),
            before: None,
            asked,
        })
        .unwrap();
        drop(tx);
        fn panics(_: Job) -> Done {
            panic!("a reader's bug")
        }
        read_jobs(&Mutex::new(rx), &done_tx, panics);
        let done = done_rx.try_recv().expect("answered");
        assert_eq!(done.asked.path, path);
        assert!(
            matches!(&done.read, Err(why) if why.contains("the reader failed")),
            "said as a failure"
        );
    }

    /// **A reading's panic is scrubbed by its job's own rule** — the values
    /// marked included: by the defaults, a marked value went to the log as
    /// itself (review stop 4, E 5). Only while the job runs.
    #[test]
    fn a_reading_scrubs_what_its_thread_says_by_its_jobs_rule() {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut aliases = shannon_core::Aliases::default();
        let handle = aliases
            .insert("pw", "hunter2seekrit", "test", true)
            .unwrap();
        let scrubber = Scrubber::new(&shannon_core::SecretsConfig::default(), &Arc::new(aliases));
        let path = PathBuf::from("/nowhere/cc-says.jsonl");
        let asked = Asked {
            id: "cc-says".into(),
            path: path.clone(),
            modified: None,
            bytes: 0,
            scrubber,
        };
        tx.send(Job {
            transcript: Transcript::new(&path),
            before: None,
            asked,
        })
        .unwrap();
        drop(tx);
        fn says(job: Job) -> Done {
            let said = scrubbed_by_this_threads_reading("unwrap on hunter2seekrit");
            Done {
                asked: job.asked,
                read: Err(said.unwrap_or_else(|| "nothing".into())),
            }
        }
        assert_eq!(scrubbed_by_this_threads_reading("x"), None, "no job yet");
        read_jobs(&Mutex::new(rx), &done_tx, says);
        let done = done_rx.try_recv().expect("answered");
        let Err(said) = &done.read else {
            panic!("premise: what it said")
        };
        assert!(
            said.contains(&handle) && !said.contains("hunter2"),
            "{said}"
        );
        assert_eq!(scrubbed_by_this_threads_reading("x"), None, "the job over");
    }
}
