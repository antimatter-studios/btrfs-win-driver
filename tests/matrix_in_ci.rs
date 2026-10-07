//! The Windows matrix runs in CI, through fs-windows-test-harness, against
//! the image this repository builds. These checks read the files as text;
//! they need no Windows and no fixture.

use std::fs;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn exists(rel: &str) -> bool {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel).exists()
}

#[test]
fn the_harness_is_fs_windows_test_harness_at_a_v4_release() {
    let chores = read("chores.yml");
    assert!(
        chores.contains(
            "HARNESS_URL: https://github.com/antimatter-studios/fs-windows-test-harness.git"
        ),
        "chores.yml does not pin fs-windows-test-harness"
    );
    let reference = chores
        .lines()
        .find_map(|l| l.trim().strip_prefix("HARNESS_REF:"))
        .expect("chores.yml has a HARNESS_REF")
        .trim()
        .to_string();
    assert!(
        reference.starts_with("v4."),
        "HARNESS_REF is {reference}, not a v4 release"
    );
    assert!(
        read("fs-windows-test-harness.toml")
            .contains(r#"binary      = "target/release/btrfs.exe""#),
        "fs-windows-test-harness.toml does not name target/release/btrfs.exe"
    );
}

#[test]
fn every_scenario_runs_on_an_image_this_repository_builds() {
    let matrix = read("test-matrix.json");
    let scenarios = matrix.matches("\"recipe\"").count();
    let builds = matrix.matches("\"op\": \"build-btrfs-image\"").count();
    assert!(scenarios > 0, "test-matrix.json has no scenarios");
    assert_eq!(
        builds, scenarios,
        "a scenario does not start by building its image"
    );
    assert!(
        read("fs-windows-test-harness.toml").contains("[ops.build-btrfs-image]"),
        "the harness config defines no build-btrfs-image op"
    );
}

/// The floor matrix.yml holds the run to is the number of scenarios, so a
/// scenario added without raising it, or one that silently stops running,
/// is caught.
#[test]
fn the_matrix_floor_is_every_scenario() {
    let scenarios = read("test-matrix.json").matches("\"recipe\"").count();
    let wf = read(".github/workflows/matrix.yml");
    assert!(
        wf.contains(&format!(
            "bash scripts/matrix-floor.sh tmp/logs/matrix.log {scenarios}"
        )),
        "matrix.yml's floor is not the {scenarios} scenarios test-matrix.json holds"
    );
    assert!(exists("scripts/run-matrix.sh"), "no scripts/run-matrix.sh");
    assert!(
        wf.contains("scripts/run-matrix.sh"),
        "matrix.yml does not run the matrix"
    );
}
