use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Create `path` and any missing parents with mode `0755`.
pub fn ensure_dir(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path_present(path) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_dir(parent)?;
        }
    }
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o755);
    }
    builder
        .create(path)
        .map_err(|error| format!("failed to create directory '{}': {error}", path.display()))
}

pub fn write_atomic(target: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        ensure_dir(parent)?;
    }
    let file_name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("file"));
    let mut tmp_name = file_name;
    tmp_name.push(".tmp");
    let tmp = match parent {
        Some(parent) => parent.join(&tmp_name),
        None => PathBuf::from(&tmp_name),
    };
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|error| format!("failed to open temp file '{}': {error}", tmp.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write '{}': {error}", tmp.display()))?;
        file.sync_all()
            .map_err(|error| format!("failed to fsync '{}': {error}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(mode);
            fs::set_permissions(&tmp, perms)
                .map_err(|error| format!("failed to set mode on '{}': {error}", tmp.display()))?;
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
        }
    }
    fs::rename(&tmp, target).map_err(|error| {
        format!(
            "failed to rename '{}' to '{}': {error}",
            tmp.display(),
            target.display()
        )
    })?;
    Ok(())
}

/// Move `target` into `<backup_root>/<run_id>/<path-relative-to-home>.backup.YYYYmmdd_HHMMSS`.
pub fn backup_existing(
    home: &Path,
    backup_root: &Path,
    run_id: &str,
    target: &Path,
    now_ms: u64,
) -> Result<PathBuf, String> {
    let relative = relative_to_home(home, target);
    let file_name = relative
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let stamped = format!("{file_name}.backup.{}", format_stamp(now_ms));
    let parent = relative.parent().unwrap_or(Path::new(""));
    let dest = backup_root.join(run_id).join(parent).join(stamped);
    if let Some(parent) = dest.parent() {
        ensure_dir(parent)?;
    }
    move_file(target, &dest)?;
    Ok(dest)
}

fn move_file(source: &Path, dest: &Path) -> Result<(), String> {
    match fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            fs::copy(source, dest).map_err(|copy_error| {
                format!(
                    "failed to copy '{}' to '{}': {copy_error}",
                    source.display(),
                    dest.display()
                )
            })?;
            fs::remove_file(source).map_err(|remove_error| {
                format!(
                    "failed to remove '{}' after backup: {remove_error}",
                    source.display()
                )
            })?;
            Ok(())
        }
        Err(error) => Err(format!(
            "failed to back up '{}' to '{}': {error}",
            source.display(),
            dest.display()
        )),
    }
}

fn relative_to_home(home: &Path, target: &Path) -> PathBuf {
    if let Ok(relative) = target.strip_prefix(home) {
        return relative.to_path_buf();
    }
    let mut relative = PathBuf::new();
    for component in target.components() {
        if let Component::Normal(part) = component {
            relative.push(part);
        }
    }
    relative
}

fn path_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn format_stamp(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}")
}

/// Howard Hinnant's `civil_from_days`, days counted from Unix epoch.
fn civil_from_days(unix_days: i64) -> (i32, u32, u32) {
    let z = unix_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (if month <= 2 { y + 1 } else { y }) as i32;
    (year, month, day)
}
