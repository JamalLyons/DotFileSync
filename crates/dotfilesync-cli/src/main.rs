#[cfg(not(target_os = "macos"))]
compile_error!("dotfilesync is designed exclusively for macOS in Milestone 1; building on Linux or Windows is unsupported.");

mod commands;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "dotfilesync",
    version,
    about = "macOS developer environment manager",
    long_about = "dotfilesync inspects, tracks, and safely reports drift across Homebrew packages and explicit configuration files.\n\nplan shows what would change. apply changes the machine only through dotfilesyncd, after approval, with backups and locks.\n\nscan, status, and plan use the per-user daemon when it is running and otherwise run in this process. apply requires the daemon. Nothing listens on the network."
)]
struct Cli {
    /// dotfilesyncd data directory (default: $DOTFILESYNC_DATA_DIR or ~/Library/Application Support/dotfilesync)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Discover installed Homebrew packages and inspect explicitly tracked configurations
    Scan(commands::scan::ScanArgs),

    /// Compare desired state from manifest with observed system state
    Status(commands::status::StatusArgs),

    /// Show the changes that would bring this machine in line with the manifest
    Plan(commands::plan::PlanArgs),

    /// Apply a reviewed plan through dotfilesyncd (requires the daemon)
    Apply(commands::apply::ApplyArgs),

    /// List or show apply runs recorded by dotfilesyncd
    Runs(commands::runs::RunsArgs),

    /// Statically parse and import package declarations from a Brewfile
    ImportBrewfile(commands::import::ImportArgs),

    /// Inspect, follow, or cancel jobs running in dotfilesyncd
    #[command(subcommand)]
    Jobs(commands::jobs::JobsCommand),

    /// Manage the per-user dotfilesyncd LaunchAgent (install never starts it; login start is opt-in)
    #[command(subcommand)]
    Service(commands::service::ServiceCommand),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = cli.data_dir.as_deref();

    match cli.command {
        Commands::Scan(args) => commands::scan::execute(args, data_dir),
        Commands::Status(args) => commands::status::execute(args, data_dir),
        Commands::Plan(args) => commands::plan::execute(args, data_dir),
        Commands::Apply(args) => commands::apply::execute(args, data_dir),
        Commands::Runs(args) => commands::runs::execute(args, data_dir),
        // Always local: writes only with --output, via create_new (never overwrites).
        Commands::ImportBrewfile(args) => commands::import::execute(args),
        Commands::Jobs(cmd) => commands::jobs::execute(cmd, data_dir),
        Commands::Service(cmd) => commands::service::execute(cmd, data_dir),
    }
}
