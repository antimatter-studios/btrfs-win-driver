#!/usr/bin/env bash
# oracle-write.sh BINARY IMAGE -- the in-place write path, judged by Btrfs's
# own tools rather than by this driver.
#
# tests/write_in_place.rs reads back what the driver wrote through the
# driver, which proves the driver agrees with itself and nothing more: a
# misreading of the format would be baked into the write and the read
# alike. Here the judges are independent of it. On a copy of IMAGE
# (scripts/build-fixtures.sh's), BINARY's `write` subcommand
#
#   1. overwrites 46 bytes of /nocow/data.bin across a page boundary;
#   2. is refused for /pattern.bin, an ordinary copy-on-write file;
#   3. is refused for a write running past the end of /nocow/data.bin;
#
# and then `btrfs check` must find nothing wrong with the image, and the
# Linux kernel must mount it and read /nocow/data.bin as the fixture's
# pattern with exactly those 46 bytes replaced, and /pattern.bin unchanged.
#
# Needs Linux, root (the kernel mount), btrfs-progs and python3: CI's
# fixtures job runs it. Ends with "oracle-write.sh: all checks passed".
set -uo pipefail

[ $# -eq 2 ] || { echo "oracle-write.sh: usage: oracle-write.sh BINARY IMAGE" >&2; exit 2; }
bin="$1"
image="$2"
for tool in btrfs python3 mount umount; do
  command -v "$tool" >/dev/null || { echo "oracle-write.sh: $tool not found (Linux, btrfs-progs)" >&2; exit 1; }
done
[ "$(id -u)" = 0 ] || { echo "oracle-write.sh: must run as root, to mount the image" >&2; exit 1; }
[ -x "$bin" ] || { echo "oracle-write.sh: $bin is not an executable; build it with cargo build first" >&2; exit 1; }
[ -f "$image" ] || { echo "oracle-write.sh: no image at $image; build it with sudo scripts/build-fixtures.sh" >&2; exit 1; }

fails=0
ok() { echo "ok - $1"; }
fail() { echo "FAIL - $1" >&2; fails=$((fails + 1)); }

work=$(mktemp -d)
copy="$work/oracle.img"
mnt="$work/mnt"
mkdir "$mnt"
cleanup() {
  umount "$mnt" 2>/dev/null || true
  rm -f "$copy" "$work/err" "$work/check.log"
  rmdir "$mnt" "$work" 2>/dev/null || true
}
trap cleanup EXIT
cp "$image" "$copy"

payload='written by the driver, read back by the kernel'
offset=4090
nocow_size=$((1024 * 1024))

# --- the writes, through the driver --------------------------------------

if printf '%s' "$payload" | "$bin" write "$copy" /nocow/data.bin --offset "$offset" >/dev/null 2>"$work/err"; then
  ok "the driver overwrote /nocow/data.bin at $offset"
else
  fail "the driver did not overwrite /nocow/data.bin: $(cat "$work/err")"
fi

if printf 'x' | "$bin" write "$copy" /pattern.bin --offset 0 >/dev/null 2>"$work/err"; then
  fail "the driver wrote /pattern.bin, a copy-on-write file"
else
  ok "the driver refused /pattern.bin: $(tail -1 "$work/err")"
fi

if printf '0123456789abcdefghij' | "$bin" write "$copy" /nocow/data.bin --offset $((nocow_size - 10)) >/dev/null 2>"$work/err"; then
  fail "the driver wrote past the end of /nocow/data.bin"
else
  ok "the driver refused a write past the end: $(tail -1 "$work/err")"
fi

# --- the judges ------------------------------------------------------------

if btrfs check --readonly "$copy" >"$work/check.log" 2>&1; then
  ok "btrfs check finds nothing wrong after the writes"
else
  fail "btrfs check rejects the image after the writes:"
  tail -30 "$work/check.log" >&2
fi

if mount -o loop,ro "$copy" "$mnt"; then
  if python3 -I - "$mnt" "$offset" "$payload" <<'PY'; then
import sys

mnt, offset, payload = sys.argv[1], int(sys.argv[2]), sys.argv[3].encode()

def pattern(size):
    return b"".join(i.to_bytes(8, "little") for i in range(size // 8))

want = bytearray(pattern(1024 * 1024))
want[offset:offset + len(payload)] = payload
bad = []
with open(f"{mnt}/nocow/data.bin", "rb") as f:
    got = f.read()
if got != want:
    first = next((i for i in range(min(len(got), len(want))) if got[i] != want[i]), None)
    bad.append(f"nocow/data.bin: {len(got)} bytes, first difference at {first}")
with open(f"{mnt}/pattern.bin", "rb") as f:
    if f.read() != pattern(3 * 1024 * 1024):
        bad.append("pattern.bin changed")
for b in bad:
    print(f"kernel read: {b}", file=sys.stderr)
sys.exit(1 if bad else 0)
PY
    ok "the kernel reads exactly the bytes written, and the refused file unchanged"
  else
    fail "the kernel reads something other than what was written"
  fi
  umount "$mnt"
else
  fail "the kernel would not mount the image after the writes"
fi

if [ "$fails" -gt 0 ]; then
  echo "oracle-write.sh: $fails check(s) failed" >&2
  exit 1
fi
echo "oracle-write.sh: all checks passed"
