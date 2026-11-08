pub mod brew;
pub mod comparison;
pub mod config;
pub mod diagnostic;
pub mod manifest;
pub mod scan;

pub use brew::{
    discover_homebrew, parse_brewfile, BrewDiscoveryResult, BrewfileParseResult, CommandOutput,
    CommandRunner, MockCommandRunner, ObservedPackage, PackageKind, RealCommandRunner, RunnerError,
    UnsupportedConstruct,
};
pub use comparison::{
    compare_status, ConfigDifference, DifferenceClassification, PackageDifference, StatusReport,
    StatusSummary,
};
pub use config::{compute_sha256, ConfigFileState, ConfigInspector, ObservedConfigFile};
pub use diagnostic::{Diagnostic, DiagnosticSeverity};
pub use manifest::{
    expand_target_path, load_manifest, parse_manifest, resolve_source_path, serialize_manifest,
    validate_manifest, ExclusionsSection, HomebrewSection, MachineOverride, Manifest,
    ManifestError, ManifestMetadata, PackageManagersSection, TrackedFile, ValidationError,
    CURRENT_SCHEMA_VERSION,
};
pub use scan::ScanReport;
