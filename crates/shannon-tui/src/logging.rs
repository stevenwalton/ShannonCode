//! Tracing initialization, the on-disk log format, and log-file location.
//!
//! **Two sinks, because there are two artifacts with two readers.**
//!
//! - `shannon.log.<date>` — the developer diary. Everything except the audit
//!   trail, at whatever verbosity `RUST_LOG` asks for. "The log is for us, the
//!   status line is for them" (`AGENTS.md`).
//! - `shannon.audit.<date>` — what the agent was authorized to do and what it
//!   did. A different reader, a different retention, and handable to someone
//!   without the debug noise attached. Deliberately **not** behind the env
//!   filter; see [`init_tracing`].
//!
//! Returns the non-blocking [`Guards`] so the caller (typically `main`) can
//! hold them for the lifetime of the program. Dropping one early loses that
//! file's last few lines.

use anyhow::{Context, Result, anyhow};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// What every file this module writes begins with — the healer's match, so a
/// new sink is protected the moment it is added rather than the next time
/// someone remembers.
const OURS: &str = "shannon.";

/// The developer diary: everything except the audit trail.
const DIARY_PREFIX: &str = "shannon.log";

/// The audit trail — what the agent was authorized to do and what it did.
/// A separate file because it has a different reader, a different retention,
/// and can be handed to someone without the debug noise attached.
const AUDIT_PREFIX: &str = "shannon.audit";

/// **Every file this module writes, by prefix** — [`init_tracing`] opens a
/// writer for each, so a sink added is added here, and the healer's test
/// plants a stale file of each (review stop 12, T7 8: it planted the
/// diary's alone, and an `OURS` narrowed to `shannon.log` left a stale
/// audit trail world-readable with the crate green).
const SINKS: [&str; 2] = [DIARY_PREFIX, AUDIT_PREFIX];

/// Owner-only, for both the directory and the files inside it.
#[cfg(unix)]
const OWNER_ONLY_FILE: u32 = 0o600;

/// Width of the message column — the last *aligned* column, after which fields
/// flow freely.
///
/// **Nothing is truncated to fit it.** A message longer than this pushes its
/// own fields right and leaves the gutter untouched, because the gutter is what
/// the eye scans and a display decision must never reach into what is stored.
/// The earlier mockup elided at this width and that was wrong: the ellipsis
/// would have been on disk forever (`[[feedback_structure_enables_scanning]]`).
const MSG_WIDTH: usize = 44;

/// This run's short id: four hex digits in every line's gutter.
///
/// Derived from the pid rather than randomly, so it is recoverable from
/// outside — `ps` will tell you which run a log belongs to, and a random id
/// could not. Two runs alive at once differ; a pid reused days later collides
/// only across files that the header's timestamp already separates.
fn run_id() -> String {
    format!("{:04x}", std::process::id() & 0xffff)
}

/// `$XDG_STATE_HOME/shannon/log/`, under the state directory every other
/// record lives in (`Config::state_dir`).
///
/// It had its own copy of the XDG rule, without the spec's "absolute paths
/// only" check the shared one makes. Then the rename gave it a reason to
/// matter: a second resolver is a second place a directory name can drift.
pub fn log_dir() -> Result<PathBuf> {
    Ok(shannon_core::Config::state_dir()
        .context("neither $XDG_STATE_HOME nor $HOME is usable")?
        .join("log"))
}

/// Configure the global tracing subscriber to write to a daily-rotated
/// log file under `log_dir()`. Returns the worker guard; keep it alive
/// for the duration of the program.
///
/// The level filter comes from `RUST_LOG` if set; otherwise defaults
/// to `info`. We use `try_init` (not `init`) so a second call fails
/// gracefully instead of panicking — relevant if any test ever links
/// the binary's modules or a benchmark sets up its own subscriber.
pub fn init_tracing() -> Result<Guards> {
    let log_dir = log_dir()?;
    prepare_log_dir(&log_dir)?;

    let run = run_id();
    let [(diary_w, diary_guard), (audit_w, audit_guard)] = SINKS
        .map(|prefix| tracing_appender::non_blocking(OwnerOnlyDaily::new(log_dir.clone(), prefix)));

    let env = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::util::SubscriberInitExt::try_init(two_sinks(run, diary_w, audit_w, env))
        .map_err(|e| anyhow!("install tracing subscriber: {e}"))?;
    Ok(Guards {
        _diary: diary_guard,
        _audit: audit_guard,
    })
}

/// **The log directory, made and kept owner-only** — what [`init_tracing`]
/// does before it opens a file, apart so a test reaches it: `init_tracing`
/// installs a global subscriber, and this was its body, held by nothing
/// (review stop 12, T7 9).
fn prepare_log_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating log dir {}", dir.display()))?;
    // Logs can carry prompt text (at debug level) and API error
    // bodies; clamp the directory to owner-only so other users on a
    // shared machine cannot read them. Set every launch — not just on
    // creation — so a directory that predates this fix is healed too.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting log dir {}", dir.display()))?;
    }
    // Same argument, one level down, and for the same reason it is done every
    // launch: files written before this existed are `0644` and stay that way
    // until something fixes them. Best-effort — a log we cannot chmod is not a
    // reason to refuse to start.
    heal_existing_logs(dir);
    Ok(())
}

/// **The routing rule: which record goes to which file.**
///
/// A named function taking its writers, rather than inline closures inside
/// [`init_tracing`], because the rule is security-relevant and `init_tracing`
/// installs a *global* subscriber — untestable by construction. Here a test can
/// build the same stack over in-memory sinks and assert where each record
/// actually lands (`[[feedback_guard_has_a_visible_half]]`).
///
/// - **The diary** takes every *event* except the audit target's, at whatever
///   verbosity `RUST_LOG` asks for.
/// - **The audit trail** takes only the audit target's events, and is
///   deliberately **not** behind `env`.
/// - **Both take every SPAN**, whatever its target.
///
/// That last line is not symmetry for its own sake. A per-layer filter applies
/// to spans as well as events, and a span is *context*, not a record — so
/// filtering it by target asks the wrong question. `target == AUDIT_TARGET`
/// rejected the session span (whose target is this module), and the audit trail
/// consequently could not say which conversation ran a command: every record
/// read `--------` while the diary's lines for the same turn carried the
/// episode. Found by Steven running a real `rm` through the gate, which is the
/// only way it could have been found — both sinks looked right in isolation.
///
/// That second point is the fix for a live defect. Audit records are emitted at
/// `info!`, so while both shared one filter, `RUST_LOG=warn` — a debug-verbosity
/// knob with nothing to do with auditing — silently destroyed the record of what
/// the agent ran on the user's machine, while `[exec] audit` still said auditing
/// was on. An audit trail that something else can switch off is not an audit
/// trail. Its one control is `[exec] audit`, checked at the emitting site
/// (`App::audit`).
fn two_sinks<D, A>(
    run: String,
    diary_w: D,
    audit_w: A,
    env: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync
where
    D: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
    A: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt;

    let diary = tracing_subscriber::fmt::layer()
        .with_writer(diary_w)
        .with_ansi(false)
        .event_format(Aligned { run: run.clone() })
        .fmt_fields(SessionFields)
        .with_filter(env)
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            // `is_span()` here is DEFENSIVE, not load-bearing: today's only span
            // is targeted at this module, so `!= AUDIT_TARGET` already admits
            // it, and removing this clause fails no test. It is stated anyway so
            // the two layers ask the same question — a span targeted at the
            // audit trail would otherwise lose the diary its context, which is
            // the bug below in a mirror (`[[feedback_diagnose]]`).
            meta.is_span() || meta.target() != AUDIT_TARGET
        }));

    let audit = tracing_subscriber::fmt::layer()
        .with_writer(audit_w)
        .with_ansi(false)
        .event_format(Aligned { run })
        .fmt_fields(SessionFields)
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.is_span() || meta.target() == AUDIT_TARGET
        }));

    tracing_subscriber::registry().with(diary).with(audit)
}

/// The flush guards for both sinks. Hold for the life of the program — dropping
/// one early loses that file's last few lines.
pub struct Guards {
    _diary: WorkerGuard,
    _audit: WorkerGuard,
}

/// Re-clamp every log file already in `dir` to owner-only.
///
/// Best-effort and silent: this runs before the subscriber is installed, so
/// there is nowhere to report to, and a log file we cannot chmod (someone
/// else's, on a shared dir) must not stop the program starting.
fn heal_existing_logs(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(OURS) {
                let _ = std::fs::set_permissions(
                    entry.path(),
                    std::fs::Permissions::from_mode(OWNER_ONLY_FILE),
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// **A daily-rotating log file that is owner-only from the moment it exists.**
///
/// Replaces `tracing_appender::rolling::daily`, which opens with
/// `append(true).create(true)` and no mode — so the file lands at
/// `0666 & !umask`, i.e. `0644` on a stock macOS or Linux umask, and 40-odd
/// such files were sitting on disk. Only the `0700` directory stood in front
/// of them, which makes the file's own bits load-bearing the moment one leaves
/// that directory: a backup, a mode-preserving tarball, or the copy someone
/// attaches to a bug report.
///
/// Setting the process umask instead would have been three lines and wrong —
/// it is global, so it would follow every tool command we spawn into files the
/// *user* creates.
///
/// Rotation is by UTC date, matching the appender it replaces, so the
/// `shannon.log.YYYY-MM-DD` names a user already has keep their meaning.
struct OwnerOnlyDaily {
    dir: PathBuf,
    /// The file stem — one per sink, so the diary and the audit trail are the
    /// same machinery pointed at different names.
    prefix: &'static str,
    /// The date the open file belongs to; `None` until the first write.
    open_for: Option<String>,
    file: Option<File>,
}

impl OwnerOnlyDaily {
    fn new(dir: PathBuf, prefix: &'static str) -> Self {
        Self {
            dir,
            prefix,
            open_for: None,
            file: None,
        }
    }

    /// Write `buf` to the file for `date`, rotating if that is not the file
    /// already open.
    ///
    /// The date is a parameter rather than read from the clock inside, so a
    /// test can cross midnight without one: rotation is the case where a
    /// *fresh* file is created, and a fresh file created `0644` is exactly the
    /// bug — a fix that only handled the first open would look identical from
    /// the outside for the length of a test.
    fn write_for(&mut self, date: &str, buf: &[u8]) -> std::io::Result<usize> {
        if self.open_for.as_deref() != Some(date) {
            let path = self.dir.join(format!("{}.{date}", self.prefix));
            let mut options = std::fs::OpenOptions::new();
            options.append(true).create(true);
            // **At creation, not after.** A chmod after opening leaves a window
            // in which the file exists world-readable, and the whole point is
            // that the bits are never wrong.
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(OWNER_ONLY_FILE);
            }
            let mut file = options.open(&path)?;
            // The header goes in on every open — see `run_header` for why that
            // is two features rather than one. Written straight to the file
            // rather than emitted as an event, because it is not one: it
            // describes the run, and routing it through the formatter would
            // give it a timestamp column and a level it has no use for.
            file.write_all(run_header(std::time::SystemTime::now()).as_bytes())?;
            self.file = Some(file);
            self.open_for = Some(date.to_string());
        }
        match self.file.as_mut() {
            Some(file) => file.write(buf),
            // Unreachable: the branch above either set it or returned `Err`.
            None => Ok(buf.len()),
        }
    }
}

impl Write for OwnerOnlyDaily {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let date = today_utc(std::time::SystemTime::now());
        self.write_for(&date, buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// `YYYY-MM-DD` in UTC, matching the filenames `tracing_appender` produced.
///
/// Hand-rolled rather than adding a date crate for one format string: `time`
/// is in the tree only transitively, and this is the whole of what we need.
fn today_utc(now: std::time::SystemTime) -> String {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the Unix epoch to a civil `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which is the standard branch-free form
/// and is what `time`/`chrono` implement underneath. It shifts the year to
/// start in March so the leap day lands at the end of a 400-year era and needs
/// no special case; `a_known_date_round_trips` pins it against dates computed
/// elsewhere, including a leap day and a century non-leap year.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// `HH:MM:SS.mmm` in UTC — the time half, since the file name carries the date.
///
/// Dropping the date from every line saves eleven columns and gives the daily
/// file an actual job. The header restates it in full, so a file whose name is
/// lost still says when it is from.
fn clock_utc(now: std::time::SystemTime) -> String {
    let since = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since.as_secs();
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}.{:03}", since.subsec_millis())
}

/// The line that opens a run's contribution to a file.
///
/// Written **every time a file is opened** — first write of a run, and again at
/// each rotation — which makes it do two jobs at once. It is the date fallback
/// Steven asked for (a file whose name is corrupted still says what day it
/// covers), and it is the run boundary a date-keyed file otherwise cannot show:
/// a second run on the same day appends a second header, so "which launch was
/// this" is answerable without a `run=` on every line.
///
/// `#` leads so it greps and skips like a comment.
fn run_header(now: std::time::SystemTime) -> String {
    format!(
        "# shannon {}  date={} UTC  run={}  pid={}  started={}T{}Z\n",
        env!("CARGO_PKG_VERSION"),
        today_utc(now),
        run_id(),
        std::process::id(),
        today_utc(now),
        clock_utc(now),
    )
}

/// **The aligned format: a fixed gutter, then free-flowing text.**
///
/// `HH:MM:SS.mmm LEVEL run episode  message  fields…`
///
/// The first four columns are naturally fixed-width, so they align without
/// anything being truncated to make them. That is the half the eye scans —
/// colour, severity, which run, which session — and reading rightward only
/// happens when something catches it. Message and fields flow after, so a long
/// message pushes only its own fields and never shifts the gutter
/// (`[[feedback_structure_enables_scanning]]`).
///
/// Three deliberate choices:
///
/// - **No ANSI here.** Escape codes on disk break editors, diffs and `grep -o`.
///   Colour belongs to a renderer, where it can also be themed through
///   `palette.rs`.
/// - **`episode` and `child` are hoisted** out of the fields and into the
///   gutter, because "which conversation" is a scanning question, not a detail.
///   `--------` when there is none (startup, config) so the column never
///   collapses and the ones after it stay put.
/// - **The audit target becomes the level `AUDIT`.** It is not a level, but it
///   is what must pop, and `with_target(false)` was discarding the deliberate
///   `target: "shannon::audit"` entirely — a marker that reached the file as
///   nothing at all.
///
/// Values are quoted with `Debug`, so a value containing a space or an `=` is
/// unambiguous to a reader and to a parser. Numbers and booleans are not, so
/// `chars=84` stays readable.
struct Aligned {
    run: String,
}

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for Aligned
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let meta = event.metadata();
        let level = if meta.target() == AUDIT_TARGET {
            "AUDIT"
        } else {
            meta.level().as_str()
        };

        let mut fields = Gutter::default();
        event.record(&mut fields);
        // **Explicit beats ambient.** The event's own `episode`/`child` names
        // the session the line is ABOUT; the span names the one the work is
        // happening IN. A line that says which session it concerns wins, which
        // is what keeps `spawned a child session` tagged with the child.
        if fields.episode.is_none() {
            fields.episode = session_from_spans(ctx);
        }

        write!(
            writer,
            "{} {level:<5} {} {} ",
            clock_utc(std::time::SystemTime::now()),
            self.run,
            fields.episode.as_deref().unwrap_or("--------"),
        )?;
        // **Pad the message only when something follows it.** Padding
        // unconditionally left trailing whitespace on every line that carries
        // no fields — invisible while reading, and the sort of thing that shows
        // up as noise in a diff or gets stripped by a tool and then does not
        // match. The column still aligns wherever alignment is what it is for.
        if fields.rest.is_empty() {
            writeln!(writer, "{}", fields.message)
        } else {
            writeln!(
                writer,
                "{:<width$} {}",
                fields.message,
                fields.rest,
                width = MSG_WIDTH,
            )
        }
    }
}

/// The innermost enclosing session span's episode, if any.
///
/// Reads the value straight out of the span's stored fields, which are exactly
/// the episode id because [`SessionFields`] stores nothing else — no parsing a
/// rendered `key=value` back apart. Iterates leaf-to-root, so a nested span
/// (a job inside a turn) wins over its parent.
fn session_from_spans<S, N>(ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>) -> Option<String>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    ctx.event_scope()?.find_map(|span| {
        let ext = span.extensions();
        let stored = ext.get::<tracing_subscriber::fmt::FormattedFields<N>>()?;
        (!stored.fields.is_empty()).then(|| shannon_core::short_hex(&stored.fields).to_string())
    })
}

/// The audit trail's target. One constant so the emitting site, the formatter
/// that gives it its own level, and the layer that routes it to its own file
/// cannot drift.
pub const AUDIT_TARGET: &str = "shannon::audit";

/// **Open the audit trail at startup, so its silence is legible.**
///
/// The file is created on its first write, and the first write is the first
/// tool call — so a session that runs no command leaves no audit file at all,
/// and "nothing was run" is indistinguishable from "auditing is broken".
/// Steven hit exactly that: a stub turn, no file, and no way to tell which.
///
/// An empty-but-present file answers it. One record, naming the configured
/// level, so `cat shannon.audit.<date>` says whether auditing is on *and*
/// whether anything ran — including when the honest answer to the second is
/// "no". Called by `main` rather than from [`init_tracing`] because the level
/// comes from the config, which is not loaded yet at that point.
///
/// `Off` writes nothing, deliberately: a file that exists to say auditing is
/// disabled would be its own small lie about what is being recorded.
///
/// Here, not in `main.rs` where it was: there its test captured through a
/// plain subscriber that takes every target, so the record sent to the diary
/// instead of the audit file passed (review stop 12, T7 10); here it is
/// tested over the routing `init_tracing` installs.
pub fn open_audit_trail(level: shannon_core::AuditLevel) {
    if level == shannon_core::AuditLevel::Off {
        return;
    }
    tracing::info!(
        target: AUDIT_TARGET,
        level = ?level,
        "audit trail opened",
    );
}

/// The span field carrying the session. One constant, read by the span and by
/// the formatter that hoists it into the gutter.
pub(crate) const SESSION_FIELD: &str = "episode";

/// **The session a dispatch is working in**, so every line emitted under it —
/// including from the tasks it spawns — says which conversation it belongs to.
///
/// Entered by the three dispatch helpers in `App` (a key event, a backend
/// message, a finished command), and propagated into spawned work by
/// `.instrument(Span::current())` at each spawn site. Before this, a line knew
/// its session only where a call site passed `episode = …` by hand, which on a
/// real 18-line log was **none of them**.
///
/// **What the gutter means: the session in whose context the work happened**,
/// not "the session this fact is about". Ten handlers legitimately touch
/// several episodes in one dispatch — `merge`, `return_result`, `pick`, a
/// multi-session delete — and they already name the others as explicit fields
/// (`base=`, `branch=`, `merged=`). An event's own `episode`/`child` field
/// therefore *beats* this span: explicit over ambient.
///
/// **`Level::ERROR` is about enablement, not severity.** A span is only created
/// when its level passes the filter, so an `info_span` would vanish under
/// `RUST_LOG=warn` and take the attribution of the surviving `warn!` lines with
/// it. ERROR is the least verbose level, so this span exists whenever anything
/// is being logged at all.
pub(crate) fn session_span(episode: &str) -> tracing::Span {
    tracing::span!(tracing::Level::ERROR, "session", episode = %episode)
}

/// **How a session span's fields are stored: the episode id, and nothing else.**
///
/// This is the `FormatFields` half of the subscriber (`N`), and it is used
/// *only* for spans — [`Aligned`] visits an event's own fields itself and never
/// calls `format_fields`. So making it store the bare value gives the formatter
/// a clean channel to read back, instead of parsing a rendered `key=value`
/// string out of `FormattedFields`.
struct SessionFields;

impl<'w> tracing_subscriber::field::MakeVisitor<tracing_subscriber::fmt::format::Writer<'w>>
    for SessionFields
{
    type Visitor = SessionVisitor<'w>;
    fn make_visitor(&self, target: tracing_subscriber::fmt::format::Writer<'w>) -> Self::Visitor {
        SessionVisitor { writer: target }
    }
}

/// Writes only [`SESSION_FIELD`]'s value, so a span's stored fields ARE the
/// episode id.
struct SessionVisitor<'w> {
    writer: tracing_subscriber::fmt::format::Writer<'w>,
}

impl tracing::field::Visit for SessionVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == SESSION_FIELD {
            let _ = write!(self.writer, "{value}");
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == SESSION_FIELD {
            let _ = write!(self.writer, "{value:?}");
        }
    }
}

impl tracing_subscriber::field::VisitOutput<std::fmt::Result> for SessionVisitor<'_> {
    fn finish(self) -> std::fmt::Result {
        Ok(())
    }
}

impl tracing_subscriber::field::VisitFmt for SessionVisitor<'_> {
    fn writer(&mut self) -> &mut dyn std::fmt::Write {
        &mut self.writer
    }
}

/// Splits an event's fields into the three things the line needs: the message,
/// the session to hoist into the gutter, and everything else.
#[derive(Default)]
struct Gutter {
    message: String,
    episode: Option<String>,
    rest: String,
}

impl Gutter {
    /// Record a value that is text — quoted, so a space or an `=` inside it
    /// cannot be mistaken for a field boundary.
    fn text(&mut self, name: &str, value: String) {
        match name {
            "message" => self.message = value,
            // Either name answers "which conversation", and only one can be in
            // the gutter; `child` is the one that matters on a job's lines.
            "episode" | "child" => {
                // The shared rule, not a fourth `take(8)` — the gutter shows
                // exactly what `:graph` shows, so the id you read on screen is
                // the id you grep for.
                self.episode
                    .get_or_insert_with(|| shannon_core::short_hex(&value).to_string());
            }
            _ => self.push(name, &format!("{value:?}")),
        }
    }

    /// Record a value that is not text, so it is written bare.
    fn bare(&mut self, name: &str, value: String) {
        if name == "message" {
            self.message = value;
        } else {
            self.push(name, &value);
        }
    }

    fn push(&mut self, name: &str, rendered: &str) {
        if !self.rest.is_empty() {
            self.rest.push_str("  ");
        }
        self.rest.push_str(name);
        self.rest.push('=');
        self.rest.push_str(rendered);
    }
}

impl tracing::field::Visit for Gutter {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.text(field.name(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // `%x` reaches us here wrapped in `format_args!`, whose `Debug` is its
        // `Display` — so this is the arm that sees most of our fields, and the
        // text they carry is unquoted until `text` quotes it.
        self.text(field.name(), format!("{value:?}"));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.bare(field.name(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.bare(field.name(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.bare(field.name(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// Render one event through the real formatter, as the file would get it.
    ///
    /// Goes through `tracing_subscriber` rather than calling `Aligned` directly:
    /// the field visitor is the part most likely to be wrong, and it is only
    /// exercised when a real macro records real values
    /// (`[[feedback_wiring_is_untested]]`).
    fn render(f: impl FnOnce()) -> String {
        // **Through the production stack**, not a hand-built subscriber beside
        // it. The first version assembled its own `fmt()` and forgot
        // `.fmt_fields(SessionFields)` — so spans were stored by the DEFAULT
        // field formatter as `episode=<id>`, and the gutter silently showed
        // `episode=` instead of an id. The test was measuring a configuration
        // production does not run ([[feedback_mutation_validates_the_path]]).
        //
        // `trace` so a `debug!` under test is visible whatever `RUST_LOG` says
        // in the environment the suite happens to run in.
        routed("trace", f).0
    }

    /// **The gutter is fixed-width, so the columns after it line up.**
    ///
    /// The whole point of the format: the eye scans the left edge without
    /// reading. If the columns drift, it has to read.
    #[test]
    fn the_gutter_columns_align_across_events() {
        let out = render(|| {
            tracing::info!(episode = "9a8b7c6daaaa", "started a new session");
            tracing::warn!(episode = "a91f0c72bbbb", "refused a user action");
        });
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        // Same prefix shape on both: 12 clock + 1 + 5 level + 1 + 4 run + 1.
        for line in &lines {
            assert_eq!(&line[12..13], " ", "{line}");
            assert_eq!(&line[18..19], " ", "{line}");
            assert_eq!(&line[23..24], " ", "{line}");
        }
        assert_eq!(&lines[0][13..18], "INFO ", "{}", lines[0]);
        assert_eq!(&lines[1][13..18], "WARN ", "{}", lines[1]);
        assert_eq!(&lines[0][19..23], "3f2a");
        // Hoisted out of the fields and shortened to the width `:graph` shows.
        assert_eq!(&lines[0][24..32], "9a8b7c6d", "{}", lines[0]);
        assert_eq!(&lines[1][24..32], "a91f0c72", "{}", lines[1]);
        assert!(!lines[0].contains("episode="), "hoisted, not duplicated");
    }

    /// Run `f` against the **production** routing stack, over two in-memory
    /// sinks, and return `(diary, audit)`.
    ///
    /// Through `two_sinks`, not a hand-built copy of it: a test that assembles
    /// its own layers proves its own path is load-bearing and says nothing
    /// about the one `init_tracing` installs
    /// (`[[feedback_mutation_validates_the_path]]`).
    fn routed(env: &str, f: impl FnOnce()) -> (String, String) {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("sink").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
            type Writer = Sink;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let diary = Arc::new(Mutex::new(Vec::new()));
        let audit = Arc::new(Mutex::new(Vec::new()));
        let subscriber = two_sinks(
            "3f2a".to_string(),
            Sink(Arc::clone(&diary)),
            Sink(Arc::clone(&audit)),
            EnvFilter::new(env),
        );
        tracing::subscriber::with_default(subscriber, f);
        let read = |b: &Arc<Mutex<Vec<u8>>>| {
            String::from_utf8_lossy(&b.lock().expect("sink").clone()).into_owned()
        };
        (read(&diary), read(&audit))
    }

    /// **The audit trail says which session ran the command.**
    ///
    /// Found by Steven running a real `rm` through the gate: the record landed
    /// correctly but its gutter read `--------`, while the diary's lines for the
    /// same turn carried the episode. A per-layer filter applies to **spans as
    /// well as events**, and the audit layer's `target == AUDIT_TARGET` rejected
    /// the session span — whose target is this module, not `shannon::audit` — so
    /// from that layer the span did not exist to be read.
    #[test]
    fn the_audit_trail_also_says_which_session_ran_it() {
        let id = shannon_core::EpisodeId::from_seed(b"the session that ran it");
        let (_diary, audit) = routed("info", || {
            session_span(&id.to_hex()).in_scope(|| {
                tracing::info!(target: AUDIT_TARGET, decision = "run", "tool call");
            });
        });
        assert!(
            audit.contains(&id.short_hex()),
            "the audit trail cannot say which conversation ran a command:\n{audit}"
        );
    }

    /// **Each record goes to exactly one file.**
    #[test]
    fn audit_and_diary_do_not_bleed_into_each_other() {
        let (diary, audit) = routed("info", || {
            tracing::info!(target: AUDIT_TARGET, decision = "allow", "tool call");
            tracing::info!("started a new session");
        });
        assert!(audit.contains("tool call"), "audit sink: {audit:?}");
        assert!(
            !audit.contains("started a new session"),
            "the diary leaked into the audit trail: {audit:?}"
        );
        assert!(diary.contains("started a new session"), "diary: {diary:?}");
        assert!(
            !diary.contains("tool call"),
            "the audit trail leaked into the diary: {diary:?}"
        );
    }

    /// **`RUST_LOG` cannot switch the audit trail off.**
    ///
    /// The defect this split fixes: audit records are `info!`, so while both
    /// sinks shared one env filter, `RUST_LOG=warn` destroyed the record of what
    /// the agent ran while `[exec] audit` still said auditing was on. The only
    /// control over auditing is that config knob.
    #[test]
    fn a_quiet_rust_log_still_records_the_audit_trail() {
        let (diary, audit) = routed("warn", || {
            tracing::info!(target: AUDIT_TARGET, decision = "allow", "tool call");
            tracing::info!("an ordinary line nobody asked for");
            tracing::warn!("a warning they did ask for");
        });
        assert!(
            audit.contains("tool call"),
            "RUST_LOG=warn silenced the audit trail: {audit:?}"
        );
        // The premise, or the assertion above passes for a filter that is simply
        // not being applied: `warn` really did suppress the ordinary `info!`.
        assert!(
            !diary.contains("an ordinary line"),
            "premise: the env filter is in force on the diary: {diary:?}"
        );
        assert!(diary.contains("a warning they did ask for"), "{diary:?}");
    }

    /// **The gutter abbreviates through the shared rule, not its own copy.**
    ///
    /// Asserted against `EpisodeId::short_hex()` rather than "is 8 characters":
    /// a length assertion passes for any coding that happens to agree today,
    /// which is exactly how four independent `8`s accumulated
    /// (`[[feedback_assert_the_subject]]`).
    #[test]
    fn the_gutter_uses_the_one_short_id_rule() {
        let id = shannon_core::EpisodeId::from_seed(b"a log line's session");
        let full = id.to_hex();
        let out = render(|| tracing::info!(episode = %full, "started a new session"));
        let gutter = &out[24..24 + id.short_hex().len()];
        assert_eq!(gutter, id.short_hex(), "{out}");
        // And the premise: the two are not trivially equal because the full id
        // is short. It is 64 chars, so something really did abbreviate.
        assert!(full.len() > id.short_hex().len(), "premise: {full} is long");
    }

    /// **A line under a session span is tagged with that session.**
    ///
    /// The mechanism: the span's stored fields ARE the episode id (because
    /// `SessionFields` stores nothing else), and the formatter reads them back
    /// through `ctx.event_scope()`.
    #[test]
    fn a_span_fills_the_gutter_for_a_line_that_names_no_session() {
        let id = shannon_core::EpisodeId::from_seed(b"span-tagged");
        let out = render(|| {
            session_span(&id.to_hex()).in_scope(|| {
                tracing::info!(chars = 84u64, "prompt submitted");
            });
        });
        assert_eq!(&out[24..32], id.short_hex(), "{out}");
        // The line itself named no session, so this could only have come from
        // the span.
        assert!(out.contains("chars=84"), "{out}");
    }

    /// **Explicit beats ambient.** A line that names its own session keeps it,
    /// even under a span for a different one — which is what keeps
    /// `spawned a child session` tagged with the child rather than the parent.
    #[test]
    fn an_events_own_session_overrides_the_span() {
        let parent = shannon_core::EpisodeId::from_seed(b"the parent");
        let child = shannon_core::EpisodeId::from_seed(b"the child");
        assert_ne!(parent.short_hex(), child.short_hex(), "premise: distinct");

        let out = render(|| {
            session_span(&parent.to_hex()).in_scope(|| {
                tracing::info!(child = %child.to_hex(), "spawned a child session");
            });
        });
        assert_eq!(&out[24..32], child.short_hex(), "the span won: {out}");
    }

    /// The innermost span wins — a job running inside a turn is the job's.
    #[test]
    fn the_innermost_session_span_wins() {
        let outer = shannon_core::EpisodeId::from_seed(b"outer session");
        let inner = shannon_core::EpisodeId::from_seed(b"inner session");
        let out = render(|| {
            session_span(&outer.to_hex()).in_scope(|| {
                session_span(&inner.to_hex()).in_scope(|| {
                    tracing::info!("a nested dispatch");
                });
            });
        });
        assert_eq!(&out[24..32], inner.short_hex(), "{out}");
    }

    /// No session (startup, config) still holds the column open.
    #[test]
    fn an_event_with_no_session_keeps_the_column() {
        let out = render(|| tracing::info!(path = "~/.config/shannon", "loaded config"));
        assert_eq!(&out[24..32], "--------", "{out}");
        assert!(out.contains("loaded config"));
    }

    /// **A long message pushes only its own fields.**
    ///
    /// Nothing is truncated to fit the column — the earlier mockup elided here
    /// and that would have put an ellipsis on disk forever.
    #[test]
    fn a_long_message_is_not_truncated() {
        let long = "a job stopped on a command it may not run unattended";
        assert!(long.len() > MSG_WIDTH, "premise: the message overflows");
        let out = render(|| tracing::warn!(command = "rm -rf /tmp/x", "{}", long));
        assert!(out.contains(long), "the message was cut: {out}");
        assert!(out.contains(r#"command="rm -rf /tmp/x""#), "{out}");
    }

    /// Text is quoted and numbers are not, so a value containing a space or an
    /// `=` cannot be read as a field boundary.
    #[test]
    fn values_are_quoted_only_when_they_are_text() {
        let out = render(|| tracing::info!(chars = 84u64, ok = true, note = "a b=c", "submitted"));
        assert!(out.contains("chars=84"), "{out}");
        assert!(out.contains("ok=true"), "{out}");
        assert!(out.contains(r#"note="a b=c""#), "{out}");
    }

    /// The audit target reads as `AUDIT` in the level column.
    ///
    /// Asserted in the audit *sink*, since the split is what puts it there —
    /// which is the point: a reader who cats the two files together can still
    /// tell the records apart at a glance.
    #[test]
    fn the_audit_target_becomes_its_own_level() {
        let (diary, audit) = routed("trace", || {
            tracing::info!(target: AUDIT_TARGET, decision = "deny", "tool call");
            tracing::info!("ordinary line");
        });
        assert_eq!(
            &audit.lines().next().expect("an audit line")[13..18],
            "AUDIT"
        );
        assert_eq!(
            &diary.lines().next().expect("a diary line")[13..18],
            "INFO "
        );
    }

    /// No ANSI reaches the file — codes on disk break editors, diffs and
    /// `grep -o`. Colour is the renderer's job.
    #[test]
    fn nothing_written_to_the_file_carries_ansi() {
        let out = render(|| tracing::error!(episode = "9a8b7c6d", "anthropic api turn failed"));
        assert!(
            !out.contains('\u{1b}'),
            "escape codes reached the file: {out:?}"
        );
    }

    /// The header states the date in full, so a file whose NAME is lost still
    /// says what it covers — and a second run appends another, which is the
    /// run boundary a date-keyed file otherwise cannot show.
    #[test]
    fn the_header_carries_the_date_and_the_run() {
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_789_600_500);
        let header = run_header(t);
        assert!(header.starts_with("# shannon "), "{header}");
        assert!(header.contains("date=2026-09-16 UTC"), "{header}");
        assert!(header.contains(&format!("run={}", run_id())), "{header}");
        assert!(header.ends_with('\n'), "a header is its own line");
    }

    /// **No line carries trailing whitespace.**
    ///
    /// The message column was padded unconditionally, so every event with no
    /// fields ended in spaces — invisible while reading the log, and the kind
    /// of thing that turns into diff noise or silently fails to match after a
    /// tool strips it. Padding now happens only when a field follows.
    #[test]
    fn no_line_ends_in_whitespace() {
        let out = render(|| {
            // One with fields and one without: the bug was only in the second,
            // so a test using either alone would have missed it.
            tracing::info!(episode = "9a8b7c6daaaa", "started a new session");
            tracing::info!(episode = "9a8b7c6daaaa", chars = 84u64, "prompt submitted");
        });
        for line in out.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace: {line:?}");
        }
        // And the alignment still holds where it is load-bearing.
        let padded = out.lines().nth(1).expect("two lines");
        assert!(padded.contains("prompt submitted  "), "{padded}");
    }

    /// Pins the date arithmetic against values computed elsewhere.
    ///
    /// A calendar is exactly the kind of thing that looks right and is wrong
    /// four years later, so the cases are the ones that break naive versions:
    /// the epoch itself, a leap day, and 1900 — a year divisible by 4 that is
    /// *not* a leap year, which the `/100` and `/400` terms exist for.
    #[test]
    fn a_known_date_round_trips() {
        assert_eq!(civil_from_days(0), (1970, 1, 1), "the epoch");
        assert_eq!(civil_from_days(-1), (1969, 12, 31), "the day before it");
        assert_eq!(civil_from_days(59), (1970, 3, 1), "1970 is not a leap year");
        assert_eq!(civil_from_days(789), (1972, 2, 29), "a leap day");
        assert_eq!(civil_from_days(-25_567), (1900, 1, 1), "a century non-leap");
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1), "so Feb had 28 days");
        assert_eq!(civil_from_days(11_017), (2000, 3, 1), "but 2000 was a leap");
    }

    /// The filename is what a user already has, so the format must not drift.
    #[test]
    fn the_filename_date_is_zero_padded_utc() {
        // 2026-09-16T23:15:00Z — late enough in the day that a local-time
        // implementation west of UTC would name the file the 16th and one east
        // would name it the 17th. The appender this replaces used UTC.
        let t = UNIX_EPOCH + Duration::from_secs(1_789_600_500);
        assert_eq!(today_utc(t), "2026-09-16");
        // Single-digit month and day keep their zeros, or the names no longer
        // sort as dates.
        assert_eq!(
            today_utc(UNIX_EPOCH + Duration::from_secs(86_400 * 4)),
            "1970-01-05"
        );
    }

    /// **The file is owner-only, and still owner-only after a rotation.**
    ///
    /// The second half is the one that matters: the bug being fixed is a file
    /// created with the wrong mode, and rotation is where a *new* file gets
    /// created mid-run. A fix that only handled the first open would pass a
    /// test that never crossed midnight.
    #[cfg(unix)]
    #[test]
    fn every_log_file_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        crate::private_file::assert_umask_grants_more(dir.path());
        let mut appender = OwnerOnlyDaily::new(dir.path().to_path_buf(), DIARY_PREFIX);

        appender
            .write_for("2026-09-16", b"first day\n")
            .expect("write");
        appender
            .write_for("2026-09-17", b"next day\n")
            .expect("write");
        appender.flush().expect("flush");

        let mut seen = 0;
        for entry in std::fs::read_dir(dir.path()).expect("read dir").flatten() {
            let mode = entry.metadata().expect("metadata").permissions().mode();
            assert_eq!(
                mode & 0o777,
                OWNER_ONLY_FILE,
                "{} is not owner-only: {:o}",
                entry.path().display(),
                mode & 0o777
            );
            seen += 1;
        }
        // Assert the subject: two dates must have produced two files, or the
        // loop above proved nothing about rotation.
        assert_eq!(seen, 2, "each date gets its own file");
    }

    /// **A log directory from before is made owner-only, and every sink's
    /// old files with it** — the directory at `0700` and each file of each
    /// [`SINKS`] at `0600`, on the next launch (`prepare_log_dir`, the part
    /// of `init_tracing` a test can reach). It planted the diary's file
    /// alone, so the audit trail's was held by nothing, and the directory's
    /// clamp by nothing at all (review stop 12, T7 8 and 9).
    #[cfg(unix)]
    #[test]
    fn a_log_dir_from_before_is_made_owner_only_with_every_sinks_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temp dir");
        let dir = root.path().join("log");
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let mode = |p: &Path| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
        crate::private_file::assert_umask_grants_more(&dir);
        let stale: Vec<PathBuf> = (SINKS.iter())
            .map(|prefix| dir.join(format!("{prefix}.2026-06-09")))
            .collect();
        for file in &stale {
            std::fs::write(file, b"from before\n").expect("write");
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }
        // Something that is not ours stays untouched — this runs over a
        // directory, and a blanket chmod of everything in it would be a
        // different and much ruder function.
        let other = dir.join("notes.txt");
        std::fs::write(&other, b"not a log\n").expect("write");
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        prepare_log_dir(&dir).expect("prepared");

        assert_eq!(mode(&dir), 0o700, "the directory is owner-only");
        for file in &stale {
            assert_eq!(
                mode(file),
                OWNER_ONLY_FILE,
                "{} was not healed",
                file.display()
            );
        }
        assert_eq!(mode(&other), 0o644, "a non-log file must be left alone");
    }

    /// Every sink's files are the healer's: each prefix begins with what it
    /// matches.
    #[test]
    fn every_sink_is_ours() {
        for prefix in SINKS {
            assert!(prefix.starts_with(OURS), "{prefix} is not under {OURS}");
        }
    }

    /// **An audit trail exists from startup, so its silence means something**
    /// — the record in the AUDIT file, naming the level, and not in the
    /// diary; `off` writes neither.
    ///
    /// Without this the file appears only on the first tool call, and a session
    /// that runs no command leaves nothing — making "nothing ran" and "auditing
    /// is broken" the same observation. Steven hit exactly that.
    #[test]
    fn opening_the_audit_trail_records_the_configured_level() {
        use shannon_core::AuditLevel;

        let (diary, audit) = routed("info", || open_audit_trail(AuditLevel::Decisions));
        assert!(audit.contains("audit trail opened"), "{audit:?}");
        assert!(
            audit.contains("Decisions"),
            "the level is the point — it says what is being recorded: {audit:?}"
        );
        assert!(!diary.contains("audit trail opened"), "{diary:?}");

        // `Off` stays silent: a file asserting that auditing is disabled would
        // be its own small lie about what is being recorded.
        let quiet = routed("info", || open_audit_trail(AuditLevel::Off));
        assert_eq!(
            quiet,
            (String::new(), String::new()),
            "audit=off still opened a trail"
        );
    }

    /// **A session's span survives a quiet filter** — the reason it is an
    /// ERROR span (`session_span`): under `RUST_LOG=warn` an INFO span is
    /// never made, and the `warn!` lines that do survive lose the session
    /// they ran in. Held by nothing: an INFO span passed the crate (review
    /// stop 12, T7 7).
    #[test]
    fn a_sessions_span_survives_a_warn_filter() {
        let (diary, _) = routed("warn", || {
            session_span("9a8b7c6daaaa").in_scope(|| tracing::warn!("a warning"));
        });
        let line = diary.lines().next().expect("the warning, logged");
        assert_eq!(
            &line[24..32],
            "9a8b7c6d",
            "the session in its gutter: {line}"
        );
    }
}
