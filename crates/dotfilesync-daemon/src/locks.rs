//! Per-resource shared/exclusive locks.
//!
//! Reads (scan/status) take shared locks and run concurrently. Future mutating operations take
//! exclusive locks, which wait only for holders of the *same* resource. A request for several
//! resources is granted all-or-nothing under one mutex, so lock ordering cannot deadlock.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

/// Resource identifiers. Keep these coarse and typed rather than free-form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceKey {
    Homebrew,
    Manifest(std::path::PathBuf),
    /// The home directory an apply run mutates. Never taken from a free-form string.
    Home(std::path::PathBuf),
    /// Test-only resources. Never derived from client-supplied strings.
    Named(&'static str),
}

#[derive(Default)]
struct Entry {
    readers: usize,
    writer: bool,
    waiting_writers: usize,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<ResourceKey, Entry>,
}

#[derive(Default)]
pub struct ResourceLocks {
    inner: Mutex<Inner>,
    cv: Condvar,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LockCancelled;

pub struct LockGuard {
    locks: Arc<ResourceLocks>,
    held: Vec<(ResourceKey, LockMode)>,
}

impl ResourceLocks {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn grantable(inner: &Inner, reqs: &[(ResourceKey, LockMode)]) -> bool {
        reqs.iter().all(|(key, mode)| match inner.entries.get(key) {
            None => true,
            Some(e) => match mode {
                // Writer preference: new readers queue behind a waiting writer.
                LockMode::Shared => !e.writer && e.waiting_writers == 0,
                LockMode::Exclusive => !e.writer && e.readers == 0,
            },
        })
    }

    /// Block until every requested lock can be granted, or `cancelled()` returns true.
    pub fn acquire(
        self: &Arc<Self>,
        reqs: &[(ResourceKey, LockMode)],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<LockGuard, LockCancelled> {
        let mut reqs = reqs.to_vec();
        reqs.sort_by(|a, b| a.0.cmp(&b.0));
        reqs.dedup_by(|a, b| {
            if a.0 == b.0 {
                if a.1 == LockMode::Exclusive {
                    b.1 = LockMode::Exclusive;
                }
                true
            } else {
                false
            }
        });

        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut registered_wait = false;
        loop {
            // A request only registers waits on its exclusive keys, and exclusive checks ignore
            // `waiting_writers`, so a waiting request never blocks itself.
            if Self::grantable(&inner, &reqs) {
                if registered_wait {
                    Self::unregister_wait(&mut inner, &reqs);
                }
                for (key, mode) in &reqs {
                    let e = inner.entries.entry(key.clone()).or_default();
                    match mode {
                        LockMode::Shared => e.readers += 1,
                        LockMode::Exclusive => e.writer = true,
                    }
                }
                return Ok(LockGuard {
                    locks: Arc::clone(self),
                    held: reqs,
                });
            }
            if cancelled() {
                if registered_wait {
                    Self::unregister_wait(&mut inner, &reqs);
                    self.cv.notify_all();
                }
                return Err(LockCancelled);
            }
            if !registered_wait {
                for (key, mode) in &reqs {
                    if *mode == LockMode::Exclusive {
                        inner
                            .entries
                            .entry(key.clone())
                            .or_default()
                            .waiting_writers += 1;
                    }
                }
                registered_wait = true;
            }
            inner = self
                .cv
                .wait_timeout(inner, Duration::from_millis(50))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    fn unregister_wait(inner: &mut Inner, reqs: &[(ResourceKey, LockMode)]) {
        for (key, mode) in reqs {
            if *mode == LockMode::Exclusive {
                if let Some(e) = inner.entries.get_mut(key) {
                    e.waiting_writers = e.waiting_writers.saturating_sub(1);
                }
            }
        }
    }

    /// Number of holders of `key` (readers, writer) — for diagnostics and tests.
    pub fn holders(&self, key: &ResourceKey) -> (usize, bool) {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner
            .entries
            .get(key)
            .map(|e| (e.readers, e.writer))
            .unwrap_or((0, false))
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let mut inner = self.locks.inner.lock().unwrap_or_else(|p| p.into_inner());
        for (key, mode) in &self.held {
            let remove = if let Some(e) = inner.entries.get_mut(key) {
                match mode {
                    LockMode::Shared => e.readers = e.readers.saturating_sub(1),
                    LockMode::Exclusive => e.writer = false,
                }
                e.readers == 0 && !e.writer && e.waiting_writers == 0
            } else {
                false
            };
            if remove {
                inner.entries.remove(key);
            }
        }
        drop(inner);
        self.locks.cv.notify_all();
    }
}
