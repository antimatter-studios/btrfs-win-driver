# Changelog

Notable changes to `btrfs-win-driver`, newest first. Each release's section is
written before it is tagged, and the GitHub release's notes are that section.

## [Unreleased]

### Added

- **Btrfs volumes mount read-only on Windows through WinFsp.** `btrfs mount
  IMAGE --drive X:` lists directories and serves files from rust-fs-btrfs
  0.10.2, crossing into subvolumes and snapshots, decompressing zlib, lzo and
  zstd extents, and reading each callback's window with a ranged read.
- **`btrfs info`, `ls` and `cat` read a volume without mounting it**, on
  Windows, Linux or macOS, with `--part N` for a partition of a whole-disk
  image.
- **The read paths are tested against a filesystem the Linux kernel wrote.**
  `scripts/build-fixtures.sh` builds the image with mkfs.btrfs and a loop
  mount; `tests/mount_reads.rs` and the 17-scenario Windows matrix read it,
  each held to a floor of executed tests.
- **CI, the release workflow and the installer follow the sibling Windows
  drivers.** A release is built for x64 and arm64, tested installed through
  the matrix before it is attached, and submitted to winget when a token is
  configured.
