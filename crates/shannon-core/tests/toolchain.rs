//! The toolchain the repository pins and the one it declares.

/// **The pin is the declared floor**: `rust-toolchain.toml` names the release
/// the root `Cargo.toml`'s `rust-version` declares, so the build that checks
/// the floor is checking the floor we claim.
#[test]
fn the_pinned_toolchain_is_the_declared_floor() {
    let pinned = include_str!("../../../rust-toolchain.toml");
    let floor = env!("CARGO_PKG_RUST_VERSION");
    let channel = format!("channel = \"{floor}.0\"");
    assert!(pinned.contains(&channel), "want {channel}: {pinned}");
}
