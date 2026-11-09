//! Bounded, rotated, redacted daemon log.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::fsutil::{now_ms, open_private_file, OpenMode};
use crate::redact::redact;

/// Largest single log line; longer lines are truncated.
const MAX_LINE: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

pub struct Logger {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    inner: Mutex<Option<(File, u64)>>,
}

impl Logger {
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> std::io::Result<Self> {
        let file = open_private_file(path, OpenMode::Append)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes: max_bytes.max(256),
            keep: keep.max(1),
            inner: Mutex::new(Some((file, size))),
        })
    }

    /// A logger that discards everything (used when the log file cannot be opened).
    pub fn disabled() -> Self {
        Self {
            path: PathBuf::new(),
            max_bytes: 0,
            keep: 0,
            inner: Mutex::new(None),
        }
    }

    pub fn info(&self, msg: &str) {
        self.write(Level::Info, msg)
    }
    pub fn warn(&self, msg: &str) {
        self.write(Level::Warn, msg)
    }
    pub fn error(&self, msg: &str) {
        self.write(Level::Error, msg)
    }

    pub fn write(&self, level: Level, msg: &str) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let Some((_, size)) = guard.as_ref() else {
            return;
        };
        let mut clean: String = msg
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        if clean.len() > MAX_LINE {
            let mut end = MAX_LINE;
            while !clean.is_char_boundary(end) {
                end -= 1;
            }
            clean.truncate(end);
            clean.push_str(" …[truncated]");
        }
        let line = format!("{} {} {}\n", now_ms(), level.as_str(), redact(&clean));
        if *size + line.len() as u64 > self.max_bytes {
            *guard = self.rotate();
        }
        if let Some((file, size)) = guard.as_mut() {
            if file.write_all(line.as_bytes()).is_ok() {
                *size += line.len() as u64;
            }
        }
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut s = self.path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    }

    fn rotate(&self) -> Option<(File, u64)> {
        let _ = fs::remove_file(self.rotated(self.keep));
        for n in (1..self.keep).rev() {
            let _ = fs::rename(self.rotated(n), self.rotated(n + 1));
        }
        let _ = fs::rename(&self.path, self.rotated(1));
        open_private_file(&self.path, OpenMode::Append)
            .ok()
            .map(|f| (f, 0))
    }
}
