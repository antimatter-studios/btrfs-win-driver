#!/usr/bin/env bash
# matrix-floor.sh LOG FLOOR -- the matrix run logged in LOG executed at
# least FLOOR scenarios, every one passing.
#
# A run that stops early reports no failures at all, so its exit status
# cannot tell it from a run that passed; only a count can. The runner's
# summary is libtest's "test result: ok. N passed; M failed; ...".
set -euo pipefail
[ $# -eq 2 ] || { echo "matrix-floor.sh: usage: matrix-floor.sh LOG FLOOR" >&2; exit 2; }
log="$1"
floor="$2"
[ -f "$log" ] || { echo "matrix-floor.sh: $log is missing -- the matrix did not run" >&2; exit 1; }
line=$(grep -E 'test result: ' "$log" | tail -1 || true)
[ -n "$line" ] || { echo "matrix-floor.sh: $log has no test result line -- the runner stopped early" >&2; exit 1; }
passed=$(sed -nE 's/.* ([0-9]+) passed.*/\1/p' <<<"$line")
failed=$(sed -nE 's/.* ([0-9]+) failed.*/\1/p' <<<"$line")
echo "matrix-floor.sh: $line"
[ "${failed:-1}" -eq 0 ] || { echo "matrix-floor.sh: $failed scenario(s) failed" >&2; exit 1; }
[ "${passed:-0}" -ge "$floor" ] || {
    echo "matrix-floor.sh: $passed scenario(s) passed, under the floor of $floor: the run stopped early or the matrix shrank" >&2
    exit 1
}
echo "matrix-floor.sh: $passed passed, floor $floor"
