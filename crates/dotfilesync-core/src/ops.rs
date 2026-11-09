//! Read-only operations shared by every execution host.
//!
//! The CLI's standalone mode and `dotfilesyncd` both call these functions, so scan and status
//! behave identically regardless of where they run. Hosts supply a [`CommandRunner`], a progress
//! callback, and a cancellation check; this module performs no I/O beyond what the existing core
//! discovery and inspection functions already do.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::brew::{discover_homebrew, CommandRunner};
use crate::comparison::{compare_status, StatusReport};
use crate::config::ConfigInspector;
use crate::diagnostic::Diagnostic;
use crate::manifest::{
    current_hostname, current_os, load_manifest, resolve_profile, Manifest, ManifestError,
    ProfileSelection, ResolvedManifest,
};
use crate::scan::ScanReport;

pub use crate::plan::run_plan;

/// Stages reported through the progress callback, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpStage {
    LoadingManifest,
    DiscoveringHomebrew,
    InspectingConfigs,
    Comparing,
    Planning,
}

impl OpStage {
    pub fn describe(self) -> &'static str {
        match self {
            OpStage::LoadingManifest => "Loading manifest",
            OpStage::DiscoveringHomebrew => "Discovering installed Homebrew packages",
            OpStage::InspectingConfigs => "Inspecting tracked configuration files",
            OpStage::Comparing => "Comparing desired and observed state",
            OpStage::Planning => "Planning changes",
        }
    }
}

#[derive(Debug, Error)]
pub enum OpError {
    #[error("operation was cancelled")]
    Cancelled,

    #[error("failed to load manifest '{path}': {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: ManifestError,
    },

    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanInput {
    /// Manifest describing tracked configs. `None` scans Homebrew only.
    pub manifest: Option<PathBuf>,
    pub home: PathBuf,
    /// Defaults to the manifest's parent directory, or `.` if it has none.
    pub repo_root: Option<PathBuf>,
    /// `None` selects a profile automatically (`[select]`, otherwise the base layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInput {
    pub manifest: PathBuf,
    pub home: PathBuf,
    pub repo_root: Option<PathBuf>,
    /// `None` selects a profile automatically (`[select]`, otherwise the base layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

fn default_repo_root(manifest: &Path, repo_root: Option<&Path>) -> PathBuf {
    repo_root.map(Path::to_path_buf).unwrap_or_else(|| {
        manifest
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

fn profile_selection(profile: &Option<String>) -> ProfileSelection {
    match profile {
        Some(name) => ProfileSelection::Explicit(name.clone()),
        None => ProfileSelection::Auto,
    }
}

fn materialized_manifest(original: &Manifest, resolved: &ResolvedManifest) -> Manifest {
    let mut manifest = Manifest::new(original.schema_version);
    manifest.metadata = original.metadata.clone();
    manifest.package_managers.homebrew = resolved.homebrew.clone();
    manifest.files = resolved.files.clone();
    manifest.exclusions = resolved.exclusions.clone();
    manifest.overrides = resolved.overrides.clone();
    manifest.setup = resolved.setup.clone();
    manifest
}

fn check(is_cancelled: &dyn Fn() -> bool) -> Result<(), OpError> {
    if is_cancelled() {
        Err(OpError::Cancelled)
    } else {
        Ok(())
    }
}

/// Discover Homebrew packages and inspect tracked configs (read-only).
///
/// A manifest that fails to load is reported as an error diagnostic in the returned report, as
/// the CLI has always done; it is not an `Err`.
pub fn run_scan(
    input: &ScanInput,
    runner: &dyn CommandRunner,
    progress: &mut dyn FnMut(OpStage),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<ScanReport, OpError> {
    check(is_cancelled)?;
    progress(OpStage::DiscoveringHomebrew);
    let homebrew = discover_homebrew(runner);
    check(is_cancelled)?;

    let mut configs = Vec::new();
    let mut diagnostics = Vec::new();

    match &input.manifest {
        Some(manifest_path) => {
            progress(OpStage::LoadingManifest);
            match load_manifest(manifest_path) {
                Ok(manifest) => {
                    check(is_cancelled)?;
                    progress(OpStage::InspectingConfigs);
                    match resolve_profile(
                        &manifest,
                        &profile_selection(&input.profile),
                        &current_hostname(),
                        current_os(),
                    ) {
                        Ok(resolved) => {
                            let repo_root =
                                default_repo_root(manifest_path, input.repo_root.as_deref());
                            let inspector = ConfigInspector::new(
                                &repo_root,
                                &input.home,
                                &resolved.exclusions,
                                &resolved.overrides,
                            );
                            configs = inspector.inspect_files(&resolved.files);
                        }
                        Err(error) => diagnostics.push(
                            Diagnostic::error(format!(
                                "Failed to resolve manifest profile at '{}'",
                                manifest_path.display()
                            ))
                            .with_detail(error.to_string()),
                        ),
                    }
                }
                Err(e) => diagnostics.push(
                    Diagnostic::error(format!(
                        "Failed to load manifest at '{}'",
                        manifest_path.display()
                    ))
                    .with_detail(e.to_string()),
                ),
            }
        }
        None => diagnostics.push(Diagnostic::info(
            "No manifest specified or found; only Homebrew packages were scanned. Use --manifest <path> to inspect tracked configs.",
        )),
    }

    check(is_cancelled)?;
    Ok(ScanReport::new(homebrew, configs, diagnostics))
}

/// Compare the manifest's desired state with the observed system (read-only).
pub fn run_status(
    input: &StatusInput,
    runner: &dyn CommandRunner,
    progress: &mut dyn FnMut(OpStage),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<StatusReport, OpError> {
    check(is_cancelled)?;
    progress(OpStage::LoadingManifest);
    let manifest = load_manifest(&input.manifest).map_err(|source| OpError::Manifest {
        path: input.manifest.clone(),
        source,
    })?;
    let resolved = resolve_profile(
        &manifest,
        &profile_selection(&input.profile),
        &current_hostname(),
        current_os(),
    )
    .map_err(|source| OpError::Manifest {
        path: input.manifest.clone(),
        source,
    })?;
    let manifest = materialized_manifest(&manifest, &resolved);

    check(is_cancelled)?;
    progress(OpStage::DiscoveringHomebrew);
    let brew = discover_homebrew(runner);

    check(is_cancelled)?;
    progress(OpStage::InspectingConfigs);
    let repo_root = default_repo_root(&input.manifest, input.repo_root.as_deref());
    let inspector = ConfigInspector::new(
        &repo_root,
        &input.home,
        &manifest.exclusions,
        &manifest.overrides,
    );
    let configs = inspector.inspect_files(&manifest.files);

    check(is_cancelled)?;
    progress(OpStage::Comparing);
    let report = compare_status(&manifest, &brew, &configs);
    check(is_cancelled)?;
    Ok(report)
}
