//! **Fake credentials, for tests — every one invented.**
//!
//! Shaped like the real thing, so the redactor, the vault and the gate are
//! tested against what they exist to catch; none is or ever was a working
//! credential (checked 11-08: the GitHub-shaped ones fail GitHub's checksum,
//! the keys parse as nothing, the hosts are reserved or placeholders).
//!
//! **One file, so the scanners can be told.** Each value's line says so to
//! gitleaks and betterleaks (`gitleaks:allow`) and to TruffleHog
//! (`trufflehog:ignore`), and GitHub's secret scanning skips this path
//! (`.github/secret_scanning.yml`). Every value a scanner reads as a
//! credential lives here (Steven, 11-08: "best to have them excluded from
//! the scans since they are known fakes"); a plain word such as `hunter2`,
//! which no scanner reads as one, is written where a test uses it. Not
//! formatted (`#[rustfmt::skip]` on its `mod`): a value must stay on the line
//! that carries its marks.
//!
//! Compiled for tests only — this crate's and, through the `test-util`
//! feature, `shannon-tui`'s.

// ---- GitHub-shaped tokens (`ghp_` and 36 more; none passes the checksum) ----

/// The token most tests lend, hold or look for.
pub const GITHUB_TOKEN: &str = "ghp_pretendvalue0123456789abcdefghijklmn"; // gitleaks:allow trufflehog:ignore
/// The same secret's value after a rotation.
pub const GITHUB_TOKEN_ROTATED: &str = "ghp_rotatedvalue9876543210abcdefghijklmn"; // gitleaks:allow trufflehog:ignore
/// A value lent from the environment.
pub const GITHUB_TOKEN_LENT: &str = "ghp_pretendlentvalue0123456789abcdefghij"; // gitleaks:allow trufflehog:ignore
/// A value the vault holds, beside one it does not.
pub const GITHUB_TOKEN_HELD: &str = "ghp_pretendheldvalue01234567abcdefghijkl"; // gitleaks:allow trufflehog:ignore
/// A value the vault does NOT hold.
pub const GITHUB_TOKEN_SOMEONE_ELSES: &str = "ghp_someoneelses0123456789abcdefghijklmn"; // gitleaks:allow trufflehog:ignore
/// Shorter than a real one: a held value, not a shape.
pub const GITHUB_TOKEN_SHORT: &str = "ghp_pretend0123456789abcdef"; // gitleaks:allow trufflehog:ignore
/// An ascending run in the token's shape.
pub const GITHUB_TOKEN_RUN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345"; // gitleaks:allow trufflehog:ignore

// ---- Keys and passwords ----------------------------------------------------

/// A short password, where the context alone says it is one.
pub const SHORT_PASSWORD: &str = "hunter2"; // gitleaks:allow trufflehog:ignore
/// A password with punctuation in it.
pub const PASSWORD_WITH_BANGS: &str = "hunter2seekrit!!"; // gitleaks:allow trufflehog:ignore
/// Sixteen mixed characters under a token's name.
pub const API_TOKEN: &str = "Zq81hhgT0pLmNvWx"; // gitleaks:allow trufflehog:ignore
/// A passphrase-like password.
pub const PASSPHRASE: &str = "correcthorse9staple"; // gitleaks:allow trufflehog:ignore
/// Fifteen characters, mixed.
pub const PASSWORD_15: &str = "Tr0ub4dor3xK9mQ"; // gitleaks:allow trufflehog:ignore
/// Seventeen characters, mixed.
pub const TOKEN_17: &str = "Xq7Lm2Pz9Wk4Rt6Yb"; // gitleaks:allow trufflehog:ignore
/// Forty-four characters of high entropy, no issuer prefix.
pub const HIGH_ENTROPY_44: &str = "Xk7pQ2mZ9vL4nR8tW1yB3cF6hJ0sD5gA2eN7uI4oP1qT"; // gitleaks:allow trufflehog:ignore
/// Twenty-four distinct characters: in the band the old entropy floor missed.
pub const KEY_24: &str = "sJ9kQ2mVx7Lp4Nt8Wz3Bc6Hf"; // gitleaks:allow trufflehog:ignore
/// Thirty-two mixed characters.
pub const KEY_32: &str = "C3J27XDCG2LmlZGEONYlgCtjfIZ4SOcM"; // gitleaks:allow trufflehog:ignore
/// Forty mixed characters, a secret access key's length.
pub const KEY_40: &str = "z9CPVNPkNa1Hedcm4pMbXDuCL1mHoOsFaQfDPrAJ"; // gitleaks:allow trufflehog:ignore
/// Thirty-two hex digits, after an auth scheme.
pub const HEX_32: &str = "0123456789abcdef0123456789abcdef"; // gitleaks:allow trufflehog:ignore
/// An issuer-shaped key the tests mark as a secret.
pub const PLACEHOLDER_KEY_1: &str = "sk-test-PLACEHOLDER-0001-abcdef"; // gitleaks:allow trufflehog:ignore
/// Another, for a second mark.
pub const PLACEHOLDER_KEY_2: &str = "sk-test-PLACEHOLDER-0002-abcdef"; // gitleaks:allow trufflehog:ignore

// ---- Credentials in URLs ---------------------------------------------------

/// The smallest URL carrying a credential.
pub const URL_MINIMAL: &str = "postgres://u:p@db/app"; // gitleaks:allow trufflehog:ignore
/// A database URL with a high-entropy password.
pub const DB_URL: &str = "postgres://user:Xk7pQ2mZ9vL4nR8tW1yB@host/db"; // gitleaks:allow trufflehog:ignore
/// A command whose URL carries the password.
pub const PSQL_WITH_URL_PASSWORD: &str = "psql postgres://admin:hunter2@db.example.org/app"; // gitleaks:allow trufflehog:ignore
/// A typed rule whose URL carries the password.
pub const DENY_PSQL_WITH_URL_PASSWORD: &str = "deny psql postgres://admin:hunter2@db.example.org/app"; // gitleaks:allow trufflehog:ignore
/// A typed rule whose COMMENT carries a URL with the password.
pub const ALLOW_WITH_URL_PASSWORD_IN_COMMENT: &str = "allow cargo test * # from postgres://admin:hunter2@db.example.org"; // gitleaks:allow trufflehog:ignore

/// Each shape of a URL that carries a credential: what it is, the line,
/// and the credential that must not survive.
pub const CREDENTIALS_IN_URLS: &[(&str, &str, &str)] = &[
    ("postgres", "DATABASE_URL=postgres://user:hunter2@db.example.com/app", "hunter2"), // gitleaks:allow trufflehog:ignore
    ("mongodb", "M=mongodb://admin:s3cr3t@cluster0.example.net:27017", "s3cr3t"), // gitleaks:allow trufflehog:ignore
    ("redis, no user", "R=redis://:mypassword@10.0.0.5:6379", "mypassword"), // gitleaks:allow trufflehog:ignore
    ("git remote", "G=https://oauth2:ghp_aaaaaaaaaaaaaaaaaaaa@github.com/o/r.git", "ghp_aaaaaaaaaaaaaaaaaaaa"), // gitleaks:allow trufflehog:ignore
    ("http proxy", "P=http://corpuser:CorpPass123@proxy.corp:3128", "CorpPass123"), // gitleaks:allow trufflehog:ignore
];

// ---- Private keys ------------------------------------------------------------

/// A key whose armour and body are folded onto one line.
pub const PEM_FOLDED_OPENSSH: &str = "-----BEGIN OPENSSH PRIVATE KEY-----b3BlbnNzaC1rZXktdjEAAAAA-----END OPENSSH PRIVATE KEY-----"; // gitleaks:allow trufflehog:ignore
/// A whole, tiny RSA block.
pub const PEM_RSA_SHORT: &str = "-----BEGIN RSA PRIVATE KEY-----\nAAAA\nBBBB\n-----END RSA PRIVATE KEY-----"; // gitleaks:allow trufflehog:ignore
/// A private key's opening armour, alone.
pub const PEM_BEGIN_PRIVATE: &str = "-----BEGIN PRIVATE KEY-----"; // gitleaks:allow trufflehog:ignore
/// An RSA key's opening armour, alone.
pub const PEM_BEGIN_RSA: &str = "-----BEGIN RSA PRIVATE KEY-----"; // gitleaks:allow trufflehog:ignore
/// An OpenSSH key's opening armour, alone.
pub const PEM_BEGIN_OPENSSH: &str = "-----BEGIN OPENSSH PRIVATE KEY-----"; // gitleaks:allow trufflehog:ignore
/// An RSA key's opening armour, on a line of its own after another.
pub const PEM_RSA_OPENING_LINE: &str = "a\n-----BEGIN RSA PRIVATE KEY-----\n"; // gitleaks:allow trufflehog:ignore
/// An RSA block that never closes.
pub const PEM_RSA_UNTERMINATED: &str = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAvXqLmn\n"; // gitleaks:allow trufflehog:ignore
/// An RSA block cut off after a line of its body.
pub const PEM_RSA_TRUNCATED: &str = "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n"; // gitleaks:allow trufflehog:ignore
/// A whole RSA block between two lines of text.
pub const PEM_RSA_FRAMED: &str = "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIB\n-----END RSA PRIVATE KEY-----\nb\n"; // gitleaks:allow trufflehog:ignore
/// A whole OpenSSH block after a line of text.
pub const PEM_OPENSSH_FRAMED: &str = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----"; // gitleaks:allow trufflehog:ignore

// ---- Tables ------------------------------------------------------------------

/// A named value measured and redacted WHOLE: each line, and a tail that
/// must not survive (`redact`'s
/// `a_named_secret_long_enough_to_matter_is_still_redacted`).
pub const NAMED_SECRETS_WITH_TAILS: &[(&str, &str)] = &[
    ("DB_PASSWORD=Xk7pQ2mZ9vL4nR8tW1yB3cF6hJ0sD5gA", "Xk7pQ"), // gitleaks:allow trufflehog:ignore
    ("password: Tr0ub4dor3xK9mQ", "Tr0ub"), // gitleaks:allow trufflehog:ignore
    ("client_secret = 7Hq2LmZ9pWx4Rt6Y", "7Hq2L"), // gitleaks:allow trufflehog:ignore
    // Punctuation inside the value: the token-sized unit's blind spot.
    ("password: hunter2!Xk7pQ2mZ9vLab", "vLab"), // gitleaks:allow trufflehog:ignore
    ("password=x7#Kp9$mQ2&vL4!nW8zR", "nW8zR"), // gitleaks:allow trufflehog:ignore
    ("DB_PASSWORD=abc/def+ghi/jklmnopqrs", "jklmn"), // gitleaks:allow trufflehog:ignore
    ("DB_PASSWORD=Summer2024!CompanyName", "CompanyName"), // gitleaks:allow trufflehog:ignore
    ("SECRET_KEY='django-insecure-k#2m$9x!vq@7z&w^p'", "vq@7z"), // gitleaks:allow trufflehog:ignore
    // Quoted, a value with a space in it is taken whole — the space is
    // where an unquoted value would end.
    (r#"PASSWORD="hunter2!Xk7p Q2mZ9vLab""#, "Q2mZ9"), // gitleaks:allow trufflehog:ignore
    // Single-quoted too: the one row with a `'` had no space in its value,
    // so it was whole either way (review stop 12, T8 M13).
    ("PASSWORD='hunter2!Xk7p Q2mZ9vLab'", "Q2mZ9"), // gitleaks:allow trufflehog:ignore
    (r#""api_key": "q8Rt4Lm2Zx7Wp1Kd9Vb3""#, "Vb3"), // gitleaks:allow trufflehog:ignore
];

/// Where the context binds a credential, a short one is taken: an
/// assignment with no spaces to an environment-style name or a flag, and a
/// quoted value bound to a password's name (`redact`'s
/// `the_floor_yields_where_the_context_binds_a_credential`).
pub const CONTEXT_BOUND_CREDENTIALS: &[&str] = &[
    "PGPASSWORD=hunter2 psql", // gitleaks:allow trufflehog:ignore
    "export DB_PASSWORD=hunter2", // gitleaks:allow trufflehog:ignore
    "SSHPASS=hunter2 sshpass -e ssh host", // gitleaks:allow trufflehog:ignore
    "MYSQL_PWD=hunter2", // gitleaks:allow trufflehog:ignore
    "GH_TOKEN=hunter2", // gitleaks:allow trufflehog:ignore
    "mysql --password=hunter2 db", // gitleaks:allow trufflehog:ignore
    "deploy --api-key=hunter2", // gitleaks:allow trufflehog:ignore
    r#"password = "hunter2""#, // gitleaks:allow trufflehog:ignore
    "db_password: 'hunter2'", // gitleaks:allow trufflehog:ignore
    r#"{"passwd": "hunter2"}"#, // gitleaks:allow trufflehog:ignore
    r#"DB_PASSWORD="hunter2""#, // gitleaks:allow trufflehog:ignore
];
