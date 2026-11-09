//! Small helpers for owner-only files, time and randomness.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use dotfilesync_protocol::current_euid;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `n` random bytes from the kernel CSPRNG, hex-encoded.
pub fn random_hex(n: usize) -> io::Result<String> {
    let mut buf = vec![0u8; n];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(hex::encode(buf))
}

pub enum OpenMode {
    Append,
    Truncate,
}

/// Open (creating with 0600 if needed) a file that must be private to the current user.
/// Symlinks are refused (`O_NOFOLLOW`), and an existing file that is not owned by us or is
/// readable by group/other is rejected rather than silently fixed.
pub fn open_private_file(path: &Path, mode: OpenMode) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    match mode {
        OpenMode::Append => opts.append(true),
        OpenMode::Truncate => opts.write(true).truncate(true),
    };
    let file = opts.open(path)?;
    verify_private(&file, path)?;
    Ok(file)
}

pub fn open_private_read(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    verify_private(&file, path)?;
    Ok(file)
}

fn verify_private(file: &File, path: &Path) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("'{}' is not a regular file", path.display()),
        ));
    }
    if meta.uid() != current_euid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("'{}' is owned by uid {}", path.display(), meta.uid()),
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "'{}' has mode {:04o}; expected owner-only (0600)",
                path.display(),
                meta.mode() & 0o7777
            ),
        ));
    }
    Ok(())
}
