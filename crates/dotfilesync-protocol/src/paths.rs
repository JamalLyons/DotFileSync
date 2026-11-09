//! Per-user data directory layout and owner-only permission checks.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::peer::current_euid;

/// Environment variable that relocates the whole data directory (used by tests and power users).
pub const DATA_DIR_ENV: &str = "DOTFILESYNC_DATA_DIR";

/// macOS `sun_path` is 104 bytes including the terminating NUL.
#[cfg(target_os = "macos")]
pub const MAX_SOCKET_PATH: usize = 103;
#[cfg(not(target_os = "macos"))]
pub const MAX_SOCKET_PATH: usize = 107;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("cannot determine the data directory: HOME is not set; pass --data-dir or set {DATA_DIR_ENV}")]
    NoHome,
    #[error("'{path}' is a symlink; refusing to use it as a private dotfilesync directory")]
    Symlink { path: PathBuf },
    #[error("'{path}' exists but is not a directory")]
    NotADirectory { path: PathBuf },
    #[error("'{path}' is owned by uid {owner}, not the current user (uid {expected}); refusing to use it")]
    WrongOwner {
        path: PathBuf,
        owner: u32,
        expected: u32,
    },
    #[error("'{path}' has mode {mode:04o}, which allows access by other users; run `chmod 700 '{path}'` and retry")]
    TooPermissive { path: PathBuf, mode: u32 },
    #[error("socket path '{path}' is {len} bytes, longer than the platform limit of {max}; use a shorter --data-dir")]
    SocketPathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
    #[error("i/o error on '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl PathError {
    fn io(path: &Path, source: io::Error) -> Self {
        PathError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// All daemon paths derived from one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    pub root: PathBuf,
    pub run_dir: PathBuf,
    pub socket: PathBuf,
    pub lock_file: PathBuf,
    pub state_dir: PathBuf,
    pub journal: PathBuf,
    pub logs_dir: PathBuf,
    pub log_file: PathBuf,
}

impl DataLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let run_dir = root.join("run");
        let state_dir = root.join("state");
        let logs_dir = root.join("logs");
        Self {
            socket: run_dir.join("dotfilesyncd.sock"),
            lock_file: run_dir.join("dotfilesyncd.lock"),
            journal: state_dir.join("jobs.jsonl"),
            log_file: logs_dir.join("dotfilesyncd.log"),
            root,
            run_dir,
            state_dir,
            logs_dir,
        }
    }

    /// Resolve the data directory: explicit override, then `$DOTFILESYNC_DATA_DIR`, then the
    /// platform default. Relative paths are made absolute against the current directory.
    pub fn resolve(explicit: Option<&Path>) -> Result<Self, PathError> {
        let root = match explicit {
            Some(p) => p.to_path_buf(),
            None => match std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
                Some(v) => PathBuf::from(v),
                None => default_root()?,
            },
        };
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(|e| PathError::io(&root, e))?
                .join(root)
        };
        Ok(Self::new(root))
    }

    pub fn check_socket_path_len(&self) -> Result<(), PathError> {
        let len = self.socket.as_os_str().len();
        if len > MAX_SOCKET_PATH {
            return Err(PathError::SocketPathTooLong {
                path: self.socket.clone(),
                len,
                max: MAX_SOCKET_PATH,
            });
        }
        Ok(())
    }

    /// Create (if needed) and validate every private directory. Daemon-side only.
    pub fn ensure_private_dirs(&self) -> Result<(), PathError> {
        for dir in [&self.root, &self.run_dir, &self.state_dir, &self.logs_dir] {
            ensure_private_dir(dir)?;
        }
        self.check_socket_path_len()
    }
}

#[cfg(target_os = "macos")]
fn default_root() -> Result<PathBuf, PathError> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or(PathError::NoHome)?;
    Ok(PathBuf::from(home).join("Library/Application Support/dotfilesync"))
}

#[cfg(not(target_os = "macos"))]
fn default_root() -> Result<PathBuf, PathError> {
    // Placeholder for the headless Linux target (#12): XDG state dir semantics.
    if let Some(state) = std::env::var_os("XDG_STATE_HOME").filter(|h| !h.is_empty()) {
        return Ok(PathBuf::from(state).join("dotfilesync"));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or(PathError::NoHome)?;
    Ok(PathBuf::from(home).join(".local/state/dotfilesync"))
}

/// Outcome of a non-creating check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirState {
    Missing,
    Private,
}

/// Verify (without creating or modifying) that `path` is a real directory owned by the
/// effective UID with no group/other permission bits. Symlinks are never followed.
pub fn check_private_dir(path: &Path) -> Result<DirState, PathError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DirState::Missing),
        Err(e) => return Err(PathError::io(path, e)),
    };
    if meta.file_type().is_symlink() {
        return Err(PathError::Symlink {
            path: path.to_path_buf(),
        });
    }
    if !meta.is_dir() {
        return Err(PathError::NotADirectory {
            path: path.to_path_buf(),
        });
    }
    let expected = current_euid();
    if meta.uid() != expected {
        return Err(PathError::WrongOwner {
            path: path.to_path_buf(),
            owner: meta.uid(),
            expected,
        });
    }
    let mode = meta.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(PathError::TooPermissive {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(DirState::Private)
}

/// Create `path` with mode 0700 if missing (parents too), then validate it. Never relaxes or
/// silently repairs the permissions of an existing directory.
pub fn ensure_private_dir(path: &Path) -> Result<(), PathError> {
    if check_private_dir(path)? == DirState::Missing {
        match fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
        {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(PathError::io(path, e)),
        }
        // DirBuilder's mode is subject to umask, which can only remove bits; re-check below.
    }
    match check_private_dir(path)? {
        DirState::Private => Ok(()),
        DirState::Missing => Err(PathError::io(
            path,
            io::Error::new(io::ErrorKind::NotFound, "directory vanished after creation"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn creates_private_dirs_with_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(tmp.path().join("data"));
        layout.ensure_private_dirs().unwrap();
        for d in [
            &layout.root,
            &layout.run_dir,
            &layout.state_dir,
            &layout.logs_dir,
        ] {
            let mode = fs::metadata(d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", d.display());
        }
    }

    #[test]
    fn refuses_permissive_existing_dir_without_changing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("open");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            ensure_private_dir(&dir),
            Err(PathError::TooPermissive { mode: 0o755, .. })
        ));
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn refuses_symlinked_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            check_private_dir(&link),
            Err(PathError::Symlink { .. })
        ));
    }

    #[test]
    fn refuses_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("file");
        fs::write(&f, b"x").unwrap();
        assert!(matches!(
            check_private_dir(&f),
            Err(PathError::NotADirectory { .. })
        ));
    }

    #[test]
    fn missing_is_reported_not_created() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nope");
        assert_eq!(check_private_dir(&p).unwrap(), DirState::Missing);
        assert!(!p.exists());
    }

    #[test]
    fn long_socket_path_rejected() {
        let layout = DataLayout::new(PathBuf::from("/").join("a".repeat(200)));
        assert!(matches!(
            layout.check_socket_path_len(),
            Err(PathError::SocketPathTooLong { .. })
        ));
    }
}
