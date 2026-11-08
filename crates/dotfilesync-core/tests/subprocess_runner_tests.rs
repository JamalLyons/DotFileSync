use dotfilesync_core::{CommandRunner, RealCommandRunner, RunnerError};
use std::time::{Duration, Instant};

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
fn test_real_runner_timeout_reports_subsecond_duration() {
    let timeout = Duration::from_millis(400);
    let runner = RealCommandRunner::new(timeout);
    let start = Instant::now();
    let res = runner.run("/bin/sh", &["-c", "sleep 3"]);
    let elapsed = start.elapsed();

    // Bound elapsed time near deadline (not waiting for 3s sleep)
    assert!(
        elapsed >= Duration::from_millis(350),
        "Elapsed time too short: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(1800),
        "Elapsed time too long (descendant not terminated): {elapsed:?}"
    );

    match res {
        Err(RunnerError::Timeout {
            program,
            timeout: err_timeout,
        }) => {
            assert_eq!(program, "/bin/sh");
            assert_eq!(err_timeout, timeout);
            let msg = format!(
                "{}",
                RunnerError::Timeout {
                    program,
                    timeout: err_timeout
                }
            );
            // Must report subsecond timeout duration accurately rather than truncating to 0s
            assert!(
                msg.contains("400ms"),
                "Expected subsecond duration '400ms' in error message, got: '{msg}'"
            );
            assert!(
                !msg.contains("after 0s"),
                "Should not truncate timeout to 0s: '{msg}'"
            );
        }
        other => panic!("Expected Timeout error, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn test_real_runner_descendant_holding_pipes_open_terminates_at_deadline() {
    // Reproduction from review:
    // Subprocess spawns a descendant holding open stdout/stderr pipes.
    // Timeout cleanup must terminate both command and descendant within the supported
    // process group model, bounding output-reader cleanup.
    let timeout = Duration::from_millis(400);
    let runner = RealCommandRunner::new(timeout);
    let start = Instant::now();

    // Spawns descendant sleep 3 and waits for it, keeping stdout/stderr pipes open
    let res = runner.run("/bin/sh", &["-c", "sleep 3 & wait"]);
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(350),
        "Elapsed time too short: {elapsed:?}"
    );
    // If process group cleanup failed, elapsed would be >= 3000ms.
    // CI-safe bound: well under 1800ms
    assert!(
        elapsed < Duration::from_millis(1800),
        "Timeout cleanup blocked on descendant holding pipes open! Took {elapsed:?}"
    );

    match res {
        Err(RunnerError::Timeout { .. }) => {}
        other => panic!("Expected Timeout error, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn test_real_runner_background_descendant_holding_pipes_open() {
    // Test case where direct shell command exits immediately, but a background
    // descendant continues running and holding open stdout/stderr pipes.
    let timeout = Duration::from_millis(400);
    let runner = RealCommandRunner::new(timeout);
    let start = Instant::now();

    let res = runner.run("/bin/sh", &["-c", "(sleep 3 &) ; exit 0"]);
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(350),
        "Elapsed time too short: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(1800),
        "Full call must be bounded by deadline even when parent exits and descendant holds pipes: took {elapsed:?}"
    );

    match res {
        Err(RunnerError::Timeout { .. }) => {}
        other => panic!("Expected Timeout error, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn test_real_runner_non_utf8_executable_path() {
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    // Non-UTF-8 bytes (0xFF)
    let non_utf8_name = OsStr::from_bytes(b"mock_bin_\xff");
    let script_path = temp_dir.path().join(non_utf8_name);

    // On macOS APFS, file creation with invalid UTF-8 bytes returns EILSEQ.
    // On Linux filesystems, arbitrary bytes are permitted.
    match fs::write(&script_path, "#!/bin/sh\necho 'non-utf8 ok'\n") {
        Ok(()) => {
            // Platform supports creating non-UTF-8 file names (Linux)
            let mut perms = fs::metadata(&script_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap();

            let runner = RealCommandRunner::default();
            let out = runner
                .run_path(&script_path, &[])
                .expect("Should execute native non-UTF-8 path successfully");
            assert_eq!(out.status_code, 0);
            assert!(out.stdout.contains("non-utf8 ok"));
        }
        Err(e) if e.raw_os_error() == Some(92) || e.kind() == std::io::ErrorKind::InvalidInput => {
            // APFS rejected creation with EILSEQ (Errno 92: Illegal byte sequence).
            // Verify runner handles non-UTF-8 PathBuf natively without corrupting into lossy string
            let runner = RealCommandRunner::default();
            let res = runner.run_path(&script_path, &[]);
            match res {
                Err(RunnerError::ExecutableNotFound(p))
                | Err(RunnerError::ExecutionFailed { program: p, .. }) => {
                    // Path display must not crash
                    assert!(!p.is_empty());
                }
                other => panic!("Expected ExecutableNotFound or ExecutionFailed, got: {other:?}"),
            }
        }
        Err(other) => panic!("Unexpected filesystem error: {other}"),
    }
}
