use dotfilesync_core::parse_brewfile;

#[test]
fn test_standard_brewfile_parsing() {
    let content = r#"
# Core CLI
brew "fish"
brew 'git'
brew "ripgrep"

# GUI casks
cask "ghostty"
cask 'zed'
"#;
    let res = parse_brewfile(content);
    assert_eq!(res.formulae, vec!["fish", "git", "ripgrep"]);
    assert_eq!(res.casks, vec!["ghostty", "zed"]);
    assert!(res.unsupported_constructs.is_empty());
}

#[test]
fn test_ordinary_names_with_substrings_import_correctly() {
    // "calendar" and "blender" contain "end"
    // comment on cask contains "system utility"
    let content = r#"
brew "calendar"
brew "blender"
cask "rectangle" # system utility
"#;
    let res = parse_brewfile(content);
    assert_eq!(res.formulae, vec!["blender", "calendar"]);
    assert_eq!(res.casks, vec!["rectangle"]);
    assert!(res.unsupported_constructs.is_empty());
}

#[test]
fn test_unsupported_trailing_expression_is_flagged() {
    let content = r#"brew "git" + extra"#;
    let res = parse_brewfile(content);
    assert!(res.formulae.is_empty());
    assert_eq!(res.unsupported_constructs.len(), 1);
    assert_eq!(
        res.unsupported_constructs[0].construct_type,
        "unsupported_expression"
    );
}

#[test]
fn test_brew_and_cask_with_options() {
    let content = r#"
brew "neovim", args: ["--HEAD"]
cask "google-chrome", args: { appdir: "/Applications" }
"#;
    let res = parse_brewfile(content);
    assert_eq!(res.formulae, vec!["neovim"]);
    assert_eq!(res.casks, vec!["google-chrome"]);
    // Should warn on options
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.message.contains("options")));
}

#[test]
fn test_unsupported_directives() {
    let content = r#"
tap "homebrew/cask-fonts"
mas "Xcode", id: 497799835
whalebrew "whale"
vscode "rust-lang.rust-analyzer"
"#;
    let res = parse_brewfile(content);
    assert!(res.formulae.is_empty());
    assert!(res.casks.is_empty());
    assert_eq!(res.unsupported_constructs.len(), 4);
    assert_eq!(res.unsupported_constructs[0].construct_type, "tap");
    assert_eq!(res.unsupported_constructs[1].construct_type, "mas");
    assert_eq!(
        res.unsupported_constructs[2].construct_type,
        "bundle_extension"
    );
    assert_eq!(
        res.unsupported_constructs[3].construct_type,
        "bundle_extension"
    );
}

#[test]
fn test_dynamic_ruby_constructs() {
    let content = r#"
if OS.mac?
  brew "coreutils"
end

def install_extras
  system "echo hello"
end

cask "app_#{ENV['ENV_TYPE']}"
"#;
    let res = parse_brewfile(content);
    assert!(res.unsupported_constructs.len() >= 4);
    for u in &res.unsupported_constructs {
        assert_eq!(u.construct_type, "dynamic_ruby");
    }
}

#[test]
fn test_malformed_syntax() {
    let content = r#"
brew unquoted_package
cask "unterminated_quote
"#;
    let res = parse_brewfile(content);
    assert_eq!(res.unsupported_constructs.len(), 2);
    assert_eq!(
        res.unsupported_constructs[0].construct_type,
        "malformed_syntax"
    );
    assert_eq!(
        res.unsupported_constructs[1].construct_type,
        "malformed_syntax"
    );
}

#[test]
fn test_original_brewfile_preserved() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let brewfile_path = manifest_dir.join("../../Brewfile");
    let original = std::fs::read_to_string(&brewfile_path).expect("Should read Brewfile");
    let res = parse_brewfile(&original);
    assert!(!res.formulae.is_empty());
    assert!(!res.casks.is_empty());

    // Verify Brewfile is completely unchanged on disk
    let after = std::fs::read_to_string(&brewfile_path).expect("Should read Brewfile");
    assert_eq!(original, after, "Brewfile must remain identical");
}
