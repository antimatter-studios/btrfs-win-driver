#!/usr/bin/env bash
#
# Build the Btrfs image the tests and the Windows matrix read.
#
# A real filesystem, made by mkfs.btrfs and populated by the Linux kernel,
# so a test that reads it checks this driver against what Btrfs actually
# writes rather than against our own idea of it. An image this repository
# built by hand would share the driver's reading of the format, and the
# two would agree with each other while disagreeing with every real
# volume.
#
# WHAT IT NEEDS. mkfs.btrfs and btrfs (btrfs-progs) and root, to
# loop-mount the image and populate it. That means Linux: CI runs it on
# ubuntu-latest. On macOS there is no mkfs.btrfs.
#
# THE CONTENT IS DELIBERATE. Every file exists because a test asserts
# something specific about it (tests/mount_reads.rs, test-matrix.json).
# Adding a case means adding a file here and asserting on it there.
#
# Usage:  scripts/build-fixtures.sh [output-dir]     (default: .fixtures)
set -euo pipefail

OUT=${1:-.fixtures}
IMG="$OUT/btrfs-content.img"

for tool in mkfs.btrfs btrfs python3 chattr lsattr; do
  command -v "$tool" >/dev/null || {
    echo "build-fixtures.sh: $tool not found. Install btrfs-progs and e2fsprogs (Linux); this cannot run on macOS." >&2
    exit 1
  }
done
[ "$(id -u)" = 0 ] || { echo "build-fixtures.sh: must run as root: the image has to be mounted to populate it" >&2; exit 1; }

mkdir -p "$OUT"
rm -f "$IMG"
MNT=$(mktemp -d)
trap 'umount "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT

# 300 MiB: comfortably over mkfs.btrfs's minimum for a single device,
# small enough to build in seconds and to hand between CI jobs.
truncate -s 300M "$IMG"
mkfs.btrfs -q -L WINTEST "$IMG"
mount -o loop "$IMG" "$MNT"

# --- the deliberate content ------------------------------------------

# A short file whose exact bytes a test compares. Small enough that Btrfs
# stores it inline, inside its extent item in the tree, rather than in a
# data extent: a different read path from every larger file here.
printf 'hello btrfs' > "$MNT/small.txt"

# Empty: a zero-length read must succeed and return nothing.
: > "$MNT/empty.txt"

# 3 MiB with a POSITION-DEPENDENT pattern: each 8-byte group holds its own
# index, little-endian. A ranged read that returns the right NUMBER of
# bytes from the WRONG OFFSET fails, which a constant fill would hide.
python3 - "$MNT/pattern.bin" <<'PY'
import sys
size = 3 * 1024 * 1024
with open(sys.argv[1], "wb") as f:
    f.write(b"".join(i.to_bytes(8, "little") for i in range(size // 8)))
PY

# A nodatacow file: the one kind the driver overwrites in place. +C on the
# directory, before anything is written into it, makes the kernel create
# each file in it NODATACOW and NODATASUM; on a file that already holds data
# the attribute does nothing. 1 MiB of the same position-dependent pattern,
# so an overwrite that lands at the wrong offset, or disturbs a byte beside
# it, shows. Checked with lsattr: a kernel that ignored +C would leave the
# write tests with nothing to write.
mkdir -p "$MNT/nocow"
chattr +C "$MNT/nocow"
python3 - "$MNT/nocow/data.bin" <<'PY'
import sys
size = 1024 * 1024
with open(sys.argv[1], "wb") as f:
    f.write(b"".join(i.to_bytes(8, "little") for i in range(size // 8)))
PY
lsattr "$MNT/nocow/data.bin" | awk '{print $1}' | grep -q C || {
  echo "build-fixtures.sh: nocow/data.bin did not come out nodatacow" >&2
  exit 1
}

# Nested directories, so path walking goes past one level.
mkdir -p "$MNT/dir/nested/deep"
printf 'level three' > "$MNT/dir/nested/deep/leaf.txt"
printf 'level one' > "$MNT/dir/one.txt"

# 200 entries: a directory whose index spans more than one tree leaf.
mkdir -p "$MNT/manyentries"
for i in $(seq 1 200); do printf 'e%s' "$i" > "$MNT/manyentries/entry-$i.txt"; done

# A symlink, and one that dangles: a dangling target is a normal thing on
# disk rather than an error.
ln -s /small.txt "$MNT/link-to-small"
ln -s /nowhere-at-all "$MNT/dangling-link"

# A name that is not ASCII. Btrfs stores names as bytes.
printf 'unicode name' > "$MNT/naïve-café.txt"

# Compressed extents, one file per algorithm the kernel writes: zlib, lzo
# and zstd each have their own decoder in the reader. The property on the
# directory makes the kernel compress what is written into it; the content
# is compressible text, so it does compress, and every line carries its
# own number so a misplaced block fails the hash.
mkdir -p "$MNT/compressed"
for algo in zlib lzo zstd; do
  btrfs property set "$MNT/compressed" compression "$algo"
  python3 - "$MNT/compressed/$algo.txt" <<'PY'
import sys
with open(sys.argv[1], "w") as f:
    for i in range(65536):
        f.write(f"compressible line {i:08d}\n")
PY
  sync
done

# A subvolume, and a read-only snapshot of it: a directory entry that
# names another tree rather than an inode, which the driver must cross.
btrfs subvolume create "$MNT/subvol" >/dev/null
printf 'inside a subvolume' > "$MNT/subvol/inside.txt"
sync
btrfs subvolume snapshot -r "$MNT/subvol" "$MNT/snap" >/dev/null

sync
umount "$MNT"
rmdir "$MNT"
trap - EXIT

echo "built $IMG"
ls -la "$IMG"
