#![allow(dead_code)]
//! Shared fixtures for daemon integration tests. Everything lives in a temp dir; nothing
//! touches the real HOME, real Homebrew, or launchd.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dotfilesync_daemon::{Daemon, DaemonConfig, PeerAuthenticator, ShutdownHandle};
use dotfilesync_protocol::{
    read_frame, write_frame, Client, DataLayout, ServerFrame, MAX_RESPONSE_FRAME,
};
use tempfile::TempDir;

pub const BREW_JSON: &str = r#"{"formulae":[{"name":"git","installed":[{"version":"2.44.0"}]}],"casks":[{"token":"ghostty","version":"1.0.0"}]}"#;

/// Behaviour of the fake `brew` executable.
#[derive(Clone, Copy)]
pub enum Brew {
    /// Answers immediately.
    Fast,
    /// Sleeps before answering `info`, then succeeds.
    Slow(f32),
    /// Records its pid and hangs on `info` (for cancellation/crash tests).
    Hang,
}

pub struct Env {
    pub tmp: TempDir,
    pub layout: DataLayout,
    pub brew: PathBuf,
    pub pid_file: PathBuf,
    pub runs_file: PathBuf,
    pub home: PathBuf,
}

impl Env {
    pub fn new(brew: Brew) -> Self {
        let tmp = tempfile::Builder::new().prefix("dfs").tempdir().unwrap();
        let layout = DataLayout::new(tmp.path().join("d"));
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let pid_file = tmp.path().join("brew.pid");
        let runs_file = tmp.path().join("brew.runs");
        let brew = write_brew(tmp.path(), brew, &pid_file, &runs_file);
        Self {
            tmp,
            layout,
            brew,
            pid_file,
            runs_file,
            home,
        }
    }

    pub fn config(&self) -> DaemonConfig {
        let mut cfg = DaemonConfig::new(self.layout.clone());
        cfg.brew_bin = Some(self.brew.clone());
        cfg.connection.handshake_timeout = Duration::from_millis(500);
        cfg.connection.heartbeat_interval = Duration::from_millis(200);
        cfg.connection.shutdown_grace = Duration::from_secs(3);
        cfg
    }

    pub fn start(&self) -> Running {
        Running::start(self.config())
    }

    pub fn client(&self) -> Client {
        Client::connect(&self.layout, "test").expect("connect")
    }

    /// Number of times the fake brew ran `info` (proves whether work was executed/replayed).
    pub fn brew_runs(&self) -> usize {
        fs::read_to_string(&self.runs_file)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    pub fn wait_for_brew_pid(&self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = fs::read_to_string(&self.pid_file) {
                if let Ok(pid) = s.trim().parse() {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "fake brew never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn write_brew(dir: &Path, mode: Brew, pid_file: &Path, runs_file: &Path) -> PathBuf {
    let info = match mode {
        Brew::Fast => format!("echo '{BREW_JSON}'"),
        Brew::Slow(secs) => format!("sleep {secs}\n    echo '{BREW_JSON}'"),
        Brew::Hang => format!("echo $$ > '{}'\n    exec sleep 30", pid_file.display()),
    };
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "Homebrew 4.3.0"
    exit 0
fi
if [ "$1" = "info" ]; then
    echo run >> '{runs}'
    {info}
    exit 0
fi
if [ "$1" = "list" ]; then
    exit 0
fi
exit 1
"#,
        runs = runs_file.display()
    );
    let path = dir.join("brew");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// In-process daemon on a background thread; shut down and joined on drop.
pub struct Running {
    pub handle: ShutdownHandle,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    pub fn start(cfg: DaemonConfig) -> Self {
        let daemon = Daemon::bind(cfg).expect("daemon bind");
        let handle = daemon.shutdown_handle();
        let thread = std::thread::spawn(move || daemon.run().expect("daemon run"));
        Self {
            handle,
            thread: Some(thread),
        }
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.handle.shutdown();
        if let Some(t) = self.thread.take() {
            t.join().expect("daemon thread");
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Authenticator that reports a fixed UID, simulating a peer owned by another user.
pub struct FixedUid(pub u32);

impl PeerAuthenticator for FixedUid {
    fn peer_uid(&self, _: &UnixStream) -> io::Result<u32> {
        Ok(self.0)
    }
}

pub fn with_auth(mut cfg: DaemonConfig, auth: impl PeerAuthenticator + 'static) -> DaemonConfig {
    cfg.authenticator = Arc::new(auth);
    cfg
}

// ---- raw wire helpers (for malformed/hostile client tests) --------------------------------

pub fn raw(layout: &DataLayout) -> UnixStream {
    let s = UnixStream::connect(&layout.socket).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s
}

pub fn send_json(s: &mut UnixStream, v: &serde_json::Value) {
    write_frame(s, &serde_json::to_vec(v).unwrap(), usize::MAX).unwrap();
}

pub fn recv(s: &mut UnixStream) -> ServerFrame {
    let body = read_frame(s, MAX_RESPONSE_FRAME).expect("frame");
    serde_json::from_slice(&body).expect("server frame")
}

pub fn hello(s: &mut UnixStream) {
    send_json(
        s,
        &serde_json::json!({"type":"hello","protocol_version":1,"client":"raw"}),
    );
    assert!(matches!(recv(s), ServerFrame::HelloAck(_)));
}

/// True if the peer closed the connection (EOF or reset) within the read timeout.
pub fn is_closed(s: &mut UnixStream) -> bool {
    use std::io::Read;
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => matches!(
            e.kind(),
            io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
        ),
    }
}

pub fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence/permission.
    unsafe { libc::kill(pid, 0) == 0 }
}

pub fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

pub fn mode_of(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

/// Kills a spawned process (and reaps it) even if a test assertion fails.
pub struct ChildGuard(pub Option<std::process::Child>);

impl ChildGuard {
    pub fn pid(&self) -> i32 {
        self.0.as_ref().unwrap().id() as i32
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Kills a stray pid on drop (e.g. a fake brew orphaned by a SIGKILLed daemon).
pub struct PidGuard(pub i32);

impl Drop for PidGuard {
    fn drop(&mut self) {
        if self.0 > 0 {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
            }
        }
    }
}
