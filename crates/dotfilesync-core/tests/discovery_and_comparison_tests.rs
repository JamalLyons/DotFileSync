use dotfilesync_core::{
    compare_status, discover_homebrew, CommandOutput, DifferenceClassification, HomebrewSection,
    Manifest, MockCommandRunner, PackageManagersSection, RunnerError, CURRENT_SCHEMA_VERSION,
};
use std::path::PathBuf;

#[test]
fn test_absent_homebrew_produces_diagnostic_not_missing_packages() {
    // Mock runner with no brew executable
    let runner = MockCommandRunner::new();

    let disc = discover_homebrew(&runner);
    assert!(!disc.available);
    assert!(disc.formulae.is_empty());
    assert!(disc.casks.is_empty());
    assert!(disc
        .diagnostics
        .iter()
        .any(|d| d.message.contains("not found")));

    // Now test comparison with a manifest declaring packages
    let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
    manifest.package_managers = PackageManagersSection {
        homebrew: HomebrewSection {
            formulae: vec!["git".into(), "fish".into()],
            casks: vec!["ghostty".into()],
        },
    };

    let report = compare_status(&manifest, &disc, &[]);

    // CRITICAL: Packages must NOT be marked Missing when Homebrew is absent!
    assert_eq!(report.summary.packages_missing, 0);
    assert_eq!(report.summary.packages_unknown, 3);
    for p in &report.package_differences {
        assert_eq!(p.classification, DifferenceClassification::Unknown);
        assert!(p.diagnostic.is_some());
    }
}

#[test]
fn test_command_timeout_triggers_fallback() {
    let brew_path = PathBuf::from("/mock/bin/brew");
    let runner = MockCommandRunner::new()
        .with_executable("brew", Some(brew_path.clone()))
        .with_response(
            "/mock/bin/brew",
            &["--version"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: "Homebrew 4.2.0\n".into(),
                stderr: "".into(),
            }),
        )
        // Simulate JSON timeout
        .with_response(
            "/mock/bin/brew",
            &["info", "--json=v2", "--installed"],
            Err(RunnerError::Timeout {
                program: "brew".into(),
                timeout_secs: 15,
            }),
        )
        // Fallback list commands succeed
        .with_response(
            "/mock/bin/brew",
            &["list", "--versions", "--formula"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: "git 2.45.0\nfish 3.7.0\n".into(),
                stderr: "".into(),
            }),
        )
        .with_response(
            "/mock/bin/brew",
            &["list", "--versions", "--cask"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: "ghostty 1.0.0\n".into(),
                stderr: "".into(),
            }),
        );

    let disc = discover_homebrew(&runner);
    assert!(disc.available);
    assert_eq!(disc.brew_version.as_deref(), Some("Homebrew 4.2.0"));
    assert_eq!(disc.formulae.len(), 2);
    assert_eq!(disc.casks.len(), 1);
    assert_eq!(disc.formulae[0].name, "fish");
    assert_eq!(disc.formulae[1].name, "git");
    assert_eq!(disc.formulae[1].version.as_deref(), Some("2.45.0"));
    assert!(disc
        .diagnostics
        .iter()
        .any(|d| d.message.contains("fallback")));
}

#[test]
fn test_package_comparison_matching_missing_unmanaged() {
    let brew_path = PathBuf::from("/mock/bin/brew");
    let runner = MockCommandRunner::new()
        .with_executable("brew", Some(brew_path.clone()))
        .with_response(
            "/mock/bin/brew",
            &["--version"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: "Homebrew 4.2.0\n".into(),
                stderr: "".into(),
            }),
        )
        .with_response(
            "/mock/bin/brew",
            &["info", "--json=v2", "--installed"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: r#"{
                    "formulae": [
                        {"name": "git", "installed": [{"version": "2.44.0"}]},
                        {"name": "unmanaged-tool", "installed": [{"version": "1.0.0"}]}
                    ],
                    "casks": [
                        {"token": "ghostty", "version": "0.9.0"}
                    ]
                }"#
                .into(),
                stderr: "".into(),
            }),
        );

    let disc = discover_homebrew(&runner);
    assert!(disc.available);

    let mut manifest = Manifest::new(CURRENT_SCHEMA_VERSION);
    manifest.package_managers = PackageManagersSection {
        homebrew: HomebrewSection {
            formulae: vec!["git".into(), "ripgrep".into()],
            casks: vec!["ghostty".into(), "zed".into()],
        },
    };

    let report = compare_status(&manifest, &disc, &[]);

    assert_eq!(report.summary.packages_matching, 2); // git and ghostty
    assert_eq!(report.summary.packages_missing, 2); // ripgrep and zed
    assert_eq!(report.summary.packages_unmanaged, 1); // unmanaged-tool
    assert_eq!(report.summary.packages_unknown, 0);

    // Verify git is matching with observed version
    let git_diff = report
        .package_differences
        .iter()
        .find(|p| p.name == "git")
        .unwrap();
    assert_eq!(git_diff.classification, DifferenceClassification::Matching);
    assert_eq!(git_diff.observed_version.as_deref(), Some("2.44.0"));

    // Verify ripgrep is missing
    let rg_diff = report
        .package_differences
        .iter()
        .find(|p| p.name == "ripgrep")
        .unwrap();
    assert_eq!(rg_diff.classification, DifferenceClassification::Missing);
    assert_eq!(rg_diff.observed_version, None);

    // Verify unmanaged-tool is unmanaged
    let unmanaged_diff = report
        .package_differences
        .iter()
        .find(|p| p.name == "unmanaged-tool")
        .unwrap();
    assert_eq!(
        unmanaged_diff.classification,
        DifferenceClassification::Unmanaged
    );
    assert_eq!(unmanaged_diff.observed_version.as_deref(), Some("1.0.0"));
}
