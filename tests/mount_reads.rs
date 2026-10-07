//! What the driver reads out of a real Btrfs filesystem.
//!
//! The image was made by `mkfs.btrfs` and populated by the Linux kernel
//! (`scripts/build-fixtures.sh`), so these compare this driver against
//! what Btrfs actually writes. Every expected value is a fact the fixture
//! script wrote deliberately.
//!
//! These need the fixture, so they fail without one: CI's fixture job
//! builds it and runs them.

mod common;

use btrfs_win_driver::mount::Mount;
use common::{fixture, mount};

fn names(m: &Mount, path: &str) -> Vec<String> {
    let mut v: Vec<String> = m
        .children(path.as_bytes())
        .unwrap_or_else(|e| panic!("list {path}: {e}"))
        .into_iter()
        .map(|c| String::from_utf8(c.name).expect("fixture names are UTF-8"))
        .collect();
    v.sort();
    v
}

/// The volume opens and the superblock is the one mkfs.btrfs wrote.
#[test]
fn the_volume_opens_with_the_label_mkfs_gave_it() {
    let m = mount();
    assert_eq!(m.label(), "WINTEST");
    let sb = m.fs.superblock();
    assert_eq!(sb.total_bytes, 300 * 1024 * 1024, "the image is 300 MiB");
    assert!(sb.sectorsize.is_power_of_two());
}

/// Exactly what the fixture script made at the root, no more and no
/// less, with `.` and `..` not among them.
#[test]
fn the_root_lists_every_entry_and_nothing_else() {
    let m = mount();
    let mut want = vec![
        "compressed",
        "dangling-link",
        "dir",
        "empty.txt",
        "link-to-small",
        "manyentries",
        "naïve-café.txt",
        "nocow",
        "pattern.bin",
        "small.txt",
        "snap",
        "subvol",
    ];
    want.sort();
    assert_eq!(names(&m, "/"), want);
}

#[test]
fn a_small_inline_file_reads_its_exact_bytes() {
    let m = mount();
    assert_eq!(m.read_path("/small.txt").expect("read"), b"hello btrfs");
}

#[test]
fn an_empty_file_reads_as_no_bytes() {
    let m = mount();
    assert_eq!(m.read_path("/empty.txt").expect("read"), Vec::<u8>::new());
}

#[test]
fn nested_paths_resolve() {
    let m = mount();
    assert_eq!(
        m.read_path("/dir/nested/deep/leaf.txt").expect("read"),
        b"level three"
    );
    assert_eq!(m.read_path("/dir/one.txt").expect("read"), b"level one");
    assert_eq!(names(&m, "/dir/nested/deep"), vec!["leaf.txt"]);
}

#[test]
fn a_non_ascii_name_resolves() {
    let m = mount();
    assert_eq!(
        m.read_path("/naïve-café.txt").expect("read"),
        b"unicode name"
    );
}

/// Every 8-byte group of pattern.bin is its own index, so a block read
/// from the wrong offset fails even when the length is right.
#[test]
fn a_large_file_reads_back_at_the_right_offsets() {
    let m = mount();
    let all = m.read_path("/pattern.bin").expect("read");
    assert_eq!(all.len(), 3 * 1024 * 1024);
    let (groups, tail) = all.as_chunks::<8>();
    assert!(tail.is_empty());
    for (index, chunk) in groups.iter().enumerate() {
        let value = u64::from_le_bytes(*chunk);
        assert_eq!(value, index as u64, "byte offset {} is wrong", index * 8);
    }
}

/// The ranged read the WinFsp callback uses returns the window asked for,
/// across an extent boundary and short at end of file.
#[test]
fn ranged_reads_return_the_window_asked_for() {
    let m = mount();
    let target = m.resolve(b"/pattern.bin").expect("resolve");
    let size = 3 * 1024 * 1024u64;

    let mut buf = vec![0u8; 64 * 1024];
    let offset = 1024 * 1024 - 32 * 1024;
    let n = m.read_at(&target, offset, &mut buf).expect("read_at");
    assert_eq!(n, buf.len());
    for (i, chunk) in buf.as_chunks::<8>().0.iter().enumerate() {
        let want = offset / 8 + i as u64;
        assert_eq!(u64::from_le_bytes(*chunk), want);
    }

    let mut tail = vec![0u8; 4096];
    let n = m.read_at(&target, size - 1000, &mut tail).expect("read_at");
    assert_eq!(n, 1000, "a read past the end is short, not an error");
}

/// zlib, lzo and zstd extents each go through their own decoder.
#[test]
fn every_compression_algorithm_decodes() {
    let m = mount();
    let want: String = (0..65536)
        .map(|i| format!("compressible line {i:08}\n"))
        .collect();
    for algo in ["zlib", "lzo", "zstd"] {
        let got = m
            .read_path(&format!("/compressed/{algo}.txt"))
            .unwrap_or_else(|e| panic!("read {algo}.txt: {e}"));
        assert!(
            got == want.as_bytes(),
            "{algo}.txt does not decode to what was written"
        );
    }
}

#[test]
fn a_directory_of_200_entries_lists_them_all() {
    let m = mount();
    let got = names(&m, "/manyentries");
    assert_eq!(got.len(), 200);
    for i in 1..=200 {
        assert!(
            got.contains(&format!("entry-{i}.txt")),
            "entry-{i}.txt is missing"
        );
    }
}

/// A subvolume and a snapshot are directories whose contents live in
/// another tree; the driver crosses into each.
#[test]
fn subvolumes_and_snapshots_are_crossed() {
    let m = mount();
    let root = m.children(b"/").expect("list /");
    for name in ["subvol", "snap"] {
        let c = root
            .iter()
            .find(|c| c.name == name.as_bytes())
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert!(c.subvolume, "{name} is a subvolume");
        assert!(c.inode.is_dir(), "{name} shows as a directory");
        assert_eq!(names(&m, &format!("/{name}")), vec!["inside.txt"]);
        assert_eq!(
            m.read_path(&format!("/{name}/inside.txt")).expect("read"),
            b"inside a subvolume"
        );
    }
}

/// A symlink is listed as one, and is not readable as file content.
#[test]
fn symlinks_are_listed_but_not_read_as_files() {
    let m = mount();
    let root = m.children(b"/").expect("list /");
    for name in ["link-to-small", "dangling-link"] {
        let c = root
            .iter()
            .find(|c| c.name == name.as_bytes())
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert!(c.inode.is_symlink(), "{name} is a symlink");
        assert!(m.read_path(&format!("/{name}")).is_err());
    }
}

#[test]
fn missing_paths_and_directories_are_refused() {
    let m = mount();
    for missing in [
        "/definitely-not-here",
        "/dir/not-here-either",
        "/small.txt/treated-as-a-directory",
        "/dir",
    ] {
        assert!(
            m.read_path(missing).is_err(),
            "{missing} must not read as a file"
        );
    }
}

#[test]
fn a_partition_index_on_an_unpartitioned_image_is_refused() {
    assert!(Mount::open(&fixture(), Some(1)).is_err());
}

#[test]
fn volume_totals_are_the_superblocks() {
    let m = mount();
    let sb = m.fs.superblock();
    let (total, free) = m.volume_totals();
    assert_eq!(total, sb.total_bytes);
    assert_eq!(free, sb.total_bytes - sb.bytes_used);
    assert!(free < total);
}
