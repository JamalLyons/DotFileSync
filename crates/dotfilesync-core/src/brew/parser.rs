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

        // 1. Check for brew declaration
        if let Some(rest) = trimmed.strip_prefix("brew ") {
            match parse_package_declaration(rest.trim()) {
                Ok((name, has_options)) => {
                    formulae.push(name);
                    if has_options {
                        diagnostics.push(Diagnostic::warning(format!(
                            "Line {line_number}: brew options on '{trimmed}' are not tracked by dotfilesync manifest"
                        )));
                    }
                }
                Err((construct_type, reason)) => {
                    unsupported_constructs.push(UnsupportedConstruct {
                        line_number,
                        raw_line: line.to_string(),
                        construct_type,
                        reason,
                    });
                }
            }
            continue;
        }

        // 2. Check for cask declaration
        if let Some(rest) = trimmed.strip_prefix("cask ") {
            match parse_package_declaration(rest.trim()) {
                Ok((name, has_options)) => {
                    casks.push(name);
                    if has_options {
                        diagnostics.push(Diagnostic::warning(format!(
                            "Line {line_number}: cask options on '{trimmed}' are not tracked by dotfilesync manifest"
                        )));
                    }
                }
                Err((construct_type, reason)) => {
                    unsupported_constructs.push(UnsupportedConstruct {
                        line_number,
                        raw_line: line.to_string(),
                        construct_type,
                        reason,
                    });
                }
            }
            continue;
        }

        // 3. Check for tap declaration
        if trimmed.starts_with("tap ") || trimmed == "tap" {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "tap".to_string(),
                reason: "Homebrew taps are not currently tracked in dotfilesync.toml".to_string(),
            });
            continue;
        }

        // 4. Check for mas declaration
        if trimmed.starts_with("mas ") || trimmed == "mas" {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "mas".to_string(),
                reason: "Mac App Store ('mas') declarations are not supported in this milestone"
                    .to_string(),
            });
            continue;
        }

        // 5. Check for whalebrew / vscode bundle extensions
        if trimmed.starts_with("whalebrew ") || trimmed.starts_with("vscode ") {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "bundle_extension".to_string(),
                reason: "Brew bundle extensions (whalebrew, vscode) are not supported".to_string(),
            });
            continue;
        }

        // 6. Check for dynamic Ruby constructs (outside quoted declarations and comments)
        if is_dynamic_ruby_line(trimmed) {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "dynamic_ruby".to_string(),
                reason: "Dynamic Ruby statement or control flow cannot be evaluated statically"
                    .to_string(),
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

/// Parses the package expression following `brew ` or `cask `.
/// Expects `"name"` or `'name'`, optionally followed by `, args: [...]` or a comment `# ...`.
/// Rejects unsupported trailing expressions like `+ extra` or string interpolations.
fn parse_package_declaration(input: &str) -> Result<(String, bool), (String, String)> {
    let trimmed = input.trim();
    let quote_char = if trimmed.starts_with('"') {
        '"'
    } else if trimmed.starts_with('\'') {
        '\''
    } else {
        return Err((
            "malformed_syntax".to_string(),
            format!("Expected opening quote for package name: '{trimmed}'"),
        ));
    };

    let inner = &trimmed[1..];
    let end_quote_idx = inner.find(quote_char).ok_or_else(|| {
        (
            "malformed_syntax".to_string(),
            format!("Unterminated quote in package declaration: '{trimmed}'"),
        )
    })?;

    let name = &inner[..end_quote_idx];
    if name.trim().is_empty() {
        return Err((
            "malformed_syntax".to_string(),
            "Package name inside quotes is empty".to_string(),
        ));
    }

    // Dynamic string interpolation check
    if name.contains("#{") {
        return Err((
            "dynamic_ruby".to_string(),
            "String interpolation in package name is not supported".to_string(),
        ));
    }

    let remainder = inner[end_quote_idx + 1..].trim();

    // Remainder can be empty, a comment (`# ...`), or options (`,...`)
    if remainder.is_empty() || remainder.starts_with('#') {
        Ok((name.to_string(), false))
    } else if let Some(options_part) = remainder.strip_prefix(',') {
        let opt_trimmed = options_part.trim();
        if opt_trimmed.contains("#{") {
            return Err((
                "dynamic_ruby".to_string(),
                "Dynamic Ruby interpolation inside options is not supported".to_string(),
            ));
        }
        Ok((name.to_string(), true))
    } else {
        // Any other trailing characters (e.g. `+ extra`, `<< something`) are unsupported Ruby expressions
        Err((
            "unsupported_expression".to_string(),
            format!("Unsupported trailing expression following package declaration: '{remainder}'"),
        ))
    }
}

/// Determines whether a line (with comments stripped) is a dynamic Ruby keyword or control statement.
fn is_dynamic_ruby_line(line: &str) -> bool {
    let code = strip_trailing_comment(line).trim();
    if code.is_empty() {
        return false;
    }

    if code.contains("#{") {
        return true;
    }

    let ruby_keywords = [
        "if", "unless", "case", "when", "def", "class", "module", "system", "Kernel.", "ENV[",
        "ENV.", "do", "end", "while", "until", "for", "begin", "rescue", "ensure", "lambda",
        "proc",
    ];

    for kw in ruby_keywords {
        if code == kw
            || code.starts_with(&format!("{kw} "))
            || code.starts_with(&format!("{kw}("))
            || code.starts_with(&format!("{kw}:"))
        {
            return true;
        }
    }

    false
}

fn strip_trailing_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_char = '\0';

    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double && prev_char != '\\' => in_single = !in_single,
            '"' if !in_single && prev_char != '\\' => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
        prev_char = ch;
    }

    line
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
    fn test_ordinary_names_with_substrings_do_not_falsely_trigger_dynamic_ruby() {
        // "calendar" contains "end"
        // "blender" contains "end"
        // comment contains "system utility"
        let input = r#"
brew "calendar"
brew "blender"
cask "rectangle" # system utility
"#;
        let res = parse_brewfile(input);
        assert_eq!(res.formulae, vec!["blender", "calendar"]);
        assert_eq!(res.casks, vec!["rectangle"]);
        assert!(res.unsupported_constructs.is_empty());
    }

    #[test]
    fn test_unsupported_trailing_expression_is_flagged() {
        let input = r#"brew "git" + extra"#;
        let res = parse_brewfile(input);
        assert!(res.formulae.is_empty());
        assert_eq!(res.unsupported_constructs.len(), 1);
        assert_eq!(
            res.unsupported_constructs[0].construct_type,
            "unsupported_expression"
        );
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
