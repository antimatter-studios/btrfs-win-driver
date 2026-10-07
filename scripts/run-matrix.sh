#!/usr/bin/env bash
# run-matrix.sh [harness args...] -- run test-matrix.json through
# fs-windows-test-harness.
#
# Sets HOST_IMAGE_DIR (from .test-env, default diskimages/, in Windows form
# under Git Bash so btrfs.exe can open it) and hands every argument to the
# harness's run-tests.sh: on a developer's machine its VM settings come
# from .test-env; in CI, --vm-host/--ssh-key/--vm-workdir/--no-ship make the
# runner its own Windows VM.
set -uo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root" || exit 1

host_image_dir=""
if [ -f .test-env ]; then
    host_image_dir=$(sed -n 's/^HOST_IMAGE_DIR=//p' .test-env | head -1)
fi
host_image_dir="${host_image_dir:-diskimages}"
case "$host_image_dir" in /*) ;; *) host_image_dir="$repo_root/$host_image_dir" ;; esac
mkdir -p "$host_image_dir"
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) host_image_dir="$(cygpath -m "$host_image_dir")" ;;
esac
export HOST_IMAGE_DIR="$host_image_dir"

exec bash "$repo_root/../fs-windows-test-harness/scripts/run-tests.sh" "$@"
