use dotfilesync_core::{parse_manifest, resolve_profile, ProfileSelection, TargetOs};

const MANIFEST: &str = include_str!("../../../dotfilesync.toml");

const GAMES: &[&str] = &["melonds", "minecraft", "roblox", "steam"];

fn resolved(sel: &ProfileSelection, hostname: &str) -> dotfilesync_core::ResolvedManifest {
    let manifest = parse_manifest(MANIFEST).expect("repo dotfilesync.toml should parse");
    resolve_profile(&manifest, sel, hostname, TargetOs::Macos).expect("profile should resolve")
}

#[test]
fn repo_manifest_parses_as_v2() {
    let manifest = parse_manifest(MANIFEST).expect("repo dotfilesync.toml should parse");
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(manifest.setup.login_shell.as_deref(), Some("fish"));
    assert!(manifest.setup.fisher);
    assert_eq!(manifest.select.default.as_deref(), Some("personal"));
    assert_eq!(
        manifest
            .select
            .hostnames
            .get("work-mac")
            .map(String::as_str),
        Some("work")
    );
    assert!(manifest.profiles.contains_key("personal"));
    assert!(manifest.profiles.contains_key("work"));
    assert_eq!(
        manifest.profiles["work"]
            .package_managers
            .homebrew
            .exclude_casks,
        vec!["melonds", "minecraft", "roblox", "steam"]
    );
}

#[test]
fn personal_matches_base_and_work_drops_games() {
    let personal = resolved(&ProfileSelection::Explicit("personal".into()), "host");
    let work = resolved(&ProfileSelection::Explicit("work".into()), "host");
    let base = resolved(&ProfileSelection::BaseOnly, "work-mac");

    assert_eq!(personal.profile.as_deref(), Some("personal"));
    assert_eq!(work.profile.as_deref(), Some("work"));
    assert_eq!(base.profile, None);

    assert_eq!(personal.homebrew.formulae, base.homebrew.formulae);
    assert_eq!(personal.homebrew.casks, base.homebrew.casks);
    assert!(!personal
        .homebrew
        .formulae
        .iter()
        .any(|name| name == "awscli"));
    assert!(!personal.homebrew.casks.iter().any(|name| name == "slack"));

    assert!(work.homebrew.formulae.iter().any(|name| name == "awscli"));
    assert_eq!(
        work.homebrew.formulae.len(),
        personal.homebrew.formulae.len() + 1
    );
    assert!(work.homebrew.casks.iter().any(|name| name == "slack"));
    assert!(work
        .homebrew
        .casks
        .iter()
        .any(|name| name == "font-0xproto"));
    assert_eq!(
        work.homebrew.casks.len(),
        personal.homebrew.casks.len() - GAMES.len() + 1
    );

    for game in GAMES {
        assert!(
            personal.homebrew.casks.iter().any(|name| name == game),
            "{game} should stay on the personal profile"
        );
        assert!(
            base.homebrew.casks.iter().any(|name| name == game),
            "{game} should stay on the base layer"
        );
        assert!(
            !work.homebrew.casks.iter().any(|name| name == game),
            "{game} should be excluded from the work profile"
        );
    }

    for resolved in [&personal, &work, &base] {
        assert!(resolved.setup.fisher);
        assert_eq!(resolved.setup.login_shell.as_deref(), Some("fish"));
        assert!(resolved
            .overrides
            .iter()
            .any(|ov| ov.target == "~/.config/fish/conf.d/local.fish"));
    }

    let personal_targets: Vec<&str> = personal
        .files
        .iter()
        .map(|file| file.target.as_str())
        .collect();
    let work_targets: Vec<&str> = work.files.iter().map(|file| file.target.as_str()).collect();
    let base_targets: Vec<&str> = base.files.iter().map(|file| file.target.as_str()).collect();
    assert!(personal_targets.is_sorted());
    assert_eq!(personal_targets, base_targets);
    assert_eq!(work_targets, base_targets);
    assert!(personal_targets.contains(&"~/.config/fish/conf.d/fnm.fish"));
    assert!(personal_targets
        .iter()
        .all(|target| !target.ends_with("local.fish")));
}

#[test]
fn auto_uses_default_personal_and_hostname_work() {
    let fallback = resolved(&ProfileSelection::Auto, "unmatched-host");
    assert_eq!(fallback.profile.as_deref(), Some("personal"));
    assert!(fallback.homebrew.casks.iter().any(|name| name == "steam"));

    let mapped = resolved(&ProfileSelection::Auto, "Work-Mac.local");
    assert_eq!(mapped.profile.as_deref(), Some("work"));
    assert!(!mapped.homebrew.casks.iter().any(|name| name == "steam"));
    assert!(mapped.homebrew.formulae.iter().any(|name| name == "awscli"));
}
