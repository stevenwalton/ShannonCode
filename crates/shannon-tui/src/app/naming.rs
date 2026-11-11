//! **What a person calls a session** — `:rename`, `:description` and
//! `:w <name>` (Steven, 10-14: "`:rename foo`: display metadata, not
//! destroying the hash"). The name and the description are the episode's
//! [`Label`]; the id stays the identity, and everything addressed by it is
//! untouched. A name is one more reading of a session token, beside `b2`, an
//! id prefix and `%` (`names_a_session`, `session_named` in `app.rs`) — so
//! every verb that takes a session takes a name, through the one door.

use super::*;

/// The longest a session name may be — core's, so the loader and the
/// command agree ([`Label::read_back`]).
const NAME_MOST: usize = Label::NAME_MOST;

/// The longest a description may be — core's, likewise.
const DESCRIPTION_MOST: usize = Label::DESCRIPTION_MOST;

/// **What a session name may be** — one word a person types: letters,
/// digits, `-`, `_` and `.`, not starting with `-` (a flag), and never a
/// token another reading already claims: what [`other_reading`] reads (`%`,
/// a buffer `b3`, an id prefix — four or more hex characters, any case), a
/// pane number (`3`), or a keyword a session command reads before any
/// session ([`session_keywords`](crate::command::session_keywords): `:delete
/// all`, `:Send ball`). So a name never makes a token ambiguous, and a
/// refusal says why at the moment of naming rather than later, at every use.
/// **Can `c` be in a session's name?** — any script's letters, the marks
/// its words are written with (an accent combining, Devanagari's virama,
/// Thai's tone marks), digits, and `-`, `_`, `.`: not whitespace (a name is
/// one token), not a control, not what reorders or hides text
/// (`text::misleads`), not what draws as nothing (`text::draws_as_nothing`:
/// two names must not read alike), not ASCII punctuation, which commands
/// read. It was `char::is_alphanumeric`, which a combining mark is not, so
/// `:rename क्षेत्र` was refused (review stop 9's verification, G 7; Steven,
/// 10-24: "we do need to handle accents and other language text").
fn in_a_name(c: char) -> bool {
    use shannon_core::text::{draws_as_nothing, misleads};
    !(c.is_whitespace() || c.is_control() || misleads(c) || draws_as_nothing(c))
        && (!c.is_ascii_punctuation() || matches!(c, '-' | '_' | '.'))
}

pub(super) fn name_syntax(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a name needs a character".into());
    }
    if name.chars().count() > NAME_MOST {
        return Err(format!("a name is at most {NAME_MOST} characters"));
    }
    if let Some(c) = name.chars().find(|&c| !in_a_name(c)) {
        return Err(format!(
            "`{}` cannot be in a name — letters, marks and digits of any script, `-`, `_` and `.`",
            c.escape_default()
        ));
    }
    if name.starts_with('-') {
        return Err(format!(
            "`{name}` would read as a flag — start it otherwise"
        ));
    }
    match other_reading(name) {
        Some(SessionName::Buffer(_)) => {
            return Err(format!("`{name}` reads as a buffer, as `:ls` numbers them"));
        }
        // Its SHAPE, whether or not any session has that id: a name a later
        // session's id could begin with would change meaning under you.
        Some(SessionName::Id) => {
            return Err(format!(
                "`{name}` looks like a session id (four or more of 0-9 and a-f), so it \
                 would be read as one — add a character outside those"
            ));
        }
        // `%` and a name share no character; asked for completeness.
        Some(SessionName::Current | SessionName::Name) => {
            return Err(format!("`{name}` reads as another session"));
        }
        None => {}
    }
    // A bare number reads two ways, and a name a third would make three: a
    // pane's to `:Send` (`:Send 2`), a buffer's to `:b` and `:decide` (`:b 2`
    // is b2, `as_listed`). Review stop 10's A 8 thought names could be all
    // digits; this check, here since round 16, says otherwise.
    if name.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "`{name}` reads as a pane number to `:Send` and a buffer's (b{name}) to `:b` — add a \
             letter"
        ));
    }
    if crate::command::session_keywords().any(|word| word == name) {
        return Err(format!(
            "`{name}` is a word session commands read first (`:delete {name}`) — choose another"
        ));
    }
    Ok(())
}

impl App {
    /// **What `episode` is called** — its [`Label`], from the buffer that
    /// holds it (the live header, which may not be on disk yet) or else from
    /// the store; empty when neither has it. Not for the draw: the store
    /// half reads a file.
    pub(crate) fn label_of(&self, episode: EpisodeId) -> Label {
        match self
            .buffers
            .sessions()
            .find(|b| b.conversation.episode().id == episode)
        {
            Some(b) => b.conversation.episode().label.clone(),
            None => self
                .store
                .load_episode(&episode)
                .map(|l| l.episode.label)
                .unwrap_or_default(),
        }
    }

    /// [`Label::tag`], for a session found by id
    /// ([`label_of`](Self::label_of)) — what a notice or a confirmation
    /// calls it.
    pub(crate) fn session_tag(&self, episode: EpisodeId) -> String {
        self.label_of(episode).tag(episode)
    }

    /// **Give `episode` its name and description** — the one door
    /// `:rename`, `:description` and `:w <name>` write through. An open
    /// session takes it in memory and its header is re-put if it is on disk
    /// ([`write_preferences`](Self::write_preferences), as `:model` does);
    /// one that is not open has its stored header re-put directly. A session
    /// in neither — never saved, in no buffer — is refused, not registered:
    /// a header with no turns behind it would be a session from nowhere.
    pub(crate) fn set_label(&mut self, episode: EpisodeId, label: Label) -> Result<(), String> {
        let holder = self
            .buffers
            .sessions()
            .find(|b| b.conversation.episode().id == episode)
            .map(|b| b.id);
        match holder {
            Some(buffer) => {
                // All or nothing: a header that could not be written takes
                // the name back off the screen too, so a refusal never sits
                // beside a border showing what it says did not happen.
                let before = self.label_of(episode);
                if let Some(b) = self.session_of_mut(buffer) {
                    b.conversation.set_label(label);
                }
                if let Err(why) = self.write_preferences(buffer) {
                    if let Some(b) = self.session_of_mut(buffer) {
                        b.conversation.set_label(before);
                    }
                    return Err(why);
                }
            }
            None => {
                if !matches!(self.store.episode_len(&episode), Ok(Some(_))) {
                    return Err(format!("{} is not saved", episode.short_hex()));
                }
                let mut stored = self
                    .store
                    .load_episode(&episode)
                    .map_err(|e| e.to_string())?
                    .episode;
                stored.label = label;
                self.store.put_episode(&stored).map_err(|e| e.to_string())?;
            }
        }
        // The tree and the inspector show it; borders and `:ls` read the
        // buffer at the next frame.
        self.refresh_graph_views();
        self.chat_dirty = true;
        Ok(())
    }

    /// **The name goes on with the work** — `label`, `from`'s, moves to the
    /// session `buffer` was just re-pointed at, the one that replaces `from`
    /// (`adopt_composed`'s `carries`: `:continue`, a squash of a session's
    /// own turns), as a rebase moves a branch name; the original stays in
    /// `:graph`, unnamed.
    ///
    /// **The replacement is written at once.** It is lazy otherwise, and a
    /// quit before it is written would lose it — and with it the name, taken
    /// off `from` already; taking it off later would leave one name on two
    /// sessions meanwhile. Written first, then taken off, so a write that
    /// fails leaves the name where it was, and says so.
    pub(crate) fn carry_label(&mut self, buffer: BufferId, from: EpisodeId, label: Label) {
        if let Some(b) = self.session_of_mut(buffer) {
            b.conversation.set_label(label.clone());
        }
        // A making's write: `HEAD` follows it by its one rule (`moves_head`)
        // — where you are, or over `from` where `HEAD` was — named or not.
        if matches!(
            self.persist_saying(buffer, Some(from)),
            Saved::Refused | Saved::Gone
        ) {
            if let Some(b) = self.session_of_mut(buffer) {
                b.conversation.set_label(Label::default());
            }
            return self.refuse(format!(
                "the name stays on {}: the session replacing it could not be written",
                from.short_hex()
            ));
        }
        // A `from` never saved, and in no buffer now, is gone with its name.
        let held = self
            .buffers
            .sessions()
            .any(|b| b.conversation.episode().id == from);
        if (held || matches!(self.store.episode_len(&from), Ok(Some(_))))
            && let Err(why) = self.set_label(from, Label::default())
        {
            self.refuse(format!(
                "the name also stays on {}: {why}",
                from.short_hex()
            ));
        }
    }

    /// **May `episode` be called `name`?** Its spelling ([`name_syntax`]),
    /// and no OTHER session has it — a name that named two sessions would be
    /// a token naming nothing. The one check before a name lands, for
    /// `:rename` and `:w <name>` alike.
    pub(super) fn free_name(&self, episode: EpisodeId, name: &str) -> Result<(), String> {
        // An id-shaped name says which it is: one that IS a session's id, or
        // only looks like one (Steven, 10-14: "Is it because it _looks_ like a
        // session ID or because _it is_ an existing session"). Refused either
        // way — by its shape (`name_syntax`).
        if let Err(why) = name_syntax(name) {
            return Err(
                match (other_reading(name), self.session_readings(name).as_slice()) {
                    (Some(SessionName::Id), [one]) => format!(
                        "`{name}` is the session id of {} — a name may not look like an \
                         id; add a character outside 0-9 and a-f",
                        self.session_tag(*one)
                    ),
                    (Some(SessionName::Id), many @ [_, _, ..]) => format!(
                        "`{name}` begins {} sessions' ids — a name may not look like a \
                         session id; add a character outside 0-9 and a-f",
                        many.len()
                    ),
                    _ => why,
                },
            );
        }
        match self
            .session_readings(name)
            .into_iter()
            .find(|e| *e != episode)
        {
            Some(other) => Err(format!(
                "`{name}` already names {} — a name is one session's",
                other.short_hex()
            )),
            None => Ok(()),
        }
    }

    /// `:rename [session] <name>`, `:rename` (say it), `:rename! [session]`
    /// (take it off). The session is named first, in any reading a session
    /// verb takes, or is the one a bare verb acts on ([`bare_session_target`]).
    /// A name is checked twice before it lands: its spelling
    /// ([`name_syntax`]) and that no other session has it — a name that
    /// named two sessions would be a token that names nothing.
    ///
    /// [`bare_session_target`]: Self::bare_session_target
    pub(crate) fn rename_command(&mut self, args: &str, bang: bool) {
        let words: Vec<&str> = args.split_whitespace().collect();
        let (session, name) = match (bang, words.as_slice()) {
            (true, []) => (None, None),
            (true, [session]) => (Some(*session), None),
            (true, _) => {
                return self
                    .refuse("`:rename!` takes one session's name off — `:rename! [session]`");
            }
            (false, []) => (None, None),
            (false, [name]) => (None, Some(*name)),
            (false, [session, name]) => (Some(*session), Some(*name)),
            (false, _) => return self.refuse("a name is one word — `:rename [session] <name>`"),
        };
        let episode = match session {
            Some(token) => match self.session_named(token) {
                Ok(episode) => episode,
                Err(why) => return self.refuse(format!("`:rename`: {why}")),
            },
            None => match self.bare_session_target(":rename") {
                Some(episode) => episode,
                None => return,
            },
        };
        let mut label = self.label_of(episode);
        let id = episode.short_hex();
        let old = label.name.clone();
        let said = match (bang, name, old.as_deref()) {
            // Bare: say it.
            (false, None, Some(old)) => {
                return self.notify(format!(
                    "`{old}` names {id} — `:rename <name>` renames it, `:rename!` takes it off"
                ));
            }
            (false, None, None) => {
                return self.notify(format!("{id} has no name — `:rename <name>` gives it one"));
            }
            (true, _, None) => return self.notify(format!("{id} has no name to take off")),
            (true, _, Some(old)) => {
                label.name = None;
                format!("took the name `{old}` off {id}")
            }
            (false, Some(name), old) => {
                if old == Some(name) {
                    return self.notify(format!("{id} is already `{name}`"));
                }
                if let Err(why) = self.free_name(episode, name) {
                    return self.refuse(format!("`:rename`: {why}"));
                }
                label.name = Some(name.to_string());
                match old {
                    Some(old) => format!("renamed `{old}` → `{name}` ({id})"),
                    None => format!("named {id} `{name}`"),
                }
            }
        };
        match self.set_label(episode, label) {
            Ok(()) => self.notify(said),
            Err(why) => self.refuse(format!("`:rename`: {why}")),
        }
    }

    /// `:description <text>`, `:description` (say it), `:description!`
    /// (take it off) — a line saying what the session is, which the
    /// inspector shows. Free text, so it takes no session token: it acts on
    /// the session a bare verb acts on ([`bare_session_target`](Self::bare_session_target)).
    /// One line of plain text ([`Label::plain`]: no control character and no
    /// invisible formatting — a bidi override would make the inspector read
    /// as something else), since it is drawn as it is written.
    pub(crate) fn description_command(&mut self, args: &str, bang: bool) {
        let text = args.trim();
        if bang && !text.is_empty() {
            return self.refuse("`:description!` takes the description off — it takes no text");
        }
        let Some(episode) = self.bare_session_target(":description") else {
            return;
        };
        let tag = self.session_tag(episode);
        let mut label = self.label_of(episode);
        let said = match (bang, text, label.description.take()) {
            (false, "", Some(about)) => return self.notify(format!("{tag}: {about}")),
            (false, "", None) => {
                return self.notify(format!(
                    "{tag} has no description — `:description <text>` gives one"
                ));
            }
            (true, _, None) => return self.notify(format!("{tag} has no description")),
            (true, _, Some(_)) => format!("took the description off {tag}"),
            (false, text, _) => {
                if text.chars().count() > DESCRIPTION_MOST {
                    return self.refuse(format!(
                        "`:description` is at most {DESCRIPTION_MOST} characters"
                    ));
                }
                if !Label::plain(text, DESCRIPTION_MOST) {
                    return self.refuse("`:description` is one line of plain text");
                }
                label.description = Some(text.to_string());
                format!("described {tag}")
            }
        };
        match self.set_label(episode, label) {
            Ok(()) => self.notify(said),
            Err(why) => self.refuse(format!("`:description`: {why}")),
        }
    }

    /// **`:w <name>`'s naming half** (vim's `cpo-F`: a `:w {file}` on a
    /// buffer with no name gives it that name) — `Some(true)` when the
    /// session in `buffer` was named `name` just now, `Some(false)` when it
    /// already was, so the write may go on; `None`, with the refusal on
    /// screen, otherwise. A session with ANOTHER name is refused, pointing at
    /// `:rename`: vim's `:w other` would write a copy and leave the name
    /// alone, and a copy is `:clone`'s.
    pub(crate) fn name_for_write(&mut self, buffer: BufferId, name: &str) -> Option<bool> {
        let Some(episode) = self.session_of(buffer).map(|b| b.conversation.episode().id) else {
            self.refuse("that session is no longer open");
            return None;
        };
        let mut label = self.label_of(episode);
        match label.name.as_deref() {
            Some(old) if old == name => return Some(false),
            Some(old) => {
                self.refuse(format!(
                    "`:write`: this session is `{old}` — `:rename {name}` renames it"
                ));
                return None;
            }
            None => {}
        }
        if let Err(why) = self.free_name(episode, name) {
            self.refuse(format!("`:write`: {why}"));
            return None;
        }
        label.name = Some(name.to_string());
        match self.set_label(episode, label) {
            Ok(()) => Some(true),
            Err(why) => {
                self.refuse(format!("`:write`: {why}"));
                None
            }
        }
    }
}
