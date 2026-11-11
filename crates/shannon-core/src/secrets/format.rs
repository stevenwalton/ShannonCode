//! What an issuer's credential actually looks like.
//!
//! See `docs/DesignDocs/secrets.md` §5. Detection is prefix-first because that
//! is what issuers designed for — a greppable prefix exists so scanners can find
//! the token — but a prefix alone only tells you where a candidate *might*
//! start. Some issuers went further and published a **structural check**, and
//! where they did, we should use it.
//!
//! # Why a checksum changes the trade completely
//!
//! Without one, a prefix found in the middle of a longer token is ambiguous:
//! `some-sk-directory-name` contains `sk-` and is not a credential. The only
//! defence is heuristic — require a separator before it, require a digit after
//! it — and heuristics are exactly what force us to *miss* the un-separated case
//! (`prefixAKIA…`) to avoid mangling ordinary text.
//!
//! A checksum removes the ambiguity instead of trading against it. GitHub's
//! classic token is `ghp_` + 30 characters of entropy + **6 characters of
//! base62-encoded CRC32** over that entropy, and GitHub's engineering write-up
//! says the reason plainly: so a scanner can confirm a candidate is structurally
//! real before acting on it. A random 40-character run passes that check with
//! probability about `1/62⁶` — roughly 1 in 5·10¹⁰.
//!
//! So for an issuer with a published checksum we can scan at **any offset**,
//! validate, and accept: high recall *and* near-zero false positives. It is
//! TruffleHog's verification idea with the verification done **locally** — no
//! network, nothing spent, and no credential used in order to test itself.
//!
//! Formats without a published check keep the heuristic path, so this is purely
//! additive: it can only turn misses into catches, never catches into misses.

/// One issuer's credential shape.
pub(super) struct Shape {
    /// The greppable prefix the issuer publishes.
    pub prefix: &'static str,
    /// What to call it when we redact it.
    pub kind: &'static str,
    /// Shortest body this issuer's format can have, counted after the prefix.
    ///
    /// Per-shape, not a global floor. A global 12 meant `hf_` + 12 characters
    /// was a HuggingFace token, so `hf_model_config-v2` — an entirely ordinary
    /// filename in an ML repository — got redacted. A property test found it.
    /// Real tokens are far longer than the floor that made short prefixes
    /// dangerous, and gitleaks' rule anatomy pairs every prefix with a length for
    /// exactly this reason.
    ///
    /// Deliberately a MINIMUM rather than the exact published length: formats
    /// change, and being wrong in the direction of "still catches it" is the
    /// direction to be wrong in.
    pub min_body: usize,
    /// An **offline** structural check over the text *following* the prefix,
    /// when the issuer published one. `Some` here is what licenses matching at
    /// an arbitrary offset — the heuristics exist only to stand in for evidence,
    /// and this *is* the evidence.
    pub check: Option<fn(&str) -> bool>,
}

/// Credential shapes we ship knowing about — a **seed, not the list**.
///
/// Prefix-matching is deliberately boring: these are formats the issuers
/// designed to be greppable (that is why they have prefixes), so the match is
/// exact rather than heuristic and the false-positive rate is ~zero.
///
/// Users extend this with [`SecretsConfig::prefixes`](super::SecretsConfig::prefixes).
/// Anyone with an internal token format has a shape we have never heard of, and
/// a hardcoded list would silently not protect them — the same concentric-rings
/// rule as everywhere else: defaults for most, config for the rest.
pub(super) const SHAPES: &[Shape] = &[
    // GitHub's classic tokens are the family with a published checksum, and all
    // five prefixes share one layout.
    Shape {
        prefix: "ghp_",
        min_body: 36,
        kind: "github token",
        check: Some(github_classic),
    },
    Shape {
        prefix: "gho_",
        min_body: 36,
        kind: "github token",
        check: Some(github_classic),
    },
    Shape {
        prefix: "ghu_",
        min_body: 36,
        kind: "github token",
        check: Some(github_classic),
    },
    Shape {
        prefix: "ghs_",
        min_body: 36,
        kind: "github token",
        check: Some(github_classic),
    },
    Shape {
        prefix: "ghr_",
        min_body: 36,
        kind: "github token",
        check: Some(github_classic),
    },
    // Fine-grained PATs use the same checksum idea but a different length; until
    // that is pinned down properly they take the heuristic path rather than a
    // check we only half-believe.
    Shape {
        prefix: "github_pat_",
        min_body: 22,
        kind: "github token",
        check: None,
    },
    Shape {
        prefix: "sk-ant-",
        min_body: 20,
        kind: "anthropic key",
        check: None,
    },
    Shape {
        prefix: "sk-proj-",
        min_body: 20,
        kind: "openai key",
        check: None,
    },
    Shape {
        prefix: "sk-",
        min_body: 20,
        kind: "api key",
        check: None,
    },
    Shape {
        prefix: "glpat-",
        min_body: 20,
        kind: "gitlab token",
        check: None,
    },
    Shape {
        prefix: "AKIA",
        min_body: 16,
        kind: "aws access key",
        check: None,
    },
    Shape {
        prefix: "ASIA",
        min_body: 16,
        kind: "aws access key",
        check: None,
    },
    Shape {
        prefix: "xoxb-",
        min_body: 10,
        kind: "slack token",
        check: None,
    },
    Shape {
        prefix: "xoxp-",
        min_body: 10,
        kind: "slack token",
        check: None,
    },
    Shape {
        prefix: "xoxa-",
        min_body: 10,
        kind: "slack token",
        check: None,
    },
    Shape {
        prefix: "xoxr-",
        min_body: 10,
        kind: "slack token",
        check: None,
    },
    Shape {
        prefix: "xoxs-",
        min_body: 10,
        kind: "slack token",
        check: None,
    },
    Shape {
        prefix: "AIza",
        min_body: 30,
        kind: "google api key",
        check: None,
    },
    Shape {
        prefix: "ya29.",
        min_body: 20,
        kind: "google oauth token",
        check: None,
    },
    Shape {
        prefix: "hf_",
        min_body: 30,
        kind: "huggingface token",
        check: None,
    },
    Shape {
        prefix: "dop_v1_",
        min_body: 20,
        kind: "digitalocean token",
        check: None,
    },
    Shape {
        prefix: "SG.",
        min_body: 20,
        kind: "sendgrid key",
        check: None,
    },
    Shape {
        prefix: "npm_",
        min_body: 36,
        kind: "npm token",
        check: None,
    },
    Shape {
        prefix: "pypi-AgEIcHlwaS5vcmc",
        min_body: 12,
        kind: "pypi token",
        check: None,
    },
];

/// Characters of entropy in a GitHub classic token, after the prefix.
const GH_ENTROPY: usize = 30;
/// Characters of base62 checksum following that entropy.
const GH_CHECKSUM: usize = 6;

/// Does `body` (everything after a `ghp_`-family prefix) carry a valid checksum?
///
/// `ghp_` + 30 entropy + 6 base62 CRC32 = 40 characters total.
///
/// `pub(super)` for `redact`'s test of a token that FAILS it — the premise
/// that the ordinary path, not the checksum, caught it.
pub(super) fn github_classic(body: &str) -> bool {
    let bytes = body.as_bytes();
    if bytes.len() < GH_ENTROPY + GH_CHECKSUM {
        return false;
    }
    let entropy = &bytes[..GH_ENTROPY];
    let checksum = &bytes[GH_ENTROPY..GH_ENTROPY + GH_CHECKSUM];
    // Both halves are base62 by construction; anything else is not this format,
    // and bailing here keeps `base62_decode` from having to express failure for
    // ordinary text.
    if !entropy.iter().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    match base62_decode(checksum) {
        Some(want) => u64::from(crc32(entropy)) == want,
        None => false,
    }
}

/// Decode base62 digits, or `None` if any character is outside the alphabet.
///
/// Returns `u64` because six base62 digits reach `62⁶` ≈ 5.7·10¹⁰, past `u32`.
/// A value that cannot be a CRC32 simply fails the comparison.
fn base62_decode(digits: &[u8]) -> Option<u64> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut value = 0u64;
    for d in digits {
        let position = ALPHABET.iter().position(|c| c == d)? as u64;
        value = value.checked_mul(62)?.checked_add(position)?;
    }
    Some(value)
}

/// CRC-32 (IEEE 802.3, reflected, the zlib/PNG one).
///
/// Bitwise rather than table-driven: this runs over 30 bytes per candidate, and
/// a 1 KiB table would be more code than the loop it saves.
fn crc32(data: &[u8]) -> u32 {
    const POLYNOMIAL: u32 = 0xEDB8_8320;
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Base62-encode `value` into exactly `width` digits, left-padded with zeros.
///
/// Only used to build valid tokens in tests — we never emit one — but it lives
/// beside the decoder so the two cannot drift apart.
#[cfg(test)]
pub(super) fn base62_encode(mut value: u64, width: usize) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut digits = vec![b'0'; width];
    let mut i = width;
    while value > 0 && i > 0 {
        i -= 1;
        digits[i] = ALPHABET[(value % 62) as usize];
        value /= 62;
    }
    String::from_utf8(digits).expect("alphabet is ascii")
}

/// A **synthetic** GitHub-shaped token with a correct checksum.
///
/// Built rather than pasted: a real token must never appear in this repository,
/// and a checksum is exactly the kind of thing that cannot be tested with a
/// made-up string. `entropy` must be 30 base62 characters.
#[cfg(test)]
pub(super) fn synthetic_github_token(prefix: &str, entropy: &str) -> String {
    assert_eq!(entropy.len(), GH_ENTROPY, "entropy must be 30 characters");
    let sum = base62_encode(u64::from(crc32(entropy.as_bytes())), GH_CHECKSUM);
    format!("{prefix}{entropy}{sum}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published test vector for CRC-32/ISO-HDLC. If this is wrong every
    /// checksum decision below is wrong in the same direction.
    #[test]
    fn crc32_matches_the_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn base62_round_trips() {
        for value in [0u64, 1, 61, 62, 3843, 0xCBF4_3926, u64::from(u32::MAX)] {
            let encoded = base62_encode(value, 6);
            assert_eq!(encoded.len(), 6);
            assert_eq!(base62_decode(encoded.as_bytes()), Some(value), "{value}");
        }
        // Outside the alphabet is a decode failure, not a wrong number.
        assert_eq!(base62_decode(b"abc-de"), None);
        assert_eq!(base62_decode(b"ab cde"), None);
    }

    /// A correctly-formed token validates; the same token with any single
    /// character changed does not. That second half is the whole point — it is
    /// what lets us match at an arbitrary offset without inventing false
    /// positives.
    #[test]
    fn a_valid_checksum_is_recognised_and_a_corrupted_one_is_not() {
        let token = synthetic_github_token("ghp_", "abcdefghijklmnopqrstuvwxyz0123");
        assert_eq!(token.len(), 40, "ghp_ + 30 + 6");
        assert!(github_classic(&token[4..]), "{token}");

        // Flip one character of the entropy: the checksum no longer describes it.
        let mut broken: Vec<u8> = token.bytes().collect();
        broken[10] = if broken[10] == b'a' { b'b' } else { b'a' };
        let broken = String::from_utf8(broken).unwrap();
        assert!(!github_classic(&broken[4..]), "{broken}");

        // Corrupt the checksum itself.
        let mut broken: Vec<u8> = token.bytes().collect();
        let last = broken.len() - 1;
        broken[last] = if broken[last] == b'0' { b'1' } else { b'0' };
        let broken = String::from_utf8(broken).unwrap();
        assert!(!github_classic(&broken[4..]), "{broken}");
    }

    /// Ordinary text that happens to be long enough must not validate. This is
    /// the property that makes offset-free scanning safe.
    #[test]
    fn ordinary_text_does_not_pass_the_checksum() {
        for body in [
            "thisisnotatokenbutitislongenoughyes",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "000000000000000000000000000000000000",
            "some-directory-name-that-is-long-abc",
            "short",
            "",
        ] {
            assert!(!github_classic(body), "false positive: {body}");
        }
    }
}
