//! **A mark, kept with its session** — a place in the conversation that you
//! or the agent marked (`navigation.md` §7), saved in the episode's header so
//! it outlives the run (Steven, 10-28: "We should store marks along with the
//! conversation, so agent marks shouldn't be destroyed when we exit").
//!
//! **By place, never by text.** A front end draws a transcript with a held
//! secret's value put back where the model wrote its handle; a mark that
//! kept the line it stands on would write that value into the header. So a
//! mark is its turn, the block of that turn it is in, and the line and
//! column in that block — what the front end draws again the same way when
//! it replays the session.

use serde::{Deserialize, Serialize};

/// **Whose a mark is** — yours (`m{a-z}`, gone to with `'`) or the agent's
/// (its `mark` tool, gone to with `` ` ``): two sets of names, `a` in each.
/// Ordered yours first, as `:marks` groups them (Steven, 10-27: "proper
/// groupings").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Whose {
    #[serde(rename = "user")]
    Yours,
    #[serde(rename = "agent")]
    Agents,
}

/// **A mark, as its session keeps it** — whose, its name, and where: a turn,
/// a block of it, a line and a column. The front end turns its own places
/// into these and back (`output::Position` in the TUI); a mark whose place
/// a replay does not draw is kept there aside, listed and saved again as it
/// was — never dropped unsaid (review stop 14, A 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// Its name, a lowercase letter.
    pub name: char,
    /// Yours or the agent's.
    pub whose: Whose,
    /// The turn it is in — its index in the session's turns, from 0, as the
    /// session's full history lists them.
    pub turn: usize,
    /// The kind of block it is in, as the front end names its kinds, and
    /// which of that kind in the turn, from 0.
    pub block: String,
    /// Which block of that kind.
    pub nth: usize,
    /// The line within the block, from 0.
    pub line: usize,
    /// The column in that line, in chars.
    pub col: usize,
    /// What the agent said the place is; empty for yours.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// **How many saved entries this build keeps unread** — two alphabets' worth,
/// yours and the agent's. Past it an entry is dropped, said in the log: a
/// header a hand or a bug filled with 100k entries was written again whole
/// at every `m`, 9 MB a keystroke (review stop 14, B F8).
pub(crate) const MAX_UNREAD: usize = 52;

/// **A header's saved marks, read one entry at a time** ([`read_saved`]).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Saved {
    /// The marks this build reads: each a [`Mark`] named `a`–`z`, the last
    /// of each (whose, name).
    pub(crate) marks: Vec<Mark>,
    /// The entries it does not, as they were — kept and written back
    /// ([`write_saved`]), at most [`MAX_UNREAD`].
    pub(crate) unread: Vec<serde_json::Value>,
    /// Entries left out: an earlier mark of a name a later one has, and
    /// an unread one past [`MAX_UNREAD`].
    pub(crate) dropped: usize,
}

/// **`entries` as marks, one at a time** (review stop 14, B F4): read as a
/// list, one entry of another shape — a newer build's `whose`, a hand edit
/// — failed the list, every mark with it, and the next save made the loss
/// permanent. An entry that is no mark of this build's, or whose name is
/// no lowercase letter (`'1'`, `'\u{202e}'`: restored, no key could reach
/// them, and the model could be handed one as its next letter — B F8), is
/// kept as it was instead, so a save by this build never erases what
/// another wrote. A name saved twice keeps its last, as a restore did.
pub(crate) fn read_saved(entries: Vec<serde_json::Value>) -> Saved {
    let mut saved = Saved::default();
    for entry in entries {
        match serde_json::from_value::<Mark>(entry.clone()) {
            Ok(mark) if mark.name.is_ascii_lowercase() => {
                let same = |m: &Mark| (m.whose, m.name) == (mark.whose, mark.name);
                if let Some(at) = saved.marks.iter().position(same) {
                    saved.marks.remove(at);
                    saved.dropped += 1;
                }
                saved.marks.push(mark);
            }
            _ if saved.unread.len() < MAX_UNREAD => saved.unread.push(entry),
            _ => saved.dropped += 1,
        }
    }
    saved
}

/// **`marks`, then `unread` as they were** — the one list a header writes,
/// so [`read_saved`] gives both back.
pub(crate) fn write_saved(marks: &[Mark], unread: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let marks = marks
        .iter()
        .filter_map(|mark| serde_json::to_value(mark).ok());
    marks.chain(unread.iter().cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(name: char, whose: Whose, turn: usize) -> Mark {
        Mark {
            name,
            whose,
            turn,
            block: "you".into(),
            nth: 0,
            line: 0,
            col: 0,
            note: String::new(),
        }
    }

    /// **Saved marks are read an entry at a time** (review stop 14, B F4 and
    /// F8): an entry of another shape, or a name no key reaches, is kept as
    /// it was — not the whole list's ruin — and a name saved twice keeps its
    /// last; past [`MAX_UNREAD`] unread entries the rest are dropped,
    /// counted. Written back, the marks come first and the unread after,
    /// each as it was.
    #[test]
    fn saved_marks_are_read_an_entry_at_a_time() {
        let a1 = mark('a', Whose::Yours, 1);
        let a2 = mark('a', Whose::Yours, 2);
        let agents_a = mark('a', Whose::Agents, 3);
        let unknown = serde_json::json!({ "name": "c", "whose": "someone", "turn": 0 });
        let bad: Vec<serde_json::Value> = ['1', 'Z', '\u{202e}']
            .into_iter()
            .map(|name| serde_json::to_value(mark(name, Whose::Yours, 0)).unwrap())
            .chain([unknown, serde_json::json!(5), serde_json::Value::Null])
            .collect();
        let mut entries: Vec<serde_json::Value> = [&a1, &agents_a, &a2]
            .into_iter()
            .map(|m| serde_json::to_value(m).unwrap())
            .collect();
        entries.extend(bad.iter().cloned());
        let saved = read_saved(entries);
        assert_eq!(
            saved.marks,
            [agents_a.clone(), a2.clone()],
            "the last of each"
        );
        assert_eq!(saved.unread, bad, "the rest as they were, in order");
        assert_eq!(saved.dropped, 1, "the earlier `a`");
        let written = write_saved(&saved.marks, &saved.unread);
        assert_eq!(
            read_saved(written),
            Saved {
                dropped: 0,
                ..saved
            }
        );

        let many = vec![serde_json::json!(0); MAX_UNREAD + 8];
        let saved = read_saved(many);
        assert_eq!(
            (saved.unread.len(), saved.dropped),
            (MAX_UNREAD, 8),
            "bounded"
        );
    }

    /// **A mark reads back as it was written**, and says whose in words a
    /// person reading the header follows — `user`, `agent` — with no note
    /// written for yours.
    #[test]
    fn a_mark_round_trips_and_reads_plainly() {
        let mark = Mark {
            name: 'a',
            whose: Whose::Agents,
            turn: 3,
            block: "tool".into(),
            nth: 1,
            line: 0,
            col: 0,
            note: "the plan".into(),
        };
        let json = serde_json::to_string(&mark).unwrap();
        assert!(json.contains(r#""whose":"agent""#), "{json}");
        assert_eq!(serde_json::from_str::<Mark>(&json).unwrap(), mark);
        let yours = Mark {
            whose: Whose::Yours,
            note: String::new(),
            ..mark
        };
        let json = serde_json::to_string(&yours).unwrap();
        assert!(
            json.contains(r#""whose":"user""#) && !json.contains("note"),
            "{json}"
        );
    }
}
