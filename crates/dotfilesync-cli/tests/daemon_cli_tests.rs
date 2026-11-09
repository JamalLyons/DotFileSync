//! CLI behaviour with and without dotfilesyncd. Every test uses a temp data dir, a fake `brew`
//! on PATH, and (for `service`) a temp HOME plus a fake `launchctl`; the real user's daemon,
//! Homebrew, HOME and launchd are never touched.

use assert_cmd::Command;
use dotfilesync_daemon::{Daemon, DaemonConfig, ShutdownHandle};
use dotfilesync_protocol::DataLayout;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

const BREW: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "Homebrew 4.3.0"; exit 0; fi
if [ "$1" = "info" ]; then
  echo '{"formulae":[{"name":"git","installed":[{"version":"2.44.0"}]}],"casks":[{"token":"ghostty","version":"1.0.0"}]}'
  exit 0
fi
exit 1
"#;

struct Env {
    tmp: tempfile::TempDir,
    bin_dir: PathBuf,
    data: PathBuf,
    home: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::Builder::new().prefix("dfc").tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let brew = bin_dir.join("brew");
        fs::write(&brew, BREW).unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Self {
            data: tmp.path().join("d"),
            tmp,
            bin_dir,
            home,
        }
    }

    fn brew(&self) -> PathBuf {
        self.bin_dir.join("brew")
    }

    fn cli(&self) -> Command {
        let mut c = Command::cargo_bin("dotfilesync").unwrap();
        c.env("DOTFILESYNC_DATA_DIR", &self.data)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin_dir.display()))
            .env("HOME", &self.home)
            .env_remove("DOTFILESYNC_BREW_BIN")
            .env_remove("DOTFILESYNC_LAUNCHCTL")
            .current_dir(self.tmp.path());
        c
    }

    fn daemon(&self) -> TestDaemon {
        let mut cfg = DaemonConfig::new(DataLayout::new(&self.data));
        cfg.brew_bin = Some(self.brew());
        let d = Daemon::bind(cfg).unwrap();
        let handle = d.shutdown_handle();
        TestDaemon {
            handle,
            thread: Some(std::thread::spawn(move || d.run().unwrap())),
        }
    }
}

struct TestDaemon {
    handle: ShutdownHandle,
    thread: Option<JoinHandle<()>>,
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn stdout_of(c: &mut Command) -> String {
    let out = c.assert().success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

fn manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sample_manifest.toml")
        .canonicalize()
        .unwrap()
}

#[test]
fn auto_mode_without_daemon_falls_back_and_says_so() {
    let env = Env::new();
    let out = env
        .cli()
        .args(["scan", "--json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("dotfilesyncd is not running"))
        .stderr(predicate::str::contains("standalone"))
        .stderr(predicate::str::contains("dotfilesync service start"))
        .get_output()
        .stdout
        .clone();
    let auto: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(auto["homebrew"]["formulae"][0]["name"], "git");

    // Explicit standalone: same report, no note.
    let explicit = env
        .cli()
        .args(["scan", "--json", "--mode", "standalone"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty())
        .get_output()
        .stdout
        .clone();
    assert_eq!(out, explicit);
    // Checking for a daemon never creates the data directory or installs anything.
    assert!(!env.data.exists());
    assert!(!env.home.join("Library").exists());
}

#[test]
fn daemon_mode_without_daemon_explains_instead_of_falling_back() {
    let env = Env::new();
    env.cli()
        .args([
            "status",
            "--manifest",
            manifest().to_str().unwrap(),
            "--mode",
            "daemon",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dotfilesyncd is not running"))
        .stderr(predicate::str::contains("dotfilesync service install"));
    env.cli()
        .args(["jobs", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not running"));
}

#[test]
fn brew_override_is_standalone_only() {
    let env = Env::new();
    let brew = env.brew();
    env.cli()
        .args([
            "scan",
            "--mode",
            "daemon",
            "--brew-bin",
            brew.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("only applies to standalone mode"));
    // Even with a daemon running, auto mode honours the override by running standalone.
    let _d = env.daemon();
    env.cli()
        .args(["scan", "--json", "--brew-bin", brew.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("running scan standalone"));
}

#[test]
fn scan_and_status_through_daemon_match_standalone_output() {
    let env = Env::new();
    let m = manifest();
    let standalone_scan = stdout_of(env.cli().args([
        "scan",
        "--json",
        "--mode",
        "standalone",
        "--home",
        env.home.to_str().unwrap(),
    ]));
    let standalone_status = stdout_of(env.cli().args([
        "status",
        "--json",
        "--mode",
        "standalone",
        "--manifest",
        m.to_str().unwrap(),
        "--home",
        env.home.to_str().unwrap(),
    ]));

    let _d = env.daemon();
    let daemon_scan = env
        .cli()
        .args(["scan", "--json", "--home", env.home.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("as job j-"))
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(daemon_scan).unwrap(), standalone_scan);

    let daemon_status = stdout_of(env.cli().args([
        "status",
        "--json",
        "--mode",
        "daemon",
        "--manifest",
        m.to_str().unwrap(),
        "--home",
        env.home.to_str().unwrap(),
    ]));
    assert_eq!(daemon_status, standalone_status);

    // Human output streams progress on stderr and the report on stdout.
    env.cli()
        .args(["scan", "--mode", "daemon"])
        .assert()
        .success()
        .stderr(predicate::str::contains("[discovering_homebrew]"))
        .stderr(predicate::str::contains("Ctrl-C detaches"));
}

#[test]
fn status_manifest_error_is_the_same_in_both_modes() {
    let env = Env::new();
    env.cli()
        .args([
            "status",
            "--manifest",
            "missing.toml",
            "--mode",
            "standalone",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Failed to read manifest from 'missing.toml'",
        ));
    let _d = env.daemon();
    env.cli()
        .args(["status", "--manifest", "missing.toml", "--mode", "daemon"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Failed to read manifest from 'missing.toml'",
        ));
}

#[test]
fn detached_job_can_be_reconnected_from_another_invocation() {
    let env = Env::new();
    let _d = env.daemon();
    let id = stdout_of(env.cli().args(["scan", "--detach"]))
        .trim()
        .to_string();
    assert!(id.starts_with("j-"), "{id}");

    env.cli()
        .args(["jobs", "watch", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("succeeded"));
    let result = stdout_of(env.cli().args(["jobs", "result", &id, "--json"]));
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["homebrew"]["formulae"][0]["name"], "git");
    env.cli()
        .args(["jobs", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id));
    env.cli()
        .args(["jobs", "show", "j-doesnotexist"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no job with id"));
}

#[test]
fn import_brewfile_stays_local_and_refuses_overwrite() {
    let env = Env::new();
    let brewfile = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/Brewfile.standard");
    let out = env.tmp.path().join("new.toml");
    let args = [
        "import-brewfile",
        "--brewfile",
        brewfile.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
    ];
    env.cli().args(args).assert().success();
    let bytes = fs::read(&out).unwrap();
    env.cli()
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"));
    assert_eq!(fs::read(&out).unwrap(), bytes);
    assert!(!env.data.exists(), "import never involves the daemon");
}

// ---- service (fake launchctl, temp HOME) --------------------------------------------------

fn fake_launchctl(env: &Env) -> (PathBuf, PathBuf) {
    let log = env.tmp.path().join("launchctl.log");
    let script = env.tmp.path().join("fake-launchctl");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nif [ \"$1\" = print ]; then echo 'Could not find service' >&2; exit 113; fi\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

#[test]
fn service_install_status_uninstall_with_fake_launchctl() {
    let env = Env::new();
    let (launchctl, log) = fake_launchctl(&env);
    let daemon_bin = env.tmp.path().join("dotfilesyncd");
    fs::write(&daemon_bin, b"#!/bin/sh\n").unwrap();
    let svc = |args: &[&str]| {
        let mut c = env.cli();
        c.env("DOTFILESYNC_LAUNCHCTL", &launchctl)
            .arg("service")
            .args(args);
        c
    };

    svc(&["install", "--daemon-path", daemon_bin.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed:"))
        .stdout(predicate::str::contains("Start at login: disabled"))
        .stdout(predicate::str::contains("The daemon was not started"));
    let plist = env
        .home
        .join("Library/LaunchAgents/io.github.jamallyons.dotfilesyncd.plist");
    let content = fs::read_to_string(&plist).unwrap();
    // The non-default data dir is baked in so daemon and CLI agree.
    assert!(content.contains(env.data.to_str().unwrap()));
    assert!(!log.exists(), "install must not call launchctl");

    svc(&["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "installed: yes (does not start at login)",
        ))
        .stdout(predicate::str::contains("loaded in launchd: no"))
        .stdout(predicate::str::contains("Daemon: not running"));

    // Start asks launchd (fake) to load and kick the agent, then reports that nothing answers.
    svc(&["start"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not answering"));
    let calls = fs::read_to_string(&log).unwrap();
    assert!(calls.contains("bootstrap gui/"), "{calls}");
    assert!(calls.contains("kickstart gui/"), "{calls}");

    fs::create_dir_all(&env.data).unwrap();
    fs::write(env.data.join("keep-me"), b"x").unwrap();
    svc(&["uninstall"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed"))
        .stdout(predicate::str::contains("Kept daemon state and logs"));
    assert!(!plist.exists());
    assert!(
        env.data.join("keep-me").exists(),
        "uninstall keeps user data"
    );
}

#[test]
fn service_stop_shuts_down_a_daemon_not_managed_by_launchd() {
    let env = Env::new();
    let (launchctl, _log) = fake_launchctl(&env);
    let d = env.daemon();
    env.cli()
        .env("DOTFILESYNC_LAUNCHCTL", &launchctl)
        .args(["service", "stop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not managed by launchd"));
    drop(d);
    assert!(!DataLayout::new(&env.data).socket.exists());
}
