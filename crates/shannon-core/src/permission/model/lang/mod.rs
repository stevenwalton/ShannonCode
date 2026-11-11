//! **Readers for the languages a flag's value or an operand may be code in.**
//!
//! A model says `sed`'s script and `awk`'s program are [`Role::Code`]; the
//! reader then asks the language what the code DOES — a `w` command writes
//! a file, `s///e` runs the pattern space, `system()` runs a command — and
//! each finding becomes an act of its own. Reasonable coverage, not
//! completeness (Steven, 10-09: `awk`, `sed`, `python -c` are languages);
//! what a reader cannot see stays an unread-code act that asks.

pub mod awk;
pub mod sed;

use super::Role;

/// One thing a piece of code does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    pub role: Role,
    /// The file named (for a file role), or what runs (for code).
    pub what: String,
}

impl Found {
    fn file(role: Role, what: impl Into<String>) -> Found {
        Found {
            role,
            what: what.into(),
        }
    }

    fn code(what: &str) -> Found {
        Found {
            role: Role::Code,
            what: what.to_string(),
        }
    }
}

/// The name a reader gives a file it cannot name — an expression, a
/// variable — so the act is placed nowhere and asks.
pub const DYNAMIC: &str = "$dynamic";
