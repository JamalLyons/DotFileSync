mod commands;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "dotfilesync",
    version,
    about = "macOS developer environment manager (read-only foundation)",
    long_about = "dotfilesync inspects, tracks, and safely reports drift across Homebrew packages and explicit configuration files without modifying machine state."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Discover installed Homebrew packages and inspect explicitly tracked configurations
    Scan(commands::scan::ScanArgs),

    /// Compare desired state from manifest with observed system state
    Status(commands::status::StatusArgs),

    /// Statically parse and import package declarations from a Brewfile
    ImportBrewfile(commands::import::ImportArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan(args) => commands::scan::execute(args),
        Commands::Status(args) => commands::status::execute(args),
        Commands::ImportBrewfile(args) => commands::import::execute(args),
    }
}
