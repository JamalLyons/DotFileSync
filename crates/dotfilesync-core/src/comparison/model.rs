use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::brew::PackageKind;
use crate::config::ConfigFileState;
use crate::diagnostic::Diagnostic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DifferenceClassification {
    Matching,
    Missing,
    Changed,
    Unmanaged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDifference {
    pub name: String,
    pub kind: PackageKind,
    pub classification: DifferenceClassification,
    pub desired: bool,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDifference {
    pub source: PathBuf,
    pub target: PathBuf,
    pub classification: DifferenceClassification,
    pub state: ConfigFileState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSummary {
    pub packages_matching: usize,
    pub packages_missing: usize,
    pub packages_unmanaged: usize,
    pub packages_unknown: usize,
    pub configs_matching: usize,
    pub configs_changed: usize,
    pub configs_missing: usize,
    pub configs_unknown: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    pub schema_version: u32,
    pub homebrew_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brew_version: Option<String>,
    pub package_differences: Vec<PackageDifference>,
    pub config_differences: Vec<ConfigDifference>,
    pub diagnostics: Vec<Diagnostic>,
    pub summary: StatusSummary,
}
