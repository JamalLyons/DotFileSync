//! Job manager: typed jobs on worker threads, progress, cancellation, retention, journaling.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use dotfilesync_core::{run_scan, run_status, OpError, OpStage, RealCommandRunner};
use dotfilesync_protocol::{
    ErrorCode, JobError, JobErrorCode, JobKind, JobOutput, JobSnapshot, JobSpec, JobStatus,
    ProgressEvent, ProtocolError,
};

use crate::fsutil::{now_ms, random_hex};
use crate::journal::{Journal, JournalEvent, RecoveredJob};
use crate::locks::{LockMode, ResourceKey, ResourceLocks};
use crate::log::Logger;

/// Progress events retained per job.
pub const MAX_EVENTS_PER_JOB: usize = 256;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub struct JobLimits {
    pub max_active: usize,
    pub max_retained: usize,
    pub command_timeout: Duration,
}

impl Default for JobLimits {
    fn default() -> Self {
        Self {
            max_active: 16,
            max_retained: 64,
            command_timeout: Duration::from_secs(120),
        }
    }
}

struct JobState {
    status: JobStatus,
    events: VecDeque<ProgressEvent>,
    next_seq: u64,
    finished_at_ms: Option<u64>,
    error: Option<JobError>,
    output: Option<JobOutput>,
}

pub struct Job {
    id: String,
    kind: JobKind,
    created_at_ms: u64,
    from_previous_run: bool,
    cancel: Arc<AtomicBool>,
    state: Mutex<JobState>,
    changed: Condvar,
}

impl Job {
    fn new(id: String, kind: JobKind) -> Self {
        Self {
            id,
            kind,
            created_at_ms: now_ms(),
            from_previous_run: false,
            cancel: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(JobState {
                status: JobStatus::Queued,
                events: VecDeque::new(),
                next_seq: 1,
                finished_at_ms: None,
                error: None,
                output: None,
            }),
            changed: Condvar::new(),
        }
    }

    fn recovered(r: &RecoveredJob) -> Self {
        let job = Self::new(r.job_id.clone(), r.kind);
        let mut job = Self {
            created_at_ms: r.created_at_ms,
            from_previous_run: true,
            ..job
        };
        let st = job.state.get_mut().unwrap_or_else(|p| p.into_inner());
        st.status = r.status;
        st.finished_at_ms = r.finished_at_ms;
        if r.status == JobStatus::Interrupted {
            st.error = Some(JobError {
                code: JobErrorCode::Interrupted,
                message:
                    "the daemon stopped before this job finished; it was not resumed or replayed"
                        .into(),
            });
        }
        job
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JobState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn snapshot_locked(&self, st: &JobState) -> JobSnapshot {
        JobSnapshot {
            job_id: self.id.clone(),
            kind: self.kind,
            status: st.status,
            created_at_ms: self.created_at_ms,
            finished_at_ms: st.finished_at_ms,
            latest_seq: st.next_seq - 1,
            cancel_requested: self.cancel.load(Ordering::SeqCst),
            from_previous_run: self.from_previous_run,
            error: st.error.clone(),
        }
    }

    pub fn snapshot(&self) -> JobSnapshot {
        let st = self.lock();
        self.snapshot_locked(&st)
    }

    fn push_event(&self, stage: &str, message: &str) {
        let mut st = self.lock();
        let seq = st.next_seq;
        st.next_seq += 1;
        st.events.push_back(ProgressEvent {
            seq,
            at_ms: now_ms(),
            stage: stage.into(),
            message: message.into(),
        });
        while st.events.len() > MAX_EVENTS_PER_JOB {
            st.events.pop_front();
        }
        drop(st);
        self.changed.notify_all();
    }

    fn set_status(&self, status: JobStatus) {
        self.lock().status = status;
        self.changed.notify_all();
    }

    fn finish(&self, status: JobStatus, output: Option<JobOutput>, error: Option<JobError>) {
        let mut st = self.lock();
        st.status = status;
        st.output = output;
        st.error = error;
        st.finished_at_ms = Some(now_ms());
        drop(st);
        self.changed.notify_all();
    }

    fn events_since(st: &JobState, since: u64) -> (Vec<ProgressEvent>, bool) {
        let truncated = st
            .events
            .front()
            .is_some_and(|e| e.seq > since.saturating_add(1));
        let events = st
            .events
            .iter()
            .filter(|e| e.seq > since)
            .cloned()
            .collect();
        (events, truncated)
    }

    /// Wait up to `timeout` for events newer than `since` or a terminal status.
    pub fn wait(&self, since: u64, timeout: Duration) -> (JobSnapshot, Vec<ProgressEvent>, bool) {
        let deadline = Instant::now() + timeout;
        let mut st = self.lock();
        loop {
            let has_new = st.next_seq - 1 > since;
            if has_new || st.status.is_terminal() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            st = self
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        let (events, truncated) = Self::events_since(&st, since);
        (self.snapshot_locked(&st), events, truncated)
    }
}

struct Registry {
    jobs: HashMap<String, Arc<Job>>,
    order: VecDeque<String>,
}

pub struct JobManager {
    registry: Mutex<Registry>,
    journal: Arc<Journal>,
    log: Arc<Logger>,
    locks: Arc<ResourceLocks>,
    brew_bin: Option<PathBuf>,
    limits: JobLimits,
    accepting: AtomicBool,
}

fn not_found(id: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::JobNotFound, format!("no job with id '{id}'"))
}

fn invalid(msg: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidParams, msg)
}

fn validate_path(name: &str, p: &Path) -> Result<(), ProtocolError> {
    let bytes = p.as_os_str().len();
    if bytes == 0 || bytes > MAX_PATH_BYTES {
        return Err(invalid(format!(
            "'{name}' must be 1..={MAX_PATH_BYTES} bytes"
        )));
    }
    if !p.is_absolute() {
        return Err(invalid(format!("'{name}' must be an absolute path")));
    }
    if std::os::unix::ffi::OsStrExt::as_bytes(p.as_os_str()).contains(&0) {
        return Err(invalid(format!("'{name}' must not contain NUL bytes")));
    }
    Ok(())
}

fn validate_spec(spec: &JobSpec) -> Result<(), ProtocolError> {
    match spec {
        JobSpec::Scan(s) => {
            if let Some(m) = &s.manifest {
                validate_path("manifest", m)?;
            }
            validate_path("home", &s.home)?;
            if let Some(r) = &s.repo_root {
                validate_path("repo_root", r)?;
            }
        }
        JobSpec::Status(s) => {
            validate_path("manifest", &s.manifest)?;
            validate_path("home", &s.home)?;
            if let Some(r) = &s.repo_root {
                validate_path("repo_root", r)?;
            }
        }
    }
    Ok(())
}

fn lock_requests(spec: &JobSpec) -> Vec<(ResourceKey, LockMode)> {
    let mut reqs = vec![(ResourceKey::Homebrew, LockMode::Shared)];
    let manifest = match spec {
        JobSpec::Scan(s) => s.manifest.clone(),
        JobSpec::Status(s) => Some(s.manifest.clone()),
    };
    if let Some(m) = manifest {
        reqs.push((ResourceKey::Manifest(m), LockMode::Shared));
    }
    reqs
}

impl JobManager {
    pub fn new(
        journal: Arc<Journal>,
        log: Arc<Logger>,
        locks: Arc<ResourceLocks>,
        brew_bin: Option<PathBuf>,
        limits: JobLimits,
        recovered: &[RecoveredJob],
    ) -> Arc<Self> {
        let mut registry = Registry {
            jobs: HashMap::new(),
            order: VecDeque::new(),
        };
        // Keep the newest recovered jobs visible so clients can see what was interrupted.
        let skip = recovered.len().saturating_sub(limits.max_retained);
        for r in &recovered[skip..] {
            registry.order.push_back(r.job_id.clone());
            registry
                .jobs
                .insert(r.job_id.clone(), Arc::new(Job::recovered(r)));
        }
        Arc::new(Self {
            registry: Mutex::new(registry),
            journal,
            log,
            locks,
            brew_bin,
            limits,
            accepting: AtomicBool::new(true),
        })
    }

    fn reg(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn get(&self, id: &str) -> Result<Arc<Job>, ProtocolError> {
        self.reg()
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        let reg = self.reg();
        reg.order
            .iter()
            .filter_map(|id| reg.jobs.get(id))
            .map(|j| j.snapshot())
            .collect()
    }

    pub fn counts(&self) -> (usize, usize) {
        let reg = self.reg();
        let active = reg
            .jobs
            .values()
            .filter(|j| !j.lock().status.is_terminal())
            .count();
        (active, reg.jobs.len())
    }

    pub fn start(self: &Arc<Self>, spec: JobSpec) -> Result<JobSnapshot, ProtocolError> {
        if !self.accepting.load(Ordering::SeqCst) {
            return Err(ProtocolError::new(
                ErrorCode::ShuttingDown,
                "daemon is shutting down",
            ));
        }
        validate_spec(&spec)?;
        let id = format!(
            "j-{}",
            random_hex(8).map_err(|e| ProtocolError::new(ErrorCode::Internal, e.to_string()))?
        );
        let job = Arc::new(Job::new(id.clone(), spec.kind()));
        {
            let mut reg = self.reg();
            let active = reg
                .jobs
                .values()
                .filter(|j| !j.lock().status.is_terminal())
                .count();
            if active >= self.limits.max_active {
                return Err(ProtocolError::new(
                    ErrorCode::Busy,
                    format!(
                        "{active} jobs are already active (limit {}); retry later",
                        self.limits.max_active
                    ),
                ));
            }
            reg.order.push_back(id.clone());
            reg.jobs.insert(id.clone(), Arc::clone(&job));
            self.evict_locked(&mut reg);
        }
        if let Err(e) = self.journal.append(&id, job.kind, JournalEvent::Started) {
            self.log
                .warn(&format!("journal append failed for job {id}: {e}"));
        }
        self.log
            .info(&format!("job {id} kind={} started", job.kind));

        let mgr = Arc::clone(self);
        let worker_job = Arc::clone(&job);
        let spawned = std::thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || mgr.run_job(&worker_job, spec));
        if let Err(e) = spawned {
            self.complete(
                &job,
                JobStatus::Failed,
                None,
                Some(JobError {
                    code: JobErrorCode::Internal,
                    message: format!("failed to spawn worker: {e}"),
                }),
            );
        }
        Ok(job.snapshot())
    }

    fn evict_locked(&self, reg: &mut Registry) {
        while reg.order.len() > self.limits.max_retained {
            // Evict the oldest *terminal* job; active jobs are never dropped.
            let pos = reg.order.iter().position(|id| {
                reg.jobs
                    .get(id)
                    .is_none_or(|j| j.lock().status.is_terminal())
            });
            match pos {
                Some(p) => {
                    if let Some(id) = reg.order.remove(p) {
                        reg.jobs.remove(&id);
                    }
                }
                None => break,
            }
        }
    }

    fn complete(
        &self,
        job: &Job,
        status: JobStatus,
        output: Option<JobOutput>,
        error: Option<JobError>,
    ) {
        job.finish(status, output, error);
        if let Some(ev) = JournalEvent::from_status(status) {
            if let Err(e) = self.journal.append(&job.id, job.kind, ev) {
                self.log
                    .warn(&format!("journal append failed for job {}: {e}", job.id));
            }
        }
        self.log.info(&format!(
            "job {} kind={} finished status={status}",
            job.id, job.kind
        ));
    }

    fn run_job(&self, job: &Arc<Job>, spec: JobSpec) {
        let cancel = Arc::clone(&job.cancel);
        let is_cancelled = move || cancel.load(Ordering::SeqCst);

        job.set_status(JobStatus::Running);
        job.push_event("acquiring_locks", "Waiting for resource locks");
        let guard = match self.locks.acquire(&lock_requests(&spec), &is_cancelled) {
            Ok(g) => g,
            Err(_) => {
                self.complete(job, JobStatus::Cancelled, None, Some(cancelled_error()));
                return;
            }
        };

        let mut runner = RealCommandRunner::new(self.limits.command_timeout)
            .with_cancellation(Arc::clone(&job.cancel));
        if let Some(bin) = &self.brew_bin {
            runner = runner.with_brew_binary(bin.clone());
        }

        let progress_job = Arc::clone(job);
        let mut progress = move |stage: OpStage| {
            let stage_name = serde_json::to_value(stage)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            progress_job.push_event(&stage_name, stage.describe());
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &spec {
            JobSpec::Scan(input) => run_scan(input, &runner, &mut progress, &is_cancelled)
                .map(|report| JobOutput::Scan { report }),
            JobSpec::Status(input) => run_status(input, &runner, &mut progress, &is_cancelled)
                .map(|report| JobOutput::Status { report }),
        }));
        drop(guard);

        match result {
            // A cancel that raced with completion still wins: never report partial work after
            // the user asked to stop.
            Ok(Ok(_)) if is_cancelled() => {
                self.complete(job, JobStatus::Cancelled, None, Some(cancelled_error()))
            }
            Ok(Ok(output)) => {
                job.push_event("done", "Completed");
                self.complete(job, JobStatus::Succeeded, Some(output), None)
            }
            Ok(Err(OpError::Cancelled)) => {
                self.complete(job, JobStatus::Cancelled, None, Some(cancelled_error()))
            }
            Ok(Err(e @ OpError::Manifest { .. })) => self.complete(
                job,
                JobStatus::Failed,
                None,
                Some(JobError {
                    code: JobErrorCode::ManifestLoad,
                    message: e.to_string(),
                }),
            ),
            Err(_) => self.complete(
                job,
                JobStatus::Failed,
                None,
                Some(JobError {
                    code: JobErrorCode::Internal,
                    message: "job worker panicked".into(),
                }),
            ),
        }
    }

    pub fn cancel(&self, id: &str) -> Result<JobSnapshot, ProtocolError> {
        let job = self.get(id)?;
        if !job.lock().status.is_terminal() {
            job.cancel.store(true, Ordering::SeqCst);
            job.push_event("cancel_requested", "Cancellation requested");
            self.log.info(&format!("job {id} cancel requested"));
        }
        Ok(job.snapshot())
    }

    pub fn result(&self, id: &str) -> Result<JobOutput, ProtocolError> {
        let job = self.get(id)?;
        let st = job.lock();
        match (st.status, &st.output) {
            (JobStatus::Succeeded, Some(out)) => Ok(out.clone()),
            (JobStatus::Succeeded, None) => Err(ProtocolError::new(
                ErrorCode::Internal,
                "job succeeded but its result is no longer available",
            )),
            (s, _) if !s.is_terminal() => Err(ProtocolError::new(
                ErrorCode::JobNotFinished,
                format!("job '{id}' is still {s}"),
            )),
            (s, _) => Err(ProtocolError::new(
                ErrorCode::JobFailed,
                format!(
                    "job '{id}' {s}{}",
                    st.error
                        .as_ref()
                        .map(|e| format!(": {}", e.message))
                        .unwrap_or_default()
                ),
            )),
        }
    }

    /// Stop accepting jobs, cancel active ones, and wait up to `grace` for them to finish.
    /// Returns the number of jobs that were still active when the grace period expired; the
    /// journal leaves those as started-without-outcome, so the next start reports them
    /// `interrupted`.
    pub fn shutdown(&self, grace: Duration) -> usize {
        self.accepting.store(false, Ordering::SeqCst);
        let active: Vec<Arc<Job>> = self
            .reg()
            .jobs
            .values()
            .filter(|j| !j.lock().status.is_terminal())
            .cloned()
            .collect();
        for job in &active {
            job.cancel.store(true, Ordering::SeqCst);
            job.push_event("cancel_requested", "Daemon shutting down");
        }
        let deadline = Instant::now() + grace;
        for job in &active {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let _ = job.wait(u64::MAX, remaining);
        }
        active
            .iter()
            .filter(|j| !j.lock().status.is_terminal())
            .count()
    }
}

fn cancelled_error() -> JobError {
    JobError {
        code: JobErrorCode::Cancelled,
        message: "job was cancelled; no partial result is reported".into(),
    }
}
