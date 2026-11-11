//! **What a finished command left running** — held until it ends, listed
//! while it runs, and ended when Shannon exits.
//!
//! `server &` finishes the moment its shell does, and the server runs on. It
//! used to be released with the shell: nothing listed it and nothing ended
//! it, so it outlived Shannon, reparented, holding a port or a lock in a
//! state nobody could see. Steven, 2026-09-30: "you yourself have left
//! processes running that have hung and this creates a weird state. So we
//! want users to have better control over that and clarity into what is
//! actually running."
//!
//! # Why the shell is not reaped
//!
//! A leftover keeps the command's process group (the shell led it), so one
//! signal to `-pgid` reaches all of it — but only while that number still
//! names it. Once the shell is reaped and the last member exits, the number
//! is free for another process to lead its own group, and a late kill lands
//! on a stranger. So the shell is left an unreaped zombie for as long as the
//! group is held: a pid that is not reaped is not reused, nor the group it
//! names. Its exit is observed without reaping (`waitid(WNOWAIT)`, in
//! `exec.rs`), and it is reaped only AFTER its entry leaves the registry, so
//! a signal sent through the registry always lands on this group.
//!
//! # Membership
//!
//! `kill(-pgid, 0)` cannot answer "is anything left": the zombie leader is a
//! member (on macOS, with only the zombie left, the call returns EPERM). The
//! group is listed instead, and a pid counts only if it is not a zombie:
//! on macOS by `proc_listpids`, on Linux by `/proc` (no call lists a group
//! there, so every process's `stat` is read, as `ps` does). Elsewhere the
//! answer is "cannot tell", and a command's leftovers are released
//! untracked, as before.
//!
//! # Limits
//!
//! A process that leaves the group (`setsid`, a daemon's double fork into a
//! session of its own) is not ours to see or end; if it still holds the
//! command's output, the result says something does. And Shannon killed
//! outright (`SIGKILL`) ends nothing: no code of ours runs after that.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::io::{AsyncRead, AsyncReadExt};

use super::Leader;

/// How often a held group is looked at: what `:ls` shows is at most this
/// old, and a group whose last process ended is let go within it.
const POLL: Duration = Duration::from_millis(500);

/// How long held groups have, on SIGTERM, to end before SIGKILL — once, for
/// all of them together, when Shannon exits. A server is asked first, so it
/// can remove its pid file or lock; one that ignores the asking is killed.
const GRACE: Duration = Duration::from_secs(1);

/// How fast a leftover's output is read once its command has finished, in
/// bytes a second. Unread, a writer blocks when the pipe fills; closed, it
/// dies of SIGPIPE at its next line. Read flat out, `yes &` cost a whole
/// core here (4 GB/s, measured); at this rate it cost 16 ms of CPU in 2 s,
/// and it runs on, held back by the pipe as any slow reader holds a writer.
pub(super) const DRAIN_RATE: u64 = 1 << 20;

/// The most commands whose leftovers are held at once. Each holds one zombie
/// in the process table; past this, a command's leftovers are ended at
/// once, and its result says so.
const MAX_HELD: usize = 64;

/// How much of a leftover's latest output is kept: enough for its last line
/// ("Listening on :8080", the error it hung on), not a log.
const TAIL: usize = 1024;

/// One process a finished command left running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftProcess {
    pub pid: i32,
    /// The program's name, as the kernel has it (`ps -o comm`).
    pub name: String,
}

/// What a command left running when its shell exited — told to the model
/// and the user with its result ([`ExecOutcome::report`](super::ExecOutcome::report)).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LeftBehind {
    /// Nothing of the command is still running.
    #[default]
    Nothing,
    /// These are still running, held: listed by `:ls`, ended when Shannon
    /// exits.
    Held(Vec<LeftProcess>),
    /// These were ended at once: Shannon already held the leftovers of as
    /// many commands as it holds (`MAX_HELD`, 64).
    Ended(Vec<LeftProcess>),
    /// Something it started still holds its output, and Shannon cannot see
    /// what: it left the command's process group, or this platform cannot be
    /// asked. It may outlive Shannon.
    Untracked,
}

/// A finished command whose processes are still running, as `:ls` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    /// The call that started it: which session's, the caller knows.
    pub tool_use_id: String,
    /// The command as approved (secret handles, never their values).
    pub command: String,
    /// When its shell exited.
    pub since: SystemTime,
    /// What is still running, as of the last look (at most `POLL`, half a
    /// second, old).
    pub processes: Vec<LeftProcess>,
    /// Bytes it has printed since its command finished.
    pub printed: u64,
    /// The last line it printed with anything on it — REDACTED here, as the
    /// command's own output is before it leaves this crate; not stripped of
    /// control characters, which is the screen's to do.
    pub last_line: Option<String>,
}

/// What a leftover has printed since its command finished: a count, and the
/// last [`TAIL`] bytes. Shared by its readers (stdout, stderr) and the
/// registry.
#[derive(Debug, Default)]
pub(super) struct Printed {
    bytes: AtomicU64,
    tail: Mutex<Vec<u8>>,
    /// Its command's redaction — the same the command's output got.
    secrets: crate::SecretsConfig,
    aliases: Arc<crate::Aliases>,
    /// The last line as shown, and the byte count it was taken at: kept
    /// until more is printed. `:ls` asks every frame, and redacting it again
    /// each time cost 240 µs a frame per chatty leftover (Reviewer B,
    /// round 17).
    shown: Mutex<(u64, Option<String>)>,
}

impl Printed {
    /// For a command run with `secrets` and `aliases`.
    pub(super) fn redacting(secrets: crate::SecretsConfig, aliases: Arc<crate::Aliases>) -> Self {
        Self {
            secrets,
            aliases,
            ..Self::default()
        }
    }

    fn record(&self, chunk: &[u8]) {
        self.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        if let Ok(mut tail) = self.tail.lock() {
            tail.extend_from_slice(&chunk[chunk.len().saturating_sub(TAIL)..]);
            let over = tail.len().saturating_sub(TAIL);
            tail.drain(..over);
        }
    }

    /// The last line with anything on it, redacted. A `\r` ends a line too:
    /// a progress bar redraws in place, and its newest frame is the last,
    /// where a split on `\n` alone joined every frame and the cut showed the
    /// oldest (Reviewer B, round 17).
    fn last_line(&self) -> Option<String> {
        let bytes = self.bytes.load(Ordering::Relaxed);
        let mut shown = self.shown.lock().ok()?;
        if shown.0 != bytes {
            let raw = {
                let tail = self.tail.lock().ok()?;
                String::from_utf8_lossy(&tail)
                    .split(['\n', '\r'])
                    .rev()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(str::to_string)
            };
            let redacted =
                raw.map(|line| crate::secrets::redact(&line, &self.secrets, &self.aliases).text);
            *shown = (bytes, redacted);
        }
        shown.1.clone()
    }
}

/// The finished commands whose processes are still running. Cloning shares
/// the one registry.
#[derive(Debug, Clone)]
pub struct Leftovers(Arc<Mutex<Registry>>);

#[derive(Debug)]
struct Registry {
    next: u64,
    cap: usize,
    held: Vec<Entry>,
}

/// One held command. It OWNS its `Leader`: signalling goes through it, and
/// it leaves the registry only by being taken out to be reaped
/// ([`Leftovers::update`]) or dropped with the registry — which kills.
#[derive(Debug)]
struct Entry {
    id: u64,
    leader: Leader,
    tool_use_id: String,
    command: String,
    since: SystemTime,
    processes: Vec<LeftProcess>,
    printed: Arc<Printed>,
}

impl Default for Leftovers {
    fn default() -> Self {
        Self::with_cap(MAX_HELD)
    }
}

impl Leftovers {
    /// A registry holding at most `cap` commands' leftovers.
    pub fn with_cap(cap: usize) -> Self {
        Self(Arc::new(Mutex::new(Registry {
            next: 0,
            cap,
            held: Vec::new(),
        })))
    }

    /// What is held now, oldest first.
    pub fn list(&self) -> Vec<Leftover> {
        self.0
            .lock()
            .map(|reg| {
                reg.held
                    .iter()
                    .map(|e| Leftover {
                        tool_use_id: e.tool_use_id.clone(),
                        command: e.command.clone(),
                        since: e.since,
                        processes: e.processes.clone(),
                        printed: e.printed.bytes.load(Ordering::Relaxed),
                        last_line: e.printed.last_line(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **End everything held**: SIGTERM to every group, up to a second
    /// (`GRACE`) for them to go, then SIGKILL to what is left. Blocks for at
    /// most that long; Shannon calls it on its way out.
    pub fn end_all(&self) {
        let groups = self.signal_all(libc::SIGTERM);
        if groups.is_empty() {
            return;
        }
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline
            && groups
                .iter()
                .any(|&g| !matches!(membership(g), Membership::Live(live) if live.is_empty()))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.signal_all(libc::SIGKILL);
    }

    /// Send `signal` to every held group through its [`Leader`], returning
    /// the pgids — to LIST, never to signal by: a number copied out of a
    /// leader can outlive it.
    fn signal_all(&self, signal: libc::c_int) -> Vec<i32> {
        let Ok(reg) = self.0.lock() else {
            return Vec::new();
        };
        reg.held
            .iter()
            .filter_map(|e| {
                e.leader.signal(signal);
                e.leader.pgid()
            })
            .collect()
    }

    /// Hold `leader`, whose shell has exited unreaped and whose `live`
    /// processes run on, until they end or Shannon does. `pipes` are its
    /// output, still open if a leftover holds them, recorded into `printed`;
    /// `tool_use_id` and `command` are the call that started it.
    pub(super) fn hold(
        &self,
        leader: Leader,
        (tool_use_id, command): (&str, &str),
        live: Vec<LeftProcess>,
        pipes: (
            Option<tokio::process::ChildStdout>,
            Option<tokio::process::ChildStderr>,
        ),
        printed: Printed,
    ) -> LeftBehind {
        let printed = Arc::new(printed);
        let id = {
            // Refused (a poisoned lock, a full registry): `leader` drops
            // unreaped, so its group is ended now.
            let Ok(mut reg) = self.0.lock() else {
                return LeftBehind::Ended(live);
            };
            if reg.held.len() >= reg.cap {
                return LeftBehind::Ended(live);
            }
            reg.next += 1;
            let id = reg.next;
            reg.held.push(Entry {
                id,
                leader,
                tool_use_id: tool_use_id.to_string(),
                command: command.to_string(),
                since: SystemTime::now(),
                processes: live.clone(),
                printed: printed.clone(),
            });
            id
        };
        let (out, err) = pipes;
        if let Some(out) = out {
            tokio::spawn(drain_slowly(out, Some(printed.clone())));
        }
        if let Some(err) = err {
            tokio::spawn(drain_slowly(err, Some(printed)));
        }
        tokio::spawn(watch(self.clone(), id));
        LeftBehind::Held(live)
    }

    /// The pgid of entry `id`, to list its group by.
    fn pgid_of(&self, id: u64) -> Option<i32> {
        let reg = self.0.lock().ok()?;
        reg.held
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.leader.pgid())
    }

    /// Record what is left of entry `id`. When nothing is, the entry is
    /// taken out — under the lock, so no signal can be sent through it after
    /// — and its `Leader` returned for the caller to reap.
    fn update(&self, id: u64, now: Membership) -> Update {
        let Ok(mut reg) = self.0.lock() else {
            return Update::Gone;
        };
        let Some(at) = reg.held.iter().position(|e| e.id == id) else {
            return Update::Gone;
        };
        match now {
            Membership::Live(live) if live.is_empty() => Update::Ended(reg.held.remove(at).leader),
            Membership::Live(live) => {
                reg.held[at].processes = live;
                Update::Held
            }
            // "Cannot tell" is not "ended": keep it, and its pgid reserved.
            Membership::Unknown | Membership::Unsupported => Update::Held,
        }
    }
}

/// What [`Leftovers::update`] found.
enum Update {
    /// Something of it still runs (or it cannot be told): look again.
    Held,
    /// Nothing of it runs: here is its leader, out of the registry, to reap.
    Ended(Leader),
    /// It is no longer in the registry.
    Gone,
}

/// Look at a held group every [`POLL`] until nothing of it is left; then
/// reap its shell. The leader lives in the registry, not here: this task
/// dropped (the runtime ending) leaves it there, to be ended at `end_all`
/// or with the registry.
async fn watch(registry: Leftovers, id: u64) {
    loop {
        tokio::time::sleep(POLL).await;
        let Some(pgid) = registry.pgid_of(id) else {
            return;
        };
        match registry.update(id, membership(pgid)) {
            Update::Held => {}
            Update::Ended(leader) => {
                let _ = leader.reap().await;
                return;
            }
            Update::Gone => return,
        }
    }
}

/// Read `pipe` until EOF at no more than [`DRAIN_RATE`], recording into
/// `printed` (a count and a tail) or, with none, keeping nothing.
pub(super) async fn drain_slowly<R: AsyncRead + Unpin>(mut pipe: R, printed: Option<Arc<Printed>>) {
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if let Some(printed) = &printed {
                    printed.record(&chunk[..n]);
                }
                let pause = Duration::from_micros(n as u64 * 1_000_000 / DRAIN_RATE);
                tokio::time::sleep(pause).await;
            }
        }
    }
}

/// What is left of a process group, as the kernel lists it.
///
/// Three answers, not two: one `None` meant both "this platform cannot list
/// a group" and "the listing failed this time", and the hold read it as
/// release while the watcher read it as keep — so a transient failure at the
/// hold let a live server go untracked (Reviewer D, round 17).
#[derive(Debug)]
pub(super) enum Membership {
    /// This platform cannot list a group (neither macOS nor Linux).
    Unsupported,
    /// It can, and this listing failed: nothing is known.
    Unknown,
    /// These are alive. A zombie is not — including the group's own exited
    /// shell.
    Live(Vec<LeftProcess>),
}

#[cfg(target_os = "macos")]
pub(super) fn membership(pgid: i32) -> Membership {
    // `<sys/proc_info.h>`; not in the `libc` crate.
    const PROC_PGRP_ONLY: u32 = 2;
    // Room for a group far larger than a command leaves; past it, the
    // first this many are listed, which still says "something is left".
    let mut pids: Vec<libc::pid_t> = vec![0; 4096];
    let width = std::mem::size_of::<libc::pid_t>();
    // SAFETY: the buffer is `pids.len() * width` bytes, as passed.
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            pgid as u32,
            pids.as_mut_ptr().cast(),
            (pids.len() * width) as libc::c_int,
        )
    };
    if bytes < 0 {
        return Membership::Unknown;
    }
    pids.truncate(bytes as usize / width);
    Membership::Live(pids.into_iter().filter_map(live_process).collect())
}

#[cfg(target_os = "macos")]
fn live_process(pid: libc::pid_t) -> Option<LeftProcess> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: plain-old-data, filled by the kernel below.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is `size` bytes, as passed.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    // A zombie has no BSD info to give — the call fails (measured, macOS
    // 15) — so the group's exited shell is not counted as running. Were
    // that to change, every command would read as leaving its shell
    // running, and `captures_output_and_exit_status` fails.
    if got != size {
        return None;
    }
    let text = |raw: &[libc::c_char]| -> String {
        let bytes: Vec<u8> = raw
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let name = Some(text(&info.pbi_name))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| text(&info.pbi_comm));
    Some(LeftProcess { pid, name })
}

/// Linux: each process `/proc` lists, read from its `stat`. A process that
/// ends between the listing and the read is not alive; `/proc` itself
/// unreadable is "nothing is known", which holds the group.
#[cfg(target_os = "linux")]
pub(super) fn membership(pgid: i32) -> Membership {
    walk_proc(
        std::path::Path::new("/proc"),
        std::process::id() as i32,
        pgid,
    )
}

/// The walk behind `membership`, on any `root`. Bytes, not text: a process's
/// name is what `prctl` set, cut at 15 bytes, so a read as text dropped it
/// and the group read as gone (review stop 17, E2 3). A walk that did not
/// find this process (`me`) saw nothing of the system: an empty `/proc` is
/// nothing known, not nothing running.
#[cfg(any(target_os = "linux", test))]
fn walk_proc(root: &std::path::Path, me: i32, pgid: i32) -> Membership {
    let Ok(procs) = std::fs::read_dir(root) else {
        return Membership::Unknown;
    };
    let mut saw_me = false;
    let live = procs
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            saw_me |= pid == me;
            let stat = std::fs::read(entry.path().join("stat")).ok()?;
            live_member(pid, &String::from_utf8_lossy(&stat), pgid)
        })
        .collect();
    match saw_me {
        true => Membership::Live(live),
        false => Membership::Unknown,
    }
}

/// `pid`, as its `/proc/<pid>/stat` line describes it, when it is a live
/// member of `pgid`. The name (`comm`, field 2) is in parentheses and may
/// itself hold spaces and `)`, so the fields after it are counted from the
/// LAST `)`: state (field 3: `Z` a zombie, `X`/`x` dead), the parent, then
/// the group (field 5). A line that does not parse is no member.
#[cfg(any(target_os = "linux", test))]
fn live_member(pid: i32, stat: &str, pgid: i32) -> Option<LeftProcess> {
    let name = stat.get(stat.find('(')? + 1..stat.rfind(')')?)?;
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_ascii_whitespace();
    let state = fields.next()?;
    let group = fields.nth(1)?.parse::<i32>().ok()?;
    (group == pgid && !matches!(state, "Z" | "X" | "x")).then(|| LeftProcess {
        pid,
        name: name.to_string(),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn membership(_pgid: i32) -> Membership {
    Membership::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry holding one entry whose leader is a real `sleep` in a group
    /// of its own, for `update` alone — told what the group holds rather than
    /// asked. The group ends when its leader drops.
    fn with_one_entry() -> (Leftovers, u64) {
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .expect("sleep runs");
        let registry = Leftovers::default();
        registry.0.lock().unwrap().held.push(Entry {
            id: 7,
            leader: Leader::new(child),
            tool_use_id: "toolu_7".into(),
            command: "server &".into(),
            since: SystemTime::now(),
            processes: Vec::new(),
            printed: Arc::default(),
        });
        (registry, 7)
    }

    /// **"Cannot tell" is not "ended"**: a group whose members cannot be
    /// listed stays held, its pgid reserved; only an empty listing lets it
    /// go — handing its leader out of the registry to be reaped — and a
    /// listing in between refreshes what `:ls` shows.
    #[tokio::test]
    async fn a_group_is_let_go_only_when_seen_empty() {
        let (registry, id) = with_one_entry();
        for unknown in [Membership::Unknown, Membership::Unsupported] {
            assert!(matches!(registry.update(id, unknown), Update::Held));
        }
        assert_eq!(registry.list().len(), 1, "kept when it cannot be listed");
        let one = vec![LeftProcess {
            pid: 1,
            name: "server".into(),
        }];
        assert!(matches!(
            registry.update(id, Membership::Live(one.clone())),
            Update::Held
        ));
        assert_eq!(registry.list()[0].processes, one, "refreshed");
        let Update::Ended(leader) = registry.update(id, Membership::Live(Vec::new())) else {
            panic!("an empty group is let go");
        };
        assert!(registry.list().is_empty(), "out of the registry");
        drop(leader); // unreaped: ends the `sleep`
    }

    /// **A leftover's last line is redacted, as its command's output is** —
    /// `:ls` shows it, and the secret a server prints at startup is the one
    /// a screen-share catches — and redacted once per change: `:ls` asks
    /// every frame, and redacting it each time cost 240 µs a frame.
    #[test]
    fn the_last_line_is_redacted_once_per_change() {
        let value = crate::fakes::GITHUB_TOKEN;
        let mut aliases = crate::Aliases::default();
        let handle = aliases.insert("gh", value, "test", true).expect("a secret");
        let printed = Printed::redacting(crate::SecretsConfig::default(), Arc::new(aliases));
        printed.record(format!("token {value}\n").as_bytes());
        let line = printed.last_line().expect("a line");
        assert!(line.contains(&handle) && !line.contains(value), "{line}");
        assert_eq!(
            printed.shown.lock().unwrap().0,
            printed.bytes.load(Ordering::Relaxed)
        );
        // More printed: taken again.
        printed.record(b"ready\n");
        assert_eq!(printed.last_line().as_deref(), Some("ready"));
    }

    /// The tail keeps the LAST of what was printed, within its bound, and
    /// the last line is the last one with anything on it — across chunks,
    /// and from one chunk larger than the whole tail.
    #[test]
    fn the_last_line_is_the_last_one_printed() {
        let printed = Printed::default();
        assert_eq!(printed.last_line(), None, "nothing printed");
        printed.record(b"starting\nlisten");
        printed.record(b"ing on :8080\n\n  \n");
        assert_eq!(printed.last_line().as_deref(), Some("listening on :8080"));
        let mut flood = vec![b'x'; 3 * TAIL];
        flood.extend_from_slice(b"\nGET / 200\n");
        printed.record(&flood);
        assert_eq!(printed.last_line().as_deref(), Some("GET / 200"));
        assert!(printed.tail.lock().unwrap().len() <= TAIL, "bounded");
        // A progress bar redrawn with `\r`: its newest frame.
        let bar = Printed::default();
        bar.record(b"[  8%]\r[ 50%]\r[ 80%]\r");
        assert_eq!(bar.last_line().as_deref(), Some("[ 80%]"));
        assert_eq!(
            printed.bytes.load(Ordering::Relaxed),
            (15 + 17 + flood.len()) as u64,
            "every byte counted, kept or not"
        );
    }

    /// **The `/proc` walk keeps a name that is not text, and does not read
    /// a listing of nothing as nothing running** (review stop 17, E2 3).
    #[test]
    fn a_proc_walk_reads_bytes_and_says_when_it_saw_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let proc_of = |pid: i32, stat: &[u8]| {
            let at = dir.path().join(pid.to_string());
            std::fs::create_dir(&at).unwrap();
            std::fs::write(at.join("stat"), stat).unwrap();
        };
        proc_of(10, b"10 (me) S 1 10 10 0 0");
        proc_of(20, b"20 (a\xE3\x82) S 1 77 77 0 0");
        let Membership::Live(live) = walk_proc(dir.path(), 10, 77) else {
            panic!("it saw itself");
        };
        assert_eq!(live.len(), 1, "a name cut mid-character: {live:?}");
        assert_eq!(live[0].pid, 20);
        assert!(
            matches!(walk_proc(dir.path(), 999, 77), Membership::Unknown),
            "this process absent: the walk saw nothing of the system"
        );
        let gone = dir.path().join("no-such");
        assert!(matches!(walk_proc(&gone, 10, 77), Membership::Unknown));
    }

    /// **Linux's `stat` line, read as the kernel writes it**: a member of the
    /// group by field 5, not by the parent (field 4) beside it; a zombie (the
    /// command's own exited shell) and a dead process are not alive; a name
    /// holding spaces and `)` still leaves the fields after it where they
    /// are, since they are counted from the LAST `)`; and a line that does
    /// not parse is no member, never a panic. (Linux reads `/proc` itself;
    /// this runs everywhere.)
    #[test]
    fn a_proc_stat_line_says_whether_its_process_is_a_live_member() {
        let line = |name: &str, state: &str, parent: i32, group: i32| {
            format!("1234 ({name}) {state} {parent} {group} {group} 34816 0 -1 4194560 120 0")
        };
        let sleep = Some(LeftProcess {
            pid: 1234,
            name: "sleep".into(),
        });
        assert_eq!(live_member(1234, &line("sleep", "S", 1, 4321), 4321), sleep);
        assert_eq!(live_member(1234, &line("sleep", "R", 1, 4321), 4321), sleep);
        assert_eq!(
            live_member(1234, &line("sleep", "S", 4321, 99), 4321),
            None,
            "by group, not parent"
        );
        assert_eq!(
            live_member(1234, &line("sleep", "S", 1, 99), 4321),
            None,
            "another group"
        );
        for gone in ["Z", "X", "x"] {
            assert_eq!(
                live_member(1234, &line("sh", gone, 1, 4321), 4321),
                None,
                "{gone}"
            );
        }
        let odd = live_member(1234, &line("a) b (c", "S", 1, 4321), 4321);
        assert_eq!(odd.map(|p| p.name), Some("a) b (c".to_string()));
        for broken in [
            "",
            "1234",
            "1234 sleep S 1 4321",
            "1234 )sleep( S 1 4321",
            "1234 (sleep) S 1",
        ] {
            assert_eq!(live_member(1234, broken, 4321), None, "{broken:?}");
        }
    }
}
