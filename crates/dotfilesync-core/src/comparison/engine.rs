use std::collections::{HashMap, HashSet};

use super::model::{
    ConfigDifference, DifferenceClassification, PackageDifference, StatusReport, StatusSummary,
};
use crate::brew::{BrewDiscoveryResult, PackageKind};
use crate::config::{ConfigFileState, ObservedConfigFile};
use crate::manifest::Manifest;

pub fn compare_status(
    manifest: &Manifest,
    brew_result: &BrewDiscoveryResult,
    configs: &[ObservedConfigFile],
) -> StatusReport {
    let mut package_differences = Vec::new();
    let mut summary = StatusSummary::default();
    let mut diagnostics = brew_result.diagnostics.clone();

    // 1. Process Packages
    if !brew_result.available {
        // Homebrew is not available: MUST NOT falsely claim packages are missing.
        for f in &manifest.package_managers.homebrew.formulae {
            package_differences.push(PackageDifference {
                name: f.clone(),
                kind: PackageKind::Formula,
                classification: DifferenceClassification::Unknown,
                desired: true,
                installed: false,
                observed_version: None,
                diagnostic: Some("Homebrew is unavailable; status cannot be verified".to_string()),
            });
            summary.packages_unknown += 1;
        }

        for c in &manifest.package_managers.homebrew.casks {
            package_differences.push(PackageDifference {
                name: c.clone(),
                kind: PackageKind::Cask,
                classification: DifferenceClassification::Unknown,
                desired: true,
                installed: false,
                observed_version: None,
                diagnostic: Some("Homebrew is unavailable; status cannot be verified".to_string()),
            });
            summary.packages_unknown += 1;
        }
    } else {
        let installed_formulae: HashMap<String, _> = brew_result
            .formulae
            .iter()
            .map(|p| (p.name.clone(), p))
            .collect();

        let installed_casks: HashMap<String, _> = brew_result
            .casks
            .iter()
            .map(|p| (p.name.clone(), p))
            .collect();

        let mut desired_formulae = HashSet::new();
        for f in &manifest.package_managers.homebrew.formulae {
            desired_formulae.insert(f.clone());
            if let Some(pkg) = installed_formulae.get(f) {
                package_differences.push(PackageDifference {
                    name: f.clone(),
                    kind: PackageKind::Formula,
                    classification: DifferenceClassification::Matching,
                    desired: true,
                    installed: true,
                    observed_version: pkg.version.clone(),
                    diagnostic: None,
                });
                summary.packages_matching += 1;
            } else {
                package_differences.push(PackageDifference {
                    name: f.clone(),
                    kind: PackageKind::Formula,
                    classification: DifferenceClassification::Missing,
                    desired: true,
                    installed: false,
                    observed_version: None,
                    diagnostic: Some(
                        "Formula is declared in manifest but not installed".to_string(),
                    ),
                });
                summary.packages_missing += 1;
            }
        }

        // Unmanaged formulae
        for (name, pkg) in &installed_formulae {
            if !desired_formulae.contains(name) {
                package_differences.push(PackageDifference {
                    name: name.clone(),
                    kind: PackageKind::Formula,
                    classification: DifferenceClassification::Unmanaged,
                    desired: false,
                    installed: true,
                    observed_version: pkg.version.clone(),
                    diagnostic: Some(
                        "Installed on system but not declared in manifest".to_string(),
                    ),
                });
                summary.packages_unmanaged += 1;
            }
        }

        let mut desired_casks = HashSet::new();
        for c in &manifest.package_managers.homebrew.casks {
            desired_casks.insert(c.clone());
            if let Some(pkg) = installed_casks.get(c) {
                package_differences.push(PackageDifference {
                    name: c.clone(),
                    kind: PackageKind::Cask,
                    classification: DifferenceClassification::Matching,
                    desired: true,
                    installed: true,
                    observed_version: pkg.version.clone(),
                    diagnostic: None,
                });
                summary.packages_matching += 1;
            } else {
                package_differences.push(PackageDifference {
                    name: c.clone(),
                    kind: PackageKind::Cask,
                    classification: DifferenceClassification::Missing,
                    desired: true,
                    installed: false,
                    observed_version: None,
                    diagnostic: Some("Cask is declared in manifest but not installed".to_string()),
                });
                summary.packages_missing += 1;
            }
        }

        // Unmanaged casks
        for (name, pkg) in &installed_casks {
            if !desired_casks.contains(name) {
                package_differences.push(PackageDifference {
                    name: name.clone(),
                    kind: PackageKind::Cask,
                    classification: DifferenceClassification::Unmanaged,
                    desired: false,
                    installed: true,
                    observed_version: pkg.version.clone(),
                    diagnostic: Some(
                        "Installed on system but not declared in manifest".to_string(),
                    ),
                });
                summary.packages_unmanaged += 1;
            }
        }
    }

    // 2. Process Config Differences
    let mut config_differences = Vec::new();
    for observed in configs {
        if (observed.state == ConfigFileState::Unreadable
            || observed.state == ConfigFileState::SourceMissing)
            && observed.diagnostic.is_some()
        {
            if let Some(err) = &observed.diagnostic {
                diagnostics.push(crate::diagnostic::Diagnostic::warning(format!(
                    "Config file '{}': {}",
                    observed.target_path.display(),
                    err
                )));
            }
        }

        let (classification, diag) = match observed.state {
            ConfigFileState::Matching | ConfigFileState::SymlinkToSource => {
                summary.configs_matching += 1;
                (
                    DifferenceClassification::Matching,
                    observed.diagnostic.clone(),
                )
            }
            ConfigFileState::MachineLocal => {
                summary.configs_matching += 1;
                (
                    DifferenceClassification::Matching,
                    Some("Preserved as machine-local override (not modified)".to_string()),
                )
            }
            ConfigFileState::Excluded => {
                // Excluded files are treated as matching / ignored
                summary.configs_matching += 1;
                (
                    DifferenceClassification::Matching,
                    Some("Excluded by manifest rule".to_string()),
                )
            }
            ConfigFileState::Changed
            | ConfigFileState::SymlinkExternal
            | ConfigFileState::BrokenSymlink => {
                summary.configs_changed += 1;
                (
                    DifferenceClassification::Changed,
                    observed.diagnostic.clone(),
                )
            }
            ConfigFileState::Missing => {
                summary.configs_missing += 1;
                (
                    DifferenceClassification::Missing,
                    observed.diagnostic.clone(),
                )
            }
            ConfigFileState::SourceMissing | ConfigFileState::Unreadable => {
                summary.configs_unknown += 1;
                (
                    DifferenceClassification::Unknown,
                    observed.diagnostic.clone(),
                )
            }
        };

        config_differences.push(ConfigDifference {
            source: observed.source_path.clone(),
            target: observed.target_path.clone(),
            classification,
            state: observed.state.clone(),
            source_hash: observed.source_hash.clone(),
            target_hash: observed.target_hash.clone(),
            diagnostic: diag,
        });
    }

    // Sort for deterministic output
    package_differences.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.classification.cmp(&b.classification))
            .then_with(|| a.name.cmp(&b.name))
    });

    config_differences.sort_by(|a, b| a.target.cmp(&b.target));

    StatusReport {
        schema_version: manifest.schema_version,
        homebrew_available: brew_result.available,
        brew_version: brew_result.brew_version.clone(),
        package_differences,
        config_differences,
        diagnostics,
        summary,
    }
}
