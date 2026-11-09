//! Per-user service lifecycle behind a small interface.
//!
//! macOS uses a launchd LaunchAgent ([`launchd::LaunchdAgent`]). A systemd user unit for the
//! headless Linux target (#12) would implement the same trait; it is not part of this change.

pub mod launchd;

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOptions {
    /// Absolute path of the `dotfilesyncd` binary the service runs.
    pub daemon_path: PathBuf,
    /// Opt-in: start the daemon automatically when the user logs in.
    pub start_at_login: bool,
    /// Optional non-default data directory passed to the daemon.
    pub data_dir: Option<PathBuf>,
    /// Replace an existing, different service definition.
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    Unchanged,
    Replaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    Unloaded,
    NotLoaded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub definition_path: PathBuf,
    pub installed: bool,
    /// `Some` only when installed.
    pub start_at_login: Option<bool>,
    /// Known to the service manager (launchd: bootstrapped into the GUI domain).
    pub loaded: bool,
    pub running: bool,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UninstallOutcome {
    pub unloaded: bool,
    pub removed_definition: bool,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("the service is not installed ({}); run `dotfilesync service install` first", path.display())]
    NotInstalled { path: PathBuf },
    #[error("a different service definition already exists at {}; re-run with --force to replace it", path.display())]
    DefinitionExists { path: PathBuf },
    #[error("{} is not a dotfilesync service definition (or is a symlink); refusing to modify it", path.display())]
    NotOurs { path: PathBuf },
    #[error("invalid daemon path {}: {reason}", path.display())]
    InvalidDaemonPath { path: PathBuf, reason: String },
    #[error("`{command}` failed: {message}")]
    Command { command: String, message: String },
    #[error("i/o error on {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Lifecycle operations. Implementations must never start the daemon from `install`, never
/// enable start-at-login unless asked, and never delete user data from `uninstall`.
pub trait ServiceManager {
    fn install(&self, opts: &InstallOptions) -> Result<InstallOutcome, ServiceError>;
    fn start(&self) -> Result<(), ServiceError>;
    fn stop(&self) -> Result<StopOutcome, ServiceError>;
    fn status(&self) -> Result<ServiceStatus, ServiceError>;
    fn uninstall(&self) -> Result<UninstallOutcome, ServiceError>;
}
