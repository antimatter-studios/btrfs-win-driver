//! Btrfs-specific superblock detection.
//!
//! Btrfs keeps its primary superblock **64 KiB** into the device, not at
//! the start (XFS) or at 1024 (ext4, EROFS), and its magic is the eight
//! ASCII bytes `_BHRfS_M` at offset `0x40` within that superblock. So
//! the magic sits at absolute byte `0x1_0040` of a partition.
//!
//! Both values are taken from `fs_btrfs::superblock` rather than retyped,
//! so the driver and the reader it drives cannot disagree about what a
//! Btrfs filesystem looks like. The tests below then pin the facts
//! literally, so a wrong constant in the reader would fail here rather
//! than being inherited silently.
//!
//! # What this means for auto-mount
//!
//! `winfsp-fs-skeleton` probes each partition by reading its first
//! **4 KiB** and handing that buffer to [`is_btrfs`]. A Btrfs magic is
//! never inside the first 4 KiB, so through the skeleton as it stands the
//! watcher and the service cannot recognise a Btrfs partition: they
//! report nothing rather than something wrong. Mounting by hand
//! (`btrfs mount IMAGE --drive X:`) is unaffected. The test
//! [`tests::a_four_kib_probe_window_cannot_see_the_magic`] records the
//! limit so it cannot be forgotten.

use fs_btrfs::superblock::{offsets, BTRFS_MAGIC, SUPER_INFO_OFFSET};

/// Absolute byte offset of the magic within a partition: the primary
/// superblock's position plus the magic's position inside it.
pub const MAGIC_OFFSET: usize = SUPER_INFO_OFFSET as usize + offsets::MAGIC;

/// How many bytes from the start of a partition [`is_btrfs`] needs.
pub const PROBE_BYTES: usize = MAGIC_OFFSET + BTRFS_MAGIC.len();

/// Whether `bytes`, read from the start of a partition, carry a Btrfs
/// superblock magic. A buffer too short to reach the magic is not a
/// match: there is nothing in it to recognise.
pub fn is_btrfs(bytes: &[u8]) -> bool {
    bytes.get(MAGIC_OFFSET..PROBE_BYTES) == Some(&BTRFS_MAGIC[..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_magic_at(offset: usize) -> Vec<u8> {
        let mut buf = vec![0u8; PROBE_BYTES + 4096];
        buf[offset..offset + 8].copy_from_slice(b"_BHRfS_M");
        buf
    }

    #[test]
    fn the_magic_is_the_ascii_bhrfs_m() {
        assert_eq!(&BTRFS_MAGIC, b"_BHRfS_M");
    }

    /// 64 KiB plus 0x40, written out literally so a wrong constant in the
    /// reader fails here.
    #[test]
    fn the_magic_is_64_kib_and_0x40_into_the_partition() {
        assert_eq!(MAGIC_OFFSET, 0x1_0040);
    }

    #[test]
    fn matches_the_magic_where_btrfs_puts_it() {
        assert!(is_btrfs(&with_magic_at(0x1_0040)));
    }

    /// The layouts of the sibling filesystems must not match: magic at
    /// the start of the device (XFS), at 1024 (ext4, EROFS), or at the
    /// superblock's start rather than 0x40 into it.
    #[test]
    fn the_magic_anywhere_else_does_not_match() {
        for offset in [0, 1024, 0x1_0000, 0x1_0038, 0x1_0048] {
            assert!(
                !is_btrfs(&with_magic_at(offset)),
                "magic at {offset:#x} is not a Btrfs superblock"
            );
        }
    }

    #[test]
    fn rejects_a_buffer_that_is_too_short() {
        assert!(!is_btrfs(&[]));
        let buf = with_magic_at(MAGIC_OFFSET);
        assert!(!is_btrfs(&buf[..PROBE_BYTES - 1]));
        assert!(is_btrfs(&buf[..PROBE_BYTES]));
    }

    #[test]
    fn rejects_a_zeroed_buffer() {
        assert!(!is_btrfs(&vec![0u8; PROBE_BYTES]));
    }

    /// The skeleton's watcher and service probe a 4 KiB window from the
    /// start of each partition. Btrfs's magic is outside it, so auto-mount
    /// through the skeleton cannot see a Btrfs volume until the skeleton
    /// reads further. This pins that fact; when the skeleton changes, this
    /// test is the one to revisit.
    #[test]
    fn a_four_kib_probe_window_cannot_see_the_magic() {
        const SKELETON_PROBE_WINDOW: usize = 4096;
        const { assert!(PROBE_BYTES > SKELETON_PROBE_WINDOW) };
        let buf = with_magic_at(MAGIC_OFFSET);
        assert!(!is_btrfs(&buf[..SKELETON_PROBE_WINDOW]));
    }
}
