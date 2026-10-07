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
- **`btrfs mount --rw` overwrites bytes of `nodatacow` files in place, and
  nothing else.** A file the kernel marked `nodatacow` in the default
  subvolume can be overwritten inside its current size through the mount;
  every other file carries the read-only attribute, and opening one for
  writing is refused at the open rather than by Windows' lazy writer after
  the application was told it succeeded.
- **`btrfs write` makes the same overwrite from the command line**, from
  stdin at `--offset`, and prints rust-fs-btrfs's reason when it refuses.
- **The in-place write is judged by `btrfs check` and the Linux kernel.**
  `scripts/oracle-write.sh` writes into the fixture through the driver, then
  checks the image and has the kernel read the file back; three Windows
  matrix scenarios write through a `--rw` mount and refuse through a
  read-only one.
