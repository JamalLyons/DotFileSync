pub mod model;
pub mod validation;

use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
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

/// Decodes a manifest path string into native path bytes.
///
/// Standard UTF-8 paths are preserved as-is. If the path contains percent-encoded
/// byte sequences (e.g. `%FF` or `%fe`), they are decoded into their raw native byte values,
/// allowing manifest files to represent arbitrary non-UTF-8 paths without violating
/// the TOML specification's UTF-8 requirement.
pub fn decode_manifest_path_bytes(path_str: &str) -> PathBuf {
    #[cfg(unix)]
    {
        let bytes = path_str.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(h1), Some(h2)) =
                    (from_hex_digit(bytes[i + 1]), from_hex_digit(bytes[i + 2]))
                {
                    decoded.push((h1 << 4) | h2);
                    i += 3;
                    continue;
                }
            }
            decoded.push(bytes[i]);
            i += 1;
        }
        PathBuf::from(OsString::from_vec(decoded))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(path_str)
    }
}

/// Encodes a native filesystem path into a valid UTF-8 manifest path string.
///
/// If the path contains valid UTF-8, it is returned as a UTF-8 string (preserving
/// standard path characters). Any non-UTF-8 byte sequences are percent-encoded
/// (`%HH`) to ensure full fidelity without lossy replacement characters.
pub fn encode_manifest_path_bytes(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => {
                let mut encoded = String::with_capacity(bytes.len() * 3);
                let mut remaining = bytes;
                while !remaining.is_empty() {
                    match std::str::from_utf8(remaining) {
                        Ok(valid) => {
                            encoded.push_str(valid);
                            break;
                        }
                        Err(err) => {
                            let (valid, after_valid) = remaining.split_at(err.valid_up_to());
                            if let Ok(valid_str) = std::str::from_utf8(valid) {
                                encoded.push_str(valid_str);
                            }
                            if let Some(err_len) = err.error_len() {
                                for &b in &after_valid[..err_len] {
                                    use std::fmt::Write;
                                    let _ = write!(encoded, "%{:02X}", b);
                                }
                                remaining = &after_valid[err_len..];
                            } else {
                                for &b in after_valid {
                                    use std::fmt::Write;
                                    let _ = write!(encoded, "%{:02X}", b);
                                }
                                break;
                            }
                        }
                    }
                }
                encoded
            }
        }
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().to_string()
    }
}

fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Expands leading `~` to the provided home directory or environment HOME.
///
/// If `home_dir` contains native non-UTF-8 bytes, those bytes are preserved intact
/// in the resulting `PathBuf` without lossy conversion.
/// If `target` contains percent-encoded byte sequences (e.g. `%FF`), they are decoded
/// into raw native path bytes.
pub fn expand_target_path(target: &str, home_dir: &Path) -> PathBuf {
    if let Some(stripped) = target.strip_prefix("~/") {
        let decoded = decode_manifest_path_bytes(stripped);
        home_dir.join(decoded)
    } else if target == "~" {
        home_dir.to_path_buf()
    } else {
        decode_manifest_path_bytes(target)
    }
}

/// Resolves a source path relative to the repository / manifest base directory.
///
/// If `base_dir` contains native non-UTF-8 bytes, those bytes are preserved intact
/// in the resulting `PathBuf` without lossy conversion.
/// If `source` contains percent-encoded byte sequences (e.g. `%FF`), they are decoded
/// into raw native path bytes.
pub fn resolve_source_path(source: &str, base_dir: &Path) -> PathBuf {
    let decoded = decode_manifest_path_bytes(source);
    base_dir.join(decoded)
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

    #[cfg(unix)]
    #[test]
    fn test_percent_encoded_path_roundtrip() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // Standard UTF-8 path
        let path = Path::new("fish/config.fish");
        assert_eq!(encode_manifest_path_bytes(path), "fish/config.fish");
        assert_eq!(decode_manifest_path_bytes("fish/config.fish"), path);

        // Path with invalid UTF-8 byte 0xFF
        let raw_bytes = b"fish/custom_\xff.conf";
        let non_utf8_path = Path::new(OsStr::from_bytes(raw_bytes));
        let encoded = encode_manifest_path_bytes(non_utf8_path);
        assert_eq!(encoded, "fish/custom_%FF.conf");

        let decoded = decode_manifest_path_bytes(&encoded);
        assert_eq!(decoded.as_os_str().as_bytes(), raw_bytes);

        // Path with multiple non-UTF-8 bytes
        let raw_bytes2 = b".config/sub_\xfe\xed/test.conf";
        let non_utf8_path2 = Path::new(OsStr::from_bytes(raw_bytes2));
        let encoded2 = encode_manifest_path_bytes(non_utf8_path2);
        assert_eq!(encoded2, ".config/sub_%FE%ED/test.conf");
        let decoded2 = decode_manifest_path_bytes(&encoded2);
        assert_eq!(decoded2.as_os_str().as_bytes(), raw_bytes2);
    }
}
