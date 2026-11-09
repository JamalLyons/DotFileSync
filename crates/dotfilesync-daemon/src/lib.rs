//! `dotfilesyncd`: the per-user dotfilesync daemon.
//!
//! The daemon hosts `dotfilesync-core` behind a Unix-domain socket in a private per-user
//! directory. Business logic stays in core (`dotfilesync_core::ops`); this crate only adds
//! transport, authentication, job management, locking, approval bookkeeping, state and service
//! management. See `docs/daemon-architecture.md`.

pub mod approval;
pub mod fsutil;
pub mod jobs;
pub mod journal;
pub mod listener;
pub mod locks;
pub mod log;
pub mod redact;
pub mod server;
pub mod service;

use std::path::{Path, PathBuf};
use thiserror::Error;

pub use server::{
    ConnectionLimits, Daemon, DaemonConfig, OsPeerAuthenticator, PeerAuthenticator, ShutdownHandle,
};

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("another dotfilesyncd already owns this data directory (lock {}); not starting a second instance", lock.display())]
    AlreadyRunning { lock: PathBuf },
    #[error("unsafe path '{}': {reason}", path.display())]
    UnsafePath { path: PathBuf, reason: String },
    #[error(transparent)]
    Path(#[from] dotfilesync_protocol::PathError),
    #[error("i/o error on '{}': {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl DaemonError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        DaemonError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
