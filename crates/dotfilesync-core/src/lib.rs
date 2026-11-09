// The core library is portable across Unix so the daemon protocol and a future headless Linux
// target (#12) can build on it. The shipped CLI and daemon binaries remain macOS-only.
#[cfg(not(unix))]
compile_error!("dotfilesync-core requires a Unix platform; Windows is unsupported.");

pub mod apply;
pub mod brew;
pub mod comparison;
pub mod config;
pub mod diagnostic;
pub mod manifest;
pub mod ops;
pub mod plan;
pub mod scan;

pub use apply::{
    apply_plan, resume_run, ApplyContext, ApplyError, RunJournal, StepRecord, StepStatus,
};
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
    current_hostname, current_os, expand_target_path, load_manifest, normalize_hostname,
    parse_manifest, resolve_profile, resolve_source_path, serialize_manifest, validate_manifest,
    ExclusionsSection, FileMode, HomebrewSection, MachineOverride, Manifest, ManifestError,
    ManifestMetadata, PackageManagersSection, Profile, ProfileHomebrew, ProfilePackageManagers,
    ProfileSelection, ResolvedManifest, SelectSection, SetupOverlay, SetupSection, TargetOs,
    TrackedFile, ValidationError, CURRENT_SCHEMA_VERSION,
};
pub use ops::{run_plan, run_scan, run_status, OpError, OpStage, ScanInput, StatusInput};
pub use plan::{ActionKind, Plan, PlanAction, PlanInput, Risk, PLAN_SCHEMA};
pub use scan::ScanReport;
