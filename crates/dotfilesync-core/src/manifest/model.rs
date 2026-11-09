use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Operating system a manifest entry can be conditioned on.
///
/// Only macOS is implemented. Other names parse and are then dropped during resolution
/// when they do not match the host [`TargetOs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetOs {
    Macos,
}

impl TargetOs {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetOs::Macos => "macos",
        }
    }

    pub fn matches(self, declared: &str) -> bool {
        declared.eq_ignore_ascii_case(self.as_str())
    }
}

/// How a tracked file is materialized on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FileMode {
    #[default]
    Copy,
    Symlink,
}

impl FileMode {
    pub fn is_copy(mode: &Self) -> bool {
        matches!(mode, FileMode::Copy)
    }
}

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

/// Package additions and removals contributed by a profile layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileHomebrew {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formulae: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_formulae: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub casks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_casks: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManagersSection {
    #[serde(default)]
    pub homebrew: HomebrewSection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePackageManagers {
    #[serde(default)]
    pub homebrew: ProfileHomebrew,
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
    /// `copy` (default) writes file bytes; `symlink` points the target at the source.
    #[serde(default, skip_serializing_if = "FileMode::is_copy")]
    pub mode: FileMode,
    /// When set, the entry is kept only when it matches the host OS (`macos`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

/// Scalar setup keys. Absent profile fields do not override an earlier layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_shell: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fisher: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Profile overlay for [`SetupSection`]. `None` means "leave the earlier layer".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupOverlay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fisher: Option<bool>,
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

/// Optional hostname → profile mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectSection {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hostnames: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// One named overlay applied after the base manifest (and after its `extends` chain).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    #[serde(default)]
    pub package_managers: ProfilePackageManagers,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<TrackedFile>,
    /// Removes a file entry by its manifest target path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_files: Vec<String>,
    #[serde(default)]
    pub setup: SetupOverlay,
    #[serde(default)]
    pub exclusions: ExclusionsSection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<MachineOverride>,
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
    #[serde(default)]
    pub select: SelectSection,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub setup: SetupSection,
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
            select: SelectSection::default(),
            profiles: BTreeMap::new(),
            setup: SetupSection::default(),
        }
    }
}
