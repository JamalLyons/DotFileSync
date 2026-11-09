//! Typed protocol messages.
//!
//! Client → daemon: a [`ClientHello`] frame, then any number of [`RequestFrame`]s.
//! Daemon → client: [`ServerFrame`]s (handshake ack, responses, notifications, fatal errors).

use dotfilesync_core::{ScanInput, ScanReport, StatusInput, StatusReport};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

// ---------------------------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientHello {
    Hello {
        protocol_version: u32,
        /// Free-form client name for diagnostics only (e.g. "dotfilesync-cli"). Never trusted.
        client: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    pub protocol_version: u32,
    pub daemon_version: String,
    pub pid: u32,
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

/// Raw request envelope. `op` and `id` are decoded before `params`, so a request with invalid
/// params still gets an error response correlated to its `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestFrame {
    #[serde(rename = "type")]
    pub frame_type: RequestFrameType,
    pub id: u64,
    pub op: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestFrameType {
    Request,
}

/// Every operation the daemon implements. All variants are struct-like so `params` is always a
/// JSON object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "params", rename_all = "snake_case")]
pub enum Request {
    Ping {},
    DaemonInfo {},
    StartJob { spec: JobSpec },
    JobStatus { job_id: String },
    ListJobs {},
    JobProgress { job_id: String, since_seq: u64 },
    WatchJob { job_id: String, since_seq: u64 },
    JobResult { job_id: String },
    CancelJob { job_id: String },
    Shutdown {},
}

/// Operations implemented by this protocol version.
pub const IMPLEMENTED_OPS: &[&str] = &[
    "ping",
    "daemon_info",
    "start_job",
    "job_status",
    "list_jobs",
    "job_progress",
    "watch_job",
    "job_result",
    "cancel_job",
    "shutdown",
];

/// Operation namespaces reserved for later issues. The daemon answers these with
/// `unsupported_operation` and never pretends they succeeded.
pub const RESERVED_OPS: &[(&str, &str)] = &[
    ("plan.", "plan/approve/apply arrives with issue #4"),
    (
        "runtime.",
        "toolchain/runtime management arrives with issue #5",
    ),
    (
        "config.",
        "config discovery and editing arrives with issue #11",
    ),
    (
        "sync.",
        "GitHub state-repo sync arrives with issues #6 and #7",
    ),
];

impl Request {
    pub fn op_name(&self) -> &'static str {
        match self {
            Request::Ping {} => "ping",
            Request::DaemonInfo {} => "daemon_info",
            Request::StartJob { .. } => "start_job",
            Request::JobStatus { .. } => "job_status",
            Request::ListJobs {} => "list_jobs",
            Request::JobProgress { .. } => "job_progress",
            Request::WatchJob { .. } => "watch_job",
            Request::JobResult { .. } => "job_result",
            Request::CancelJob { .. } => "cancel_job",
            Request::Shutdown {} => "shutdown",
        }
    }

    /// Build the wire envelope for this request.
    pub fn into_frame(self, id: u64) -> RequestFrame {
        let op = self.op_name().to_string();
        let params = match serde_json::to_value(&self) {
            Ok(Value::Object(mut map)) => map.remove("params").unwrap_or(Value::Null),
            _ => Value::Null,
        };
        RequestFrame {
            frame_type: RequestFrameType::Request,
            id,
            op,
            params,
        }
    }

    /// Decode a typed request from an envelope, distinguishing unknown, reserved and malformed.
    pub fn from_frame(frame: &RequestFrame) -> Result<Request, ProtocolError> {
        let op = frame.op.as_str();
        if let Some((_, why)) = RESERVED_OPS
            .iter()
            .find(|(prefix, _)| op.starts_with(prefix))
        {
            return Err(ProtocolError::new(
                ErrorCode::UnsupportedOperation,
                format!("operation '{op}' is not supported by this daemon: {why}"),
            ));
        }
        if !IMPLEMENTED_OPS.contains(&op) {
            return Err(ProtocolError::new(
                ErrorCode::UnknownOperation,
                format!("unknown operation '{}'", truncate(op, 64)),
            ));
        }
        let params = match &frame.params {
            Value::Null => Value::Object(Default::default()),
            other => other.clone(),
        };
        let mut obj = serde_json::Map::new();
        obj.insert("op".into(), Value::String(op.to_string()));
        obj.insert("params".into(), params);
        serde_json::from_value(Value::Object(obj)).map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidParams,
                format!("invalid params for '{op}': {e}"),
            )
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// A typed job. There is deliberately no variant that carries a command, script or executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSpec {
    Scan(ScanInput),
    Status(StatusInput),
}

impl JobSpec {
    pub fn kind(&self) -> JobKind {
        match self {
            JobSpec::Scan(_) => JobKind::Scan,
            JobSpec::Status(_) => JobKind::Status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Scan,
    Status,
    /// A kind recorded by a newer daemon version; shown but not interpreted.
    #[serde(other)]
    Unknown,
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            JobKind::Scan => "scan",
            JobKind::Status => "status",
            JobKind::Unknown => "unknown",
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Responses and notifications
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    HelloAck(HelloAck),
    Response {
        id: u64,
        outcome: Outcome,
    },
    Notification {
        /// The id of the request (e.g. `watch_job`) that subscribed to these notifications.
        id: u64,
        notification: Notification,
    },
    /// A connection-level error. The daemon closes the connection after sending it.
    Error {
        error: ProtocolError,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(Response),
    Err(ProtocolError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Pong,
    DaemonInfo(DaemonInfo),
    Job(JobSnapshot),
    Jobs {
        jobs: Vec<JobSnapshot>,
    },
    Progress {
        job: JobSnapshot,
        events: Vec<ProgressEvent>,
        /// True when older events were dropped from the bounded buffer.
        truncated: bool,
    },
    JobResult(JobOutput),
    ShuttingDown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notification {
    Progress {
        job_id: String,
        event: ProgressEvent,
    },
    /// Sent periodically during long watches so both sides detect dead connections.
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub daemon_version: String,
    pub protocol_version: u32,
    pub pid: u32,
    pub uptime_ms: u64,
    pub data_dir: std::path::PathBuf,
    pub active_jobs: usize,
    pub retained_jobs: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// Started by a previous daemon process that stopped before recording an outcome.
    /// Never replayed automatically.
    Interrupted,
    #[serde(other)]
    Unknown,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, JobStatus::Queued | JobStatus::Running)
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Interrupted => "interrupted",
            JobStatus::Unknown => "unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub job_id: String,
    pub kind: JobKind,
    pub status: JobStatus,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    /// Sequence number of the newest progress event (0 = none yet).
    pub latest_seq: u64,
    pub cancel_requested: bool,
    /// True for jobs recovered from the journal of a previous daemon process.
    pub from_previous_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobError {
    pub code: JobErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobErrorCode {
    ManifestLoad,
    Cancelled,
    Interrupted,
    Internal,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub seq: u64,
    pub at_ms: u64,
    pub stage: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobOutput {
    Scan { report: ScanReport },
    Status { report: StatusReport },
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    IncompatibleVersion,
    Unauthorized,
    MalformedRequest,
    FrameTooLarge,
    HandshakeRequired,
    UnknownOperation,
    UnsupportedOperation,
    InvalidParams,
    JobNotFound,
    JobNotFinished,
    JobFailed,
    Busy,
    ShuttingDown,
    Internal,
    /// An error code introduced by a newer daemon.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:?})", self.message, self.code)
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn request_round_trips_through_envelope() {
        let req = Request::StartJob {
            spec: JobSpec::Status(StatusInput {
                manifest: PathBuf::from("/tmp/m.toml"),
                home: PathBuf::from("/tmp/home"),
                repo_root: None,
                profile: None,
            }),
        };
        let frame = req.clone().into_frame(7);
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"request\""));
        assert!(json.contains("\"op\":\"start_job\""));
        let decoded: RequestFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(Request::from_frame(&decoded).unwrap(), req);
    }

    #[test]
    fn unit_like_requests_accept_missing_params() {
        let frame: RequestFrame =
            serde_json::from_str(r#"{"type":"request","id":1,"op":"ping"}"#).unwrap();
        assert_eq!(Request::from_frame(&frame).unwrap(), Request::Ping {});
    }

    #[test]
    fn reserved_unknown_and_invalid_are_distinguished() {
        let mk = |op: &str, params: Value| RequestFrame {
            frame_type: RequestFrameType::Request,
            id: 1,
            op: op.into(),
            params,
        };
        assert_eq!(
            Request::from_frame(&mk("plan.apply", Value::Null))
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedOperation
        );
        assert_eq!(
            Request::from_frame(&mk("sync.push", Value::Null))
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedOperation
        );
        assert_eq!(
            Request::from_frame(&mk("exec", Value::Null))
                .unwrap_err()
                .code,
            ErrorCode::UnknownOperation
        );
        assert_eq!(
            Request::from_frame(&mk("job_status", serde_json::json!({"job_id": 5})))
                .unwrap_err()
                .code,
            ErrorCode::InvalidParams
        );
    }

    #[test]
    fn job_spec_has_no_command_field() {
        // An attacker-supplied "command" key is not a field of any job kind.
        let res: Result<JobSpec, _> = serde_json::from_value(serde_json::json!({
            "kind": "shell", "command": "rm -rf ~"
        }));
        assert!(res.is_err());
    }

    #[test]
    fn every_response_variant_round_trips_inside_a_server_frame() {
        let job = JobSnapshot {
            job_id: "j-1".into(),
            kind: JobKind::Scan,
            status: JobStatus::Running,
            created_at_ms: 1,
            finished_at_ms: None,
            latest_seq: 2,
            cancel_requested: false,
            from_previous_run: false,
            error: None,
        };
        let report = ScanReport::new(Default::default(), vec![], vec![]);
        let responses = vec![
            Response::Pong,
            Response::Job(job.clone()),
            Response::Jobs {
                jobs: vec![job.clone()],
            },
            Response::Progress {
                job: job.clone(),
                events: vec![],
                truncated: false,
            },
            Response::JobResult(JobOutput::Scan { report }),
            Response::ShuttingDown,
        ];
        for r in responses {
            let frame = ServerFrame::Response {
                id: 3,
                outcome: Outcome::Ok(r),
            };
            let json = serde_json::to_string(&frame).unwrap();
            let back: ServerFrame =
                serde_json::from_str(&json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_eq!(back, frame);
        }
        let n = ServerFrame::Notification {
            id: 1,
            notification: Notification::Progress {
                job_id: "j".into(),
                event: ProgressEvent {
                    seq: 1,
                    at_ms: 0,
                    stage: "s".into(),
                    message: "m".into(),
                },
            },
        };
        let back: ServerFrame = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(back, n);
    }

    #[test]
    fn unknown_error_codes_from_newer_daemons_decode() {
        let e: ProtocolError =
            serde_json::from_str(r#"{"code":"brand_new","message":"x"}"#).unwrap();
        assert_eq!(e.code, ErrorCode::Unknown);
    }
}
