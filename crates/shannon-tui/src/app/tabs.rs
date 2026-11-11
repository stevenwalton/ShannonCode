//! **The tab pages** — vim's: a named layout of panes, "like different
//! monitors or different workspaces" (Steven, 10-20;
//! `docs/DesignDocs/tabs.md`). The tab you are in is the app's `workspace`
//! — its prompt windows with it, each its own draft and aim (Q2: the aim is
//! per tab; Steven, 10-26: "the prompt pane should be unique to the tab");
//! every other one is parked here, in tab order, until it is shown
//! ([`Tabs::switch`], the one door both cross by). Buffers are the app's, so
//! a session shown in two tabs is one buffer in two windows.

use super::Window;
use crate::wm::{BufferId, PaneId};
use crate::workspace::Workspace;

/// **What a walk of the windows goes over** (`tabs.md` §6b): the tab you
/// are in — what is drawn, the aim, what you can see — or every tab — what
/// holds a buffer anywhere, what must reach each window on it. Every walk
/// that asks "which windows hold it" takes one, so each caller says which
/// it means: a walk widened or narrowed in silence is the class this is for
/// (`tabs.md` §2, "the `workspace` accessor narrows in silence").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Over {
    ThisTab,
    EveryTab,
}

/// **A tab not shown** — its panes with their windows: what the app holds
/// of the tab you are in (`workspace`). Its prompts' drafts and aims are
/// their windows' (`PromptView`), so parked with them; the aim was a field
/// of its own here, swapped beside the workspace, while the prompt was one
/// composition on the app.
pub(super) struct Tab {
    pub(super) workspace: Workspace<Window>,
}

/// **A tab's place in the order** — what it holds while parked (`None`
/// while it is the one shown: its arrangement is then the app's), and the
/// name you gave it. The name is the place's, not the arrangement's, so it
/// stays where it is whichever tab is shown, and moves with the place
/// (`:tabmove`): `:tabrename`'s (Q3, Steven: "Let's not just allow the
/// session renaming but also tab renaming. This will help people
/// organize."); `None`, the tab line says what its focused pane shows.
struct Page {
    parked: Option<Tab>,
    name: Option<String>,
}

impl Page {
    fn holding(tab: Option<Tab>) -> Self {
        Page {
            parked: tab,
            name: None,
        }
    }
}

/// **Every tab**, in tab order — what each parked one holds, and each one's
/// name — and the windows of tabs closed since the reconcile last took
/// them.
pub(super) struct Tabs {
    pages: Vec<Page>,
    at: usize,
    /// **A closed tab's windows**, until `App::reconcile_pane_state` takes
    /// them with every workspace's closed ones: closing a tab closes its
    /// windows, and what must happen when a window goes (a decision closed
    /// is denied) has one door.
    dropped: Vec<(PaneId, Window)>,
}

impl Tabs {
    /// One tab, the one shown.
    pub(super) fn one() -> Self {
        Tabs {
            pages: vec![Page::holding(None)],
            at: 0,
            dropped: Vec::new(),
        }
    }

    /// How many tabs, the shown one with them.
    pub(super) fn count(&self) -> usize {
        self.pages.len()
    }

    /// The shown tab's place, from 0.
    pub(super) fn current(&self) -> usize {
        self.at
    }

    /// The parked tabs, each with its place, in tab order.
    pub(super) fn parked(&self) -> impl Iterator<Item = (usize, &Tab)> {
        (self.pages.iter().enumerate()).filter_map(|(at, page)| Some((at, page.parked.as_ref()?)))
    }

    /// [`parked`](Self::parked), to change.
    pub(super) fn parked_mut(&mut self) -> impl Iterator<Item = (usize, &mut Tab)> {
        (self.pages.iter_mut().enumerate())
            .filter_map(|(at, page)| Some((at, page.parked.as_mut()?)))
    }

    /// **The name given to tab `at`** (from 0), if one was.
    pub(super) fn name(&self, at: usize) -> Option<&str> {
        self.pages.get(at)?.name.as_deref()
    }

    /// **Name the shown tab** — `None` takes its name off — giving back the
    /// name it had.
    pub(super) fn rename_current(&mut self, name: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.pages[self.at].name, name)
    }

    /// **Show tab `to`**: what the app holds of the shown one — `workspace`
    /// — parked in its place, and tab `to`'s put in its. The one door
    /// between tabs, so a field a tab gains is swapped here or is not a
    /// tab's (the destructuring lists them). `false`, and nothing moved, for
    /// the shown tab or one that is not there.
    pub(super) fn switch(&mut self, to: usize, workspace: &mut Workspace<Window>) -> bool {
        let Some(mut tab) = self.pages.get_mut(to).and_then(|page| page.parked.take()) else {
            return false;
        };
        let Tab { workspace: parked } = &mut tab;
        std::mem::swap(workspace, parked);
        self.pages[self.at].parked = Some(tab);
        self.at = to;
        true
    }

    /// **Move the shown tab to place `to`** (from 0), the others keeping
    /// their order — `:tabmove`'s. `false`, and nothing moved, for a place
    /// not there.
    pub(super) fn move_current(&mut self, to: usize) -> bool {
        if to >= self.pages.len() {
            return false;
        }
        let page = self.pages.remove(self.at);
        self.pages.insert(to, page);
        self.at = to;
        true
    }

    /// **A new tab after the shown one, shown** — vim's `:tabnew` (measured,
    /// Neovim 0.12.6: it opens after the current tab).
    pub(super) fn open_after(&mut self, tab: Tab, workspace: &mut Workspace<Window>) {
        let to = self.at + 1;
        self.pages.insert(to, Page::holding(Some(tab)));
        let shown = self.switch(to, workspace);
        debug_assert!(shown, "the tab just put there");
    }

    /// **Close tab `at`** — the shown one ([`close_current`](Self::close_current))
    /// or a parked one, the shown one staying shown: its windows kept for
    /// the reconcile, as a closed tab's are. `false`, and nothing closed,
    /// for the only tab or one not there.
    pub(super) fn close(&mut self, at: usize, workspace: &mut Workspace<Window>) -> bool {
        if at == self.at {
            return self.close_current(workspace);
        }
        if at >= self.pages.len() {
            return false;
        }
        if let Some(closed) = self.pages.remove(at).parked {
            self.dropped.extend(closed.workspace.into_windows());
        }
        if at < self.at {
            self.at -= 1;
        }
        true
    }

    /// **Close the shown tab**, showing the one to its right, or to its left
    /// if it was the last (measured, Neovim 0.12.6): its windows kept for
    /// the reconcile ([`take_dropped`](Self::take_dropped)). `false`, and
    /// nothing closed, for the only tab — vim's E784.
    fn close_current(&mut self, workspace: &mut Workspace<Window>) -> bool {
        let closing = self.at;
        let next = match closing + 1 < self.pages.len() {
            true => closing + 1,
            false if closing > 0 => closing - 1,
            false => return false,
        };
        let shown = self.switch(next, workspace);
        debug_assert!(shown, "a neighbour that is there");
        // Parked at its place by the switch, so there to be taken.
        if let Some(closed) = self.pages.remove(closing).parked {
            self.dropped.extend(closed.workspace.into_windows());
        }
        if closing < self.at {
            self.at -= 1;
        }
        true
    }

    /// **Close every tab but the shown one** — `:tabonly`'s: their windows
    /// kept for the reconcile, as [`close_current`](Self::close_current)
    /// keeps a closed tab's. How many closed.
    pub(super) fn close_others(&mut self) -> usize {
        let mut pages = std::mem::take(&mut self.pages);
        let shown = pages.remove(self.at);
        let closed = pages.len();
        let windows = (pages.into_iter()).filter_map(|page| page.parked);
        self.dropped
            .extend(windows.flat_map(|tab| tab.workspace.into_windows()));
        self.pages = vec![shown];
        self.at = 0;
        closed
    }

    /// The windows of the tabs closed since last asked, taken.
    pub(super) fn take_dropped(&mut self) -> Vec<(PaneId, Window)> {
        std::mem::take(&mut self.dropped)
    }
}

/// **One tab, as the tab line says it** ([`App::tab_labels`](super::App::tab_labels)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TabLabel {
    /// Its number, from 1 — what `{N}gt` takes.
    pub(crate) number: usize,
    /// What its focused pane shows.
    pub(crate) name: String,
    /// The tab you are in.
    pub(crate) current: bool,
    /// **The alerts' mark, when a session it holds is marked** — the glyph
    /// `:ls`'s mark column shows (`[tui.alerts] glyph`; an alert with `mark
    /// = true`, news by default): the hotlist's "new output", the user's to
    /// define (`[[feedback_alerts_user_definable]]`).
    pub(crate) mark: Option<String>,
    /// **A decision waits in it** — a decision pane there with something to
    /// answer: the hotlist's top tier, "needs you" (`tabs.md` Q1 (a): "the
    /// tabline marks that tab needs-you").
    pub(crate) decides: bool,
}

/// **Where a session's window is, as a sentence about it says it**
/// ([`App::where_shown`](super::App::where_shown)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shown {
    /// A window in the tab you are in holds it.
    Here,
    /// Only another tab's — the first, in tab order, numbered as `gt`
    /// counts them.
    InTab(usize),
    /// No window holds it.
    Nowhere,
}

impl Shown {
    /// The tab to name, when it is not this one.
    pub(crate) fn tab(self) -> Option<usize> {
        match self {
            Shown::InTab(tab) => Some(tab),
            Shown::Here | Shown::Nowhere => None,
        }
    }
}

/// **Which way `:tabnext` and `:tabprevious` go** — `gt` and `gT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Toward {
    Next,
    Previous,
}

/// A number written in digits alone — no sign, which `usize::from_str`
/// would take, so `+2` is never read as tab 2.
pub(super) fn digits(n: &str) -> Option<usize> {
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .then(|| n.parse().ok())
        .flatten()
}

/// **Which tab `arg` names**, from 0, standing in tab `at` of `count` — the
/// argument `:tabnext`, `:tabonly` and `:tabclose` take, and `:tabmove`'s
/// relative ones (vim's, measured, Neovim 0.12.6, `tabs.md` §6b): `N`, tab
/// N, from 1; `+N` / `-N`, N to the right or the left (bare `+` / `-`: one),
/// never wrapped as `gt` wraps; `$`, the last; `.`, this one (vim's range,
/// `:.tabclose`). No such tab is refused, as vim's E475 is, saying where you
/// are. The same words as a range before the command (`:+tabmove`,
/// `:$tabnext`), which `App::counted` hands here as the argument.
pub(super) fn tab_named(arg: &str, at: usize, count: usize) -> Result<usize, String> {
    let not_one = || format!("`{arg}` is not a tab number — N, +N, -N, . or $");
    let steps = |n: &str| match n {
        "" => Some(1),
        n => digits(n),
    };
    let to = if arg == "." {
        Some(at)
    } else if arg == "$" {
        count.checked_sub(1)
    } else if let Some(n) = arg.strip_prefix('+') {
        (at.checked_add(steps(n).ok_or_else(not_one)?)).filter(|&to| to < count)
    } else if let Some(n) = arg.strip_prefix('-') {
        at.checked_sub(steps(n).ok_or_else(not_one)?)
    } else {
        (digits(arg).ok_or_else(not_one)?)
            .checked_sub(1)
            .filter(|&to| to < count)
    };
    to.ok_or_else(|| no_tab(arg, at, count))
}

/// The refusal for a tab that is not there: which you asked for, and where
/// you are.
fn no_tab(arg: &str, at: usize, count: usize) -> String {
    format!("there is no tab {arg} — this is tab {} of {count}", at + 1)
}

/// **Where `:tabmove {arg}` puts the tab at `at`**, of `count` (places from
/// 0) — vim's, measured (Neovim 0.12.6, `tabs.md` §6b): bare or `$`, the
/// end; `N`, after tab N (`0`, the front; N itself or the tab before it, no
/// move); `+N` / `-N`, N places right or left, as [`tab_named`] reads them.
/// Past either end is refused, as vim's E475 is — never wrapped, unlike `gt`.
pub(super) fn move_to(arg: &str, at: usize, count: usize) -> Result<usize, String> {
    if arg.is_empty() {
        return Ok(count - 1);
    }
    if arg == "$" || arg == "." || arg.starts_with(['+', '-']) {
        return tab_named(arg, at, count);
    }
    let n = digits(arg)
        .ok_or_else(|| format!("`{arg}` is not a tab number — N (after tab N), +N, -N or $"))?;
    // Tab N's place once this one is lifted out: one less, when it was to
    // the right.
    (n <= count)
        .then(|| if n > at { n - 1 } else { n })
        .ok_or_else(|| no_tab(arg, at, count))
}

impl super::App {
    /// `:[N]tabnew [N|id|name]` — **a new tab after this one**, or after tab
    /// N (vim's count, measured: `:0tabnew` at the front — r13-F L4; past
    /// the last, at the end), or after the tab a range names (`:.tabnew`
    /// this one, `:+tabnew` the next, `:-tabnew` the one before — so just
    /// before this one — `:$tabnew` the last; measured), on a fresh session
    /// ([`fresh_session`](Self::fresh_session), what `:new` shows), or on the
    /// session named, as `:b N` names one — made the active session as `:b
    /// N` makes it. The range is read first: refused, nothing is made.
    pub(super) fn tabnew_command(&mut self, count: Option<&str>, args: &str) {
        let (at, tabs) = (self.tabs.current(), self.tabs.count());
        // Where it goes, as a place among the tabs once it is in: after tab
        // N is place N.
        let place = match count {
            None => None,
            Some(range) => match digits(range) {
                Some(n) => Some(n),
                None => match tab_named(range, at, tabs) {
                    Ok(tab) => Some(tab + 1),
                    Err(why) => return self.refuse(format!("`:tabnew`: {why}")),
                },
            },
        };
        let arg = args.trim();
        let buffer = match arg {
            "" => self.fresh_session(),
            arg => match self.session_buffer_named(&super::as_listed(arg)) {
                Ok(super::Named::Buffer(buffer)) => buffer,
                Ok(super::Named::Session(episode)) => match self.buffer_for_episode(episode) {
                    Ok(buffer) => buffer,
                    Err(why) => return self.refuse(format!("`:tabnew`: {why}")),
                },
                Err(why) => return self.refuse(format!("`:tabnew`: {why}")),
            },
        };
        self.open_tab(buffer);
        if let Some(place) = place {
            self.tabs.move_current(place.min(self.tabs.count() - 1));
        }
        if !arg.is_empty()
            && let Some(id) = self.session_of(buffer).map(|b| b.conversation.episode().id)
        {
            self.set_head(id);
        }
    }

    /// **The tab `verb`'s argument names**, from 0 — this one when bare,
    /// else [`tab_named`]'s; refused, said, when there is none.
    fn tab_argument(&mut self, verb: &str, args: &str) -> Option<usize> {
        let (at, count) = (self.tabs.current(), self.tabs.count());
        match args.trim() {
            "" => Some(at),
            arg => match tab_named(arg, at, count) {
                Ok(tab) => Some(tab),
                Err(why) => {
                    self.refuse(format!("`:{verb}`: {why}"));
                    None
                }
            },
        }
    }

    /// `:tabclose[!] [N|+N|-N|$]` — **close the tab you are in, or the one
    /// named** ([`tab_named`]; vim's, measured: `:tabclose 3` closes tab 3
    /// and leaves you where you are), through [`close_tab`](Self::close_tab).
    /// Refused on the only tab (vim's E784), and over the work held in it —
    /// a request waiting, a call or a review, or a prompt's draft — unless
    /// `force`d (Steven, 10-26: "It is an explicit command. So not a deny, a
    /// close"): a request is left waiting, hidden; a draft dropped, said.
    /// The argument was taken and dropped until T4, so `:tabclose 3` closed
    /// the tab you were in.
    pub(super) fn tabclose_command(&mut self, args: &str, force: bool) {
        let Some(close) = self.tab_argument("tabclose", args) else {
            return;
        };
        // `:qa` — `:q` from the prompt bar says "the only prompt" while
        // another pane is open (r12-F L2).
        if self.tabs.count() == 1 {
            return self.refuse("`:tabclose`: this is the only tab — `:qa` leaves");
        }
        let held = self.held_in_tab(close);
        if !force && !held.is_empty() {
            return self.refuse(self.tab_holds("tabclose", close, &held));
        }
        let here = close == self.tabs.current();
        self.close_tab(close);
        // Another tab's close is out of sight, said as `:tabonly`'s is; the
        // tab you are in closing is the screen changing (r13-F L9) — but for
        // a draft it dropped, which is said.
        let n = close + 1;
        match (here, super::held::dropped(held.drafts.len())) {
            (false, dropped) => {
                let dropped = dropped.map_or(String::new(), |d| format!("; {d}"));
                self.notify(format!(
                    "closed tab {n} — its sessions stay open (`:ls`){dropped}"
                ));
            }
            (true, Some(dropped)) => self.notify(format!("closed tab {n} — {dropped}")),
            (true, None) => {}
        }
    }

    /// `:tabonly[!] [N|+N|-N|$]` — **close every tab but this one**, or but
    /// the one named ([`tab_named`]), shown first (vim's, measured, Neovim
    /// 0.12.6): their windows through the reconcile, as
    /// [`close_tab`](Self::close_tab)'s are. Refused over the work held in
    /// another tab unless `force`d, as `:tabclose` is, naming the first;
    /// with one tab, said, as vim's "Already only one tab page" is.
    pub(super) fn tabonly_command(&mut self, args: &str, force: bool) {
        let Some(keep) = self.tab_argument("tabonly", args) else {
            return;
        };
        if self.tabs.count() == 1 {
            return self.notify("this is the only tab");
        }
        let held: Vec<(usize, super::HeldWork)> = (0..self.tabs.count())
            .filter(|&tab| tab != keep)
            .map(|tab| (tab, self.held_in_tab(tab)))
            .collect();
        if !force && let Some((tab, first)) = held.iter().find(|(_, held)| !held.is_empty()) {
            return self.refuse(self.tab_holds("tabonly", *tab, first));
        }
        let drafts: usize = held.iter().map(|(_, held)| held.drafts.len()).sum();
        self.show_tab(keep);
        let closed = self.tabs.close_others();
        self.sync_tabline();
        self.reconcile_pane_state();
        let tabs = match closed {
            1 => "1 tab".to_string(),
            n => format!("{n} tabs"),
        };
        let dropped = super::held::dropped(drafts).map_or(String::new(), |d| format!("; {d}"));
        self.notify(format!(
            "closed {tabs} — their sessions stay open (`:ls`){dropped}"
        ));
    }

    /// **The work held in tab `at`** (from 0) — a request on a pane there,
    /// a call or a review, and the drafts in its prompts: what `:tabclose`
    /// and `:tabonly` will not close without `!`.
    fn held_in_tab(&self, at: usize) -> super::HeldWork {
        let panes: Vec<PaneId> = (self.windows())
            .map(|(pane, _)| pane)
            .filter(|&pane| self.tab_of(pane) == Some(at))
            .collect();
        self.held_in(&panes)
    }

    /// **Why `verb` will not close `tab`** (from 0): the remedy first, then
    /// the work `held` there, with the tab when it is not this one and the
    /// key that goes there.
    fn tab_holds(&self, verb: &str, tab: usize, held: &super::HeldWork) -> String {
        let remedy = held.remedy(&format!("`:{verb}!`"));
        let holds = held.holds();
        match tab == self.tabs.current() {
            true => format!("`:{verb}`: not closed — {remedy}; {holds}"),
            false => {
                let n = tab + 1;
                format!("`:{verb}`: not closed — {remedy}; in tab {n} (`{n}gt`), {holds}")
            }
        }
    }

    /// `:tabmove [N|+N|-N|$]` — **move the tab you are in** ([`move_to`]).
    /// With one tab, said: there is no tab line to see it move on (vim is
    /// silent; the `gt` rule, r13-F L3).
    pub(super) fn tabmove_command(&mut self, args: &str) {
        if self.tabs.count() == 1 {
            return self.notify("this is the only tab");
        }
        let at = self.tabs.current();
        match move_to(args.trim(), at, self.tabs.count()) {
            // **No move, said**: vim counts the tabs before the move, so from
            // tab 2 `:tabmove 1` ("after tab 1") and `:tabmove 2` leave it
            // where it is, silently — which read as broken (Steven, 10-26:
            // "`:tabm1` doesn't appear to work").
            Ok(to) if to == at => self.notify(format!(
                "tab {} is there already — `:tabmove` counts the tabs before the move: `0` \
                 first, `$` last, `+N` / `-N` N over",
                at + 1
            )),
            Ok(to) => {
                self.tabs.move_current(to);
            }
            Err(why) => self.refuse(format!("`:tabmove`: {why}")),
        }
    }

    /// `:tabrename <name>`, `:tabrename` (say it), `:tabrename!` (take it
    /// off) — **name the tab you are in**, `:rename`'s grammar for a tab
    /// (Q3): the tab line shows the name instead of what its focused pane
    /// shows. One line of plain text ([`Label::plain`], at most a session
    /// name's length), spaces and all: a tab's name is read, never typed
    /// back as a token, so it need not be one word as a session's is.
    ///
    /// [`Label::plain`]: super::Label::plain
    pub(super) fn tabrename_command(&mut self, args: &str, bang: bool) {
        use super::Label;
        let (text, tab) = (args.trim(), self.tabs.current() + 1);
        let old = self.tabs.name(tab - 1).map(str::to_string);
        let said = match (bang, text, old) {
            (true, "", None) => return self.notify(format!("tab {tab} has no name to take off")),
            (true, "", Some(old)) => format!("took the name `{old}` off tab {tab}"),
            (true, _, _) => {
                return self.refuse("`:tabrename!` takes the tab's name off — it takes no text");
            }
            (false, "", Some(old)) => {
                return self.notify(format!(
                    "tab {tab} is `{old}` — `:tabrename <name>` renames it, `:tabrename!` \
                     takes it off"
                ));
            }
            (false, "", None) => {
                return self.notify(format!(
                    "tab {tab} has no name; it shows what its focused pane shows — \
                     `:tabrename <name>` gives it one"
                ));
            }
            (false, name, old) => {
                if old.as_deref() == Some(name) {
                    return self.notify(format!("tab {tab} is already `{name}`"));
                }
                if name.chars().count() > Label::NAME_MOST {
                    return self.refuse(format!(
                        "`:tabrename`: a name is at most {} characters",
                        Label::NAME_MOST
                    ));
                }
                if !Label::plain(name, Label::NAME_MOST) {
                    return self.refuse("`:tabrename`: a name is one line of plain text");
                }
                match old {
                    Some(old) => format!("renamed tab {tab} `{old}` → `{name}`"),
                    None => format!("named tab {tab} `{name}`"),
                }
            }
        };
        let name = (!bang).then(|| text.to_string());
        self.tabs.rename_current(name);
        self.notify(said);
    }

    /// `:tabnext [N|+N|-N|$]` — bare, the next tab, as `gt`; else the tab
    /// [`tab_named`] names. `:tabprevious [N]` — the previous, or N back, as
    /// `gT` and `{N}gT` ([`go_to_tab`](Self::go_to_tab)): a count only, as
    /// vim's (measured: its `+1` and `$` are E475). `:tabnext +2` read as
    /// tab 2 until T4 — `usize`'s parse takes a sign.
    pub(super) fn tabnext_command(&mut self, args: &str, toward: Toward) {
        let arg = args.trim();
        match (toward, arg) {
            (_, "") => self.go_to_tab(toward, None),
            (Toward::Next, _) => {
                if let Some(to) = self.tab_argument("tabnext", arg) {
                    self.show_tab(to);
                }
            }
            (Toward::Previous, _) => match digits(arg) {
                Some(n) if n > 0 => self.go_to_tab(toward, Some(n)),
                _ => self.refuse(format!(
                    "`:tabprevious`: `{arg}` is not a count — N goes N tabs back"
                )),
            },
        }
    }

    /// **`gt` / `gT` and `:tabnext` / `:tabprevious`, with a count or a
    /// number** — one rule for the keys and the commands, vim's (measured,
    /// Neovim 0.12.6, `tabs.md` §6b): bare, the next or previous tab,
    /// wrapping; next with N, tab N; previous with N, N back, wrapping. No
    /// tab N is SAID for both — vim's `:tabnext 9` refuses (E475) and its
    /// `9gt` does nothing, which here would read as a dead key
    /// (`[[feedback_visible_refusals]]`; my call, against vim's silence).
    pub(super) fn go_to_tab(&mut self, toward: Toward, n: Option<usize>) {
        let (count, at) = (self.tabs.count(), self.tabs.current());
        // One tab: nowhere to go, said — the `9gt` rule above, for the same
        // reason (vim is silent here too; r12-F L1).
        if count == 1 {
            return self.notify("this is the only tab — `:tabnew` opens another");
        }
        let to = match (toward, n) {
            (Toward::Next, None) => (at + 1) % count,
            (Toward::Next, Some(n)) => match tab_named(&n.to_string(), at, count) {
                Ok(to) => to,
                Err(why) => return self.refuse(why),
            },
            (Toward::Previous, n) => (at + count - n.unwrap_or(1) % count) % count,
        };
        self.show_tab(to);
    }

    /// **Show tab `to`** (from 0) — the tab you are in parked, `to`'s
    /// arrangement on screen, its prompts' drafts and aims with it
    /// ([`Tabs::switch`]), and what a parked tab missed brought up to date
    /// ([`tab_shown`](Self::tab_shown)). `false`, and nothing moved, for the
    /// tab shown or one not there.
    pub(super) fn show_tab(&mut self, to: usize) -> bool {
        if to == self.tabs.current() || to >= self.tabs.count() {
            return false;
        }
        let shown = (self.tabs).switch(to, &mut self.workspace);
        debug_assert!(shown, "a parked tab that is there");
        self.tab_shown();
        true
    }

    /// **What each pane is called, in every tab** — its number as its
    /// corner shows it (`<C-w>w`'s ring) in the tab you are in, and `t2:3`
    /// in another, tab 2's pane 3 (`tabs.md` §3): `:ls`'s pane column, made
    /// once a listing.
    pub(crate) fn pane_labels(&self) -> std::collections::HashMap<PaneId, String> {
        let mut labels: std::collections::HashMap<PaneId, String> = (self.workspace.ring_order())
            .into_iter()
            .enumerate()
            .map(|(at, pane)| (pane, (at + 1).to_string()))
            .collect();
        for (tab, parked) in self.tabs.parked() {
            let ring = parked.workspace.ring_order().into_iter().enumerate();
            labels.extend(ring.map(|(at, pane)| (pane, format!("t{}:{}", tab + 1, at + 1))));
        }
        labels
    }

    /// **`pane`'s number as its corner shows it** — its place in its own
    /// tab's ring, from 1, whichever tab it is in (`pane_labels` says `t2:3`
    /// of another tab's; a sentence that names the tab already wants the
    /// `3`). `None` for a pane gone.
    pub(crate) fn corner_of(&self, pane: PaneId) -> Option<usize> {
        let ring = |workspace: &Workspace<Window>| {
            (workspace.ring_order().iter()).position(|&p| p == pane)
        };
        (ring(&self.workspace))
            .or_else(|| (self.tabs.parked()).find_map(|(_, tab)| ring(&tab.workspace)))
            .map(|at| at + 1)
    }

    /// **A click on the tab line goes to the tab under it** — vim's mouse on
    /// its tab line, `{N}gt` without the count, found by the layout the line
    /// is drawn with ([`view::tab_at`](crate::view::tab_at)). `false` for a
    /// click off the line, which the panes take; on it, the line's — past
    /// the last label, or on the tab you are in, doing nothing.
    pub(crate) fn click_tab_line(
        &mut self,
        frame: ratatui::layout::Rect,
        pos: ratatui::layout::Position,
    ) -> bool {
        let line = self.workspace.frame_layout(frame).tabline;
        if !line.contains(pos) {
            return false;
        }
        let at = crate::view::tab_at(&self.tab_labels(), line.width, pos.x - line.x);
        if let Some(n) = at {
            self.show_tab(n - 1);
        }
        true
    }

    /// **Where `buffer`'s session is held by a window** — shown, or beneath
    /// a view: in this tab, else in which other, else nowhere. What a
    /// sentence naming where something of it waits reads (a review's, a
    /// decision's), so "beside it" is never said of a pane in a tab you
    /// cannot see.
    pub(crate) fn where_shown(&self, buffer: BufferId) -> Shown {
        if self
            .panes_with_session(buffer, Over::ThisTab)
            .next()
            .is_some()
        {
            return Shown::Here;
        }
        match (self.panes_with_session(buffer, Over::EveryTab).next())
            .and_then(|pane| self.tab_of(pane))
        {
            Some(tab) => Shown::InTab(tab + 1),
            None => Shown::Nowhere,
        }
    }

    /// **The tab line's row, kept on every tab** — one while there is more
    /// than one tab, none with one (vim's `showtabline=1`): each workspace
    /// reserves it above its panes ([`Workspace::reserve_top`]), so a tab
    /// shown later lays out under it, and its clamp leaves the row.
    /// Asked where the count changes: a tab opened, a tab closed.
    fn sync_tabline(&mut self) {
        let rows = u16::from(self.tabs.count() > 1);
        for workspace in self.workspaces_mut() {
            workspace.reserve_top(rows);
        }
    }

    /// **Every tab's arrangement** — the one shown, then each parked one in
    /// tab order: for what each holds that every tab must agree on, set where
    /// it changes (the tab line's row, `sync_tabline`; the prompt's declared
    /// height, `:set tui.prompt.height`). Set on the tab shown alone, a
    /// parked tab came back with the old one (r12-A M-2).
    pub(super) fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace<Window>> {
        std::iter::once(&mut self.workspace)
            .chain((self.tabs.parked_mut()).map(|(_, tab)| &mut tab.workspace))
    }

    /// **What the tab line says of each tab**, in tab order: its number (as
    /// `gt` counts), the name you gave it (`:tabrename`), else what its
    /// focused pane shows — the session's handle, the aim's from the prompt
    /// or a decision, else the view's name (Q3, vim's: the active window's
    /// buffer) — whether it is the tab you are in, and its hotlist
    /// (`Milestone 5.md`'s weechat tiers, announce-don't-seize): a decision
    /// waiting there, and the alerts' mark on a session it holds.
    pub(crate) fn tab_labels(&self) -> Vec<TabLabel> {
        let current = self.tabs.current();
        let alerts = self.status_alerts();
        let label = |at: usize, workspace: &Workspace<Window>| {
            let mut sessions: Vec<BufferId> = Vec::new();
            let mut decides = false;
            for (pane, window) in workspace.windows() {
                sessions.extend(window.session());
                decides |= window.decision().is_some() && self.decides_for(pane).is_some();
            }
            let mark = (sessions.into_iter())
                .find_map(|b| alerts.mark(&mut super::OfSession::new(self, b)))
                .map(str::to_string);
            // A companion is its host's window (F1): a tab standing in the
            // inspector is named as the tree's window is, not "inspector"
            // (r12-F L6).
            let focused = workspace.focused();
            let focused = (workspace.window(focused))
                .and_then(Window::companion_host)
                .unwrap_or(focused);
            let window = workspace.window(focused);
            let session = match window.map(|w| &w.view) {
                Some(super::View::Session(view)) => Some(view.buffer),
                Some(super::View::Decision(view)) => Some(view.decides.0),
                // That prompt's own aim (10-26).
                Some(super::View::Prompt(prompt)) => Some(self.live_aim(prompt.aim)),
                _ => None,
            };
            let name = match (self.tabs.name(at), session.and_then(|b| self.session_of(b))) {
                (Some(given), _) => given.to_string(),
                (None, Some(session)) => session.conversation.episode().handle(),
                (None, None) => {
                    window.map_or_else(String::new, |w| w.view.kind().title().to_string())
                }
            };
            TabLabel {
                number: at + 1,
                name,
                current: at == current,
                mark,
                decides,
            }
        };
        let mut labels: Vec<TabLabel> = (self.tabs.parked())
            .map(|(at, tab)| label(at, &tab.workspace))
            .collect();
        labels.push(label(current, &self.workspace));
        labels.sort_by_key(|l| l.number);
        labels
    }

    /// **`:tabs`' page** — vim's listing: `Tab page N` (and the name you gave
    /// it), then its windows in ring order, each numbered as its corner is in
    /// its tab, `>` the one you are in, `%` the prompt a tab with two writes
    /// in (`App::writing_in`), `+` a prompt holding a draft (vim's
    /// "modified"). A heading names its tab by a pane in it (`Row::Tab`), a
    /// window line its pane (`Row::Pane`): `<CR>` goes to the tab, or stands
    /// you in the window. Read live by its pane (`listing_lines`), as `:ls`.
    pub(crate) fn tabs_lines(&self) -> crate::page::Lines {
        use crate::page::Row;
        let mut out = crate::page::Lines::default();
        let current = self.tabs.current();
        let parked: std::collections::HashMap<usize, &Workspace<Window>> = (self.tabs.parked())
            .map(|(at, tab)| (at, &tab.workspace))
            .collect();
        for at in 0..self.tabs.count() {
            let workspace = match at == current {
                true => &self.workspace,
                false => match parked.get(&at) {
                    Some(workspace) => *workspace,
                    None => continue,
                },
            };
            let ring = workspace.ring_order();
            let Some(&first) = ring.first() else {
                continue;
            };
            let name = (self.tabs.name(at)).map_or(String::new(), |n| format!("  \"{n}\""));
            out.row(&Row::Tab(first), &format!("Tab page {}{name}", at + 1));
            let two_prompts = (workspace.panes_of(crate::workspace::PaneKind::Prompt)).count() > 1;
            for (n, pane) in ring.into_iter().enumerate() {
                let Some(window) = workspace.window(pane) else {
                    continue;
                };
                let mark = match () {
                    _ if at == current && pane == workspace.focused() => '>',
                    _ if two_prompts && workspace.prompt_pane() == Some(pane) => '%',
                    _ => ' ',
                };
                let draft = window
                    .prompt_view()
                    .is_some_and(super::PromptView::has_draft);
                let line = format!(
                    "{mark}{} {:>2}  {}",
                    if draft { '+' } else { ' ' },
                    n + 1,
                    self.window_said(window)
                );
                out.row(&Row::Pane(pane), &line);
            }
        }
        out
    }

    /// **What a window shows, in a few words** — `:tabs`' line for it: a
    /// session by its `:ls` number and handle, a prompt by where it goes, a
    /// decision pane by whose, a view by its name.
    fn window_said(&self, window: &Window) -> String {
        use super::View;
        let session = |buffer: BufferId| match self.session_of(buffer) {
            Some(session) => {
                format!(
                    "b{} {}",
                    buffer.number(),
                    session.conversation.episode().handle()
                )
            }
            None => format!("b{}", buffer.number()),
        };
        match &window.view {
            View::Session(view) => session(view.buffer),
            // Aimed at no session (`shannon tutor`, before one is called):
            // said so, not the page's number as if a message went there.
            View::Prompt(prompt) => match self.live_aim(prompt.aim) {
                aim if self.session_of(aim).is_some() => format!("prompt → {}", session(aim)),
                _ => "prompt → no agent".to_string(),
            },
            View::Decision(decision) => format!("decision for {}", session(decision.decides.0)),
            view => view.kind().title().to_string(),
        }
    }

    // `pin_aim` set the parked aim to where the prompt went before a tab was
    // parked — the aim was `None`, "follow the foreground", and the
    // foreground is the app's, moved by every tab, so a tab parked following
    // it came back aimed at another tab's session (Q2: "switching back
    // restores it"). Every prompt window is born with an aim of its own now
    // (`PromptView::aim`), so there is nothing to pin.

    /// **A new tab after this one, on session `buffer`, shown** — vim's
    /// `:tabnew` with Shannon's `:new` (`tabs.md` §4): the default
    /// arrangement, a transcript over the prompt, born on `buffer` at its
    /// entry, and the tab's prompt aimed there.
    pub(super) fn open_tab(&mut self, buffer: BufferId) {
        let pane = self.open_tab_holding(Window::on(buffer));
        self.put_place(pane, self.entry_of(buffer));
        self.set_foreground(buffer);
    }

    /// **A new tab after this one, holding `window` over its prompt,
    /// shown** — the one way a tab is made: `:tabnew`'s fresh transcript,
    /// the first window `:tab {cmd}` opens (`App::new_window`), the window
    /// `<C-w>T` moves. Its prompt aimed where this tab's goes — `:tab help`
    /// shows no session; a caller showing one aims at it after, as every
    /// door into a session does (`set_foreground`, `enter_session`, a
    /// transcript focused): the window's own session here answered alike
    /// at every caller (mutated GREEN, 10-26), and went. Its prompt is its
    /// own: an empty draft (Steven, 10-26: "A draft per tab"). The window's
    /// pane back.
    pub(super) fn open_tab_holding(&mut self, window: Window) -> PaneId {
        use crate::workspace::PaneKind;
        let prompt = Window::prompt(&self.registers, self.prompt_target());
        let workspace = Workspace::new(self.config.tui.prompt.height, window, prompt);
        let pane = (workspace.windows())
            .find(|(_, w)| w.view.kind() != PaneKind::Prompt)
            .map(|(pane, _)| pane)
            .expect("born holding a window that is no prompt: `new_window` refuses one");
        let tab = Tab { workspace };
        (self.tabs).open_after(tab, &mut self.workspace);
        self.sync_tabline();
        self.tab_shown();
        pane
    }

    /// `:tab {cmd}` — **run `cmd`, the first window it opens in a new tab**
    /// after this one (vim's `:tab`, measured: `:tab help` from tab 1 of 3
    /// is tab 2 of 4, the help alone in it): a window `cmd` would split off
    /// or open across the top, and what it would show in the pane you stand
    /// in (`host_pane`, `session_host`), is born there instead — through
    /// the one door, `App::new_window`. A command that opens no window opens
    /// no tab, as vim's.
    pub(super) fn tab_command(&mut self, args: &str) -> super::CmdEffect {
        let line = args.trim();
        if line.is_empty() {
            self.refuse(
                "`:tab {cmd}` runs a command in a new tab — `:tab help`, `:tab b 2`, `:tab sp`",
            );
            return super::CmdEffect::Done(super::Flow::Stay);
        }
        // **Said as `:tab`'s**: the command's own refusal, "not a command:
        // `:ball`", named no `:tab` (Steven, 10-26).
        if matches!(
            crate::command::resolve(line),
            crate::command::Resolved::Unknown
        ) {
            let name = crate::command::typed_name(line);
            self.refuse(format!(
                "`:tab {name}`: `:{name}` is not a command — `:tab` runs one, as `:tab help`"
            ));
            return super::CmdEffect::Done(super::Flow::Stay);
        }
        self.into_a_new_tab = true;
        let effect = self.dispatch_command(line);
        self.into_a_new_tab = false;
        // What the command leaves to the terminal (`$EDITOR` for `:squash`
        // and `:child`, a pager) runs under `:tab` too: the window it opens
        // after is born in a tab (r13-A 7).
        match effect {
            super::CmdEffect::Done(_) => effect,
            later => super::CmdEffect::InTab(Box::new(later)),
        }
    }

    /// `:[N]ball [N]` (`:ba`, `:sball`) — **a window for each session**
    /// (vim's `:ball`, "one window for each buffer in the buffer list";
    /// measured, Neovim 0.12.6): each session `:ls` lists that no window in
    /// this tab shows gets one, split below in `:ls` order; at most N
    /// sessions, the one you are in counted first, as vim's count does
    /// (`:ball 2` from b: b, a). Focus stays where it was.
    ///
    /// **`:tab ball`: a tab for each** — each session no window in ANY tab
    /// shows, in a tab of its own after this one, in `:ls` order, ending in
    /// the last (vim's, which reuses a tab already holding a buffer). The
    /// flag is taken here: one `:tab` is every tab this opens.
    ///
    /// **Nothing is closed** (my call): vim's `:ball` also closes what it
    /// does not need — a second window on a buffer, those past N — and here
    /// a window carries its session's decision pane, which closed with it
    /// would be denied.
    pub(super) fn ball_command(&mut self, count: Option<&str>, args: &str) {
        use crate::{wm::SplitDir, workspace::NewPane};
        let tabbed = std::mem::take(&mut self.into_a_new_tab);
        let verb = if tabbed { "tab ball" } else { "ball" };
        let Some(arg) = self.counted(verb, count, args) else {
            return;
        };
        let most = match arg {
            "" => usize::MAX,
            n => match digits(n) {
                Some(n) if n > 0 => n,
                _ => return self.refuse(format!("`:{verb}`: `{n}` is not a count — `:ball 2`")),
            },
        };
        let here = self.prompt_target();
        let mut sessions = self.listed_sessions();
        if sessions.is_empty() {
            return self.refuse(format!("`:{verb}`: {}", super::NO_SESSION_LISTED));
        }
        if most < usize::MAX {
            sessions.sort_by_key(|&b| b != here);
        }
        let over = if tabbed {
            Over::EveryTab
        } else {
            Over::ThisTab
        };
        let wanted: Vec<BufferId> = (sessions.into_iter().take(most))
            .filter(|&b| self.panes_with_session(b, over).next().is_none())
            .collect();
        if wanted.is_empty() {
            return self.notify(match tabbed {
                true => "`:tab ball`: every session is in a tab already",
                false => "`:ball`: every session has a window here already",
            });
        }
        let named: Vec<String> = wanted.iter().map(|b| format!("b{}", b.number())).collect();
        if tabbed {
            for &buffer in &wanted {
                self.open_tab(buffer);
            }
            let tabs = match wanted.len() {
                1 => "a tab".to_string(),
                n => format!("{n} tabs"),
            };
            return self.notify(format!("`:tab ball`: {tabs} — {}", named.join(", ")));
        }
        let focused = self.workspace.focused();
        let mut opened = 0;
        let mut below = self.existing_host();
        for &buffer in &wanted {
            let born = match below {
                Some(target) => super::Born::Split {
                    target,
                    dir: SplitDir::Horizontal,
                    side: NewPane::After,
                },
                None => super::Born::AcrossTop,
            };
            let Some(pane) = self.new_window(Window::on(buffer), born) else {
                break;
            };
            self.put_place(pane, self.entry_of(buffer));
            below = Some(pane);
            opened += 1;
        }
        if self.workspace.window(focused).is_some() {
            self.workspace.focus_pane(focused);
        }
        let windows = match opened {
            0 => "no window".to_string(),
            1 => "a window".to_string(),
            n => format!("{n} windows"),
        };
        match opened == wanted.len() {
            true => self.notify(format!("`:ball`: {windows} — {}", named.join(", "))),
            false => self.refuse(format!(
                "`:ball`: {windows} of {} — no room for the rest ({})",
                wanted.len(),
                named[opened..].join(", ")
            )),
        }
    }

    /// `<C-w>T` — **move this window to a new tab** after this one (vim's,
    /// measured: it leaves its tab and is the new one's). The window itself,
    /// where you were in it and all; its companions made again beside it
    /// there, as a host brings them wherever it opens (`pair_companion`);
    /// a decision on its session following it at the event's end
    /// (`settle_decisions` keeps a pane only under its host, which has left,
    /// and opens it again under the window that shows it — armed, new under
    /// your cursor). Standing in a companion, its host moves. Claimed, not
    /// closed: a window moving is no decision about what it shows. Marked
    /// for `:Send`, it stays marked (r13-A 8: a fresh slot dropped it).
    /// `{N}<C-w>T` puts the new tab before tab N (vim's, measured; past the
    /// last, at the end): the count was dropped (r13-F L2).
    ///
    /// Refused, said: on the prompt (every tab has its own), on a decision
    /// pane (it opens beside its session — move the session's window), and
    /// on the tab's only window (vim says "Already only one window"; this
    /// points at `:tab sp`, the copy — r13-F L1).
    pub(super) fn move_window_to_tab(&mut self, count: Option<usize>) {
        use crate::workspace::PaneKind;
        let focused = self.workspace.focused();
        let pane = self.host_of(focused).unwrap_or(focused);
        let kind = match self.kind_of(pane) {
            Some(PaneKind::Prompt) => {
                return self.refuse("`<C-w>T`: the prompt stays — every tab has its own");
            }
            Some(PaneKind::Tool) => {
                return self.refuse(
                    "`<C-w>T`: a decision pane opens beside its session — move the session's window",
                );
            }
            Some(kind) => kind,
            None => return,
        };
        let others = (self.workspace.windows())
            .filter(|&(p, w)| p != pane && w.companion_host() != Some(pane))
            .filter(|(_, w)| !matches!(w.view.kind(), PaneKind::Prompt | PaneKind::Tool))
            .count();
        if others == 0 {
            return self.refuse(
                "`<C-w>T`: this is the tab's only window — `:tab sp` opens a copy in a new tab",
            );
        }
        let marked = self.workspace.is_marked(pane);
        let window = match self.workspace.close(pane) {
            true => self.workspace.claim_closed(pane),
            false => None,
        };
        let Some(window) = window else {
            return self.refuse("`<C-w>T`: this window cannot leave its tab");
        };
        // Its companions after it: a refusal above leaves them as they were
        // (r13-A 3). Claimed, they answer for nothing.
        self.release_companion(pane);
        // The aim, if it was on the window that left, moved to what this
        // tab still shows before the tab is parked — and said, of this tab:
        // the re-aim's own notice ("now aiming at …") would read as the new
        // tab's, whose prompt goes to the window moved.
        let left_at = self.tabs.current();
        let reaimed = self.aim_fallback();
        if let Some(fallback) = reaimed {
            self.aim_prompt_at(fallback);
        }
        let moved = self.open_tab_holding(window);
        if let Some(n) = count {
            self.tabs
                .move_current(n.saturating_sub(1).min(self.tabs.count() - 1));
        }
        if marked {
            self.workspace.set_mark(moved, true);
        }
        self.pair_companion(moved, kind);
        self.workspace.focus_pane(moved);
        // The tab left, numbered as it is now: one further on if the new
        // tab went before it.
        let left = match self.tabs.current() <= left_at {
            true => left_at + 2,
            false => left_at + 1,
        };
        if let Some(tag) = reaimed.and_then(|b| self.session_of(b)) {
            let tag = self.session_tag(tag.conversation.episode().id);
            self.notify(format!("tab {left}'s prompt now goes to {tag}"));
        }
    }

    /// **Close tab `at`** (from 0) — the tab you are in, showing its
    /// neighbour (the right one, else the left: vim's), or another, leaving
    /// you where you are (vim's `:tabclose N`): its windows through the
    /// reconcile that every window gone goes through (a decision pane closed
    /// leaves its call or review waiting, hidden, wherever its session is
    /// shown — no close answers, since 10-26; this said a call was denied
    /// while its session was shown in another tab, review stop 12, T5 F8).
    /// `false`, and nothing closed, for the only tab (vim's E784) or one not
    /// there.
    pub(super) fn close_tab(&mut self, at: usize) -> bool {
        let shown = at == self.tabs.current();
        if !(self.tabs).close(at, &mut self.workspace) {
            return false;
        }
        self.sync_tabline();
        self.reconcile_pane_state();
        if shown {
            self.tab_shown();
        }
        true
    }

    /// **What a parked tab missed, made up as it is shown** — as a view set
    /// aside comes back (`App::bring_back`): each tree window takes the
    /// tree's marks (parked, it was told none made since, and asked first at
    /// the event's end it would give the tree its stale ones — V4b verify,
    /// H1); each decision pane is new under your cursor, armed again; and
    /// the arrangement is fitted to the frame. Its pages and listings are synced by the
    /// next frame, as every drawn window is (`sync_pages`,
    /// `renew_listings`); its trees' rows were kept current while parked
    /// (`refresh_graph_views` walks every tab). The independent site list
    /// this answers: `docs/Reviews/2026-10-26-tabs/sites.md`, COMES-BACK.
    fn tab_shown(&mut self) {
        for (_, window) in self.workspace.windows_mut() {
            if let Some(view) = window.tree_mut()
                && let Some(tree) = self.buffers.tree(view.tree())
            {
                tree.show_marks(view);
            }
            // **A decision coming back is new under your cursor**, as one
            // arriving is: fitted and armed again by the one door that does
            // both (`settle_decisions`, which takes a pane showing nothing
            // recorded for new). Parked, its delay ran out unseen, so a key
            // in the burst after `gt` answered a call no frame had drawn
            // since (`docs/Reviews/2026-10-26-tabs/sites.md`, finding 5; the
            // review stop 8, E 1-2 class).
            if let Some(decision) = window.decision_mut() {
                decision.shows = None;
            }
        }
        // Fitted to the frame as it is now: a resize while it was parked
        // clamped only the tab shown then (`Event::Resize`).
        if let Some((cols, rows)) = self.frame {
            self.workspace.clamp(cols, rows);
        }
        // No re-aim: the aim came back with this tab's prompt windows, each
        // its own, and a parked tab's transcripts cannot close — only
        // its companions and decision panes can (`close_window`); the one
        // that leaves a tab as it is parked (`<C-w>T`) re-aims it first.
        // Called here, it changed nothing any test could reach (mutated,
        // 10-26).
        //
        // **And no re-aim by focus**: a switch is no focus move, but
        // `settle_focus` read the new tab's focus as one and `focus_moved`
        // aimed the prompt at the focused transcript — over the aim the tab
        // came back with, unannounced (Q2: "switching back restores it";
        // r12-A M-1). What a focus move ends is ended (a chord, a selection,
        // an insert session in a prompt, begun in the tab left:
        // `end_what_focus_left`), and the focus is recorded as seen — **a
        // decision pane too**: you come back to the pane you left, vim's
        // (Steven, 10-26: "back to the last pane we were in … a tab being 'a
        // working space'. So let's not change the state when moving away").
        // It fell to the prompt, `settle_focus` reading the return as falling
        // into the pane; it is safe either way, the pane armed again above,
        // so a key — or a close in it, which denies — in the burst after
        // `gt` answers nothing (`arming`).
        self.end_what_focus_left();
        self.focus_seen = Some((self.workspace.focused(), self.focus()));
        self.chat_dirty = true;
    }
}
