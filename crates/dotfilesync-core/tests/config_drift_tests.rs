use std::fs;
use tempfile::tempdir;

use dotfilesync_core::{
    ConfigFileState, ConfigInspector, ExclusionsSection, MachineOverride, TrackedFile,
};

#[test]
fn test_config_matching_and_changed_and_missing() {
    let repo_temp = tempdir().unwrap();
    let home_temp = tempdir().unwrap();

    let repo_root = repo_temp.path();
    let home_dir = home_temp.path();

    // Setup source files in repo
    let source_dir = repo_root.join("config");
    fs::create_dir_all(&source_dir).unwrap();

    let match_src = source_dir.join("match.conf");
    fs::write(&match_src, "key=value\n").unwrap();

    let drift_src = source_dir.join("drift.conf");
    fs::write(&drift_src, "setting=default\n").unwrap();

    let missing_src = source_dir.join("missing.conf");
    fs::write(&missing_src, "should=exist\n").unwrap();

    // Setup target files in machine home
    let target_dir = home_dir.join(".config");
    fs::create_dir_all(&target_dir).unwrap();

    let match_tgt = target_dir.join("match.conf");
    fs::write(&match_tgt, "key=value\n").unwrap(); // identical

    let drift_tgt = target_dir.join("drift.conf");
    fs::write(&drift_tgt, "setting=custom_machine_override\n").unwrap(); // drifted

    // missing_tgt is not created

    let exclusions = ExclusionsSection::default();
    let overrides = Vec::new();

    let inspector = ConfigInspector::new(repo_root, home_dir, &exclusions, &overrides);

    let files = vec![
        TrackedFile {
            source: "config/match.conf".into(),
            target: "~/.config/match.conf".into(),
            machine_local: false,
            description: None,
        },
        TrackedFile {
            source: "config/drift.conf".into(),
            target: "~/.config/drift.conf".into(),
            machine_local: false,
            description: None,
        },
        TrackedFile {
            source: "config/missing.conf".into(),
            target: "~/.config/missing.conf".into(),
            machine_local: false,
            description: None,
        },
    ];

    let results = inspector.inspect_files(&files);
    assert_eq!(results.len(), 3);

    // match
    assert_eq!(results[0].state, ConfigFileState::Matching);
    assert_eq!(results[0].source_hash, results[0].target_hash);

    // drift
    assert_eq!(results[1].state, ConfigFileState::Changed);
    assert_ne!(results[1].source_hash, results[1].target_hash);
    assert!(results[1].diagnostic.as_ref().unwrap().contains("drift"));

    // missing
    assert_eq!(results[2].state, ConfigFileState::Missing);
    assert!(results[2].target_hash.is_none());
}

#[test]
fn test_machine_local_override_is_preserved() {
    let repo_temp = tempdir().unwrap();
    let home_temp = tempdir().unwrap();

    let repo_root = repo_temp.path();
    let home_dir = home_temp.path();

    let target_dir = home_dir.join(".config/fish/conf.d");
    fs::create_dir_all(&target_dir).unwrap();
    let local_file = target_dir.join("local.fish");
    fs::write(&local_file, "set -gx SECRET_TOKEN 12345\n").unwrap();

    let exclusions = ExclusionsSection::default();
    let overrides = vec![MachineOverride {
        target: "~/.config/fish/conf.d/local.fish".into(),
        description: Some("Machine local tokens".into()),
    }];

    let inspector = ConfigInspector::new(repo_root, home_dir, &exclusions, &overrides);

    let file = TrackedFile {
        source: "fish/conf.d/local.fish".into(),
        target: "~/.config/fish/conf.d/local.fish".into(),
        machine_local: true,
        description: None,
    };

    let observed = inspector.inspect_file(&file);
    assert_eq!(observed.state, ConfigFileState::MachineLocal);
    assert!(observed
        .diagnostic
        .as_ref()
        .unwrap()
        .contains("Machine-local override present"));
}

#[test]
fn test_explicit_exclusions_path_and_pattern() {
    let repo_temp = tempdir().unwrap();
    let home_temp = tempdir().unwrap();

    let repo_root = repo_temp.path();
    let home_dir = home_temp.path();

    let exclusions = ExclusionsSection {
        paths: vec!["~/.config/exact_exclude.conf".into()],
        patterns: vec!["*.backup.*".into(), "*.key".into()],
    };
    let overrides = Vec::new();

    let inspector = ConfigInspector::new(repo_root, home_dir, &exclusions, &overrides);

    let files = vec![
        TrackedFile {
            source: "config/exact_exclude.conf".into(),
            target: "~/.config/exact_exclude.conf".into(),
            machine_local: false,
            description: None,
        },
        TrackedFile {
            source: "config/settings.backup.20261108".into(),
            target: "~/.config/settings.backup.20261108".into(),
            machine_local: false,
            description: None,
        },
        TrackedFile {
            source: "config/priv.key".into(),
            target: "~/.config/priv.key".into(),
            machine_local: false,
            description: None,
        },
    ];

    let results = inspector.inspect_files(&files);
    for r in results {
        assert_eq!(r.state, ConfigFileState::Excluded);
    }
}

#[cfg(unix)]
#[test]
fn test_symlink_edge_cases() {
    use std::os::unix::fs::symlink;

    let repo_temp = tempdir().unwrap();
    let home_temp = tempdir().unwrap();

    let repo_root = repo_temp.path();
    let home_dir = home_temp.path();

    let source = repo_root.join("valid_source.conf");
    fs::write(&source, "symlink source\n").unwrap();

    let external_dest = repo_temp.path().join("other.conf");
    fs::write(&external_dest, "external content\n").unwrap();

    let target_dir = home_dir.join(".config");
    fs::create_dir_all(&target_dir).unwrap();

    // 1. Symlink pointing directly to source
    let symlink_valid = target_dir.join("to_source.conf");
    symlink(&source, &symlink_valid).unwrap();

    // 2. Symlink pointing elsewhere
    let symlink_external = target_dir.join("to_other.conf");
    symlink(&external_dest, &symlink_external).unwrap();

    // 3. Broken symlink
    let symlink_broken = target_dir.join("broken.conf");
    symlink(repo_root.join("nonexistent.conf"), &symlink_broken).unwrap();

    let exclusions = ExclusionsSection::default();
    let overrides = Vec::new();
    let inspector = ConfigInspector::new(repo_root, home_dir, &exclusions, &overrides);

    let file_to_source = TrackedFile {
        source: "valid_source.conf".into(),
        target: "~/.config/to_source.conf".into(),
        machine_local: false,
        description: None,
    };
    let obs1 = inspector.inspect_file(&file_to_source);
    assert_eq!(obs1.state, ConfigFileState::SymlinkToSource);

    let file_external = TrackedFile {
        source: "valid_source.conf".into(),
        target: "~/.config/to_other.conf".into(),
        machine_local: false,
        description: None,
    };
    let obs2 = inspector.inspect_file(&file_external);
    assert_eq!(obs2.state, ConfigFileState::SymlinkExternal);

    let file_broken = TrackedFile {
        source: "valid_source.conf".into(),
        target: "~/.config/broken.conf".into(),
        machine_local: false,
        description: None,
    };
    let obs3 = inspector.inspect_file(&file_broken);
    assert_eq!(obs3.state, ConfigFileState::BrokenSymlink);
}

#[test]
fn test_missing_source_in_repo() {
    let repo_temp = tempdir().unwrap();
    let home_temp = tempdir().unwrap();

    let repo_root = repo_temp.path();
    let home_dir = home_temp.path();

    let exclusions = ExclusionsSection::default();
    let overrides = Vec::new();
    let inspector = ConfigInspector::new(repo_root, home_dir, &exclusions, &overrides);

    let file = TrackedFile {
        source: "nonexistent_source.conf".into(),
        target: "~/.config/something.conf".into(),
        machine_local: false,
        description: None,
    };

    let observed = inspector.inspect_file(&file);
    assert_eq!(observed.state, ConfigFileState::SourceMissing);
}
