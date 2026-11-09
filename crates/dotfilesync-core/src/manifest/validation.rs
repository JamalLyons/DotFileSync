use std::collections::HashSet;
use std::path::{Component, Path};
use thiserror::Error;

use super::model::{Manifest, TrackedFile, CURRENT_SCHEMA_VERSION};
use super::profile::profile_validation_errors;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("Unsupported schema version {version}; expected {expected}")]
    UnsupportedSchemaVersion { version: u32, expected: u32 },

    #[error("Empty source path for tracked file targeting '{target}'")]
    EmptySourcePath { target: String },

    #[error("Source path '{source_path}' must be a relative path within repository root")]
    AbsoluteSourcePath { source_path: String },

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

    #[error("Unknown profile '{name}' referenced from {context}")]
    UnknownProfile { name: String, context: String },

    #[error("Profile cycle detected: {cycle}")]
    ProfileCycle { cycle: String },

    #[error("Invalid OS condition '{value}'")]
    InvalidOsCondition { value: String },
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

    validate_files(&manifest.files, &mut errors);
    for profile in manifest.profiles.values() {
        validate_files(&profile.files, &mut errors);
    }
    errors.extend(profile_validation_errors(manifest));

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

fn validate_files(files: &[TrackedFile], errors: &mut Vec<ValidationError>) {
    let mut seen_targets = HashSet::new();

    for file in files {
        if file.source.trim().is_empty() {
            errors.push(ValidationError::EmptySourcePath {
                target: file.target.clone(),
            });
        } else if Path::new(&file.source).is_absolute() {
            errors.push(ValidationError::AbsoluteSourcePath {
                source_path: file.source.clone(),
            });
        }

        if file.target.trim().is_empty() {
            errors.push(ValidationError::EmptyTargetPath {
                source_path: file.source.clone(),
            });
        }

        let source_path = Path::new(&file.source);
        if source_path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            errors.push(ValidationError::IllegalPathTraversal {
                source_path: file.source.clone(),
            });
        }

        let normalized_target = file.target.trim().to_string();
        if !seen_targets.insert(normalized_target) {
            errors.push(ValidationError::DuplicateTargetPath {
                target: file.target.clone(),
            });
        }

        for sensitive in SENSITIVE_PATH_PATTERNS {
            if file.target.contains(sensitive) || file.source.contains(sensitive) {
                errors.push(ValidationError::SensitiveCredentialPath {
                    path: file.target.clone(),
                });
                break;
            }
        }
    }
}
