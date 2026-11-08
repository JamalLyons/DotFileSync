use std::collections::HashSet;
use std::path::{Component, Path};
use thiserror::Error;

use super::model::{Manifest, CURRENT_SCHEMA_VERSION};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("Unsupported schema version {version}; expected {expected}")]
    UnsupportedSchemaVersion { version: u32, expected: u32 },

    #[error("Empty source path for tracked file targeting '{target}'")]
    EmptySourcePath { target: String },

    #[error("Empty target path for tracked file with source '{source_path}'")]
    EmptyTargetPath { source_path: String },

    #[error("Duplicate target path '{target}'")]
    DuplicateTargetPath { target: String },

    #[error("Source path '{source_path}' contains illegal directory traversal ('..')")]
    IllegalPathTraversal { source_path: String },

    #[error("Dangerous or sensitive credential path tracked: '{path}'")]
    SensitiveCredentialPath { path: String },

    #[error("Invalid exclusion pattern '{pattern}': {reason}")]
    InvalidExclusionPattern { pattern: String, reason: String },
}

const SENSITIVE_PATH_PATTERNS: &[&str] = &[
    ".ssh",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    ".aws/credentials",
    ".gnupg",
    ".docker/config.json",
    "keychain",
    "credentials.json",
];

pub fn validate_manifest(manifest: &Manifest) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if manifest.schema_version != CURRENT_SCHEMA_VERSION {
        errors.push(ValidationError::UnsupportedSchemaVersion {
            version: manifest.schema_version,
            expected: CURRENT_SCHEMA_VERSION,
        });
    }

    let mut seen_targets = HashSet::new();

    for file in &manifest.files {
        if file.source.trim().is_empty() {
            errors.push(ValidationError::EmptySourcePath {
                target: file.target.clone(),
            });
        }

        if file.target.trim().is_empty() {
            errors.push(ValidationError::EmptyTargetPath {
                source_path: file.source.clone(),
            });
        }

        // Check for directory traversal in source
        let source_path = Path::new(&file.source);
        if source_path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            errors.push(ValidationError::IllegalPathTraversal {
                source_path: file.source.clone(),
            });
        }

        // Check for duplicate targets
        let normalized_target = file.target.trim().to_string();
        if !seen_targets.insert(normalized_target) {
            errors.push(ValidationError::DuplicateTargetPath {
                target: file.target.clone(),
            });
        }

        // Check sensitive credential locations
        for sensitive in SENSITIVE_PATH_PATTERNS {
            if file.target.contains(sensitive) || file.source.contains(sensitive) {
                errors.push(ValidationError::SensitiveCredentialPath {
                    path: file.target.clone(),
                });
                break;
            }
        }
    }

    // Validate exclusion glob patterns
    for pattern in &manifest.exclusions.patterns {
        if let Err(e) = glob::Pattern::new(pattern) {
            errors.push(ValidationError::InvalidExclusionPattern {
                pattern: pattern.clone(),
                reason: e.to_string(),
            });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
