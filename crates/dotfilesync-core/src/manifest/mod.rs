pub mod model;
pub mod validation;

use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub use model::*;
pub use validation::*;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("Failed to read manifest file '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse TOML manifest: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Failed to serialize manifest to TOML: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("Manifest validation failed with {0} errors: {1}")]
    Validation(usize, String),
}

pub fn parse_manifest(content: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest = toml::from_str(content)?;
    if let Err(errors) = validate_manifest(&manifest) {
        let msg = errors
            .iter()
            .map(|e| format!("- {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ManifestError::Validation(errors.len(), msg));
    }
    Ok(manifest)
}

/// Loads and validates a manifest from disk.
///
/// Manifest Representational Limits:
/// Manifest files are required to be valid UTF-8 text per the TOML specification. If the manifest
/// file on disk contains non-UTF-8 bytes, `load_manifest` explicitly rejects it with a
/// `ManifestError::Io` error containing `std::io::ErrorKind::InvalidData`.
pub fn load_manifest(path: impl AsRef<Path>) -> Result<Manifest, ManifestError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_manifest(&content)
}

pub fn serialize_manifest(manifest: &Manifest) -> Result<String, ManifestError> {
    Ok(toml::to_string_pretty(manifest)?)
}

/// Expands leading `~` to the provided home directory or environment HOME.
///
/// If `home_dir` contains native non-UTF-8 bytes, those bytes are preserved intact
/// in the resulting `PathBuf` without lossy conversion.
pub fn expand_target_path(target: &str, home_dir: &Path) -> PathBuf {
    if let Some(stripped) = target.strip_prefix("~/") {
        home_dir.join(stripped)
    } else if target == "~" {
        home_dir.to_path_buf()
    } else {
        PathBuf::from(target)
    }
}

/// Resolves a source path relative to the repository / manifest base directory.
///
/// If `base_dir` contains native non-UTF-8 bytes, those bytes are preserved intact
/// in the resulting `PathBuf` without lossy conversion.
pub fn resolve_source_path(source: &str, base_dir: &Path) -> PathBuf {
    base_dir.join(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_manifest_parsing() {
        let toml_str = r#"
schema_version = 1

[metadata]
name = "my-test"
description = "Test configuration"

[package_managers.homebrew]
formulae = ["git", "fish"]
casks = ["zed"]

[[files]]
source = "fish/config.fish"
target = "~/.config/fish/config.fish"

[exclusions]
paths = ["fish/conf.d/local.fish"]
patterns = ["*.backup.*"]

[[overrides]]
target = "~/.config/fish/conf.d/local.fish"
description = "Private secrets"
"#;
        let manifest = parse_manifest(toml_str).expect("Should parse valid manifest");
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(
            manifest.package_managers.homebrew.formulae,
            vec!["git", "fish"]
        );
        assert_eq!(manifest.package_managers.homebrew.casks, vec!["zed"]);
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.exclusions.paths.len(), 1);
        assert_eq!(manifest.overrides.len(), 1);
    }

    #[test]
    fn test_unsupported_version() {
        let toml_str = r#"
schema_version = 99
"#;
        let err = parse_manifest(toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::Validation(..)));
    }

    #[test]
    fn test_directory_traversal_rejected() {
        let toml_str = r#"
schema_version = 1

[[files]]
source = "../../../etc/passwd"
target = "~/.config/evil"
"#;
        let err = parse_manifest(toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::Validation(..)));
    }

    #[test]
    fn test_sensitive_credential_path_rejected() {
        let toml_str = r#"
schema_version = 1

[[files]]
source = "ssh/id_rsa"
target = "~/.ssh/id_rsa"
"#;
        let err = parse_manifest(toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::Validation(..)));
    }
}
