use anyhow::{Context, Result};
use clap::Args;
use dotfilesync_core::{
    compare_status, discover_homebrew, load_manifest, ConfigInspector, RealCommandRunner,
};
use std::path::{Path, PathBuf};

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
}

pub fn execute(args: StatusArgs) -> Result<()> {
    let manifest = load_manifest(&args.manifest)
        .with_context(|| format!("Failed to read manifest from '{}'", args.manifest.display()))?;

    let home_dir = args
        .home
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    let repo_root = args.repo_root.unwrap_or_else(|| {
        args.manifest
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("."))
    });

    let runner = RealCommandRunner::default();
    let brew_result = discover_homebrew(&runner);

    let inspector = ConfigInspector::new(
        &repo_root,
        &home_dir,
        &manifest.exclusions,
        &manifest.overrides,
    );
    let configs = inspector.inspect_files(&manifest.files);

    let status_report = compare_status(&manifest, &brew_result, &configs);

    if args.json {
        println!("{}", json::render_json(&status_report)?);
    } else {
        println!("{}", human::render_status_report(&status_report));
    }

    Ok(())
}
