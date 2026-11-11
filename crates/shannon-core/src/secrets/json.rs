//! **Every string in a JSON value, through one function** — a tool call's
//! input, which is JSON and must stay JSON.
//!
//! The one walk both scrubs take: the redactor's ([`redact_json`]) and the
//! secrets mask's (`Aliases::mask_message`). There were two, and the TUI's
//! kept keys raw and scrubbed each value without its key, so
//! `{"password": "…"}` passed where the same JSON as text was redacted
//! (review stop 4, E 2). A second walker is a second rule.
//!
//! [`redact_json`]: super::redact::redact_json

use serde_json::Value;

/// `value` with every key and every scalar passed through `f` — a string or a
/// number as the value of the key it sits under (`Some(key)`, which an
/// array's items inherit: `"credentials": ["…"]`), a key as nobody's
/// (`None`). A number `f` changes becomes the string `f` made of it: a
/// marked `123456789012` leaves as its handle, not as itself.
pub(crate) fn map_strings(
    value: &Value,
    f: &mut impl FnMut(&str, Option<&str>) -> String,
) -> Value {
    walk(value, None, f)
}

fn walk(
    value: &Value,
    under: Option<&str>,
    f: &mut impl FnMut(&str, Option<&str>) -> String,
) -> Value {
    match value {
        Value::String(s) => Value::String(f(s, under)),
        Value::Number(n) => {
            let written = n.to_string();
            let out = f(&written, under);
            if out == written {
                value.clone()
            } else {
                Value::String(out)
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| walk(v, under, f)).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (f(k, None), walk(v, Some(k), f)))
                .collect(),
        ),
        Value::Bool(_) | Value::Null => value.clone(),
    }
}
