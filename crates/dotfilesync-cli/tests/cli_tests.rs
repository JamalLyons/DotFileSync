use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::tempdir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "dotfilesync inspects, tracks, and safely reports drift",
        ));
}

#[test]
fn test_cli_import_brewfile_standard() {
    let path = fixture_path("Brewfile.standard");
    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    cmd.args(["import-brewfile", "--brewfile", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Imported 4 formulae and 2 casks statically.",
        ));
}

#[test]
fn test_cli_import_brewfile_dynamic_json() {
    let path = fixture_path("Brewfile.dynamic");
    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    cmd.args([
        "import-brewfile",
        "--brewfile",
        path.to_str().unwrap(),
        "--json",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "\"construct_type\": \"dynamic_ruby\"",
    ))
    .stdout(predicate::str::contains("\"construct_type\": \"tap\""));
}

#[test]
fn test_cli_status_json() {
    let temp_home = tempdir().unwrap();
    let temp_repo = tempdir().unwrap();
    let path = fixture_path("sample_manifest.toml");

    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    cmd.args([
        "status",
        "--manifest",
        path.to_str().unwrap(),
        "--home",
        temp_home.path().to_str().unwrap(),
        "--repo-root",
        temp_repo.path().to_str().unwrap(),
        "--json",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("\"schema_version\": 1"))
    .stdout(predicate::str::contains("\"package_differences\":"))
    .stdout(predicate::str::contains("\"config_differences\":"));
}

#[test]
fn test_cli_scan_json() {
    let temp_home = tempdir().unwrap();
    let temp_repo = tempdir().unwrap();
    let path = fixture_path("sample_manifest.toml");

    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    cmd.args([
        "scan",
        "--manifest",
        path.to_str().unwrap(),
        "--home",
        temp_home.path().to_str().unwrap(),
        "--repo-root",
        temp_repo.path().to_str().unwrap(),
        "--json",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("\"homebrew\":"))
    .stdout(predicate::str::contains("\"configs\":"));
}
