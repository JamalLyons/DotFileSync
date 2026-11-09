//! `plan` is stable and can run without a daemon. `apply` does not mutate unless asked,
//! and it refuses to apply when dotfilesyncd is not running.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const BREW: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "Homebrew 4.3.0"; exit 0; fi
if [ "$1" = "info" ]; then
  echo '{"formulae":[{"name":"git","installed":[{"version":"2.44.0"}]}],"casks":[]}'
  exit 0
fi
exit 0
"#;

struct Env {
    tmp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    manifest: PathBuf,
    target: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::Builder::new().prefix("dfp").tempdir().unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let brew = bin.join("brew");
        fs::write(&brew, BREW).unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("config.txt"), "desired\n").unwrap();
        let manifest = repo.join("dotfilesync.toml");
        fs::write(
            &manifest,
            "schema_version = 1\n\n[[files]]\nsource = \"config.txt\"\ntarget = \"~/.config/demo/config.txt\"\n",
        )
        .unwrap();
        let target = home.join(".config/demo/config.txt");
        Self {
            home,
            repo,
            manifest,
            target,
            tmp,
        }
    }

    fn cli(&self) -> Command {
        let mut cmd = Command::cargo_bin("dotfilesync").unwrap();
        cmd.env("DOTFILESYNC_DATA_DIR", self.tmp.path().join("d"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.tmp.path().join("bin").display()),
            )
            .env("HOME", &self.home)
            .env_remove("DOTFILESYNC_BREW_BIN")
            .current_dir(self.tmp.path());
        cmd
    }
}

#[test]
fn plan_json_is_identical_across_two_runs() {
    let env = Env::new();
    let args = [
        "plan",
        "--json",
        "--manifest",
        env.manifest.to_str().unwrap(),
        "--home",
        env.home.to_str().unwrap(),
        "--repo-root",
        env.repo.to_str().unwrap(),
        "--mode",
        "standalone",
    ];
    let first = env
        .cli()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second = env
        .cli()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(first, second);
    let plan: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert!(!plan["actions"].as_array().unwrap().is_empty());
}

#[test]
fn plan_human_format_lists_each_action() {
    let env = Env::new();
    env.cli()
        .args([
            "plan",
            "--manifest",
            env.manifest.to_str().unwrap(),
            "--home",
            env.home.to_str().unwrap(),
            "--repo-root",
            env.repo.to_str().unwrap(),
            "--mode",
            "standalone",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[low] [rev] [copy]"))
        .stdout(predicate::str::contains("action"));
}

#[test]
fn apply_without_daemon_errors() {
    let env = Env::new();
    env.cli()
        .args([
            "apply",
            "--yes",
            "--manifest",
            env.manifest.to_str().unwrap(),
            "--home",
            env.home.to_str().unwrap(),
            "--repo-root",
            env.repo.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dotfilesync service start"));
    assert!(!env.target.exists());
}

#[test]
fn apply_non_tty_without_yes_does_not_apply() {
    let env = Env::new();
    env.cli()
        .args([
            "apply",
            "--manifest",
            env.manifest.to_str().unwrap(),
            "--home",
            env.home.to_str().unwrap(),
            "--repo-root",
            env.repo.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run"))
        .stdout(predicate::str::contains("[low] [rev] [copy]"));
    assert!(
        !env.target.exists(),
        "non-TTY apply without --yes must not create the target"
    );
}
