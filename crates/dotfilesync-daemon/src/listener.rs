//! Single-instance lock and safe Unix-socket binding.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use dotfilesync_protocol::current_euid;

use crate::DaemonError;

/// Holds an exclusive `flock` on the run directory's lock file for the daemon's lifetime.
/// The kernel drops it when the process exits for any reason, so it can never go stale.
pub struct InstanceLock {
    _file: File,
}

pub fn acquire_instance_lock(path: &Path) -> Result<InstanceLock, DaemonError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| DaemonError::io(path, e))?;
    let meta = file.metadata().map_err(|e| DaemonError::io(path, e))?;
    if !meta.is_file() || meta.uid() != current_euid() || meta.mode() & 0o077 != 0 {
        return Err(DaemonError::UnsafePath {
            path: path.to_path_buf(),
            reason: "lock file must be a regular 0600 file owned by the current user".into(),
        });
    }
    // SAFETY: valid fd owned by `file`.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(DaemonError::AlreadyRunning {
                lock: path.to_path_buf(),
            });
        }
        return Err(DaemonError::io(path, err));
    }
    Ok(InstanceLock { _file: file })
}

/// A bound listener that removes its socket on drop only if the path still refers to the
/// socket inode this process created.
pub struct BoundSocket {
    pub listener: UnixListener,
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl BoundSocket {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Bind `path`. Requires the instance lock, which proves no other daemon for this data dir is
/// alive. An existing path is removed only if it is a socket we own that refuses connections.
pub fn bind_socket(path: &Path, _lock: &InstanceLock) -> Result<BoundSocket, DaemonError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_symlink() || !ft.is_socket() {
                return Err(DaemonError::UnsafePath {
                    path: path.to_path_buf(),
                    reason: "exists and is not a socket; refusing to remove it".into(),
                });
            }
            if meta.uid() != current_euid() {
                return Err(DaemonError::UnsafePath {
                    path: path.to_path_buf(),
                    reason: format!(
                        "socket is owned by uid {}; refusing to remove it",
                        meta.uid()
                    ),
                });
            }
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(DaemonError::UnsafePath {
                        path: path.to_path_buf(),
                        reason: "another process is listening on this socket".into(),
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                    fs::remove_file(path).map_err(|e| DaemonError::io(path, e))?;
                }
                Err(e) => return Err(DaemonError::io(path, e)),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(DaemonError::io(path, e)),
    }

    let listener = UnixListener::bind(path).map_err(|e| DaemonError::io(path, e))?;
    // The run dir is 0700, so nobody else can reach the socket in the instant before chmod.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| DaemonError::io(path, e))?;
    let meta = fs::symlink_metadata(path).map_err(|e| DaemonError::io(path, e))?;
    if !meta.file_type().is_socket() || meta.uid() != current_euid() || meta.mode() & 0o777 != 0o600
    {
        return Err(DaemonError::UnsafePath {
            path: path.to_path_buf(),
            reason: "socket permissions could not be verified as 0600".into(),
        });
    }
    Ok(BoundSocket {
        listener,
        path: path.to_path_buf(),
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        if let Ok(meta) = fs::symlink_metadata(&self.path) {
            if meta.dev() == self.dev && meta.ino() == self.ino {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}
