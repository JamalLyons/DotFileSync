use anyhow::Result;
use clap::Args;
use dotfilesync_core::{run_scan, RealCommandRunner, ScanInput, ScanReport};
use dotfilesync_protocol::{JobOutput, JobSpec};
use std::path::{Path, PathBuf};

use super::mode::{self, Mode, Route};
use super::remote::{self, Remote};
use crate::output::{human, json};

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Path to manifest file for tracked configurations (optional)
    #[arg(short, long)]
    pub manifest: Option<PathBuf>,

    /// Output report in JSON format
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

pub fn execute(args: ScanArgs, data_dir: Option<&Path>) -> Result<()> {
    let brew_bin = args
        .brew_bin
        .clone()
        .or_else(|| std::env::var_os("DOTFILESYNC_BREW_BIN").map(PathBuf::from));
    let home = args
        .home
        .clone()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    // Use the explicit manifest, or `./dotfilesync.toml` if it exists.
    let manifest = args.manifest.clone().or_else(|| {
        let default_p = PathBuf::from("dotfilesync.toml");
        default_p.is_file().then_some(default_p)
    });

    let standalone_only = brew_bin.as_ref().map(|_| "--brew-bin/DOTFILESYNC_BREW_BIN");
    let report: ScanReport = match mode::route(args.mode, data_dir, "scan", standalone_only)? {
        Route::Standalone { note } => {
            if let Some(n) = note {
                eprintln!("{n}");
            }
            let mut runner = RealCommandRunner::default();
            if let Some(bin) = brew_bin {
                runner = runner.with_brew_binary(bin);
            }
            let input = ScanInput {
                manifest,
                home,
                repo_root: args.repo_root.clone(),
            };
            run_scan(&input, &runner, &mut |_| {}, &|| false)?
        }
        Route::Daemon(mut client) => {
            let input = ScanInput {
                manifest: manifest.as_deref().map(mode::absolute).transpose()?,
                home: mode::absolute(&home)?,
                repo_root: args.repo_root.as_deref().map(mode::absolute).transpose()?,
            };
            match remote::run_job(&mut client, JobSpec::Scan(input), args.detach, args.json)? {
                Remote::Detached(job) => {
                    println!("{}", job.job_id);
                    return Ok(());
                }
                Remote::Finished(JobOutput::Scan { report }) => report,
                Remote::Finished(other) => return Err(remote::unexpected(&other)),
                Remote::Failed(job) => return Err(remote::failure(&job)),
            }
        }
    };

    print_report(&report, args.json)
}

pub fn print_report(report: &ScanReport, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", json::render_json(report)?);
    } else {
        println!("{}", human::render_scan_report(report));
    }
    Ok(())
}
