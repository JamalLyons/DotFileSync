//! Running a job through dotfilesyncd and waiting for its result.

use anyhow::{anyhow, bail, Result};
use dotfilesync_protocol::{Client, JobOutput, JobSnapshot, JobSpec, JobStatus};

pub enum Remote {
    Detached(JobSnapshot),
    Finished(JobOutput),
    /// Terminal but not successful (failed, cancelled, interrupted).
    Failed(JobSnapshot),
}

/// Start `spec`, stream progress to stderr (unless `quiet`), and fetch the result.
/// Ctrl-C (or any disconnect) only detaches this client; the job keeps running.
pub fn run_job(client: &mut Client, spec: JobSpec, detach: bool, quiet: bool) -> Result<Remote> {
    let job = client.start_job(spec)?;
    let id = job.job_id.clone();
    eprintln!(
        "dotfilesyncd (pid {}) is running {} as job {id}.",
        client.daemon().pid,
        job.kind
    );
    if detach {
        eprintln!("Detached. Follow it with `dotfilesync jobs watch {id}`.");
        return Ok(Remote::Detached(job));
    }
    if !quiet {
        eprintln!(
            "Ctrl-C detaches without cancelling; cancel with `dotfilesync jobs cancel {id}`."
        );
    }
    let final_snapshot = client.watch_job(&id, 0, &mut |ev| {
        if !quiet {
            eprintln!("  [{}] {}", ev.stage, ev.message);
        }
    })?;
    finish(client, &final_snapshot)
}

pub fn finish(client: &mut Client, job: &JobSnapshot) -> Result<Remote> {
    match job.status {
        JobStatus::Succeeded => Ok(Remote::Finished(client.job_result(&job.job_id)?)),
        JobStatus::Queued | JobStatus::Running => {
            bail!("job {} is still {}", job.job_id, job.status)
        }
        _ => Ok(Remote::Failed(job.clone())),
    }
}

pub fn failure(job: &JobSnapshot) -> anyhow::Error {
    let detail = job
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    anyhow!("job {} {}: {detail}", job.job_id, job.status)
}

pub fn unexpected(output: &JobOutput) -> anyhow::Error {
    anyhow!("daemon returned an unexpected result kind: {output:?}")
}
