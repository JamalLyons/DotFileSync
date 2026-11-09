//! Job lifecycle: real scan/status via core, progress, cancellation, disconnect survival,
//! reconnect, responsiveness and shutdown.

mod common;

use common::*;
use dotfilesync_core::{run_scan, RealCommandRunner, ScanInput, StatusInput};
use dotfilesync_protocol::{
    ClientError, ErrorCode, JobErrorCode, JobKind, JobOutput, JobSpec, JobStatus,
};
use std::fs;
use std::time::{Duration, Instant};

fn scan_spec(env: &Env) -> JobSpec {
    JobSpec::Scan(ScanInput {
        manifest: None,
        home: env.home.clone(),
        repo_root: None,
    })
}

fn wait_status(env: &Env, id: &str, want: JobStatus) -> dotfilesync_protocol::JobSnapshot {
    let mut c = env.client();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let j = c.job_status(id).unwrap();
        if j.status == want {
            return j;
        }
        assert!(Instant::now() < deadline, "job {id} stuck in {}", j.status);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn scan_job_matches_standalone_core_output() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut c = env.client();
    let job = c.start_job(scan_spec(&env)).unwrap();
    assert_eq!(job.kind, JobKind::Scan);
    let done = c.watch_job(&job.job_id, 0, &mut |_| {}).unwrap();
    assert_eq!(done.status, JobStatus::Succeeded);
    let JobOutput::Scan { report } = c.job_result(&job.job_id).unwrap() else {
        panic!("wrong kind")
    };

    let runner = RealCommandRunner::default().with_brew_binary(env.brew.clone());
    let standalone = run_scan(
        &ScanInput {
            manifest: None,
            home: env.home.clone(),
            repo_root: None,
        },
        &runner,
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    assert_eq!(
        report, standalone,
        "daemon and standalone share one implementation"
    );
    assert!(report.homebrew.formulae.iter().any(|p| p.name == "git"));
}

#[test]
fn status_job_with_fixture_manifest_and_manifest_failure() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut c = env.client();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sample_manifest.toml")
        .canonicalize()
        .unwrap();
    let job = c
        .start_job(JobSpec::Status(StatusInput {
            manifest,
            home: env.home.clone(),
            repo_root: None,
        }))
        .unwrap();
    c.watch_job(&job.job_id, 0, &mut |_| {}).unwrap();
    assert!(matches!(
        c.job_result(&job.job_id).unwrap(),
        JobOutput::Status { .. }
    ));

    let bad = c
        .start_job(JobSpec::Status(StatusInput {
            manifest: env.tmp.path().join("missing.toml"),
            home: env.home.clone(),
            repo_root: None,
        }))
        .unwrap();
    let done = c.watch_job(&bad.job_id, 0, &mut |_| {}).unwrap();
    assert_eq!(done.status, JobStatus::Failed);
    assert_eq!(done.error.unwrap().code, JobErrorCode::ManifestLoad);
    match c.job_result(&bad.job_id) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::JobFailed),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn progress_events_are_ordered_and_streamed() {
    let env = Env::new(Brew::Slow(0.3));
    let _d = env.start();
    let mut c = env.client();
    let job = c.start_job(scan_spec(&env)).unwrap();
    let mut seen = Vec::new();
    let done = c
        .watch_job(&job.job_id, 0, &mut |e| seen.push(e.clone()))
        .unwrap();
    assert_eq!(done.status, JobStatus::Succeeded);
    let stages: Vec<_> = seen.iter().map(|e| e.stage.as_str()).collect();
    assert_eq!(stages, ["acquiring_locks", "discovering_homebrew", "done"]);
    assert!(seen.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(done.latest_seq, seen.last().unwrap().seq);
}

#[test]
fn job_survives_client_disconnect_and_reconnect_sees_progress_and_result() {
    let env = Env::new(Brew::Slow(1.0));
    let _d = env.start();
    let id = {
        let mut c = env.client();
        let job = c.start_job(scan_spec(&env)).unwrap();
        // Start watching, then vanish mid-watch (drop the connection).
        job.job_id
    };
    // A brand-new connection sees the same job still running.
    let mut c2 = env.client();
    let j = c2.job_status(&id).unwrap();
    assert!(
        !j.status.is_terminal(),
        "job must not be cancelled by disconnect"
    );
    assert!(!j.cancel_requested);

    // Catch up on progress from seq 0, then resume watching from the last seen seq.
    let (_, missed, truncated) = c2.job_progress(&id, 0).unwrap();
    assert!(!truncated);
    assert!(missed.iter().any(|e| e.stage == "acquiring_locks"));
    let last = missed.last().unwrap().seq;
    let mut later = Vec::new();
    let done = c2.watch_job(&id, last, &mut |e| later.push(e.seq)).unwrap();
    assert_eq!(done.status, JobStatus::Succeeded);
    assert!(
        later.iter().all(|s| *s > last),
        "no duplicate events after reconnect"
    );
    assert!(matches!(
        c2.job_result(&id).unwrap(),
        JobOutput::Scan { .. }
    ));
}

#[test]
fn watcher_disconnect_mid_stream_does_not_cancel() {
    let env = Env::new(Brew::Slow(0.8));
    let _d = env.start();
    let id = env.client().start_job(scan_spec(&env)).unwrap().job_id;
    {
        // A raw client subscribes to the progress stream, reads one notification, and
        // disappears while the daemon is still streaming to it.
        let mut s = raw(&env.layout);
        hello(&mut s);
        send_json(
            &mut s,
            &serde_json::json!({"type":"request","id":1,"op":"watch_job","params":{"job_id":id,"since_seq":0}}),
        );
        assert!(matches!(
            recv(&mut s),
            dotfilesync_protocol::ServerFrame::Notification { id: 1, .. }
        ));
    }
    let j = wait_status(&env, &id, JobStatus::Succeeded);
    assert!(!j.cancel_requested);
    env.client().ping().unwrap();
}

#[test]
fn cancellation_stops_job_and_kills_its_subprocess() {
    let env = Env::new(Brew::Hang);
    let _d = env.start();
    let mut c = env.client();
    let job = c.start_job(scan_spec(&env)).unwrap();
    let brew_pid = env.wait_for_brew_pid();
    let _guard = PidGuard(brew_pid);
    assert!(process_alive(brew_pid));

    let snap = c.cancel_job(&job.job_id).unwrap();
    assert!(snap.cancel_requested);
    let t = Instant::now();
    let done = wait_status(&env, &job.job_id, JobStatus::Cancelled);
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "cancel must not wait for the subprocess"
    );
    assert_eq!(done.error.unwrap().code, JobErrorCode::Cancelled);
    assert!(
        wait_until(Duration::from_secs(3), || !process_alive(brew_pid)),
        "the runner must kill the subprocess it owns"
    );
    // No partial result is reported for a cancelled job.
    match c.job_result(&job.job_id) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::JobFailed),
        other => panic!("unexpected {other:?}"),
    }
    // Cancelling again is idempotent.
    assert_eq!(
        c.cancel_job(&job.job_id).unwrap().status,
        JobStatus::Cancelled
    );
}

#[test]
fn slow_discovery_does_not_block_other_clients() {
    let env = Env::new(Brew::Hang);
    let _d = env.start();
    let id = env.client().start_job(scan_spec(&env)).unwrap().job_id;
    let _guard = PidGuard(env.wait_for_brew_pid());
    for _ in 0..5 {
        let t = Instant::now();
        let mut c = env.client();
        c.ping().unwrap();
        c.list_jobs().unwrap();
        assert!(t.elapsed() < Duration::from_millis(500));
    }
    env.client().cancel_job(&id).unwrap();
}

#[test]
fn unknown_job_and_unfinished_result_are_typed_errors() {
    let env = Env::new(Brew::Hang);
    let _d = env.start();
    let mut c = env.client();
    match c.job_status("j-nope") {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::JobNotFound),
        other => panic!("unexpected {other:?}"),
    }
    let id = c.start_job(scan_spec(&env)).unwrap().job_id;
    let _guard = PidGuard(env.wait_for_brew_pid());
    match c.job_result(&id) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::JobNotFinished),
        other => panic!("unexpected {other:?}"),
    }
    c.cancel_job(&id).unwrap();
}

#[test]
fn active_job_limit_returns_busy() {
    let env = Env::new(Brew::Hang);
    let mut cfg = env.config();
    cfg.jobs.max_active = 1;
    let _d = Running::start(cfg);
    let mut c = env.client();
    let id = c.start_job(scan_spec(&env)).unwrap().job_id;
    let _guard = PidGuard(env.wait_for_brew_pid());
    match c.start_job(scan_spec(&env)) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::Busy),
        other => panic!("unexpected {other:?}"),
    }
    c.cancel_job(&id).unwrap();
}

#[test]
fn shutdown_cancels_running_jobs_and_cleans_up_subprocesses() {
    let env = Env::new(Brew::Hang);
    let d = env.start();
    let mut c = env.client();
    let id = c.start_job(scan_spec(&env)).unwrap().job_id;
    let brew_pid = env.wait_for_brew_pid();
    let _guard = PidGuard(brew_pid);
    c.shutdown().unwrap();
    d.stop();
    assert!(
        !process_alive(brew_pid),
        "shutdown must stop job subprocesses"
    );
    assert!(!env.layout.socket.exists());
    // The journal records the job as cancelled, so a restart does not call it interrupted.
    let journal = fs::read_to_string(&env.layout.journal).unwrap();
    assert!(
        journal.contains(&id) && journal.contains("\"cancelled\""),
        "{journal}"
    );
}

#[test]
fn start_job_after_shutdown_request_is_refused() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut c = env.client();
    c.shutdown().unwrap();
    // The connection may still be served briefly; new work must not be accepted.
    match c.start_job(scan_spec(&env)) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, ErrorCode::ShuttingDown),
        Err(_) => {} // connection already closed by shutdown is also acceptable
        Ok(j) => panic!("job {} accepted during shutdown", j.job_id),
    }
}
