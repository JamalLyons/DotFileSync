//! Journal recovery, log redaction/rotation, sensitive-data omission, and tests against the
//! real `dotfilesyncd` binary (no TCP listener, crash recovery, SIGTERM shutdown).

mod common;

use common::*;
use dotfilesync_core::ScanInput;
use dotfilesync_daemon::journal::{Journal, JournalEvent};
use dotfilesync_daemon::log::Logger;
use dotfilesync_protocol::{
    Client, ClientError, ErrorCode, JobKind, JobOutput, JobSpec, JobStatus,
};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

fn all_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(all_files(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn interrupted_jobs_are_reported_and_never_replayed() {
    let env = Env::new(Brew::Fast);
    env.layout.ensure_private_dirs().unwrap();
    {
        // Journal from a previous run: one finished job, one that never finished.
        let (j, _) = Journal::open_and_recover(&env.layout.journal).unwrap();
        j.append("j-done", JobKind::Scan, JournalEvent::Started)
            .unwrap();
        j.append("j-done", JobKind::Scan, JournalEvent::Succeeded)
            .unwrap();
        j.append("j-crashed", JobKind::Status, JournalEvent::Started)
            .unwrap();
    }
    // Plus a corrupt line, which must be skipped rather than abort startup.
    let mut raw_journal = fs::read_to_string(&env.layout.journal).unwrap();
    raw_journal.push_str("{not json\n");
    fs::write(&env.layout.journal, raw_journal).unwrap();

    let d = env.start();
    let mut c = env.client();
    let jobs = c.list_jobs().unwrap();
    let crashed = jobs.iter().find(|j| j.job_id == "j-crashed").unwrap();
    assert_eq!(crashed.status, JobStatus::Interrupted);
    assert!(crashed.from_previous_run);
    assert!(crashed
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("not resumed or replayed"));
    let done = jobs.iter().find(|j| j.job_id == "j-done").unwrap();
    assert_eq!(done.status, JobStatus::Succeeded);
    // Results are memory-only, so a previous run's success has no result to hand back.
    match c.job_result("j-crashed") {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::JobFailed),
        other => panic!("unexpected {other:?}"),
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(env.brew_runs(), 0, "recovery must not re-run work");
    drop(c);
    d.stop();

    // The interruption was journaled, so a second restart does not re-detect it.
    let journal = fs::read_to_string(&env.layout.journal).unwrap();
    assert!(journal.contains("\"interrupted\""));
    assert!(
        !journal.contains("{not json"),
        "compaction drops corrupt lines"
    );
    let log = fs::read_to_string(&env.layout.log_file).unwrap();
    assert!(log.contains("skipped 1 corrupt line"));
    assert!(log.contains("1 job(s) from the previous run were interrupted"));
}

#[test]
fn journal_is_compacted_to_a_bound() {
    let env = Env::new(Brew::Fast);
    env.layout.ensure_private_dirs().unwrap();
    {
        let (j, _) = Journal::open_and_recover(&env.layout.journal).unwrap();
        for i in 0..500 {
            let id = format!("j-{i}");
            j.append(&id, JobKind::Scan, JournalEvent::Started).unwrap();
            j.append(&id, JobKind::Scan, JournalEvent::Succeeded)
                .unwrap();
        }
    }
    let (_, report) = Journal::open_and_recover(&env.layout.journal).unwrap();
    assert_eq!(
        report.jobs.len(),
        dotfilesync_daemon::journal::JOURNAL_RETAIN
    );
    assert_eq!(report.jobs.last().unwrap().job_id, "j-499");
    let lines = fs::read_to_string(&env.layout.journal)
        .unwrap()
        .lines()
        .count();
    assert_eq!(lines, 2 * dotfilesync_daemon::journal::JOURNAL_RETAIN);
}

#[test]
fn log_lines_are_redacted_and_logs_rotate_within_bounds() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("d.log");
    let log = Logger::open(&path, 2048, 2).unwrap();
    log.info(
        "push with token ghp_abcdefghijklmnopqrstuvwxyz0123 to https://me:hunter2@github.com/x",
    );
    log.warn("password=hunter2 Authorization: Bearer sekrit-value");
    let text = fs::read_to_string(&path).unwrap();
    for secret in ["ghp_abcdef", "hunter2", "sekrit-value"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert!(text.contains("[REDACTED]"));
    // Control characters cannot forge extra log lines.
    log.info("line one\nFAKE ERROR injected");
    let last = fs::read_to_string(&path).unwrap();
    assert!(!last.lines().any(|l| l.starts_with("FAKE")));

    for i in 0..500 {
        log.info(&format!("filler line {i} {}", "x".repeat(40)));
    }
    let files: Vec<_> = fs::read_dir(tmp.path()).unwrap().flatten().collect();
    assert!(
        files.len() <= 3,
        "current + 2 rotated files, got {}",
        files.len()
    );
    for f in files {
        let meta = f.metadata().unwrap();
        assert!(meta.len() <= 2048, "{:?} is {} bytes", f.path(), meta.len());
        assert_eq!(mode_of(&f.path()), 0o600);
    }
}

#[test]
fn state_and_logs_omit_config_bodies_paths_and_secrets() {
    let env = Env::new(Brew::Fast);
    // A tracked config whose body contains secrets.
    let repo = env.tmp.path().join("repo");
    fs::create_dir_all(repo.join("cfg")).unwrap();
    let secret_body = "API_TOKEN=ghp_SENTINELSECRET0000000000000000 BODY_SENTINEL_42";
    fs::write(repo.join("cfg/app.conf"), secret_body).unwrap();
    fs::create_dir_all(env.home.join(".config")).unwrap();
    fs::write(env.home.join(".config/app.conf"), secret_body).unwrap();
    let manifest = repo.join("dotfilesync.toml");
    fs::write(
        &manifest,
        "schema_version = 1\n\n[[files]]\nsource = \"cfg/app.conf\"\ntarget = \"~/.config/app.conf\"\n",
    )
    .unwrap();

    let d = env.start();
    let mut c = env.client();
    let job = c
        .start_job(JobSpec::Scan(ScanInput {
            manifest: Some(manifest.clone()),
            home: env.home.clone(),
            repo_root: None,
        }))
        .unwrap();
    c.watch_job(&job.job_id, 0, &mut |_| {}).unwrap();
    let JobOutput::Scan { report } = c.job_result(&job.job_id).unwrap() else {
        panic!()
    };
    assert_eq!(report.configs.len(), 1, "{report:?}");
    drop(c);
    d.stop();

    for f in all_files(&env.layout.root) {
        let bytes = fs::read(&f).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        for needle in [
            "SENTINELSECRET",
            "BODY_SENTINEL_42",
            "app.conf",
            manifest.to_str().unwrap(),
            env.home.to_str().unwrap(),
        ] {
            assert!(
                !text.contains(needle),
                "{} contains {needle:?}",
                f.display()
            );
        }
    }
}

// ---- real binary ------------------------------------------------------------------------

fn spawn_daemon(env: &Env) -> ChildGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_dotfilesyncd"))
        .arg("--data-dir")
        .arg(&env.layout.root)
        .arg("--brew-bin")
        .arg(&env.brew)
        .env_remove("DOTFILESYNC_DATA_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let guard = ChildGuard(Some(child));
    assert!(
        wait_until(Duration::from_secs(10), || Client::connect(
            &env.layout,
            "t"
        )
        .is_ok()),
        "daemon binary did not come up"
    );
    guard
}

#[test]
fn daemon_binary_opens_no_tcp_or_udp_sockets() {
    let env = Env::new(Brew::Fast);
    let guard = spawn_daemon(&env);
    let pid = guard.pid().to_string();
    // Exercise it first so any lazily created sockets would exist.
    let mut c = env.client();
    let job = c
        .start_job(JobSpec::Scan(ScanInput {
            manifest: None,
            home: env.home.clone(),
            repo_root: None,
        }))
        .unwrap();
    c.watch_job(&job.job_id, 0, &mut |_| {}).unwrap();

    let inet = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid, "-i"])
        .output()
        .expect("lsof is available on macOS");
    let inet_out = String::from_utf8_lossy(&inet.stdout);
    assert!(
        inet_out.trim().is_empty(),
        "daemon has network sockets:\n{inet_out}"
    );
    // Sanity-check that lsof can see this process's sockets at all.
    let unix = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid, "-U"])
        .output()
        .unwrap();
    let unix_out = String::from_utf8_lossy(&unix.stdout);
    assert!(
        unix_out.contains("dotfilesyncd.sock"),
        "lsof did not list the Unix socket:\n{unix_out}"
    );
}

#[test]
fn crash_recovery_marks_running_job_interrupted_without_replay() {
    let env = Env::new(Brew::Hang);
    let mut guard = spawn_daemon(&env);
    let id = env
        .client()
        .start_job(JobSpec::Scan(ScanInput {
            manifest: None,
            home: env.home.clone(),
            repo_root: None,
        }))
        .unwrap()
        .job_id;
    let brew_pid = env.wait_for_brew_pid();
    let _brew_guard = PidGuard(brew_pid);

    // Simulate a crash: SIGKILL leaves the socket file and an unfinished journal entry.
    let mut child = guard.0.take().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(env.layout.socket.exists(), "stale socket left behind");
    assert!(Client::connect(&env.layout, "t")
        .err()
        .unwrap()
        .is_not_running());
    unsafe {
        libc::kill(brew_pid, libc::SIGKILL);
    }
    let runs_before = env.brew_runs();

    let _guard2 = spawn_daemon(&env);
    let mut c = env.client();
    let j = c.job_status(&id).unwrap();
    assert_eq!(j.status, JobStatus::Interrupted);
    assert!(j.from_previous_run);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        env.brew_runs(),
        runs_before,
        "interrupted work must not be replayed"
    );
}

#[test]
fn sigterm_shuts_down_gracefully_and_removes_socket() {
    let env = Env::new(Brew::Fast);
    let mut guard = spawn_daemon(&env);
    let mut child = guard.0.take().unwrap();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(!env.layout.socket.exists());
    // The lock is released: a new daemon can start immediately.
    let _again = spawn_daemon(&env);
}
