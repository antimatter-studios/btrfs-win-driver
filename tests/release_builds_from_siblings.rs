//! The release builds from the same sibling checkouts CI does.
//!
//! The dependencies are siblings pinned in `chores.yml`; `ci.yml` and
//! `matrix.yml` clone them from those pins, and the release must too, or
//! cargo finds no `../rust-fs-core` and no tag can build an installer.
//! These checks read the workflows as text.

use std::fs;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

#[test]
fn the_release_asks_for_no_submodules() {
    let release = read(".github/workflows/release.yml");
    assert!(
        !release.contains("submodules:"),
        "release.yml checks out submodules, which this repository does not have"
    );
}

#[test]
fn the_release_clones_every_sibling_from_the_pins_in_chores_yml() {
    let release = read(".github/workflows/release.yml");
    for pin in [
        "FS_CORE_URL",
        "FS_BTRFS_URL",
        "SKELETON_URL",
        "WINFSP_RS_URL",
    ] {
        assert!(
            release.contains(&format!("$(pin {pin})")),
            "release.yml does not clone the sibling {pin} names, read from chores.yml"
        );
    }
}
