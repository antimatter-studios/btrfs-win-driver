//! A release is tested as a user installs it, and only then published;
//! winget follows on its own.
//!
//! Checking only that `Setup.exe /quiet` exits would let a tag attach an
//! installer whose installed driver had never mounted an image. These
//! checks read the files as text; they need no Windows.
//! `matrix_in_ci.rs` covers the harness and the images.
//!
//! - The matrix is one reusable workflow, `matrix.yml`. CI calls it with a
//!   development build; given an installer, it installs it silently, runs
//!   the matrix against the `btrfs.exe` the installer put in Program Files,
//!   then uninstalls it and checks nothing is left behind.
//! - The release calls it with the installer it just built, and attaches
//!   to the GitHub release only from a job that needs that run, with the
//!   manual checklist for the real-device check.
//! - A winget job, after the attach, submits the new version, and only
//!   when a token for it is configured.

use std::fs;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn exists(rel: &str) -> bool {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel).exists()
}

/// The text of the job `name` in a workflow: from its key to the next
/// job key at the same indent.
fn job<'a>(workflow: &'a str, name: &str) -> Option<&'a str> {
    let start = workflow.find(&format!("\n  {name}:\n"))? + 1;
    let rest = &workflow[start + 1..];
    let end = rest
        .match_indices("\n  ")
        .find(|(i, _)| {
            let line = &rest[i + 1..];
            let first = line.chars().nth(2);
            first.is_some_and(|c| c.is_ascii_alphanumeric())
                && line[2..]
                    .split('\n')
                    .next()
                    .is_some_and(|l| l.trim_end().ends_with(':'))
        })
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    Some(&workflow[start..start + 1 + end])
}

#[test]
fn the_matrix_is_one_reusable_workflow_that_can_test_an_installed_driver() {
    let matrix = read(".github/workflows/matrix.yml");
    assert!(
        matrix.contains("workflow_call:"),
        "matrix.yml is not a reusable workflow"
    );
    assert!(
        matrix.contains("installer:"),
        "matrix.yml takes no installer input"
    );
    assert!(
        matrix.contains("verify-silent.ps1"),
        "matrix.yml does not install the installer silently (installer/verify-silent.ps1)"
    );
    assert!(
        matrix.contains(r"btrfs-win-driver\btrfs.exe"),
        "matrix.yml does not run the btrfs.exe the installer put in Program Files"
    );
    assert!(
        matrix.contains("verify-uninstall.ps1"),
        "matrix.yml does not uninstall the installed driver and check nothing is left"
    );
    assert!(
        exists("installer/verify-uninstall.ps1"),
        "no installer/verify-uninstall.ps1"
    );
    assert!(
        matrix.contains("scripts/matrix-floor.sh"),
        "matrix.yml does not hold the run to a floor of executed scenarios"
    );
}

#[test]
fn ci_runs_the_matrix_through_the_reusable_workflow() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("uses: ./.github/workflows/matrix.yml"),
        "ci.yml does not run the matrix through matrix.yml"
    );
}

#[test]
fn the_release_attaches_only_what_the_installed_matrix_passed() {
    let release = read(".github/workflows/release.yml");
    let build = job(&release, "build").expect("release.yml has a build job");
    assert!(
        !build.contains("gh release upload"),
        "the build job still attaches its installers before anything has tested them installed"
    );
    let installed =
        job(&release, "installed-matrix").expect("release.yml has no installed-matrix job");
    assert!(
        installed.contains("uses: ./.github/workflows/matrix.yml")
            && installed.contains("installer:"),
        "installed-matrix does not run matrix.yml with the built installer"
    );
    let attach = job(&release, "attach").expect("release.yml has no attach job");
    assert!(
        attach.contains("gh release upload"),
        "the attach job attaches nothing"
    );
    assert!(
        attach.contains("installed-matrix"),
        "the attach job does not wait for installed-matrix"
    );
    assert!(
        attach.contains("RELEASE-CHECKLIST.md"),
        "the attach job does not attach the manual checklist"
    );
    assert!(
        exists("installer/RELEASE-CHECKLIST.md"),
        "no installer/RELEASE-CHECKLIST.md"
    );
}

#[test]
fn winget_is_submitted_after_the_attach_and_only_with_a_token() {
    let release = read(".github/workflows/release.yml");
    let winget = job(&release, "winget").expect("release.yml has no winget job");
    assert!(
        winget.contains("attach"),
        "the winget job does not wait for the attach"
    );
    assert!(
        winget.contains("WINGET_TOKEN"),
        "the winget job reads no WINGET_TOKEN"
    );
    assert!(
        winget.contains("wingetcreate"),
        "the winget job does not use wingetcreate to submit the new version"
    );
}
