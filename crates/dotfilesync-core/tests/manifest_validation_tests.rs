use dotfilesync_core::{
    parse_manifest, validate_manifest, HomebrewSection, Manifest, PackageManagersSection,
    TrackedFile, ValidationError, CURRENT_SCHEMA_VERSION,
};

#[test]
fn test_valid_manifest_passes_validation() {
    let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
    manifest.package_managers = PackageManagersSection {
        homebrew: HomebrewSection {
            formulae: vec!["git".into(), "fish".into()],
            casks: vec!["ghostty".into()],
        },
    };
    manifest.files = vec![
        TrackedFile {
            source: "fish/config.fish".into(),
            target: "~/.config/fish/config.fish".into(),
            machine_local: false,
            description: Some("Fish config".into()),
        },
        TrackedFile {
            source: "zed/settings.json".into(),
            target: "~/.config/zed/settings.json".into(),
            machine_local: false,
            description: None,
        },
    ];

    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_unsupported_schema_version() {
    let manifest = Manifest::new(999);
    let errs = validate_manifest(&manifest).unwrap_err();
    assert!(errs.iter().any(|e| matches!(
        e,
        ValidationError::UnsupportedSchemaVersion { version: 999, .. }
    )));
}

#[test]
fn test_empty_source_and_target() {
    let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
    manifest.files = vec![
        TrackedFile {
            source: "   ".into(),
            target: "~/.config/foo".into(),
            machine_local: false,
            description: None,
        },
        TrackedFile {
            source: "foo".into(),
            target: "".into(),
            machine_local: false,
            description: None,
        },
    ];
    let errs = validate_manifest(&manifest).unwrap_err();
    assert_eq!(errs.len(), 2);
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidationError::EmptySourcePath { .. })));
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidationError::EmptyTargetPath { .. })));
}

#[test]
fn test_duplicate_target_rejected() {
    let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
    manifest.files = vec![
        TrackedFile {
            source: "foo".into(),
            target: "~/.config/common.conf".into(),
            machine_local: false,
            description: None,
        },
        TrackedFile {
            source: "bar".into(),
            target: "~/.config/common.conf".into(),
            machine_local: false,
            description: None,
        },
    ];
    let errs = validate_manifest(&manifest).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidationError::DuplicateTargetPath { .. })));
}

#[test]
fn test_directory_traversal_rejected() {
    let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
    manifest.files = vec![TrackedFile {
        source: "../secrets/token.txt".into(),
        target: "~/.config/token".into(),
        machine_local: false,
        description: None,
    }];
    let errs = validate_manifest(&manifest).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidationError::IllegalPathTraversal { .. })));
}

#[test]
fn test_sensitive_credential_paths_rejected() {
    let sensitive_paths = [
        ("ssh/key", "~/.ssh/id_rsa"),
        ("ssh/id_ed25519", "~/.config/key"),
        ("aws", "~/.aws/credentials"),
        ("gpg", "~/.gnupg/secring.gpg"),
    ];

    for (src, tgt) in sensitive_paths {
        let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
        manifest.files = vec![TrackedFile {
            source: src.into(),
            target: tgt.into(),
            machine_local: false,
            description: None,
        }];
        let errs = validate_manifest(&manifest).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, ValidationError::SensitiveCredentialPath { .. })),
            "Expected sensitive credential error for target '{tgt}'"
        );
    }
}

#[test]
fn test_invalid_exclusion_glob_pattern() {
    let toml_str = r#"
schema_version = 1

[exclusions]
patterns = ["[-a-z"]
"#;
    let err = parse_manifest(toml_str).unwrap_err();
    assert!(format!("{err}").contains("Invalid exclusion pattern"));
}
