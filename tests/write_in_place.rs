//! What the driver writes into a real Btrfs filesystem, and what it
//! refuses to.
//!
//! Each test writes into its own copy of the fixture (`mkfs.btrfs`,
//! populated by the kernel: `scripts/build-fixtures.sh`), so no write leaks
//! into another test or into the image the read tests use. These read the
//! result back through the driver, which proves the driver agrees with
//! itself; that the kernel reads the same bytes and `btrfs check` finds
//! nothing wrong is `scripts/oracle-write.sh`'s job, run beside these in CI.

mod common;

use std::path::{Path, PathBuf};

use btrfs_win_driver::mount::Mount;
use fs_btrfs::Error;

/// The nodatacow file build-fixtures.sh makes, and its size.
const NOCOW: &str = "/nocow/data.bin";
const NOCOW_SIZE: usize = 1024 * 1024;

/// A copy of the fixture for one test, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "btrfs-win-driver-{}-{test}.img",
            std::process::id()
        ));
        std::fs::copy(common::fixture(), &path).expect("copy the fixture");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn open_rw(&self) -> Mount {
        Mount::open_rw(self.path(), None).expect("Mount::open_rw on a copy of the fixture")
    }

    /// The file at `path`, read through a fresh read-only mount, so nothing
    /// cached by the writing mount can stand in for what reached the image.
    fn reread(&self, path: &str) -> Vec<u8> {
        Mount::open(self.path(), None)
            .expect("Mount::open after the write")
            .read_path(path)
            .unwrap_or_else(|e| panic!("read {path} after the write: {e}"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// build-fixtures.sh's position-dependent content: each 8-byte group,
/// little-endian, is its own index.
fn pattern(len: usize) -> Vec<u8> {
    (0..(len / 8) as u64).flat_map(u64::to_le_bytes).collect()
}

/// The nodatacow file is offered for writing, and every extent of it can
/// be overwritten in place.
#[test]
fn a_nodatacow_file_is_offered_for_writing() {
    let scratch = Scratch::new("offered");
    let m = scratch.open_rw();
    assert!(m.is_writable());
    let target = m.resolve(NOCOW.as_bytes()).expect("resolve");
    assert!(target.tree.is_none(), "{NOCOW} is in the default subvolume");
    assert!(m.offers_write(true, &target.inode));
    assert!(m.can_write(&target).expect("can_write"));
}

/// An overwrite that crosses two page boundaries lands exactly where it
/// was asked to, and not one byte beside it moves.
#[test]
fn an_overwrite_lands_in_place_and_nothing_beside_it_moves() {
    let scratch = Scratch::new("overwrite");
    let offset = 4090usize;
    let data = vec![0xA5u8; 5000];
    {
        let m = scratch.open_rw();
        let target = m.resolve(NOCOW.as_bytes()).expect("resolve");
        let n = m
            .write_in_place(&target, offset as u64, &data)
            .expect("write_in_place");
        assert_eq!(n, data.len());
    }
    let mut want = pattern(NOCOW_SIZE);
    want[offset..offset + data.len()].copy_from_slice(&data);
    let got = scratch.reread(NOCOW);
    assert_eq!(got.len(), NOCOW_SIZE, "the size did not change");
    assert!(
        got == want,
        "the file is not the pattern with exactly those bytes replaced"
    );
}

/// A write running past the end would grow the file, which allocates: it
/// is refused whole, and no part of it is written.
#[test]
fn a_write_that_would_grow_the_file_is_refused_and_writes_nothing() {
    let scratch = Scratch::new("grow");
    {
        let m = scratch.open_rw();
        let target = m.resolve(NOCOW.as_bytes()).expect("resolve");
        let r = m.write_in_place(&target, (NOCOW_SIZE - 10) as u64, &[0xEE; 20]);
        assert!(
            matches!(r, Err(Error::UnsupportedFeature(_))),
            "a growing write gave {r:?}"
        );
    }
    assert!(
        scratch.reread(NOCOW) == pattern(NOCOW_SIZE),
        "the refused write changed the file"
    );
}

/// An ordinary copy-on-write file is neither offered nor written: an
/// in-place write there would leave the checksum and extent trees
/// describing data that is no longer on disk.
#[test]
fn a_copy_on_write_file_is_not_offered_and_is_refused() {
    let scratch = Scratch::new("cow");
    {
        let m = scratch.open_rw();
        let target = m.resolve(b"/pattern.bin").expect("resolve");
        assert!(!m.offers_write(true, &target.inode));
        assert!(!m.can_write(&target).expect("can_write"));
        let r = m.write_in_place(&target, 0, b"overwritten");
        assert!(
            matches!(r, Err(Error::UnsupportedFeature(_))),
            "a copy-on-write file gave {r:?}"
        );
    }
    assert!(
        scratch.reread("/pattern.bin") == pattern(3 * 1024 * 1024),
        "the refused write changed pattern.bin"
    );
}

/// A file reached through another subvolume is read-only: the handle into
/// that tree cannot write.
#[test]
fn a_file_in_another_subvolume_is_refused_as_read_only() {
    let scratch = Scratch::new("subvol");
    {
        let m = scratch.open_rw();
        let target = m.resolve(b"/subvol/inside.txt").expect("resolve");
        assert!(target.tree.is_some(), "inside.txt is in another tree");
        assert!(!m.can_write(&target).expect("can_write"));
        let r = m.write_in_place(&target, 0, b"x");
        assert!(
            matches!(r, Err(Error::ReadOnly)),
            "another subvolume gave {r:?}"
        );
    }
    assert_eq!(scratch.reread("/subvol/inside.txt"), b"inside a subvolume");
}

/// A mount opened without `--rw` offers nothing and writes nothing, the
/// nodatacow file included.
#[test]
fn a_read_only_mount_offers_nothing_and_refuses_every_write() {
    let m = common::mount();
    assert!(!m.is_writable());
    let target = m.resolve(NOCOW.as_bytes()).expect("resolve");
    assert!(!m.offers_write(true, &target.inode));
    assert!(!m.can_write(&target).expect("can_write"));
    let r = m.write_in_place(&target, 0, b"x");
    assert!(
        matches!(r, Err(Error::ReadOnly)),
        "a read-only mount gave {r:?}"
    );
}
