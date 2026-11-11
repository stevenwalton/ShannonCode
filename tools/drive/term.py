"""A pty harness that ANSWERS the terminal queries a real emulator answers.

Without this, every crossterm round trip (DSR cursor position, the kitty
keyboard-protocol query) times out after 2s and the app sees failures no real
terminal would produce -- which is an artifact of the harness, not a bug.
"""
import os, pty, select, sys, time, signal, fcntl, termios, struct, re, tempfile

# One throwaway config + state home per harness run, shared by every launch in
# it — a drive that quits and relaunches still finds its own sessions, and
# never the user's. Without it a drive read the user's config (their `resume`,
# model and rules changed what was on screen), wrote into their session store,
# and — once marked secrets were kept in the keychain — resuming their last
# session asked the OS for those secrets mid-run.
#
# A secret marked in a drive (`:secret <text>`) is kept in the vault, a file
# under `XDG_STATE_HOME` — this run's home too, so it is thrown away with it
# (the keychain and `keep_marked` are retired; review stop 16, C 11).
_HOME = tempfile.mkdtemp(prefix="shannon-drive-home-")


def hermetic(env):
    """`env` with the app's config and state pointed into this run's home,
    unless the caller chose its own — and Claude Code's home too: without
    it every `:graph` read the user's own Claude Code conversations for
    their titles, and the knowledge screen their Claude Code memory
    (10-18, found when Shannon began asking before reading them)."""
    env.setdefault("XDG_CONFIG_HOME", os.path.join(_HOME, "config"))
    env.setdefault("XDG_STATE_HOME", os.path.join(_HOME, "state"))
    env.setdefault("CLAUDE_CONFIG_DIR", CLAUDE_HOME)
    return env


# Claude Code's home for this run — empty unless a drive plants a
# conversation in it (`plant_conversation`).
CLAUDE_HOME = os.path.join(_HOME, "claude")


def plant_conversation(conversation_id, prompt, reply):
    """One conversation where Claude Code keeps the cwd's (its own naming of
    a directory, for a path under 200 characters), in this run's home."""
    import json
    name = "".join(c if c.isascii() and c.isalnum() else "-" for c in os.getcwd())
    here = os.path.join(CLAUDE_HOME, "projects", name)
    os.makedirs(here, exist_ok=True)
    lines = [
        {"type": "user", "uuid": "u1", "parentUuid": None, "isSidechain": False,
         "timestamp": "01", "message": {"role": "user", "content": prompt}},
        {"type": "assistant", "uuid": "a1", "parentUuid": "u1", "isSidechain": False,
         "timestamp": "02", "message": {"id": "M1", "model": "claude-x", "role": "assistant",
                                        "content": [{"type": "text", "text": reply}]}},
    ]
    with open(os.path.join(here, conversation_id + ".jsonl"), "w") as f:
        f.writelines(json.dumps(line) + "\n" for line in lines)
    return here


class Term:
    def __init__(self, argv, env=None, cols=100, rows=30, cwd=None):
        e = dict(os.environ)
        e.update(env or {})
        e.setdefault("TERM", "xterm-256color")
        # Before the fork, so the run stops on the cause: after a `cargo clean`
        # the child's exec failed, it exited, and the run went on to crash at
        # an unrelated check (10-04).
        if not os.access(argv[0], os.X_OK):
            sys.exit("drive: no executable %s -- build first: cargo build --bin shannon "
                     "(from the repo root)" % argv[0])
        self.cols, self.rows = cols, rows
        self.status = None
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            # A failed `execve` RETURNS, and the child would then go on running
            # the test script — 19 confusing failures instead of one clear one.
            # Happens whenever this is run from outside the repo root, or before
            # `cargo build` (Reviewer B).
            try:
                # `cwd`: where the app starts (it reads its start from it);
                # the binary is named from here first, since `argv[0]` is
                # relative to the repo root.
                if cwd is not None:
                    argv = [os.path.abspath(argv[0]), *argv[1:]]
                    os.chdir(cwd)
                os.execve(argv[0], argv, e)
            except OSError as err:
                sys.stderr.write("drive: cannot exec %s: %s\n" % (argv[0], err))
            os._exit(127)
        self.winsize(cols, rows)
        self.log = []
        self.closed = False

    def winsize(self, cols, rows):
        self.cols, self.rows = cols, rows
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def _reply(self, chunk):
        # DSR cursor position -> ESC[row;colR   (row/col are 1-based)
        for _ in re.findall(rb'\x1b\[6n', chunk):
            os.write(self.fd, b'\x1b[1;1R')
        # kitty keyboard protocol query -> "flags 0" (supported, none set)
        for _ in re.findall(rb'\x1b\[\?u', chunk):
            os.write(self.fd, b'\x1b[?0u')
        # primary device attributes -> VT100 with AVO
        for _ in re.findall(rb'\x1b\[(?:0)?c', chunk):
            os.write(self.fd, b'\x1b[?1;2c')

    def read(self, seconds):
        if self.closed:
            return b""
        out = b""
        end = time.time() + seconds
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try:
                    d = os.read(self.fd, 65536)
                except OSError:
                    break
                if not d:
                    break
                self._reply(d)
                out += d
        self.log.append(out)
        return out

    def send(self, data, settle=0.5):
        """Write keys and settle. A closed pty raises `EIO` on macOS rather
        than reporting EOF, so a key sent to an app that has just exited would
        take the whole run down with a traceback -- and "did it exit?" is a
        thing we deliberately test. Treat it as gone."""
        if isinstance(data, str):
            data = data.encode().decode("unicode_escape").encode("latin1")
        try:
            os.write(self.fd, data)
        except OSError:
            self.closed = True
            return b""
        return self.read(settle)

    def alive(self):
        """False once the child has exited. The reap is remembered: a second
        `waitpid` on a reaped child raises `ECHILD`, and a run whose child had
        already been seen to exit crashed with that instead of answering."""
        if self.status is None:
            try:
                pid, status = os.waitpid(self.pid, os.WNOHANG)
            except ChildProcessError:
                return False
            if pid:
                self.status = status
        return self.status is None

    def wait_exit(self, seconds=5.0):
        """True once the child has exited, polling until `seconds`.

        A bare `alive()` after a fixed settle is a coin flip -- `:qa` has to
        drain the store, restore the terminal and unwind before the process is
        reaped, and on a loaded machine that outruns any settle you pick. A
        flaky check is worse than no check: it teaches you to ignore the run.
        """
        end = time.time() + seconds
        while time.time() < end:
            if not self.alive():
                return True
            time.sleep(0.05)
        return False

    def close(self):
        try:
            os.kill(self.pid, signal.SIGKILL); os.waitpid(self.pid, 0)
        except Exception:
            pass

def plain(b):
    s = b.decode("utf8", "replace")
    s = re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]', '', s)
    s = re.sub(r'\x1b[\]P][^\x07\x1b]*(\x07|\x1b\\)?', '', s)
    return re.sub(r'\s{3,}', ' ', s)

def shannon(**kw):
    env = {"SHANNON_API_KEY_FILE": "/dev/null"}
    env.update(kw.pop("env", {}))
    return Term(["./target/debug/shannon"], env=hermetic(env), **kw)


class Screen:
    """A minimal VT parser -- enough for ratatui, which positions with CUP and
    then writes runs of text. Gives a real grid to read, instead of a stream of
    cell writes that says nothing about what is on screen."""

    def __init__(self, cols=100, rows=30):
        self.cols, self.rows = cols, rows
        self.grid = [[" "] * cols for _ in range(rows)]
        self.cy = self.cx = 0

    def resize(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.grid = [[" "] * cols for _ in range(rows)]
        self.cy = self.cx = 0

    def feed(self, data):
        s = data.decode("utf8", "replace")
        i = 0
        while i < len(s):
            ch = s[i]
            if ch == "\x1b":
                # A CSI whole: parameters, intermediates, the final byte
                # (ECMA-48). DECSCUSR's `ESC [ 2 SP q` has an intermediate;
                # matched without one, its `2 q` was written as text where
                # the cursor stood.
                m = re.match(r'\x1b\[([0-?]*)[ -/]*([@-~])', s[i:])
                if m:
                    args, cmd = m.group(1), m.group(2)
                    nums = [int(x) for x in args.split(";") if x.isdigit()]
                    if cmd == "H":
                        self.cy = (nums[0] - 1) if nums else 0
                        self.cx = (nums[1] - 1) if len(nums) > 1 else 0
                    elif cmd == "J":
                        self.grid = [[" "] * self.cols for _ in range(self.rows)]
                        self.cy = self.cx = 0
                    elif cmd == "K":
                        if 0 <= self.cy < self.rows:
                            for x in range(self.cx, self.cols):
                                self.grid[self.cy][x] = " "
                    i += m.end()
                    continue
                m = re.match(r'\x1b[\]P][^\x07\x1b]*(\x07|\x1b\\)?', s[i:])
                if m:
                    i += m.end()
                    continue
                i += 2
                continue
            if ch == "\r":
                self.cx = 0
            elif ch == "\n":
                self.cy += 1
                self.cx = 0
            elif ch >= " ":
                if 0 <= self.cy < self.rows and 0 <= self.cx < self.cols:
                    self.grid[self.cy][self.cx] = ch
                self.cx += 1
            i += 1

    def text(self):
        return "\n".join("".join(r).rstrip() for r in self.grid)

    def find(self, needle):
        return needle in self.text()


class Session(Term):
    """A `Term` that keeps a `Screen`, and can force a full repaint so the grid
    is complete rather than whatever cells last changed."""

    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.screen = Screen(self.cols, self.rows)

    def read(self, seconds):
        out = super().read(seconds)
        self.screen.feed(out)
        return out

    def winsize(self, cols, rows):
        super().winsize(cols, rows)
        if hasattr(self, "screen"):
            self.screen.resize(cols, rows)

    def repaint(self, settle=0.6):
        """`<C-l>` -- a full redraw, so the grid holds the whole screen."""
        self.send("\x0c", settle=settle)
        return self.screen.text()

    def wait_for(self, seen, seconds=3.0):
        """Read until the grid shows what `seen` asks (a needle, or a
        predicate over the text), or `seconds` pass -- so a slow answer on a
        loaded machine is still an answer, where a fixed settle missed it.
        Returns whether it was seen."""
        test = seen if callable(seen) else (lambda text: seen in text)
        end = time.time() + seconds
        while not test(self.screen.text()):
            if time.time() >= end:
                return False
            self.read(0.1)
        return True


def start(args=(), **kw):
    """Launch the binary (with `args`, e.g. `["setup"]`) under the hermetic
    home. `SHANNON_API_KEY_FILE=/dev/null` names an EMPTY key file, so a
    launch is never a first launch (which opens setup by itself) unless a
    drive asks for that on purpose."""
    env = {"SHANNON_API_KEY_FILE": "/dev/null"}
    env.update(kw.pop("env", {}))
    s = Session(["./target/debug/shannon", *args], env=hermetic(env), **kw)
    s.read(2.5)
    return s
