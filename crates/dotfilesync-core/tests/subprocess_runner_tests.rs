use dotfilesync_core::{CommandRunner, RealCommandRunner, RunnerError};
use std::time::Duration;

#[cfg(unix)]
#[test]
fn test_real_runner_large_stdout_does_not_deadlock() {
    let runner = RealCommandRunner::new(Duration::from_secs(5));
    // Generates ~150,000 bytes on stdout, well exceeding standard 64KB OS pipe capacity
    let res = runner.run(
        "/bin/sh",
        &["-c", "head -c 150000 /dev/zero | tr '\\0' 'A'"],
    );
    let out = res.expect("Should drain large stdout without pipe deadlock");
    assert_eq!(out.status_code, 0);
    assert_eq!(out.stdout.len(), 150000);
    assert!(out.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn test_real_runner_large_stderr_does_not_deadlock() {
    let runner = RealCommandRunner::new(Duration::from_secs(5));
    // Generates ~150,000 bytes on stderr
    let res = runner.run(
        "/bin/sh",
        &["-c", "head -c 150000 /dev/zero | tr '\\0' 'B' >&2"],
    );
    let out = res.expect("Should drain large stderr without pipe deadlock");
    assert_eq!(out.status_code, 0);
    assert_eq!(out.stderr.len(), 150000);
}

#[cfg(unix)]
#[test]
fn test_real_runner_nonzero_exit() {
    let runner = RealCommandRunner::new(Duration::from_secs(5));
    let res = runner.run("/bin/sh", &["-c", "echo 'failed step' >&2; exit 42"]);
    match res {
        Err(RunnerError::NonZeroExit { code, stderr, .. }) => {
            assert_eq!(code, 42);
            assert!(stderr.contains("failed step"));
        }
        other => panic!("Expected NonZeroExit error, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn test_real_runner_timeout() {
    // 500ms timeout on a command that sleeps for 3s
    let runner = RealCommandRunner::new(Duration::from_millis(500));
    let res = runner.run("/bin/sh", &["-c", "sleep 3"]);
    match res {
        Err(RunnerError::Timeout { .. }) => {}
        other => panic!("Expected Timeout error, got: {other:?}"),
    }
}
