//! Minimal durable job journal (append-only JSON lines, metadata only).
//!
//! Records contain a job id, kind, lifecycle event and timestamp. They never contain job
//! params, paths, results, config bodies, approval tokens or subprocess output.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dotfilesync_protocol::{JobKind, JobStatus};

use crate::fsutil::{now_ms, open_private_file, open_private_read, OpenMode};

/// Jobs kept after startup compaction.
pub const JOURNAL_RETAIN: usize = 200;
/// Lines longer than this are treated as corrupt.
const MAX_RECORD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEvent {
    Started,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl JournalEvent {
    pub fn from_status(status: JobStatus) -> Option<Self> {
        Some(match status {
            JobStatus::Succeeded => JournalEvent::Succeeded,
            JobStatus::Failed => JournalEvent::Failed,
            JobStatus::Cancelled => JournalEvent::Cancelled,
            JobStatus::Interrupted => JournalEvent::Interrupted,
            _ => return None,
        })
    }

    fn status(self) -> JobStatus {
        match self {
            JournalEvent::Started => JobStatus::Running,
            JournalEvent::Succeeded => JobStatus::Succeeded,
            JournalEvent::Failed => JobStatus::Failed,
            JournalEvent::Cancelled => JobStatus::Cancelled,
            JournalEvent::Interrupted => JobStatus::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecord {
    pub job_id: String,
    pub kind: JobKind,
    pub event: JournalEvent,
    pub at_ms: u64,
}

/// A job reconstructed from a previous daemon run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredJob {
    pub job_id: String,
    pub kind: JobKind,
    pub status: JobStatus,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct RecoveryReport {
    pub jobs: Vec<RecoveredJob>,
    pub newly_interrupted: usize,
    pub corrupt_lines: usize,
}

pub struct Journal {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Journal {
    /// Load the journal, mark unfinished jobs `interrupted`, and compact it.
    pub fn open_and_recover(path: &Path) -> io::Result<(Self, RecoveryReport)> {
        let mut report = RecoveryReport::default();
        let mut order: Vec<String> = Vec::new();
        let mut jobs: HashMap<String, RecoveredJob> = HashMap::new();

        match open_private_read(path) {
            Ok(file) => {
                let mut reader = BufReader::new(file);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let n = reader
                        .by_ref()
                        .take(MAX_RECORD as u64 + 1)
                        .read_until(b'\n', &mut line)?;
                    if n == 0 {
                        break;
                    }
                    if line.len() > MAX_RECORD {
                        report.corrupt_lines += 1;
                        // Skip the rest of an over-long line.
                        let mut sink = Vec::new();
                        reader.read_until(b'\n', &mut sink)?;
                        continue;
                    }
                    let rec: JournalRecord = match serde_json::from_slice(&line) {
                        Ok(r) => r,
                        Err(_) => {
                            report.corrupt_lines += 1;
                            continue;
                        }
                    };
                    let entry = jobs.entry(rec.job_id.clone()).or_insert_with(|| {
                        order.push(rec.job_id.clone());
                        RecoveredJob {
                            job_id: rec.job_id.clone(),
                            kind: rec.kind,
                            status: JobStatus::Running,
                            created_at_ms: rec.at_ms,
                            finished_at_ms: None,
                        }
                    });
                    if rec.event != JournalEvent::Started {
                        entry.status = rec.event.status();
                        entry.finished_at_ms = Some(rec.at_ms);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        let now = now_ms();
        for job in jobs.values_mut() {
            if !job.status.is_terminal() {
                job.status = JobStatus::Interrupted;
                job.finished_at_ms = Some(now);
                report.newly_interrupted += 1;
            }
        }

        let keep_from = order.len().saturating_sub(JOURNAL_RETAIN);
        report.jobs = order[keep_from..]
            .iter()
            .filter_map(|id| jobs.remove(id))
            .collect();

        let journal = Self {
            path: path.to_path_buf(),
            lock: Mutex::new(()),
        };
        journal.rewrite(&report.jobs)?;
        Ok((journal, report))
    }

    fn rewrite(&self, jobs: &[RecoveredJob]) -> io::Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut tmp = self.path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let _ = fs::remove_file(&tmp);
        {
            let mut f = open_private_file(&tmp, OpenMode::Truncate)?;
            let mut buf = Vec::new();
            for j in jobs {
                let started = JournalRecord {
                    job_id: j.job_id.clone(),
                    kind: j.kind,
                    event: JournalEvent::Started,
                    at_ms: j.created_at_ms,
                };
                write_line(&mut buf, &started)?;
                if let Some(ev) = JournalEvent::from_status(j.status) {
                    let fin = JournalRecord {
                        job_id: j.job_id.clone(),
                        kind: j.kind,
                        event: ev,
                        at_ms: j.finished_at_ms.unwrap_or(j.created_at_ms),
                    };
                    write_line(&mut buf, &fin)?;
                }
            }
            f.write_all(&buf)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)
    }

    pub fn append(&self, job_id: &str, kind: JobKind, event: JournalEvent) -> io::Result<()> {
        let rec = JournalRecord {
            job_id: job_id.to_string(),
            kind,
            event,
            at_ms: now_ms(),
        };
        let mut buf = Vec::new();
        write_line(&mut buf, &rec)?;
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = open_private_file(&self.path, OpenMode::Append)?;
        f.write_all(&buf)?;
        f.sync_data()
    }
}

fn write_line(buf: &mut Vec<u8>, rec: &JournalRecord) -> io::Result<()> {
    serde_json::to_writer(&mut *buf, rec).map_err(io::Error::other)?;
    buf.push(b'\n');
    Ok(())
}
