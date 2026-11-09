//! launchd LaunchAgent lifecycle via an injected recording runner and a temp LaunchAgents dir.
//! Nothing here calls the real `launchctl` or touches the real ~/Library/LaunchAgents.

use dotfilesync_core::{CommandOutput, CommandRunner, RunnerError};
use dotfilesync_daemon::service::launchd::{LaunchdAgent, LABEL};
use dotfilesync_daemon::service::{
    InstallOptions, InstallOutcome, ServiceError, ServiceManager, StopOutcome,
};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const UID: u32 = 4242;

/// Fake launchctl: records every call and simulates whether the agent is loaded.
#[derive(Default)]
struct FakeLaunchctl {
    calls: Mutex<Vec<Vec<String>>>,
    loaded: Mutex<bool>,
}

impl FakeLaunchctl {
    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.join(" "))
            .collect()
    }
    fn set_loaded(&self, v: bool) {
        *self.loaded.lock().unwrap() = v;
    }
}

impl CommandRunner for FakeLaunchctl {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RunnerError> {
        assert_eq!(program, "/bin/launchctl", "only launchctl may be invoked");
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.calls.lock().unwrap().push(args.clone());
        let ok = |stdout: &str| {
            Ok(CommandOutput {
                status_code: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        };
        let mut loaded = self.loaded.lock().unwrap();
        match args[0].as_str() {
            "print" if *loaded => ok(&format!(
                "gui/{UID}/{LABEL} = {{\n\tstate = running\n\tpid = 777\n}}\n"
            )),
            "print" => Err(RunnerError::NonZeroExit {
                program: program.into(),
                code: 113,
                stderr: "Could not find service".into(),
            }),
            "bootstrap" => {
                *loaded = true;
                ok("")
            }
            "bootout" => {
                *loaded = false;
                ok("")
            }
            "kickstart" => ok(""),
            other => panic!("unexpected launchctl subcommand {other}"),
        }
    }
    fn run_path(&self, _: &Path, _: &[&OsStr]) -> Result<CommandOutput, RunnerError> {
        unreachable!("launchd agent uses run()")
    }
    fn find_executable(&self, _: &str) -> Option<PathBuf> {
        None
    }
}

struct Fixture {
    tmp: tempfile::TempDir,
    fake: Arc<FakeLaunchctl>,
    agent: LaunchdAgent,
    daemon: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeLaunchctl::default());
    let agent = LaunchdAgent::new(&tmp.path().join("LaunchAgents"), UID, fake.clone());
    let daemon = tmp.path().join("bin/dotfilesyncd");
    fs::create_dir_all(daemon.parent().unwrap()).unwrap();
    fs::write(&daemon, b"#!/bin/sh\n").unwrap();
    Fixture {
        tmp,
        fake,
        agent,
        daemon,
    }
}

fn opts(f: &Fixture, start_at_login: bool) -> InstallOptions {
    InstallOptions {
        daemon_path: f.daemon.clone(),
        start_at_login,
        data_dir: None,
        force: false,
    }
}

#[test]
fn install_writes_plist_but_never_loads_or_starts() {
    let f = fixture();
    assert_eq!(
        f.agent.install(&opts(&f, false)).unwrap(),
        InstallOutcome::Installed
    );
    assert!(f.fake.calls().is_empty(), "install must not call launchctl");
    let plist = fs::read_to_string(f.agent.plist_path()).unwrap();
    assert!(plist.contains(&format!("<string>{LABEL}</string>")));
    assert!(plist.contains(&format!("<string>{}</string>", f.daemon.display())));
    let compact: String = plist.split_whitespace().collect();
    assert!(
        compact.contains("<key>RunAtLoad</key><false/>"),
        "login start is opt-in"
    );
    assert!(
        !plist.contains("KeepAlive"),
        "KeepAlive would imply RunAtLoad"
    );
    assert!(!plist.contains("UserName") && !plist.contains("LaunchDaemons"));

    let st = f.agent.status().unwrap();
    assert!(st.installed && !st.loaded && !st.running);
    assert_eq!(st.start_at_login, Some(false));
}

#[test]
fn start_at_login_is_explicit_and_reinstall_is_idempotent() {
    let f = fixture();
    f.agent.install(&opts(&f, true)).unwrap();
    assert_eq!(f.agent.status().unwrap().start_at_login, Some(true));
    assert_eq!(
        f.agent.install(&opts(&f, true)).unwrap(),
        InstallOutcome::Unchanged
    );
    // Changing the definition requires --force.
    assert!(matches!(
        f.agent.install(&opts(&f, false)),
        Err(ServiceError::DefinitionExists { .. })
    ));
    let mut o = opts(&f, false);
    o.force = true;
    assert_eq!(f.agent.install(&o).unwrap(), InstallOutcome::Replaced);
    assert_eq!(f.agent.status().unwrap().start_at_login, Some(false));
}

#[test]
fn install_rejects_bad_daemon_paths_and_escapes_xml() {
    let f = fixture();
    let mut o = opts(&f, false);
    o.daemon_path = PathBuf::from("relative/dotfilesyncd");
    assert!(matches!(
        f.agent.install(&o),
        Err(ServiceError::InvalidDaemonPath { .. })
    ));
    o.daemon_path = f.tmp.path().join("missing");
    assert!(matches!(
        f.agent.install(&o),
        Err(ServiceError::InvalidDaemonPath { .. })
    ));

    let weird = f.tmp.path().join("a&b<c>");
    fs::write(&weird, b"x").unwrap();
    o.daemon_path = weird;
    o.data_dir = Some(f.tmp.path().join("data dir"));
    f.agent.install(&o).unwrap();
    let plist = fs::read_to_string(f.agent.plist_path()).unwrap();
    assert!(plist.contains("a&amp;b&lt;c&gt;"));
    assert!(plist.contains("<string>--data-dir</string>"));
}

#[test]
fn start_requires_install_then_bootstraps_and_kickstarts() {
    let f = fixture();
    assert!(matches!(
        f.agent.start(),
        Err(ServiceError::NotInstalled { .. })
    ));
    assert!(f.fake.calls().iter().all(|c| !c.starts_with("bootstrap")));

    f.agent.install(&opts(&f, false)).unwrap();
    f.agent.start().unwrap();
    let plist = f.agent.plist_path().display().to_string();
    assert_eq!(
        f.fake.calls(),
        vec![
            format!("print gui/{UID}/{LABEL}"),
            format!("bootstrap gui/{UID} {plist}"),
            format!("kickstart gui/{UID}/{LABEL}"),
        ]
    );
    // Already loaded: only kickstart.
    f.agent.start().unwrap();
    assert_eq!(
        f.fake.calls()[3..],
        [
            format!("print gui/{UID}/{LABEL}"),
            format!("kickstart gui/{UID}/{LABEL}")
        ]
    );
    let st = f.agent.status().unwrap();
    assert!(st.loaded && st.running);
    assert_eq!(st.pid, Some(777));
}

#[test]
fn stop_unloads_only_when_loaded() {
    let f = fixture();
    f.agent.install(&opts(&f, false)).unwrap();
    assert_eq!(f.agent.stop().unwrap(), StopOutcome::NotLoaded);
    f.fake.set_loaded(true);
    assert_eq!(f.agent.stop().unwrap(), StopOutcome::Unloaded);
    assert!(f
        .fake
        .calls()
        .contains(&format!("bootout gui/{UID}/{LABEL}")));
    assert!(
        f.agent.plist_path().exists(),
        "stop keeps the definition installed"
    );
}

#[test]
fn uninstall_unloads_and_removes_only_our_definition_and_keeps_user_data() {
    let f = fixture();
    let data = f.tmp.path().join("data");
    let configs = f.tmp.path().join("my-configs");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&configs).unwrap();
    fs::write(data.join("state.jsonl"), b"keep").unwrap();
    fs::write(configs.join("config.fish"), b"keep").unwrap();

    f.agent.install(&opts(&f, true)).unwrap();
    f.fake.set_loaded(true);
    let out = f.agent.uninstall().unwrap();
    assert!(out.unloaded && out.removed_definition);
    assert!(!f.agent.plist_path().exists());
    assert_eq!(fs::read(data.join("state.jsonl")).unwrap(), b"keep");
    assert_eq!(fs::read(configs.join("config.fish")).unwrap(), b"keep");

    // Nothing installed: harmless no-op.
    let out = f.agent.uninstall().unwrap();
    assert!(!out.unloaded && !out.removed_definition);
}

#[test]
fn foreign_or_symlinked_definition_is_never_modified() {
    let f = fixture();
    fs::create_dir_all(f.agent.plist_path().parent().unwrap()).unwrap();
    fs::write(f.agent.plist_path(), b"<plist>someone else's</plist>").unwrap();
    assert!(matches!(
        f.agent.uninstall(),
        Err(ServiceError::NotOurs { .. })
    ));
    assert!(matches!(f.agent.start(), Err(ServiceError::NotOurs { .. })));
    assert_eq!(
        fs::read(f.agent.plist_path()).unwrap(),
        b"<plist>someone else's</plist>"
    );
    assert!(
        f.fake.calls().is_empty(),
        "no launchctl calls for a foreign definition"
    );

    fs::remove_file(f.agent.plist_path()).unwrap();
    let target = f.tmp.path().join("elsewhere.plist");
    fs::write(&target, format!("<string>{LABEL}</string>")).unwrap();
    std::os::unix::fs::symlink(&target, f.agent.plist_path()).unwrap();
    assert!(matches!(
        f.agent.uninstall(),
        Err(ServiceError::NotOurs { .. })
    ));
    assert!(target.exists());
}
