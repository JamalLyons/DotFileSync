//! Blocking client for `dotfilesyncd`, shared by the CLI and future GUI.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

use crate::frame::{read_frame, write_frame, FrameError};
use crate::messages::*;
use crate::paths::{check_private_dir, DataLayout, DirState, PathError};
use crate::peer::{current_euid, peer_euid};
use crate::{MAX_REQUEST_FRAME, MAX_RESPONSE_FRAME, PROTOCOL_VERSION};

#[derive(Debug, Error)]
pub enum ClientError {
    /// No daemon is listening (no data dir, no socket, or connection refused).
    #[error("dotfilesyncd is not running (socket {}: {reason})", socket.display())]
    NotRunning { socket: PathBuf, reason: String },
    #[error("refusing to connect: {0}")]
    InsecureDataDir(#[from] PathError),
    #[error("refusing to talk to a daemon owned by uid {daemon_uid}; this client runs as uid {expected}")]
    ImpostorDaemon { daemon_uid: u32, expected: u32 },
    #[error("daemon speaks protocol {supported:?}, this client speaks {requested}; upgrade so client and daemon versions match")]
    IncompatibleVersion { supported: Vec<u32>, requested: u32 },
    #[error("daemon rejected this connection: {0}")]
    Unauthorized(String),
    #[error("daemon returned an error: {0}")]
    Remote(ProtocolError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("connection error: {0}")]
    Io(#[from] io::Error),
}

impl ClientError {
    /// True when falling back to standalone execution is appropriate.
    pub fn is_not_running(&self) -> bool {
        matches!(self, ClientError::NotRunning { .. })
    }
}

impl From<FrameError> for ClientError {
    fn from(e: FrameError) -> Self {
        match e {
            FrameError::Closed => ClientError::Protocol("daemon closed the connection".into()),
            FrameError::TooLarge { len, max } => {
                ClientError::Protocol(format!("daemon frame of {len} bytes exceeds {max}"))
            }
            FrameError::Io(e) => ClientError::Io(e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub client_name: String,
    pub protocol_version: u32,
    /// Read timeout for each frame. Watches receive heartbeats well within this.
    pub io_timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            client_name: "dotfilesync-client".into(),
            protocol_version: PROTOCOL_VERSION,
            io_timeout: Duration::from_secs(30),
        }
    }
}

pub struct Client {
    stream: UnixStream,
    next_id: u64,
    hello: HelloAck,
}

impl Client {
    pub fn connect(layout: &DataLayout, client_name: &str) -> Result<Self, ClientError> {
        Self::connect_with(
            layout,
            ClientOptions {
                client_name: client_name.into(),
                ..Default::default()
            },
        )
    }

    pub fn connect_with(layout: &DataLayout, opts: ClientOptions) -> Result<Self, ClientError> {
        let not_running = |reason: &str| ClientError::NotRunning {
            socket: layout.socket.clone(),
            reason: reason.into(),
        };
        // Verify the directories are ours before trusting anything inside them. Never create.
        for dir in [&layout.root, &layout.run_dir] {
            if check_private_dir(dir)? == DirState::Missing {
                return Err(not_running("data directory does not exist"));
            }
        }
        let stream = match UnixStream::connect(&layout.socket) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(not_running("no socket")),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                return Err(not_running("connection refused (stale socket)"))
            }
            Err(e) => return Err(ClientError::Io(e)),
        };
        let expected = current_euid();
        let daemon_uid = peer_euid(&stream)?;
        if daemon_uid != expected {
            return Err(ClientError::ImpostorDaemon {
                daemon_uid,
                expected,
            });
        }
        stream.set_read_timeout(Some(opts.io_timeout))?;
        stream.set_write_timeout(Some(opts.io_timeout))?;
        Self::handshake(stream, &opts)
    }

    fn handshake(mut stream: UnixStream, opts: &ClientOptions) -> Result<Self, ClientError> {
        let hello = ClientHello::Hello {
            protocol_version: opts.protocol_version,
            client: opts.client_name.clone(),
        };
        let body = serde_json::to_vec(&hello).map_err(|e| ClientError::Protocol(e.to_string()))?;
        write_frame(&mut stream, &body, MAX_REQUEST_FRAME)?;
        let frame = read_server_frame(&mut stream)?;
        match frame {
            ServerFrame::HelloAck(ack) => Ok(Self {
                stream,
                next_id: 1,
                hello: ack,
            }),
            ServerFrame::Error { error } => Err(match error.code {
                ErrorCode::IncompatibleVersion => {
                    let supported = error
                        .details
                        .as_ref()
                        .and_then(|d| d.get("supported"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    ClientError::IncompatibleVersion {
                        supported,
                        requested: opts.protocol_version,
                    }
                }
                ErrorCode::Unauthorized => ClientError::Unauthorized(error.message),
                _ => ClientError::Remote(error),
            }),
            other => Err(ClientError::Protocol(format!(
                "unexpected handshake reply: {other:?}"
            ))),
        }
    }

    pub fn daemon(&self) -> &HelloAck {
        &self.hello
    }

    fn send(&mut self, req: Request) -> Result<u64, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let body = serde_json::to_vec(&req.into_frame(id))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        write_frame(&mut self.stream, &body, MAX_REQUEST_FRAME)?;
        Ok(id)
    }

    /// Send a request and wait for its response.
    pub fn call(&mut self, req: Request) -> Result<Response, ClientError> {
        let id = self.send(req)?;
        loop {
            match read_server_frame(&mut self.stream)? {
                ServerFrame::Response { id: rid, outcome } if rid == id => {
                    return match outcome {
                        Outcome::Ok(r) => Ok(r),
                        Outcome::Err(e) => Err(ClientError::Remote(e)),
                    }
                }
                ServerFrame::Error { error } => return Err(ClientError::Remote(error)),
                // Stray notifications from an earlier watch are ignored.
                _ => continue,
            }
        }
    }

    pub fn ping(&mut self) -> Result<(), ClientError> {
        match self.call(Request::Ping {})? {
            Response::Pong => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn daemon_info(&mut self) -> Result<DaemonInfo, ClientError> {
        match self.call(Request::DaemonInfo {})? {
            Response::DaemonInfo(i) => Ok(i),
            other => Err(unexpected(other)),
        }
    }

    pub fn start_job(&mut self, spec: JobSpec) -> Result<JobSnapshot, ClientError> {
        expect_job(self.call(Request::StartJob { spec })?)
    }

    pub fn job_status(&mut self, job_id: &str) -> Result<JobSnapshot, ClientError> {
        expect_job(self.call(Request::JobStatus {
            job_id: job_id.into(),
        })?)
    }

    pub fn cancel_job(&mut self, job_id: &str) -> Result<JobSnapshot, ClientError> {
        expect_job(self.call(Request::CancelJob {
            job_id: job_id.into(),
        })?)
    }

    pub fn list_jobs(&mut self) -> Result<Vec<JobSnapshot>, ClientError> {
        match self.call(Request::ListJobs {})? {
            Response::Jobs { jobs } => Ok(jobs),
            other => Err(unexpected(other)),
        }
    }

    pub fn job_progress(
        &mut self,
        job_id: &str,
        since_seq: u64,
    ) -> Result<(JobSnapshot, Vec<ProgressEvent>, bool), ClientError> {
        match self.call(Request::JobProgress {
            job_id: job_id.into(),
            since_seq,
        })? {
            Response::Progress {
                job,
                events,
                truncated,
            } => Ok((job, events, truncated)),
            other => Err(unexpected(other)),
        }
    }

    pub fn job_result(&mut self, job_id: &str) -> Result<JobOutput, ClientError> {
        match self.call(Request::JobResult {
            job_id: job_id.into(),
        })? {
            Response::JobResult(o) => Ok(o),
            other => Err(unexpected(other)),
        }
    }

    /// Stream progress events newer than `since_seq` until the job is terminal.
    /// Dropping the client (or the process exiting) mid-watch does not cancel the job.
    pub fn watch_job(
        &mut self,
        job_id: &str,
        since_seq: u64,
        on_event: &mut dyn FnMut(&ProgressEvent),
    ) -> Result<JobSnapshot, ClientError> {
        let id = self.send(Request::WatchJob {
            job_id: job_id.into(),
            since_seq,
        })?;
        loop {
            match read_server_frame(&mut self.stream)? {
                ServerFrame::Notification {
                    id: nid,
                    notification: Notification::Progress { event, .. },
                } if nid == id => on_event(&event),
                ServerFrame::Notification { .. } => {}
                ServerFrame::Response { id: rid, outcome } if rid == id => {
                    return match outcome {
                        Outcome::Ok(r) => expect_job(r),
                        Outcome::Err(e) => Err(ClientError::Remote(e)),
                    }
                }
                ServerFrame::Error { error } => return Err(ClientError::Remote(error)),
                _ => {}
            }
        }
    }

    pub fn shutdown(&mut self) -> Result<(), ClientError> {
        match self.call(Request::Shutdown {})? {
            Response::ShuttingDown => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

fn read_server_frame(stream: &mut UnixStream) -> Result<ServerFrame, ClientError> {
    let body = read_frame(stream, MAX_RESPONSE_FRAME)?;
    serde_json::from_slice(&body)
        .map_err(|e| ClientError::Protocol(format!("undecodable daemon frame: {e}")))
}

fn expect_job(r: Response) -> Result<JobSnapshot, ClientError> {
    match r {
        Response::Job(j) => Ok(j),
        other => Err(unexpected(other)),
    }
}

fn unexpected(r: Response) -> ClientError {
    ClientError::Protocol(format!("unexpected response: {r:?}"))
}
