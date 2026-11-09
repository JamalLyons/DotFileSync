//! `dotfilesync jobs ...`: inspect, follow, and cancel daemon jobs from any terminal.

use anyhow::Result;
use clap::Subcommand;
use dotfilesync_protocol::{JobOutput, JobSnapshot};
use std::path::Path;

use super::mode;
use super::remote::{self, Remote};
use super::{scan, status};
use crate::output::json;

#[derive(Debug, Subcommand)]
pub enum JobsCommand {
    /// List jobs known to the daemon, including interrupted jobs from a previous run
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one job's status
    Show {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Reconnect to a job: print progress after SEQ, then wait for it to finish
    Watch {
        job_id: String,
        /// Only show events with a sequence number greater than this
        #[arg(long, default_value_t = 0)]
        since: u64,
    },
    /// Print the report of a finished job
    Result {
        job_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Request cancellation of a running job (also stops its subprocesses)
    Cancel { job_id: String },
}

fn line(j: &JobSnapshot) -> String {
    let mut s = format!("{}  {:<6}  {}", j.job_id, j.kind.to_string(), j.status);
    if j.cancel_requested && !j.status.is_terminal() {
        s.push_str(" (cancel requested)");
    }
    if j.from_previous_run {
        s.push_str("  [previous daemon run]");
    }
    if let Some(e) = &j.error {
        s.push_str(&format!("  - {}", e.message));
    }
    s
}

pub fn execute(cmd: JobsCommand, data_dir: Option<&Path>) -> Result<()> {
    let mut client = mode::connect(data_dir)?;
    match cmd {
        JobsCommand::List { json: as_json } => {
            let jobs = client.list_jobs()?;
            if as_json {
                println!("{}", json::render_json(&jobs)?);
            } else if jobs.is_empty() {
                println!("No jobs.");
            } else {
                for j in &jobs {
                    println!("{}", line(j));
                }
            }
        }
        JobsCommand::Show {
            job_id,
            json: as_json,
        } => {
            let j = client.job_status(&job_id)?;
            if as_json {
                println!("{}", json::render_json(&j)?);
            } else {
                println!("{}", line(&j));
            }
        }
        JobsCommand::Watch { job_id, since } => {
            let (job, missed, truncated) = client.job_progress(&job_id, since)?;
            if truncated {
                eprintln!("(older progress events were discarded by the daemon)");
            }
            let mut last = since;
            for ev in &missed {
                println!("  [{}] {}", ev.stage, ev.message);
                last = last.max(ev.seq);
            }
            let job = if job.status.is_terminal() {
                job
            } else {
                client.watch_job(&job_id, last, &mut |ev| {
                    println!("  [{}] {}", ev.stage, ev.message);
                })?
            };
            println!("{}", line(&job));
        }
        JobsCommand::Result {
            job_id,
            json: as_json,
        } => {
            let job = client.job_status(&job_id)?;
            match remote::finish(&mut client, &job)? {
                Remote::Finished(JobOutput::Scan { report }) => {
                    scan::print_report(&report, as_json)?
                }
                Remote::Finished(JobOutput::Status { report }) => {
                    status::print_report(&report, as_json)?
                }
                Remote::Failed(job) => return Err(remote::failure(&job)),
                Remote::Detached(_) => unreachable!(),
            }
        }
        JobsCommand::Cancel { job_id } => {
            let j = client.cancel_job(&job_id)?;
            println!("{}", line(&j));
        }
    }
    Ok(())
}
