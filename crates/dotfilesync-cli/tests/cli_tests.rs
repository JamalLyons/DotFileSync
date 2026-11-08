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

#[cfg(unix)]
#[test]
fn test_cli_status_human_absent_machine_local_distinguishable() {
    let temp_home = tempdir().unwrap();
    let temp_repo = tempdir().unwrap();
    let temp_bin = tempdir().unwrap();
    let mock_brew = create_mock_brew_script(temp_bin.path());

    // Create repo source for ordinary missing file
    let src_ordinary = temp_repo.path().join("ordinary.conf");
    fs::write(&src_ordinary, "ordinary content\n").unwrap();

    // Create custom manifest in temp_repo
    let manifest_path = temp_repo.path().join("manifest.toml");
    let manifest_content = r#"
schema_version = 1

[[files]]
source = "ordinary.conf"
target = "~/.config/ordinary.conf"

[[files]]
source = "fish/conf.d/local.fish"
target = "~/.config/fish/conf.d/local.fish"
machine_local = true
"#;
    fs::write(&manifest_path, manifest_content).unwrap();

    let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
    let assert = cmd
        .args([
            "status",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--home",
            temp_home.path().to_str().unwrap(),
            "--repo-root",
            temp_repo.path().to_str().unwrap(),
            "--brew-bin",
            mock_brew.to_str().unwrap(),
        ])
        .assert()
        .success();

    let output_str = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // Ordinary missing file must be in MISSING CONFIG FILES with its Source:
    assert!(output_str.contains("MISSING CONFIG FILES (1):"));
    assert!(output_str.contains("ordinary.conf"));

    // Absent machine-local override must be clearly distinguished:
    // It must appear in ABSENT MACHINE-LOCAL OVERRIDES and NOT imply repo restoration!
    assert!(
        output_str.contains("ABSENT MACHINE-LOCAL OVERRIDES (not created on this machine) (1):")
    );
    assert!(output_str.contains(".config/fish/conf.d/local.fish"));
    assert!(output_str.contains("intentionally local; no repository restoration required"));
}

#[cfg(target_os = "macos")]
#[test]
fn test_live_macos_scan_and_status_controlled_fixtures() {
    // Strengthen live macOS integration coverage with controlled fixtures and meaningful assertions.
    // Avoids inspecting the CI runner's actual home configuration.
    // Does not assert package counts that depend on the hosted image.
    let brew_path = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file());

    let brew_bin = match brew_path {
        Some(p) => p,
        None => {
            eprintln!("Skipping live macOS Homebrew test: no brew executable found");
            return;
        }
    };

    let temp_home = tempdir().unwrap();
    let temp_repo = tempdir().unwrap();

    // Create controlled manifest in temp_repo
    let manifest_path = temp_repo.path().join("controlled_manifest.toml");
    let manifest_content = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["git"]
casks = []

[[files]]
source = "sample.conf"
target = "~/.config/sample.conf"

[exclusions]
paths = []
patterns = ["*.backup.*"]
"#;
    fs::write(&manifest_path, manifest_content).unwrap();

    // Create sample.conf in temp_repo
    let sample_conf = temp_repo.path().join("sample.conf");
    fs::write(&sample_conf, "managed_key = true\n").unwrap();

    // 1. Live scan with controlled temp home and fixture manifest
    let mut scan_cmd = Command::cargo_bin("dotfilesync").unwrap();
    let scan_assert = scan_cmd
        .args([
            "scan",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--home",
            temp_home.path().to_str().unwrap(),
            "--repo-root",
            temp_repo.path().to_str().unwrap(),
            "--brew-bin",
            brew_bin.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let scan_out = String::from_utf8(scan_assert.get_output().stdout.clone()).unwrap();
    let scan_json: serde_json::Value =
        serde_json::from_str(&scan_out).expect("Should parse scan JSON");

    assert_eq!(scan_json["homebrew"]["available"], true);
    assert_eq!(scan_json["homebrew"]["packages_enumerated"], true);
    assert!(scan_json["homebrew"]["brew_version"].is_string());
    assert!(scan_json["homebrew"]["formulae"].is_array());
    assert!(scan_json["homebrew"]["casks"].is_array());
    // Only explicitly tracked configs from the controlled manifest inspected
    assert_eq!(scan_json["configs"].as_array().unwrap().len(), 1);

    // 2. Live status with controlled temp home and fixture manifest
    let mut status_cmd = Command::cargo_bin("dotfilesync").unwrap();
    let status_assert = status_cmd
        .args([
            "status",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--home",
            temp_home.path().to_str().unwrap(),
            "--repo-root",
            temp_repo.path().to_str().unwrap(),
            "--brew-bin",
            brew_bin.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();

    let status_out = String::from_utf8(status_assert.get_output().stdout.clone()).unwrap();
    let status_json: serde_json::Value =
        serde_json::from_str(&status_out).expect("Should parse status JSON");

    assert_eq!(status_json["homebrew_available"], true);
    assert_eq!(status_json["summary"]["packages_unknown"], 0);
    // Config target was not in temp_home, so exactly 1 missing config
    assert_eq!(status_json["summary"]["configs_missing"], 1);
}
