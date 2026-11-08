use anyhow::Result;
use clap::Args;
use dotfilesync_core::{
    discover_homebrew, load_manifest, ConfigInspector, Diagnostic, RealCommandRunner, ScanReport,
};
use std::path::{Path, PathBuf};

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

    /// Override Homebrew binary path (primarily for testing and controlled environments)
    #[arg(long, hide = true)]
    pub brew_bin: Option<PathBuf>,
}

pub fn execute(args: ScanArgs) -> Result<()> {
    let mut runner = RealCommandRunner::default();
    let brew_bin = args
        .brew_bin
        .or_else(|| std::env::var_os("DOTFILESYNC_BREW_BIN").map(PathBuf::from));
    if let Some(bin) = brew_bin {
        runner = runner.with_brew_binary(bin);
    }
    let homebrew_result = discover_homebrew(&runner);

    let home_dir = args
        .home
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    // Check if manifest is specified or default `./dotfilesync.toml` exists
    let manifest_path = args.manifest.or_else(|| {
        let default_p = PathBuf::from("dotfilesync.toml");
        if default_p.is_file() {
            Some(default_p)
        } else {
            None
        }
    });

    let mut configs = Vec::new();
    let mut diagnostics = Vec::new();

    if let Some(manifest_path) = manifest_path {
        match load_manifest(&manifest_path) {
            Ok(manifest) => {
                let repo_root = args.repo_root.unwrap_or_else(|| {
                    manifest_path
                        .parent()
                        .map(Path::to_path_buf)
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or_else(|| PathBuf::from("."))
                });

                let inspector = ConfigInspector::new(
                    &repo_root,
                    &home_dir,
                    &manifest.exclusions,
                    &manifest.overrides,
                );
                configs = inspector.inspect_files(&manifest.files);
            }
            Err(e) => {
                diagnostics.push(
                    Diagnostic::error(format!(
                        "Failed to load manifest at '{}'",
                        manifest_path.display()
                    ))
                    .with_detail(e.to_string()),
                );
            }
        }
    } else {
        diagnostics.push(Diagnostic::info(
            "No manifest specified or found; only Homebrew packages were scanned. Use --manifest <path> to inspect tracked configs.",
        ));
    }

    let report = ScanReport::new(homebrew_result, configs, diagnostics);

    if args.json {
        println!("{}", json::render_json(&report)?);
    } else {
        println!("{}", human::render_scan_report(&report));
    }

    Ok(())
}
