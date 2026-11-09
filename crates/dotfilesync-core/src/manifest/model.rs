use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomebrewSection {
    #[serde(default)]
    pub formulae: Vec<String>,
    #[serde(default)]
    pub casks: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManagersSection {
    #[serde(default)]
    pub homebrew: HomebrewSection,
}

/// A tracked configuration file entry mapping repository source to machine target.
///
/// Representation and Encoding:
/// TOML manifest files require UTF-8 encoded text. To allow manifests to reference arbitrary
/// non-UTF-8 native filesystem paths without violating the TOML specification or losing byte fidelity,
/// path strings support standard URI percent-encoding (`%HH`, e.g. `%FF`). Standard UTF-8 paths
/// are preserved directly as-is.
///
/// On macOS, the default APFS filesystem requires path components to be valid UTF-8 sequences and
/// returns `EILSEQ` (Errno 92) if non-UTF-8 file entries are created, which `ConfigInspector`
/// detects and reports as a specific diagnostic. Non-APFS volumes (such as FAT32, exFAT, or NFS)
/// support arbitrary byte paths on macOS. In all cases, native non-UTF-8 paths originating from
/// external environment roots (such as a non-UTF-8 base repo directory or HOME directory) or
/// decoded percent-encoded paths are preserved without lossy transformation during path resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedFile {
    /// Relative path within the repository (UTF-8)
    pub source: String,
    /// Destination path on the target machine (supports `~` expansion, UTF-8)
    pub target: String,
    /// Whether this file is expected to be a machine-local override (not synced)
    #[serde(default)]
    pub machine_local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionsSection {
    /// Exact file or directory paths to exclude
    #[serde(default)]
    pub paths: Vec<String>,
    /// Glob patterns to exclude (e.g. "*.backup.*", "*token*")
    #[serde(default)]
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineOverride {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ManifestMetadata>,
    #[serde(default)]
    pub package_managers: PackageManagersSection,
    #[serde(default)]
    pub files: Vec<TrackedFile>,
    #[serde(default)]
    pub exclusions: ExclusionsSection,
    #[serde(default)]
    pub overrides: Vec<MachineOverride>,
}

impl Manifest {
    pub fn new(schema_version: u32) -> Self {
        Self {
            schema_version,
            metadata: None,
            package_managers: PackageManagersSection::default(),
            files: Vec::new(),
            exclusions: ExclusionsSection::default(),
            overrides: Vec::new(),
        }
    }
}
