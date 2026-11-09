//! Accept loop and per-connection protocol handling.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dotfilesync_protocol::{
    current_euid, peer_euid, read_frame, write_frame, ClientHello, DaemonInfo, DataLayout,
    ErrorCode, FrameError, HelloAck, Notification, Outcome, ProtocolError, Request, RequestFrame,
    Response, ServerFrame, MAX_REQUEST_FRAME, MAX_RESPONSE_FRAME, PROTOCOL_VERSION,
    SUPPORTED_VERSIONS,
};

use crate::jobs::{JobLimits, JobManager};
use crate::journal::Journal;
use crate::listener::{acquire_instance_lock, bind_socket, BoundSocket, InstanceLock};
use crate::locks::ResourceLocks;
use crate::log::Logger;
use crate::DaemonError;

/// Source of the connecting peer's UID. Production uses [`OsPeerAuthenticator`]; tests may
/// inject a different implementation to simulate another user. Requests never carry a UID.
pub trait PeerAuthenticator: Send + Sync {
    fn peer_uid(&self, stream: &UnixStream) -> io::Result<u32>;
}

/// Kernel peer credentials (`getpeereid` / `SO_PEERCRED`).
pub struct OsPeerAuthenticator;

impl PeerAuthenticator for OsPeerAuthenticator {
    fn peer_uid(&self, stream: &UnixStream) -> io::Result<u32> {
        peer_euid(stream)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionLimits {
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub write_timeout: Duration,
    pub max_connections: usize,
    pub heartbeat_interval: Duration,
    pub shutdown_grace: Duration,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),
            write_timeout: Duration::from_secs(10),
            max_connections: 32,
            heartbeat_interval: Duration::from_secs(5),
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

pub struct DaemonConfig {
    pub layout: DataLayout,
    /// Homebrew binary chosen by the daemon owner at startup. Never taken from requests.
    pub brew_bin: Option<PathBuf>,
    pub authenticator: Arc<dyn PeerAuthenticator>,
    pub connection: ConnectionLimits,
    pub jobs: JobLimits,
    pub log_max_bytes: u64,
    pub log_keep: usize,
}

impl DaemonConfig {
    pub fn new(layout: DataLayout) -> Self {
        Self {
            layout,
            brew_bin: None,
            authenticator: Arc::new(OsPeerAuthenticator),
            connection: ConnectionLimits::default(),
            jobs: JobLimits::default(),
            log_max_bytes: 1024 * 1024,
            log_keep: 3,
        }
    }
}

/// Cloneable handle to request shutdown from another thread or a signal watcher.
#[derive(Clone)]
pub struct ShutdownHandle(Arc<AtomicBool>);

impl ShutdownHandle {
    pub fn shutdown(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_shutdown(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

struct Shared {
    jobs: Arc<JobManager>,
    log: Arc<Logger>,
    auth: Arc<dyn PeerAuthenticator>,
    owner_uid: u32,
    limits: ConnectionLimits,
    shutdown: ShutdownHandle,
    connections: AtomicUsize,
    started: Instant,
    data_dir: PathBuf,
}

pub struct Daemon {
    socket: BoundSocket,
    _lock: InstanceLock,
    shared: Arc<Shared>,
    pub recovered_interrupted: usize,
}

impl Daemon {
    /// Prepare directories, take the single-instance lock, recover the journal and bind.
    pub fn bind(config: DaemonConfig) -> Result<Self, DaemonError> {
        let layout = &config.layout;
        layout.ensure_private_dirs()?;
        let lock = acquire_instance_lock(&layout.lock_file)?;

        let log = Arc::new(
            Logger::open(&layout.log_file, config.log_max_bytes, config.log_keep)
                .map_err(|e| DaemonError::io(&layout.log_file, e))?,
        );
        let (journal, recovery) = Journal::open_and_recover(&layout.journal)
            .map_err(|e| DaemonError::io(&layout.journal, e))?;
        if recovery.corrupt_lines > 0 {
            log.warn(&format!(
                "journal: skipped {} corrupt line(s)",
                recovery.corrupt_lines
            ));
        }
        if recovery.newly_interrupted > 0 {
            log.warn(&format!(
                "journal: {} job(s) from the previous run were interrupted and will not be replayed",
                recovery.newly_interrupted
            ));
        }

        let socket = bind_socket(&layout.socket, &lock)?;
        socket
            .listener
            .set_nonblocking(true)
            .map_err(|e| DaemonError::io(&layout.socket, e))?;

        let owner_uid = current_euid();
        let jobs = JobManager::new(
            Arc::new(journal),
            Arc::clone(&log),
            ResourceLocks::new(),
            config.brew_bin.clone(),
            config.jobs.clone(),
            &recovery.jobs,
            crate::jobs::JobPaths {
                runs_dir: layout.runs_dir.clone(),
                backups_dir: layout.backups_dir.clone(),
                owner_uid,
            },
        );
        log.info(&format!(
            "dotfilesyncd {} started pid={} protocol={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            PROTOCOL_VERSION
        ));
        Ok(Self {
            socket,
            _lock: lock,
            shared: Arc::new(Shared {
                jobs,
                log,
                auth: Arc::clone(&config.authenticator),
                owner_uid,
                limits: config.connection.clone(),
                shutdown: ShutdownHandle(Arc::new(AtomicBool::new(false))),
                connections: AtomicUsize::new(0),
                started: Instant::now(),
                data_dir: layout.root.clone(),
            }),
            recovered_interrupted: recovery.newly_interrupted,
        })
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shared.shutdown.clone()
    }

    pub fn socket_path(&self) -> &std::path::Path {
        self.socket.path()
    }

    /// Serve until shutdown is requested, then cancel jobs, wait for the grace period, and
    /// remove the socket. The instance lock is released when `self` drops.
    pub fn run(self) -> Result<(), DaemonError> {
        let shared = &self.shared;
        while !shared.shutdown.is_shutdown() {
            match self.socket.listener.accept() {
                Ok((stream, _)) => {
                    if let Err(e) = stream.set_nonblocking(false) {
                        shared.log.warn(&format!("connection setup failed: {e}"));
                        continue;
                    }
                    let current = shared.connections.fetch_add(1, Ordering::SeqCst);
                    if current >= shared.limits.max_connections {
                        shared.connections.fetch_sub(1, Ordering::SeqCst);
                        reject(
                            stream,
                            &shared.limits,
                            ProtocolError::new(ErrorCode::Busy, "too many connections"),
                        );
                        continue;
                    }
                    let conn_shared = Arc::clone(shared);
                    let spawned = std::thread::Builder::new()
                        .name("dotfilesyncd-conn".into())
                        .spawn(move || {
                            handle_connection(&conn_shared, stream);
                            conn_shared.connections.fetch_sub(1, Ordering::SeqCst);
                        });
                    if spawned.is_err() {
                        shared.connections.fetch_sub(1, Ordering::SeqCst);
                        shared.log.error("failed to spawn connection thread");
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    shared.log.error(&format!("accept failed: {e}"));
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        shared
            .log
            .info("shutdown requested; cancelling active jobs");
        let unfinished = shared.jobs.shutdown(shared.limits.shutdown_grace);
        if unfinished > 0 {
            shared.log.warn(&format!(
                "{unfinished} job(s) did not stop within the grace period; they will be reported as interrupted"
            ));
        }
        shared.log.info("dotfilesyncd stopped");
        Ok(())
    }
}

fn send(stream: &mut UnixStream, frame: &ServerFrame) -> Result<(), FrameError> {
    let body = serde_json::to_vec(frame).map_err(|e| FrameError::Io(io::Error::other(e)))?;
    if body.len() > MAX_RESPONSE_FRAME {
        if let ServerFrame::Response { id, .. } = frame {
            let err = ServerFrame::Response {
                id: *id,
                outcome: Outcome::Err(ProtocolError::new(
                    ErrorCode::Internal,
                    "response exceeds the maximum frame size",
                )),
            };
            let body = serde_json::to_vec(&err).map_err(|e| FrameError::Io(io::Error::other(e)))?;
            return write_frame(stream, &body, MAX_RESPONSE_FRAME);
        }
    }
    write_frame(stream, &body, MAX_RESPONSE_FRAME)
}

fn reject(mut stream: UnixStream, limits: &ConnectionLimits, error: ProtocolError) {
    let _ = stream.set_write_timeout(Some(limits.write_timeout));
    let _ = send(&mut stream, &ServerFrame::Error { error });
}

fn handle_connection(shared: &Shared, mut stream: UnixStream) {
    // 1. Authenticate with kernel credentials before reading a single byte.
    match shared.auth.peer_uid(&stream) {
        Ok(uid) if uid == shared.owner_uid => {}
        Ok(uid) => {
            shared
                .log
                .warn(&format!("rejected connection from uid {uid}"));
            reject(
                stream,
                &shared.limits,
                ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "only the user that owns this daemon may connect",
                ),
            );
            return;
        }
        Err(e) => {
            shared
                .log
                .warn(&format!("could not read peer credentials: {e}"));
            return;
        }
    }

    let _ = stream.set_write_timeout(Some(shared.limits.write_timeout));
    let _ = stream.set_read_timeout(Some(shared.limits.handshake_timeout));

    // 2. Versioned handshake.
    let body = match read_frame(&mut stream, MAX_REQUEST_FRAME) {
        Ok(b) => b,
        Err(FrameError::TooLarge { len, max }) => {
            let _ = send(&mut stream, &too_large(len, max));
            return;
        }
        Err(_) => return,
    };
    let protocol_version = match serde_json::from_slice::<ClientHello>(&body) {
        Ok(ClientHello::Hello {
            protocol_version, ..
        }) => protocol_version,
        Err(_) => {
            let _ = send(
                &mut stream,
                &ServerFrame::Error {
                    error: ProtocolError::new(
                        ErrorCode::HandshakeRequired,
                        "the first frame must be a hello handshake",
                    ),
                },
            );
            return;
        }
    };
    if !SUPPORTED_VERSIONS.contains(&protocol_version) {
        let _ = send(
            &mut stream,
            &ServerFrame::Error {
                error: ProtocolError::new(
                    ErrorCode::IncompatibleVersion,
                    format!(
                        "client protocol version {protocol_version} is not supported; daemon supports {SUPPORTED_VERSIONS:?}"
                    ),
                )
                .with_details(serde_json::json!({
                    "supported": SUPPORTED_VERSIONS,
                    "requested": protocol_version,
                })),
            },
        );
        return;
    }
    let ack = ServerFrame::HelloAck(HelloAck {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
    });
    if send(&mut stream, &ack).is_err() {
        return;
    }

    // 3. Request loop.
    let _ = stream.set_read_timeout(Some(shared.limits.idle_timeout));
    loop {
        let body = match read_frame(&mut stream, MAX_REQUEST_FRAME) {
            Ok(b) => b,
            Err(FrameError::TooLarge { len, max }) => {
                // The oversized body was not read, so the stream cannot be resynchronised.
                let _ = send(&mut stream, &too_large(len, max));
                return;
            }
            Err(_) => return, // closed, timed out, or broken: the client's jobs keep running
        };
        let frame: RequestFrame = match serde_json::from_slice(&body) {
            Ok(f) => f,
            Err(e) => {
                let id = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_u64()));
                let error = ProtocolError::new(
                    ErrorCode::MalformedRequest,
                    format!("malformed request: {e}"),
                );
                let frame = match id {
                    Some(id) => ServerFrame::Response {
                        id,
                        outcome: Outcome::Err(error),
                    },
                    None => ServerFrame::Error { error },
                };
                let fatal = id.is_none();
                if send(&mut stream, &frame).is_err() || fatal {
                    return;
                }
                continue;
            }
        };
        let id = frame.id;
        let request = match Request::from_frame(&frame) {
            Ok(r) => r,
            Err(e) => {
                if send(
                    &mut stream,
                    &ServerFrame::Response {
                        id,
                        outcome: Outcome::Err(e),
                    },
                )
                .is_err()
                {
                    return;
                }
                continue;
            }
        };

        if let Request::WatchJob { job_id, since_seq } = &request {
            if watch(shared, &mut stream, id, job_id, *since_seq).is_err() {
                return;
            }
            continue;
        }

        let outcome = match dispatch(shared, request) {
            Ok(r) => Outcome::Ok(r),
            Err(e) => Outcome::Err(e),
        };
        if send(&mut stream, &ServerFrame::Response { id, outcome }).is_err() {
            return;
        }
    }
}

fn too_large(len: usize, max: usize) -> ServerFrame {
    ServerFrame::Error {
        error: ProtocolError::new(
            ErrorCode::FrameTooLarge,
            format!("frame of {len} bytes exceeds the {max}-byte request limit"),
        ),
    }
}

fn dispatch(shared: &Shared, request: Request) -> Result<Response, ProtocolError> {
    match request {
        Request::Ping {} => Ok(Response::Pong),
        Request::DaemonInfo {} => {
            let (active, retained) = shared.jobs.counts();
            Ok(Response::DaemonInfo(DaemonInfo {
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                protocol_version: PROTOCOL_VERSION,
                pid: std::process::id(),
                uptime_ms: shared.started.elapsed().as_millis() as u64,
                data_dir: shared.data_dir.clone(),
                active_jobs: active,
                retained_jobs: retained,
            }))
        }
        Request::StartJob { spec } => {
            if shared.shutdown.is_shutdown() {
                return Err(ProtocolError::new(
                    ErrorCode::ShuttingDown,
                    "daemon is shutting down",
                ));
            }
            shared.jobs.start(spec).map(Response::Job)
        }
        Request::JobStatus { job_id } => shared
            .jobs
            .get(&job_id)
            .map(|j| Response::Job(j.snapshot())),
        Request::ListJobs {} => Ok(Response::Jobs {
            jobs: shared.jobs.list(),
        }),
        Request::JobProgress { job_id, since_seq } => {
            let job = shared.jobs.get(&job_id)?;
            let (snapshot, events, truncated) = job.wait(since_seq, Duration::ZERO);
            Ok(Response::Progress {
                job: snapshot,
                events,
                truncated,
            })
        }
        Request::JobResult { job_id } => shared.jobs.result(&job_id).map(Response::JobResult),
        Request::CancelJob { job_id } => shared.jobs.cancel(&job_id).map(Response::Job),
        Request::PlanCreate { input } => shared
            .jobs
            .start_plan(input, shared.owner_uid)
            .map(Response::Job),
        Request::PlanApprove { plan_id, digest } => shared
            .jobs
            .approve(&plan_id, &digest, shared.owner_uid)
            .map(Response::Approval),
        Request::PlanApply {
            plan_id,
            token,
            allow_sudo,
        } => shared
            .jobs
            .apply(&plan_id, &token, allow_sudo, shared.owner_uid)
            .map(Response::Job),
        Request::PlanResume {
            run_id,
            token,
            allow_sudo,
        } => shared
            .jobs
            .resume(&run_id, &token, allow_sudo, shared.owner_uid)
            .map(Response::Job),
        Request::PlanRuns {} => Ok(Response::Runs {
            runs: shared.jobs.runs(),
        }),
        Request::PlanRunStatus { run_id } => shared.jobs.run_status(&run_id).map(Response::Run),
        Request::Shutdown {} => {
            shared.log.info("shutdown requested over IPC");
            shared.shutdown.shutdown();
            Ok(Response::ShuttingDown)
        }
        Request::WatchJob { .. } => unreachable!("handled by watch()"),
    }
}

fn watch(
    shared: &Shared,
    stream: &mut UnixStream,
    id: u64,
    job_id: &str,
    mut since: u64,
) -> Result<(), FrameError> {
    let job = match shared.jobs.get(job_id) {
        Ok(j) => j,
        Err(e) => {
            return send(
                stream,
                &ServerFrame::Response {
                    id,
                    outcome: Outcome::Err(e),
                },
            )
        }
    };
    loop {
        let (snapshot, events, _) = job.wait(since, shared.limits.heartbeat_interval);
        let had_events = !events.is_empty();
        for event in events {
            since = since.max(event.seq);
            send(
                stream,
                &ServerFrame::Notification {
                    id,
                    notification: Notification::Progress {
                        job_id: job_id.to_string(),
                        event,
                    },
                },
            )?;
        }
        if snapshot.status.is_terminal() && snapshot.latest_seq <= since {
            return send(
                stream,
                &ServerFrame::Response {
                    id,
                    outcome: Outcome::Ok(Response::Job(snapshot)),
                },
            );
        }
        if !had_events {
            // Also detects clients that went away: the write fails and the watch ends,
            // leaving the job itself untouched.
            send(
                stream,
                &ServerFrame::Notification {
                    id,
                    notification: Notification::Heartbeat,
                },
            )?;
        }
    }
}
