//! The conversation/history graph (design.md §1) — the architectural centre.
//!
//! This is the in-memory model the persistence layer (a `Store` trait) and the
//! TUI's graph view will consume. It grows in slices: content-addressed
//! [`Turn`]s first, then `Episode`s and the DAG that links them. `Conversation`
//! stays the stable lifecycle façade the TUI drives; the graph grows *behind*
//! it, so no frontend code changes until a slice is ready to be consumed.
//!
//! - Slice 1: the atomic [`Turn`] + its content-address [`TurnId`].
//! - Slice 2 (here): the [`Episode`] (ordered turns + config + parent edge), the
//!   in-memory [`Graph`] read-side, and the one recursive [`Node`] abstraction
//!   that walks every level of the fractal structure uniformly.

pub mod episode;
// `graph.rs` inside the `graph` module: the parent module *is* the graph
// subsystem, this file is its core `Graph` type. The name match trips
// `module_inception`; the re-export below means callers never write `graph::graph`.
#[allow(clippy::module_inception)]
pub mod graph;
pub mod mark;
pub mod merge;
pub mod turn;

pub use episode::{
    EdgeKind, Episode, EpisodeId, Inherited, Label, Origin, SHORT_ID_LEN, Segment, short_hex,
};
pub use graph::{Graph, MessageOutline, Node, OUTLINE_CHARS, Placed, outline_of};
pub use mark::{Mark, Whose};
pub use merge::{
    MergeError, MergeStraightIn, MergeStrategy, Pick, Piece, Prepend, ReRoot, Squash, SquashMerge,
};
pub use turn::{Effort, Follows, Occurrence, Provenance, Turn, TurnConfig, TurnId};

use std::fmt::Write as _;

/// Encode 32 id bytes as 64 lowercase hex chars — the one representation used
/// for both id types' `to_hex`, their (inspectable) serde form, ref files, and
/// `cat`-able logs. Shared so the two id newtypes can't drift.
pub(crate) fn encode_hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Decode 64 hex chars (either case) into 32 bytes; `None` if the string is the
/// wrong length or contains a non-hex char. The inverse of [`encode_hex32`],
/// used to parse ids back from JSON and ref files.
pub(crate) fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, [hi, lo]) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = (*hi as char).to_digit(16)?;
        let lo = (*lo as char).to_digit(16)?;
        out[i] = ((hi as u8) << 4) | (lo as u8);
    }
    Some(out)
}
