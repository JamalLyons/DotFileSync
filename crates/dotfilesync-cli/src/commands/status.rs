use anyhow::{anyhow, Result};
use clap::Args;
use dotfilesync_core::{run_status, OpError, RealCommandRunner, StatusInput, StatusReport};
use dotfilesync_protocol::{JobErrorCode, JobOutput, JobSpec};
use std::path::{Path, PathBuf};

use super::mode::{self, Mode, Route};
use super::remote::{self, Remote};
use crate::output::{human, json};

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Path to manifest file (required)
    #[arg(short, long)]
    pub manifest: PathBuf,

    /// Output status in JSON format
    #[arg(long)]
    pub json: bool,

    /// Override home directory path (primarily for testing)
    #[arg(long)]
    pub home: Option<PathBuf>,

    /// Override repository root path (defaults to manifest parent dir or current directory)
    #[arg(long)]
    pub repo_root: Option<PathBuf>,

    /// Where to run: auto (daemon if running, else in-process), daemon, or standalone
    #[arg(long, value_enum, default_value_t = Mode::Auto)]
    pub mode: Mode,

    /// With the daemon: start the job, print its id, and return without waiting
    #[arg(long)]
    pub detach: bool,

    /// Override Homebrew binary path (standalone only; primarily for testing)
    #[arg(long, hide = true)]
    pub brew_bin: Option<PathBuf>,
}

fn manifest_error(path: &Path, cause: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("Failed to read manifest from '{}': {cause}", path.display())
}

pub fn execute(args: StatusArgs, data_dir: Option<&Path>) -> Result<()> {
    let brew_bin = args
        .brew_bin
        .clone()
        .or_else(|| std::env::var_os("DOTFILESYNC_BREW_BIN").map(PathBuf::from));
    let home = args
        .home
        .clone()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    let standalone_only = brew_bin.as_ref().map(|_| "--brew-bin/DOTFILESYNC_BREW_BIN");
    let report: StatusReport = match mode::route(args.mode, data_dir, "status", standalone_only)? {
        Route::Standalone { note } => {
            if let Some(n) = note {
                eprintln!("{n}");
            }
            let mut runner = RealCommandRunner::default();
            if let Some(bin) = brew_bin {
                runner = runner.with_brew_binary(bin);
            }
            let input = StatusInput {
                manifest: args.manifest.clone(),
                home,
                repo_root: args.repo_root.clone(),
            };
            match run_status(&input, &runner, &mut |_| {}, &|| false) {
                Ok(r) => r,
                // Same error chain as before the daemon existed.
                Err(OpError::Manifest { source, .. }) => {
                    return Err(anyhow::Error::new(source).context(format!(
                        "Failed to read manifest from '{}'",
                        args.manifest.display()
                    )))
                }
                Err(e) => return Err(e.into()),
            }
        }
        Route::Daemon(mut client) => {
            let input = StatusInput {
                manifest: mode::absolute(&args.manifest)?,
                home: mode::absolute(&home)?,
                repo_root: args.repo_root.as_deref().map(mode::absolute).transpose()?,
            };
            match remote::run_job(&mut client, JobSpec::Status(input), args.detach, args.json)? {
                Remote::Detached(job) => {
                    println!("{}", job.job_id);
                    return Ok(());
                }
                Remote::Finished(JobOutput::Status { report }) => report,
                Remote::Finished(other) => return Err(remote::unexpected(&other)),
                Remote::Failed(job) => {
                    return Err(match &job.error {
                        Some(e) if e.code == JobErrorCode::ManifestLoad => {
                            manifest_error(&args.manifest, &e.message)
                        }
                        _ => remote::failure(&job),
                    })
                }
            }
        }
    };

    print_report(&report, args.json)
}

pub fn print_report(report: &StatusReport, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", json::render_json(report)?);
    } else {
        println!("{}", human::render_status_report(report));
    }
    Ok(())
}
