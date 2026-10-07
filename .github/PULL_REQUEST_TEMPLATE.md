## Summary

<1-3 bullets describing what changed and why>

## Test plan

- [ ] CI green: portable checks, the fixture tests against a real Btrfs image, the Windows check and the WinFsp matrix
- [ ] If touching the read paths: a scenario in `test-matrix.json` or a test in `tests/mount_reads.rs` covers it, against content `scripts/build-fixtures.sh` writes
- [ ] If touching the installer: Setup.exe install + uninstall on a clean Windows VM

## Skeleton coupling

- [ ] No new `crate::` code that belongs in `winfsp-fs-skeleton` (drive-letter / partition / device / disk-arrival patterns live there)
- [ ] If a skeleton fix is needed, it lands and is tagged there first; this PR moves `SKELETON_REF` in `chores.yml`

## Notes

<anything reviewers should know>
