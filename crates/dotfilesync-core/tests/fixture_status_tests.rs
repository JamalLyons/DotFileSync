use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

use dotfilesync_core::{
    compare_status, load_manifest, CommandOutput, ConfigInspector, DifferenceClassification,
    MockCommandRunner, PackageKind,
};

#[test]
fn test_fixture_status_missing_packages_and_changed_config() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = manifest_dir.join("../../fixtures/sample_manifest.toml");
    let manifest = load_manifest(&manifest_path).expect("Should load sample manifest");

    // Mock Homebrew runner
    // Installed: git (2.44.0), ghostty (1.0.0), plus an unmanaged tool
    // Missing: missing-formula-tool, ripgrep, missing-gui-app
    let brew_path = PathBuf::from("/usr/local/bin/brew");
    let runner = MockCommandRunner::new()
        .with_executable("brew", Some(brew_path.clone()))
        .with_response(
            "/usr/local/bin/brew",
            &["--version"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: "Homebrew 4.3.0\n".into(),
                stderr: "".into(),
            }),
        )
        .with_response(
            "/usr/local/bin/brew",
            &["info", "--json=v2", "--installed"],
            Ok(CommandOutput {
                status_code: 0,
                stdout: r#"{
                    "formulae": [
                        {"name": "git", "installed": [{"version": "2.44.0"}]},
                        {"name": "unmanaged-cli", "installed": [{"version": "0.5.1"}]}
                    ],
                    "casks": [
                        {"token": "ghostty", "version": "1.0.0"}
                    ]
                }"#
                .into(),
                stderr: "".into(),
            }),
        );

    let disc = dotfilesync_core::discover_homebrew(&runner);
    assert!(disc.available);

    // Setup temp environment for configs
    let repo_dir = tempdir().unwrap();
    let home_dir = tempdir().unwrap();

    // Create repo source files
    let src_cfg = repo_dir.path().join("sample_config.fish");
    fs::write(
        &src_cfg,
        "# Repo template config\nset -gx MY_ENV production\n",
    )
    .unwrap();

    let src_match = repo_dir.path().join("sample_matching.conf");
    fs::write(&src_match, "# Static matching config\nenabled = true\n").unwrap();

    let src_missing = repo_dir.path().join("sample_missing.conf");
    fs::write(&src_missing, "# Config that will be missing on machine\n").unwrap();

    // Create target files in simulated home
    let target_dir = home_dir.path().join(".config/sample");
    fs::create_dir_all(&target_dir).unwrap();

    // Changed config (drift)
    let tgt_cfg = target_dir.join("sample_config.fish");
    fs::write(
        &tgt_cfg,
        "# Local drifted config\nset -gx MY_ENV local_dev_override\n",
    )
    .unwrap();

    // Matching config
    let tgt_match = target_dir.join("sample_matching.conf");
    fs::write(&tgt_match, "# Static matching config\nenabled = true\n").unwrap();

    // Machine-local override config
    let tgt_local = target_dir.join("local.conf");
    fs::write(
        &tgt_local,
        "# Machine local private settings\nSECRET=local_only\n",
    )
    .unwrap();

    // Missing config: sample_missing.conf is NOT created on machine

    let inspector = ConfigInspector::new(
        repo_dir.path(),
        home_dir.path(),
        &manifest.exclusions,
        &manifest.overrides,
    );
    let configs = inspector.inspect_files(&manifest.files);

    let report = compare_status(&manifest, &disc, &configs);

    // Assert summary numbers
    assert_eq!(report.summary.packages_matching, 2); // git, ghostty
    assert_eq!(report.summary.packages_missing, 3); // ripgrep, missing-formula-tool, missing-gui-app
    assert_eq!(report.summary.packages_unmanaged, 1); // unmanaged-cli
    assert_eq!(report.summary.configs_matching, 2); // sample_matching.conf, local.conf (override)
    assert_eq!(report.summary.configs_changed, 1); // sample_config.fish
    assert_eq!(report.summary.configs_missing, 1); // sample_missing.conf

    // Verify missing packages
    let missing_formulae: Vec<&str> = report
        .package_differences
        .iter()
        .filter(|p| {
            p.classification == DifferenceClassification::Missing && p.kind == PackageKind::Formula
        })
        .map(|p| p.name.as_str())
        .collect();
    assert!(missing_formulae.contains(&"missing-formula-tool"));
    assert!(missing_formulae.contains(&"ripgrep"));

    let missing_casks: Vec<&str> = report
        .package_differences
        .iter()
        .filter(|p| {
            p.classification == DifferenceClassification::Missing && p.kind == PackageKind::Cask
        })
        .map(|p| p.name.as_str())
        .collect();
    assert!(missing_casks.contains(&"missing-gui-app"));

    // Verify changed config
    let changed_config = report
        .config_differences
        .iter()
        .find(|c| c.classification == DifferenceClassification::Changed)
        .expect("Should find changed config");
    assert!(changed_config.target.ends_with("sample_config.fish"));
    assert_ne!(changed_config.source_hash, changed_config.target_hash);

    // Save fixture JSON for inspection and documentation
    let json_output = serde_json::to_string_pretty(&report).unwrap();
    let out_path = manifest_dir.join("../../fixtures/example_status_output.json");
    fs::write(&out_path, &json_output).unwrap();
}
