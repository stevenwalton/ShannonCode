//! **What a parameter expansion says about the variables it touches.**
//!
//! What this does NOT read is the value of `X`: the text here is text the
//! model wrote, and a check that has it must use it. (The reader that spelled
//! a word's `${N:-w}` defaults out as paths went with the engine that matched
//! words by text; the acts reader places a known variable through the
//! executor's environment instead — `permission::acts::substitute_known`.)

/// **The variables a word assigns by default** — `${N:=w}`, `${N=w}`, an
/// element too (`${PATH[0]:=w}`), nested or not. `pipeline` had a parse of
/// its own that knew no subscript, so `${PATH[0]:=/tmp}` moved `PATH` past
/// the guard `${PATH:=/tmp}` meets (review, Reviewer C). An indirect
/// `${!N:=w}` assigns the variable `N` names, which is not known here: not
/// counted.
pub(crate) fn assigned(word: &str) -> impl Iterator<Item = &str> {
    word.match_indices("${").filter_map(move |(at, _)| {
        let body = &word[at + 2..];
        let name = &body[..name_len(body)];
        if name.is_empty() {
            return None;
        }
        let (_, rest) = subscripted(&body[name.len()..]);
        (rest.starts_with(":=") || rest.starts_with('=')).then_some(name)
    })
}

/// What follows a name, past its subscript if it has one (`[0]`, `[@]`) —
/// and whether it had one.
fn subscripted(rest: &str) -> (bool, &str) {
    match rest.strip_prefix('[') {
        Some(inner) => (true, inner.split_once(']').map_or("", |(_, after)| after)),
        None => (false, rest),
    }
}

/// The length of the parameter name `body` begins with: an identifier, a
/// positional number, or one special parameter.
fn name_len(body: &str) -> usize {
    match body.bytes().next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => body
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count(),
        Some(b) if b.is_ascii_digit() => body.bytes().take_while(u8::is_ascii_digit).count(),
        Some(b'@' | b'*' | b'#' | b'?' | b'-' | b'$' | b'!') => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default assignment names its variable, subscript or not; a read
    /// with a default, a length and an indirect assignment name none.
    #[test]
    fn an_assignment_by_default_names_its_variable() {
        fn names(w: &str) -> Vec<&str> {
            assigned(w).collect()
        }
        assert_eq!(names("${PATH:=/tmp}"), ["PATH"]);
        assert_eq!(names("${PATH=/tmp}"), ["PATH"]);
        assert_eq!(names("${PATH[0]:=/tmp}"), ["PATH"]);
        assert_eq!(names("a${X:=1}b${Y=2}"), ["X", "Y"]);
        assert!(names("${PATH:-/tmp}").is_empty());
        assert!(names("${#PATH}").is_empty());
        assert!(
            names("${!N:=w}").is_empty(),
            "indirect: the variable is not known"
        );
        assert!(names("${").is_empty());
    }
}
