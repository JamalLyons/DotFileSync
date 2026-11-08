use super::model::{BrewfileParseResult, UnsupportedConstruct};
use crate::diagnostic::Diagnostic;

pub fn parse_brewfile(content: &str) -> BrewfileParseResult {
    let mut formulae = Vec::new();
    let mut casks = Vec::new();
    let mut unsupported_constructs = Vec::new();
    let mut diagnostics = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Check for dynamic Ruby constructs
        if is_dynamic_ruby(trimmed) {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "dynamic_ruby".to_string(),
                reason: "Dynamic Ruby statement, control flow, or method invocation cannot be evaluated statically".to_string(),
            });
            continue;
        }

        // Check for brew declaration
        if let Some(rest) = trimmed.strip_prefix("brew ") {
            match extract_package_entry(rest) {
                Ok((name, has_options)) => {
                    formulae.push(name);
                    if has_options {
                        diagnostics.push(
                            Diagnostic::warning(format!(
                                "Line {line_number}: brew options on '{rest}' are not tracked by dotfilesync manifest"
                            ))
                        );
                    }
                }
                Err(err) => {
                    unsupported_constructs.push(UnsupportedConstruct {
                        line_number,
                        raw_line: line.to_string(),
                        construct_type: "malformed_brew".to_string(),
                        reason: err,
                    });
                }
            }
            continue;
        }

        // Check for cask declaration
        if let Some(rest) = trimmed.strip_prefix("cask ") {
            match extract_package_entry(rest) {
                Ok((name, has_options)) => {
                    casks.push(name);
                    if has_options {
                        diagnostics.push(
                            Diagnostic::warning(format!(
                                "Line {line_number}: cask options on '{rest}' are not tracked by dotfilesync manifest"
                            ))
                        );
                    }
                }
                Err(err) => {
                    unsupported_constructs.push(UnsupportedConstruct {
                        line_number,
                        raw_line: line.to_string(),
                        construct_type: "malformed_cask".to_string(),
                        reason: err,
                    });
                }
            }
            continue;
        }

        // Check for tap declaration
        if trimmed.starts_with("tap ") {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "tap".to_string(),
                reason: "Homebrew taps are not currently tracked in dotfilesync.toml".to_string(),
            });
            continue;
        }

        // Check for mas declaration
        if trimmed.starts_with("mas ") {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "mas".to_string(),
                reason: "Mac App Store ('mas') declarations are not supported in this milestone"
                    .to_string(),
            });
            continue;
        }

        // Check for whalebrew / vscode
        if trimmed.starts_with("whalebrew ") || trimmed.starts_with("vscode ") {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "bundle_extension".to_string(),
                reason: "Brew bundle extensions (whalebrew, vscode) are not supported".to_string(),
            });
            continue;
        }

        // Anything else is unrecognized
        unsupported_constructs.push(UnsupportedConstruct {
            line_number,
            raw_line: line.to_string(),
            construct_type: "unrecognized".to_string(),
            reason: "Unrecognized Brewfile statement or directive".to_string(),
        });
    }

    formulae.sort();
    formulae.dedup();
    casks.sort();
    casks.dedup();

    if unsupported_constructs.is_empty() {
        diagnostics.push(Diagnostic::info(format!(
            "Successfully parsed Brewfile: {} formulae, {} casks",
            formulae.len(),
            casks.len()
        )));
    } else {
        diagnostics.push(Diagnostic::warning(format!(
            "Parsed Brewfile with {} formulae, {} casks, and {} unsupported/dynamic constructs",
            formulae.len(),
            casks.len(),
            unsupported_constructs.len()
        )));
    }

    BrewfileParseResult {
        formulae,
        casks,
        unsupported_constructs,
        diagnostics,
    }
}

fn is_dynamic_ruby(line: &str) -> bool {
    let tokens = [
        "if ", "unless ", "case ", "when ", "def ", "class ", "module ", "system ", "Kernel.",
        "ENV[", "ENV.", "#{", "do |", ".each", "lambda", "proc", "rescue ", "ensure", "end",
    ];
    for token in tokens {
        if line.starts_with(token) || line.contains(token) {
            return true;
        }
    }
    false
}

/// Extracts a quoted string from the beginning of the string, and detects if options follow.
/// e.g. `"fish"` or `'git', args: ['--HEAD']`
fn extract_package_entry(input: &str) -> Result<(String, bool), String> {
    let trimmed = input.trim();
    let quote_char = if trimmed.starts_with('"') {
        '"'
    } else if trimmed.starts_with('\'') {
        '\''
    } else {
        return Err(format!(
            "Expected opening quote in package declaration: '{trimmed}'"
        ));
    };

    let inner = &trimmed[1..];
    if let Some(end_quote_idx) = inner.find(quote_char) {
        let name = &inner[..end_quote_idx];
        if name.trim().is_empty() {
            return Err("Package name inside quotes is empty".to_string());
        }
        let remainder = inner[end_quote_idx + 1..].trim();
        // If remainder contains more than a comment (e.g., `, args: [...]`), flag options
        let has_options = if remainder.is_empty() || remainder.starts_with('#') {
            false
        } else {
            remainder.starts_with(',')
        };
        Ok((name.to_string(), has_options))
    } else {
        Err(format!(
            "Unterminated quote in package declaration: '{trimmed}'"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clean_brewfile() {
        let input = r#"
# Core CLI
brew "fish"
brew "git"
brew 'gh'

# GUI apps
cask "ghostty"
cask "zed"
"#;
        let res = parse_brewfile(input);
        assert_eq!(res.formulae, vec!["fish", "gh", "git"]);
        assert_eq!(res.casks, vec!["ghostty", "zed"]);
        assert!(res.unsupported_constructs.is_empty());
    }

    #[test]
    fn test_parse_dynamic_and_unsupported_brewfile() {
        let input = r#"
tap "homebrew/cask-fonts"
brew "git"

if OS.mac?
  cask "docker"
end

mas "Xcode", id: 497799835
whalebrew "whale"
"#;
        let res = parse_brewfile(input);
        assert_eq!(res.formulae, vec!["git"]);
        assert_eq!(res.unsupported_constructs.len(), 5);
        assert_eq!(res.unsupported_constructs[0].construct_type, "tap");
        assert_eq!(res.unsupported_constructs[1].construct_type, "dynamic_ruby");
        assert_eq!(res.unsupported_constructs[2].construct_type, "dynamic_ruby");
        assert_eq!(res.unsupported_constructs[3].construct_type, "mas");
        assert_eq!(
            res.unsupported_constructs[4].construct_type,
            "bundle_extension"
        );
    }
}
