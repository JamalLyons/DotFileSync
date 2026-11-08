use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[cfg(unix)]
fn create_mock_brew_script(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script_path = dir.join("mock_brew.sh");
    let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "Homebrew 4.3.0"
    exit 0
fi
if [ "$1" = "info" ]; then
    echo '{"formulae":[{"name":"git","installed":[{"version":"2.44.0"}]}],"casks":[{"token":"ghostty","version":"1.0.0"}]}'
    exit 0
fi
if [ "$1" = "list" ]; then
    if [ "$3" = "--formula" ]; then
        echo "git 2.44.0"
    elif [ "$3" = "--cask" ]; then
        echo "ghostty 1.0.0"
    fi
    exit 0
fi
exit 1
"#;
    fs::write(&script_path, script).unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();
    script_path
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
fn test_cli_import_brewfile_creates_output_and_refuses_overwrite() {
    let temp_dir = tempdir().unwrap();
    let brewfile_path = fixture_path("Brewfile.standard");
    let output_path = temp_dir.path().join("generated_manifest.toml");

    // 1. First run creates the new manifest
    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    cmd.args([
        "import-brewfile",
        "--brewfile",
        brewfile_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("Wrote manifest skeleton to:"));

    assert!(output_path.exists());
    let initial_bytes = fs::read(&output_path).unwrap();

    // 2. Second run must refuse to overwrite existing file and leave bytes unchanged
    let mut cmd_overwrite = Command::cargo_bin("dotfilesync").unwrap();
    cmd_overwrite
        .args([
            "import-brewfile",
            "--brewfile",
            brewfile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "already exists; refusing to overwrite",
        ));

    let after_bytes = fs::read(&output_path).unwrap();
    assert_eq!(
        initial_bytes, after_bytes,
        "Existing manifest bytes must remain completely unchanged"
    );
}

#[cfg(unix)]
#[test]
fn test_cli_status_json_fixture_isolated() {
    let temp_home = tempdir().unwrap();
    let temp_repo = tempdir().unwrap();
    let temp_bin = tempdir().unwrap();
    let mock_brew = create_mock_brew_script(temp_bin.path());
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
        "--brew-bin",
        mock_brew.to_str().unwrap(),
        "--json",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("\"schema_version\": 1"))
    .stdout(predicate::str::contains("\"package_differences\":"))
    .stdout(predicate::str::contains("\"config_differences\":"))
    .stdout(predicate::str::contains("\"homebrew_available\": true"));
}

#[cfg(unix)]
#[test]
fn test_cli_scan_json_fixture_isolated() {
    let temp_home = tempdir().unwrap();
    let temp_repo = tempdir().unwrap();
    let temp_bin = tempdir().unwrap();
    let mock_brew = create_mock_brew_script(temp_bin.path());
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
        "--brew-bin",
        mock_brew.to_str().unwrap(),
        "--json",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("\"available\": true"))
    .stdout(predicate::str::contains("\"packages_enumerated\": true"))
    .stdout(predicate::str::contains("\"configs\":"));
}
