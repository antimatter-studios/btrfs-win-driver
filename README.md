# btrfs-win-driver

Mount and read **Btrfs** volumes on Windows, as drive letters, through
[WinFsp](https://winfsp.dev/), on the pure-Rust
[rust-fs-btrfs](https://github.com/antimatter-studios/rust-fs-btrfs) reader.

The driver runs in **user mode**. A fault in it takes down the mount, not
Windows, which is the trade against a kernel driver: it is slower, and it
cannot crash the machine. It mounts **read-only**, which is what you want
when the reason you are reading a Linux disk on Windows is to get data off it.

The Windows plumbing -- the SCM service, the disk-arrival watcher,
WinFsp.Launcher integration, the partition-table walker, raw-device I/O and the
installer templates -- comes from
[winfsp-fs-skeleton](https://github.com/antimatter-studios/winfsp-fs-skeleton).
This repository supplies the Btrfs part: the superblock probe, the WinFsp
`FileSystemContext` over the reader, and the tests.

## Status

Read-only, and tested against real filesystems. First release pending.

- `btrfs info`, `ls` and `cat` read an image or device from the command line,
  on Windows, Linux or macOS.
- `btrfs mount IMAGE --drive X:` mounts a volume read-only through WinFsp,
  with `--part N` for one partition of a whole-disk image.
- **Subvolumes and snapshots** are crossed into, so they appear as the
  directories they are under Linux.
- **Compressed files** (zlib, lzo and zstd) decompress, and every checksum type
  Btrfs uses is verified by the reader.
- The ranged-read path serves exactly the window Windows asks for, so a
  sequential read of a large file costs its size once.

Not yet:

- **Writes.** rust-fs-btrfs can overwrite bytes in place in a `nodatacow`
  file and copy-on-write a `nodatasum` one; it cannot yet create, grow, rename
  or delete. A writable mount limited to that is the next step.
- **Auto-mount.** The `BtrfsWatcher` service and `btrfs watch` come from the
  skeleton, which recognises a filesystem by its first 4 KiB. Btrfs's magic is
  64 KiB in, so a plugged-in Btrfs disk is not recognised; mount it by hand.
- **Symlinks** are listed as reparse points but not followed.
- **Multi-device pools** (RAID across several disks) open only through the
  reader's API, not the CLI or the mount.
- **Synology and other md/LVM disks** need the md/LVM layer first.

## Usage

```
btrfs info  <image> [--part N]           # label, UUID, sizes, checksum, devices
btrfs ls    <image> [path] [--part N]    # one directory, crossing into subvolumes
btrfs cat   <image> <path> [--part N]    # a regular file's bytes, to stdout
btrfs mount <image> --drive X: [--part N]
```

`mount` needs Windows with WinFsp installed and a build with
`--features mount`. It blocks until Ctrl-C, which unmounts. `<image>` may be a
raw device: `\\.\PhysicalDrive2`, `\\.\X:` and `\\?\STORAGE#Disk#...` are read
with sector-aligned I/O, so 4Kn drives work.

## Install

Once released, `winget install AntimatterStudios.btrfs-win-driver`, or the
`btrfs-win-driver-<ver>-<arch>-Setup.exe` from Releases (`x64` or `arm64`).
The installer chains WinFsp if it is missing, puts `btrfs.exe` and
`Mount-Btrfs.ps1` in `C:\Program Files\btrfs-win-driver\`, and adds a
**Mount as btrfs** verb to `.img` files in Explorer.

## Build

```
chore siblings                              # the sibling checkouts, at the pins in chores.yml
cargo build --release                       # the CLI, any platform
cargo build --release --features mount      # + the WinFsp mount (Windows)
```

The manifest path-depends on `../rust-fs-core`, `../rust-fs-btrfs`,
`../winfsp-fs-skeleton` and `../winfsp-rs`. `chores.yml` pins each, URL beside
ref, and CI reads the same pins. The WinFsp build needs WinFsp 2.1+, LLVM
(`libclang` for bindgen) and LLVM-MinGW for the `*-pc-windows-gnullvm`
targets; `.github/workflows/ci.yml`'s Windows job is the reference setup.

## Testing

Three layers, all in CI:

| what | where | against |
|---|---|---|
| unit and text checks | `cargo test` | the code, and the workflows read as text |
| the read paths | `cargo test --features fixtures` | a Btrfs image `mkfs.btrfs` made and the Linux kernel populated |
| the mount | `test-matrix.json` through fs-windows-test-harness | that image, mounted through WinFsp on a Windows runner |

The image is built by `scripts/build-fixtures.sh` (Linux, root, btrfs-progs):
inline, empty, multi-extent and compressed files, a 200-entry directory,
symlinks, a non-ASCII name, a subvolume and a read-only snapshot. An image
this repository built itself would share the driver's reading of the format,
so the oracle is the kernel's own writes. A fixture test with no image fails
rather than skipping; both the fixture job and the matrix hold their runs to a
floor of executed tests.

To run the matrix on your own Windows VM, see
[fs-windows-test-harness](https://github.com/antimatter-studios/fs-windows-test-harness)
and `fs-windows-test-harness.toml`.

## Changelog

The latest releases; every change, with the reasoning behind it, is in
[CHANGELOG.md](CHANGELOG.md).

No release yet.

## Licence

This repository's source is **MIT** ([LICENSE](LICENSE)), as is
rust-fs-btrfs.

The executable links winfsp-fs-skeleton in every build, and with
`--features mount` the WinFsp Rust bindings
([winfsp-rs](https://github.com/antimatter-studios/winfsp-rs)) too; both are
**GPL-3.0**. MIT code may be combined into a GPL-3.0 work, so there is no
conflict, but a built `btrfs.exe` is distributed under GPL-3.0 as a whole,
which is why the installer shows the GPL. The MIT grant covers this
repository's source, which can be reused on its own terms.
