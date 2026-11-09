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
    assert!(
        res.formulae.is_empty(),
        "coreutils inside if block must not be imported"
    );
    assert!(
        res.casks.is_empty(),
        "interpolated cask must not be imported"
    );
    assert!(res.unsupported_constructs.len() >= 4);
    for u in &res.unsupported_constructs {
        assert_eq!(u.construct_type, "dynamic_ruby");
    }
}

#[test]
fn test_conditional_block_packages_not_imported_and_warned() {
    let content = r#"
brew "git"

if OS.mac?
  brew "coreutils"
  cask "docker"
end

unless OS.linux?
  brew "htop"
end
"#;
    let res = parse_brewfile(content);
    // CRITICAL: Unconditional git must be imported; conditional packages must NOT enter desired lists!
    assert_eq!(res.formulae, vec!["git"]);
    assert!(res.casks.is_empty());
    assert!(!res.formulae.contains(&"coreutils".to_string()));
    assert!(!res.formulae.contains(&"htop".to_string()));
    assert!(!res.casks.contains(&"docker".to_string()));

    // Must emit warnings specifically for skipped conditional packages
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.message.contains("coreutils")));
    assert!(res.diagnostics.iter().any(|d| d.message.contains("docker")));
    assert!(res.diagnostics.iter().any(|d| d.message.contains("htop")));

    // Must record unsupported constructs
    assert!(res
        .unsupported_constructs
        .iter()
        .any(|u| u.raw_line.contains("coreutils")));
    assert!(res
        .unsupported_constructs
        .iter()
        .any(|u| u.raw_line.contains("docker")));
    assert!(res
        .unsupported_constructs
        .iter()
        .any(|u| u.raw_line.contains("htop")));
}

#[test]
fn test_trailing_conditional_modifiers_not_imported_and_warned() {
    let content = r#"
brew "wget", args: ["with-iri"] if OS.mac?
cask "docker" unless OS.linux?
brew "curl" if condition
"#;
    let res = parse_brewfile(content);
    assert!(
        res.formulae.is_empty(),
        "Conditional brew declarations must not be imported"
    );
    assert!(
        res.casks.is_empty(),
        "Conditional cask declarations must not be imported"
    );

    // Warnings must be emitted
    assert!(res.diagnostics.iter().any(|d| d.message.contains("wget")));
    assert!(res.diagnostics.iter().any(|d| d.message.contains("docker")));
    assert!(res.diagnostics.iter().any(|d| d.message.contains("curl")));

    // Unsupported constructs must be recorded with dynamic_ruby type
    assert_eq!(res.unsupported_constructs.len(), 3);
    for u in &res.unsupported_constructs {
        assert_eq!(u.construct_type, "dynamic_ruby");
        assert!(u.reason.contains("conditional"));
    }
}

#[test]
fn test_nested_ruby_blocks_handled_conservatively() {
    let content = r#"
if OS.mac?
  if Hardware::CPU.arm?
    brew "arm-tool"
  end
  brew "intel-tool"
end
"#;
    let res = parse_brewfile(content);
    assert!(res.formulae.is_empty());
    assert!(!res.formulae.contains(&"arm-tool".to_string()));
    assert!(!res.formulae.contains(&"intel-tool".to_string()));
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.message.contains("arm-tool")));
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.message.contains("intel-tool")));
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

#[test]
fn test_beginner_variable_does_not_suppress_following_brew_declaration() {
    let content = r#"
beginner = true
brew "git"
"#;
    let res = parse_brewfile(content);
    assert_eq!(res.formulae, vec!["git"]);
    assert_eq!(res.unsupported_constructs.len(), 1);
    assert_eq!(res.unsupported_constructs[0].construct_type, "unrecognized");
}
