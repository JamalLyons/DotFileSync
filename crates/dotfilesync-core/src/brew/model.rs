use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Formula,
    Cask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedPackage {
    pub name: String,
    pub kind: PackageKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrewDiscoveryResult {
    /// Whether the Homebrew executable was located
    pub available: bool,
    /// Whether package inventory enumeration succeeded
    pub packages_enumerated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brew_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brew_version: Option<String>,
    pub formulae: Vec<ObservedPackage>,
    pub casks: Vec<ObservedPackage>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedConstruct {
    pub line_number: usize,
    pub raw_line: String,
    pub construct_type: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrewfileParseResult {
    pub formulae: Vec<String>,
    pub casks: Vec<String>,
    pub unsupported_constructs: Vec<UnsupportedConstruct>,
    pub diagnostics: Vec<Diagnostic>,
}
