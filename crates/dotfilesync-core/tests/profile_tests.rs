use dotfilesync_core::{
    parse_manifest, resolve_profile, ManifestError, ProfileSelection, TargetOs,
};

fn resolve(
    toml: &str,
    sel: &ProfileSelection,
    hostname: &str,
) -> dotfilesync_core::ResolvedManifest {
    let manifest = parse_manifest(toml).expect("manifest should parse");
    resolve_profile(&manifest, sel, hostname, TargetOs::Macos).expect("profile should resolve")
}

#[test]
fn base_only_ignores_profiles() {
    let toml = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["git", "fish"]
casks = ["zed"]

[[files]]
source = "fish/config.fish"
target = "~/.config/fish/config.fish"

[profiles.work.package_managers.homebrew]
formulae = ["awscli"]
"#;
    let resolved = resolve(toml, &ProfileSelection::BaseOnly, "work-mac");
    assert_eq!(resolved.profile, None);
    assert_eq!(resolved.homebrew.formulae, vec!["fish", "git"]);
    assert_eq!(resolved.homebrew.casks, vec!["zed"]);
    assert_eq!(resolved.files.len(), 1);
}

#[test]
fn overlay_adds_packages() {
    let toml = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["git"]
casks = ["zed"]

[profiles.work.package_managers.homebrew]
formulae = ["awscli", "git"]
casks = ["slack"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Explicit("work".into()), "host");
    assert_eq!(resolved.profile.as_deref(), Some("work"));
    assert_eq!(resolved.homebrew.formulae, vec!["awscli", "git"]);
    assert_eq!(resolved.homebrew.casks, vec!["slack", "zed"]);
}

#[test]
fn exclude_removes_package_from_earlier_layer() {
    let toml = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["git", "minecraft"]
casks = ["steam", "zed"]

[profiles.work.package_managers.homebrew]
exclude_formulae = ["minecraft"]
exclude_casks = ["steam"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Explicit("work".into()), "host");
    assert_eq!(resolved.homebrew.formulae, vec!["git"]);
    assert_eq!(resolved.homebrew.casks, vec!["zed"]);
}

#[test]
fn later_layer_readds_excluded_package() {
    let toml = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["minecraft"]

[profiles.personal.package_managers.homebrew]
exclude_formulae = ["minecraft"]

[profiles.work]
extends = ["personal"]

[profiles.work.package_managers.homebrew]
formulae = ["minecraft", "awscli"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Explicit("work".into()), "host");
    assert_eq!(resolved.homebrew.formulae, vec!["awscli", "minecraft"]);
}

#[test]
fn conflicting_file_target_later_layer_wins() {
    let toml = r#"
schema_version = 1

[[files]]
source = "base/config.fish"
target = "~/.config/fish/config.fish"
mode = "copy"

[[profiles.work.files]]
source = "work/config.fish"
target = "~/.config/fish/config.fish"
mode = "symlink"

[profiles.work]
exclude_files = []
"#;
    let resolved = resolve(toml, &ProfileSelection::Explicit("work".into()), "host");
    assert_eq!(resolved.files.len(), 1);
    assert_eq!(resolved.files[0].source, "work/config.fish");
    assert!(matches!(
        resolved.files[0].mode,
        dotfilesync_core::FileMode::Symlink
    ));
}

#[test]
fn exclude_files_removes_by_target() {
    let toml = r#"
schema_version = 1

[[files]]
source = "zed/settings.json"
target = "~/.config/zed/settings.json"

[[files]]
source = "fish/config.fish"
target = "~/.config/fish/config.fish"

[profiles.work]
exclude_files = ["~/.config/zed/settings.json"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Explicit("work".into()), "host");
    assert_eq!(resolved.files.len(), 1);
    assert_eq!(resolved.files[0].source, "fish/config.fish");
}

#[test]
fn extends_chain_is_depth_first_and_cycle_errors() {
    let toml = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["git"]

[profiles.base_extra.package_managers.homebrew]
formulae = ["ripgrep"]

[profiles.work]
extends = ["base_extra"]

[profiles.work.package_managers.homebrew]
formulae = ["awscli"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Explicit("work".into()), "host");
    assert_eq!(resolved.homebrew.formulae, vec!["awscli", "git", "ripgrep"]);

    let cycle = r#"
schema_version = 1

[profiles.a]
extends = ["b"]

[profiles.b]
extends = ["a"]
"#;
    let err = parse_manifest(cycle).unwrap_err();
    assert!(matches!(err, ManifestError::Validation(..)), "{err}");
    assert!(err.to_string().contains("cycle"), "{err}");
}

#[test]
fn hostname_auto_selects_profile_and_strips_local_suffix() {
    let toml = r#"
schema_version = 1

[select]
hostnames = { "jamals-mbp" = "personal", "work-mac" = "work" }

[profiles.personal.package_managers.homebrew]
casks = ["slack"]

[profiles.work.package_managers.homebrew]
casks = ["zoom"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Auto, "Jamals-MBP.local");
    assert_eq!(resolved.profile.as_deref(), Some("personal"));
    assert_eq!(resolved.homebrew.casks, vec!["slack"]);
}

#[test]
fn default_profile_is_fallback_when_hostname_is_unknown() {
    let toml = r#"
schema_version = 1

[select]
hostnames = { "jamals-mbp" = "work" }
default = "personal"

[profiles.personal.package_managers.homebrew]
formulae = ["fish"]

[profiles.work.package_managers.homebrew]
formulae = ["awscli"]
"#;
    let resolved = resolve(toml, &ProfileSelection::Auto, "other-host");
    assert_eq!(resolved.profile.as_deref(), Some("personal"));
    assert_eq!(resolved.homebrew.formulae, vec!["fish"]);
}

#[test]
fn unknown_explicit_profile_errors() {
    let toml = r#"
schema_version = 1

[profiles.personal.package_managers.homebrew]
formulae = ["fish"]
"#;
    let manifest = parse_manifest(toml).unwrap();
    let err = resolve_profile(
        &manifest,
        &ProfileSelection::Explicit("nope".into()),
        "host",
        TargetOs::Macos,
    )
    .unwrap_err();
    assert!(matches!(err, ManifestError::UnknownProfile { .. }), "{err}");
}

#[test]
fn os_mismatch_is_filtered() {
    let toml = r#"
schema_version = 1

[[files]]
source = "mac/only.conf"
target = "~/.config/mac.conf"
os = "macos"

[[files]]
source = "linux/only.conf"
target = "~/.config/linux.conf"
os = "linux"

[profiles.linuxbox]
os = "linux"

[profiles.linuxbox.package_managers.homebrew]
formulae = ["linux-only"]

[profiles.macbox]
os = "macos"

[profiles.macbox.package_managers.homebrew]
formulae = ["mac-only"]

[[profiles.macbox.files]]
source = "mac/extra.conf"
target = "~/.config/mac-extra.conf"
os = "linux"
"#;
    let mac = resolve(toml, &ProfileSelection::Explicit("macbox".into()), "host");
    assert_eq!(mac.homebrew.formulae, vec!["mac-only"]);
    let targets: Vec<_> = mac.files.iter().map(|file| file.target.as_str()).collect();
    assert_eq!(targets, vec!["~/.config/mac.conf"]);

    let skipped = resolve(toml, &ProfileSelection::Explicit("linuxbox".into()), "host");
    assert!(skipped.homebrew.formulae.is_empty());
    assert_eq!(skipped.files.len(), 1);
    assert_eq!(skipped.files[0].target, "~/.config/mac.conf");
}

#[test]
fn manifest_without_profiles_resolves_to_base() {
    let toml = r#"
schema_version = 1

[package_managers.homebrew]
formulae = ["git"]

[[files]]
source = "fish/config.fish"
target = "~/.config/fish/config.fish"
"#;
    let manifest = parse_manifest(toml).expect("base manifest should parse");
    assert_eq!(manifest.schema_version, 1);
    let resolved = resolve_profile(
        &manifest,
        &ProfileSelection::Auto,
        "any-host",
        TargetOs::Macos,
    )
    .unwrap();
    assert_eq!(resolved.profile, None);
    assert_eq!(resolved.homebrew.formulae, vec!["git"]);
    assert_eq!(resolved.files.len(), 1);
}

#[test]
fn unknown_profile_in_select_is_a_validation_error() {
    let toml = r#"
schema_version = 1

[select]
default = "missing"
"#;
    let err = parse_manifest(toml).unwrap_err();
    assert!(matches!(err, ManifestError::Validation(..)), "{err}");
    assert!(err.to_string().contains("missing"), "{err}");
}
