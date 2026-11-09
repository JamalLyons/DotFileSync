use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::model::{ConfigFileState, ObservedConfigFile};
use crate::manifest::{
    expand_target_path, resolve_source_path, ExclusionsSection, MachineOverride, TrackedFile,
};

pub struct ConfigInspector<'a> {
    repo_root: &'a Path,
    home_dir: &'a Path,
    exclusions: &'a ExclusionsSection,
    overrides: &'a [MachineOverride],
}

impl<'a> ConfigInspector<'a> {
    pub fn new(
        repo_root: &'a Path,
        home_dir: &'a Path,
        exclusions: &'a ExclusionsSection,
        overrides: &'a [MachineOverride],
    ) -> Self {
        Self {
            repo_root,
            home_dir,
            exclusions,
            overrides,
        }
    }

    pub fn inspect_files(&self, files: &[TrackedFile]) -> Vec<ObservedConfigFile> {
        files.iter().map(|f| self.inspect_file(f)).collect()
    }

    pub fn inspect_file(&self, file: &TrackedFile) -> ObservedConfigFile {
        let source_path = resolve_source_path(&file.source, self.repo_root);
        let target_path = expand_target_path(&file.target, self.home_dir);

        // 1. Check exclusions
        if self.is_excluded(&file.source, &file.target, &target_path) {
            return ObservedConfigFile {
                source_path,
                target_path,
                state: ConfigFileState::Excluded,
                source_hash: None,
                target_hash: None,
                target_size: None,
                is_symlink: false,
                symlink_target: None,
                diagnostic: Some("Explicitly excluded in manifest".to_string()),
            };
        }

        // 2. Check machine-local override
        let is_override = file.machine_local
            || self.overrides.iter().any(|o| {
                expand_target_path(&o.target, self.home_dir) == target_path
                    || o.target == file.target
            });

        if is_override {
            let target_exists = target_path.exists();

            return ObservedConfigFile {
                source_path,
                target_path,
                state: if target_exists {
                    ConfigFileState::MachineLocal
                } else {
                    ConfigFileState::Missing
                },
                source_hash: None,
                target_hash: None,
                target_size: None,
                is_symlink: false,
                symlink_target: None,
                diagnostic: Some(
                    if target_exists {
                        "Preserved as machine-local override (never read, hashed, or overwritten)"
                    } else {
                        "Machine-local override target is missing on machine"
                    }
                    .to_string(),
                ),
            };
        }

        // 3. Inspect source file boundaries and symlinks
        let repo_root_canonical = match self.repo_root.canonicalize() {
            Ok(c) => c,
            Err(_) => self.repo_root.to_path_buf(),
        };

        if Path::new(&file.source).is_absolute()
            || Path::new(&file.source)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return ObservedConfigFile {
                source_path,
                target_path,
                state: ConfigFileState::SourceOutsideRoot,
                source_hash: None,
                target_hash: None,
                target_size: None,
                is_symlink: false,
                symlink_target: None,
                diagnostic: Some(
                    "Source path is absolute or contains directory traversal ('..')".to_string(),
                ),
            };
        }

        let source_metadata = match std::fs::symlink_metadata(&source_path) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                let diagnostic = if e.raw_os_error() == Some(libc::EILSEQ) {
                    format!("Cannot read source file in repository (illegal byte sequence / EILSEQ): {e}")
                } else {
                    format!("Cannot read source file in repository: {e}")
                };
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::Unreadable,
                    source_hash: None,
                    target_hash: None,
                    target_size: None,
                    is_symlink: false,
                    symlink_target: None,
                    diagnostic: Some(diagnostic),
                };
            }
        };

        if source_metadata.is_none() {
            return ObservedConfigFile {
                source_path,
                target_path,
                state: ConfigFileState::SourceMissing,
                source_hash: None,
                target_hash: None,
                target_size: None,
                is_symlink: false,
                symlink_target: None,
                diagnostic: Some("Source file does not exist in repository".to_string()),
            };
        }

        // Canonicalize source to ensure it does not escape repo root through symlinks
        match source_path.canonicalize() {
            Ok(canonical_source) => {
                if !canonical_source.starts_with(&repo_root_canonical) {
                    return ObservedConfigFile {
                        source_path,
                        target_path,
                        state: ConfigFileState::SourceOutsideRoot,
                        source_hash: None,
                        target_hash: None,
                        target_size: None,
                        is_symlink: false,
                        symlink_target: None,
                        diagnostic: Some(
                            "Source path escapes repository root via symlink or traversal"
                                .to_string(),
                        ),
                    };
                }
            }
            Err(e) => {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::Unreadable,
                    source_hash: None,
                    target_hash: None,
                    target_size: None,
                    is_symlink: false,
                    symlink_target: None,
                    diagnostic: Some(format!("Failed to resolve canonical source path: {e}")),
                };
            }
        }

        let source_hash = match compute_sha256(&source_path) {
            Ok(h) => Some(h),
            Err(e) => {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::Unreadable,
                    source_hash: None,
                    target_hash: None,
                    target_size: None,
                    is_symlink: false,
                    symlink_target: None,
                    diagnostic: Some(format!("Failed to compute hash for source file: {e}")),
                };
            }
        };

        // 4. Inspect target file (using symlink_metadata so symlinks aren't traversed silently)
        let target_metadata = match std::fs::symlink_metadata(&target_path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::Missing,
                    source_hash,
                    target_hash: None,
                    target_size: None,
                    is_symlink: false,
                    symlink_target: None,
                    diagnostic: Some("Target file does not exist on machine".to_string()),
                };
            }
            Err(e) => {
                let diagnostic = if e.raw_os_error() == Some(libc::EILSEQ) {
                    format!(
                        "Cannot read target file on machine (illegal byte sequence / EILSEQ): {e}"
                    )
                } else {
                    format!("Cannot read target file on machine: {e}")
                };
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::Unreadable,
                    source_hash,
                    target_hash: None,
                    target_size: None,
                    is_symlink: false,
                    symlink_target: None,
                    diagnostic: Some(diagnostic),
                };
            }
        };

        let is_symlink = target_metadata.file_type().is_symlink();
        if is_symlink {
            let link_target = match std::fs::read_link(&target_path) {
                Ok(t) => t,
                Err(e) => {
                    return ObservedConfigFile {
                        source_path,
                        target_path,
                        state: ConfigFileState::Unreadable,
                        source_hash,
                        target_hash: None,
                        target_size: None,
                        is_symlink: true,
                        symlink_target: None,
                        diagnostic: Some(format!("Failed to read symlink destination: {e}")),
                    };
                }
            };

            let resolved_link = if link_target.is_relative() {
                target_path
                    .parent()
                    .unwrap_or(Path::new(""))
                    .join(&link_target)
            } else {
                link_target.clone()
            };

            // Does link point to source?
            let points_to_source = resolved_link == source_path
                || std::fs::canonicalize(&resolved_link).ok()
                    == std::fs::canonicalize(&source_path).ok();

            if points_to_source {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::SymlinkToSource,
                    source_hash: source_hash.clone(),
                    target_hash: source_hash,
                    target_size: None,
                    is_symlink: true,
                    symlink_target: Some(link_target),
                    diagnostic: Some(
                        "Target is a symlink pointing directly to repository source".to_string(),
                    ),
                };
            } else if !resolved_link.exists() {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::BrokenSymlink,
                    source_hash,
                    target_hash: None,
                    target_size: None,
                    is_symlink: true,
                    symlink_target: Some(link_target),
                    diagnostic: Some(
                        "Target is a broken symlink (destination does not exist)".to_string(),
                    ),
                };
            } else {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::SymlinkExternal,
                    source_hash,
                    target_hash: None,
                    target_size: None,
                    is_symlink: true,
                    symlink_target: Some(link_target),
                    diagnostic: Some("Target is a symlink pointing outside repository".to_string()),
                };
            }
        }

        // Regular file comparison
        let target_size = Some(target_metadata.len());
        let target_hash = match compute_sha256(&target_path) {
            Ok(h) => Some(h),
            Err(e) => {
                return ObservedConfigFile {
                    source_path,
                    target_path,
                    state: ConfigFileState::Unreadable,
                    source_hash,
                    target_hash: None,
                    target_size,
                    is_symlink: false,
                    symlink_target: None,
                    diagnostic: Some(format!("Failed to compute target file checksum: {e}")),
                };
            }
        };

        let state = if source_hash == target_hash {
            ConfigFileState::Matching
        } else {
            ConfigFileState::Changed
        };

        let diagnostic = if state == ConfigFileState::Changed {
            Some("Target content differs from repository source (drift detected)".to_string())
        } else {
            None
        };

        ObservedConfigFile {
            source_path,
            target_path,
            state,
            source_hash,
            target_hash,
            target_size,
            is_symlink: false,
            symlink_target: None,
            diagnostic,
        }
    }

    fn is_excluded(&self, source_raw: &str, target_raw: &str, target_path: &Path) -> bool {
        // Direct path matches
        for excluded in &self.exclusions.paths {
            let expanded_excluded = expand_target_path(excluded, self.home_dir);
            if excluded == source_raw
                || excluded == target_raw
                || expanded_excluded == target_path
                || target_path.ends_with(excluded)
                || Path::new(source_raw).ends_with(excluded)
            {
                return true;
            }
        }

        // Pattern matches
        for pattern_str in &self.exclusions.patterns {
            if let Ok(pattern) = glob::Pattern::new(pattern_str) {
                if pattern.matches(source_raw) || pattern.matches(target_raw) {
                    return true;
                }
                if let Some(filename) = target_path.file_name().and_then(|n| n.to_str()) {
                    if pattern.matches(filename) {
                        return true;
                    }
                }
            }
        }

        false
    }
}

pub fn compute_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    let hash = hasher.finalize();
    Ok(hex::encode(hash))
}
