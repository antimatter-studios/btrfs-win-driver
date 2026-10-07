//! The Btrfs image the fixture tests read, and a failure naming how to
//! build it when there is none.
//!
//! The image is a real filesystem built by `mkfs.btrfs` and populated by
//! the kernel -- `scripts/build-fixtures.sh` lists what is in it and why.
//! It is gitignored, so a fresh clone has none.
//!
//! **Nothing here skips.** A test that cannot find its fixture fails,
//! naming the command that provides it: a skipped test reads exactly like
//! a passing one. CI's portable jobs therefore run only the unit tests
//! (`--lib --bins`) and the text checks; the job that builds the fixture
//! runs everything.

use std::path::PathBuf;

use btrfs_win_driver::mount::Mount;

/// The fixture: `BTRFS_TEST_IMAGE` if set (CI sets it), else
/// `.fixtures/btrfs-content.img`. Panics when neither exists.
pub fn fixture() -> PathBuf {
    let path = match std::env::var("BTRFS_TEST_IMAGE") {
        Ok(explicit) => PathBuf::from(explicit),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".fixtures/btrfs-content.img"),
    };
    assert!(
        path.exists(),
        "no Btrfs fixture at {}. Build it on Linux with `sudo scripts/build-fixtures.sh` \
         (needs mkfs.btrfs and root), or set BTRFS_TEST_IMAGE.",
        path.display()
    );
    path
}

/// The fixture, opened through the driver's own mount path.
pub fn mount() -> Mount {
    Mount::open(&fixture(), None).expect("Mount::open on the Btrfs fixture")
}
