use super::model::{BrewfileParseResult, UnsupportedConstruct};
use crate::diagnostic::Diagnostic;

#[derive(Debug)]
struct OpenBlock {
    line_number: usize,
    keyword: String,
}

pub fn parse_brewfile(content: &str) -> BrewfileParseResult {
    let mut formulae = Vec::new();
    let mut casks = Vec::new();
    let mut unsupported_constructs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut block_stack: Vec<OpenBlock> = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let code = strip_trailing_comment(trimmed).trim();
        if code.is_empty() {
            continue;
        }

        // 1. Check for block closing: `end`
        if is_block_ender(code) {
            if let Some(open) = block_stack.pop() {
                unsupported_constructs.push(UnsupportedConstruct {
                    line_number,
                    raw_line: line.to_string(),
                    construct_type: "dynamic_ruby".to_string(),
                    reason: format!(
                        "Ruby control-flow block '{}' (opened at line {}) closed with 'end'",
                        open.keyword, open.line_number
                    ),
                });
            } else {
                unsupported_constructs.push(UnsupportedConstruct {
                    line_number,
                    raw_line: line.to_string(),
                    construct_type: "dynamic_ruby".to_string(),
                    reason: "Unexpected 'end' without matching block opening; scope cannot be safely established".to_string(),
                });
            }
            continue;
        }

        // 2. Check for block continuation: `elsif`, `else`, `when`, `rescue`, `ensure`
        if is_block_continuation(code) {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "dynamic_ruby".to_string(),
                reason: format!(
                    "Ruby control-flow branch '{}' cannot be evaluated statically",
                    code
                ),
            });
            continue;
        }

        // 3. Check for block opening statements
        if let Some(kw) = is_block_starter(code) {
            block_stack.push(OpenBlock {
                line_number,
                keyword: kw.to_string(),
            });
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "dynamic_ruby".to_string(),
                reason: format!(
                    "Ruby control-flow block '{}' cannot be evaluated statically",
                    kw
                ),
            });
            continue;
        }

        // 4. Inside an unsupported block: declarations must not enter desired package list
        if let Some(open_block) = block_stack.last() {
            if trimmed.starts_with("brew ") || trimmed.starts_with("cask ") {
                unsupported_constructs.push(UnsupportedConstruct {
                    line_number,
                    raw_line: line.to_string(),
                    construct_type: "dynamic_ruby".to_string(),
                    reason: format!(
                        "Package declaration inside unsupported Ruby control-flow block '{}' (opened at line {}) cannot be evaluated statically and will not be imported",
                        open_block.keyword, open_block.line_number
                    ),
                });
                diagnostics.push(Diagnostic::warning(format!(
                    "Line {line_number}: Conditional package declaration '{trimmed}' inside '{}' block (line {}) will not be imported",
                    open_block.keyword, open_block.line_number
                )));
            } else {
                unsupported_constructs.push(UnsupportedConstruct {
                    line_number,
                    raw_line: line.to_string(),
                    construct_type: "dynamic_ruby".to_string(),
                    reason: format!(
                        "Statement inside unsupported Ruby control-flow block '{}' (opened at line {})",
                        open_block.keyword, open_block.line_number
                    ),
                });
            }
            continue;
        }

        // 5. Unconditional declarations
        // Check for brew declaration
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
                    diagnostics.push(Diagnostic::warning(format!(
                        "Line {line_number}: Conditional or unsupported package declaration '{trimmed}' will not be imported: {reason}"
                    )));
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

        // Check for cask declaration
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
                    diagnostics.push(Diagnostic::warning(format!(
                        "Line {line_number}: Conditional or unsupported package declaration '{trimmed}' will not be imported: {reason}"
                    )));
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

        // Check for tap declaration
        if trimmed.starts_with("tap ") || trimmed == "tap" {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "tap".to_string(),
                reason: "Homebrew taps are not currently tracked in dotfilesync.toml".to_string(),
            });
            continue;
        }

        // Check for mas declaration
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

        // Check for whalebrew / vscode bundle extensions
        if trimmed.starts_with("whalebrew ") || trimmed.starts_with("vscode ") {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number,
                raw_line: line.to_string(),
                construct_type: "bundle_extension".to_string(),
                reason: "Brew bundle extensions (whalebrew, vscode) are not supported".to_string(),
            });
            continue;
        }

        // Check for dynamic Ruby constructs (outside quoted declarations and comments)
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

    // Check for unclosed blocks at EOF
    if !block_stack.is_empty() {
        for open in &block_stack {
            unsupported_constructs.push(UnsupportedConstruct {
                line_number: open.line_number,
                raw_line: format!("Unclosed block '{}'", open.keyword),
                construct_type: "dynamic_ruby".to_string(),
                reason: format!(
                    "Unclosed Ruby control-flow block '{}' opened at line {}; scope cannot be safely established",
                    open.keyword, open.line_number
                ),
            });
            diagnostics.push(Diagnostic::warning(format!(
                "Unclosed Ruby control-flow block '{}' opened at line {}; scope cannot be safely established",
                open.keyword, open.line_number
            )));
        }
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
/// Rejects trailing conditional expressions (e.g. `if OS.mac?`, `unless OS.linux?`) and
/// unsupported trailing expressions like `+ extra` or string interpolations.
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

    // Dynamic string interpolation check in package name
    if name.contains("#{") {
        return Err((
            "dynamic_ruby".to_string(),
            "String interpolation in package name is not supported".to_string(),
        ));
    }

    let remainder = inner[end_quote_idx + 1..].trim();
    let code_remainder = strip_trailing_comment(remainder).trim();

    // Check for trailing conditional expressions (if/unless) even after package options
    if let Some((kw, _)) = has_trailing_conditional(code_remainder) {
        return Err((
            "dynamic_ruby".to_string(),
            format!("Package declaration has trailing '{kw}' conditional modifier and cannot be imported unconditionally"),
        ));
    }

    if code_remainder.is_empty() {
        Ok((name.to_string(), false))
    } else if let Some(options_part) = code_remainder.strip_prefix(',') {
        let opt_trimmed = options_part.trim();
        if opt_trimmed.contains("#{") {
            return Err((
                "dynamic_ruby".to_string(),
                "Dynamic Ruby interpolation inside options is not supported".to_string(),
            ));
        }
        Ok((name.to_string(), true))
    } else {
        Err((
            "unsupported_expression".to_string(),
            format!(
                "Unsupported trailing expression following package declaration: '{code_remainder}'"
            ),
        ))
    }
}

/// Detects trailing `if` or `unless` statement modifiers outside of quoted string arguments.
fn has_trailing_conditional(input: &str) -> Option<(&'static str, usize)> {
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_char = '\0';
    let bytes = input.as_bytes();
    let len = bytes.len();

    let mut i = 0;
    while i < len {
        let ch = bytes[i] as char;

        if ch == '\'' && !in_double && prev_char != '\\' {
            in_single = !in_single;
            prev_char = ch;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single && prev_char != '\\' {
            in_double = !in_double;
            prev_char = ch;
            i += 1;
            continue;
        }

        if !in_single && !in_double {
            // Check for 'if'
            if i + 2 <= len && &bytes[i..i + 2] == b"if" {
                let is_prev_boundary = i == 0 || is_word_boundary(bytes[i - 1] as char);
                let is_next_boundary =
                    i + 2 == len || is_word_boundary(bytes[i + 2] as char) || bytes[i + 2] == b'(';
                if is_prev_boundary && is_next_boundary {
                    return Some(("if", i));
                }
            }
            // Check for 'unless'
            if i + 6 <= len && &bytes[i..i + 6] == b"unless" {
                let is_prev_boundary = i == 0 || is_word_boundary(bytes[i - 1] as char);
                let is_next_boundary =
                    i + 6 == len || is_word_boundary(bytes[i + 6] as char) || bytes[i + 6] == b'(';
                if is_prev_boundary && is_next_boundary {
                    return Some(("unless", i));
                }
            }
        }

        prev_char = ch;
        i += 1;
    }

    None
}

fn is_word_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, ',' | ']' | '}' | ')' | ';' | ':')
}

fn is_block_starter(code: &str) -> Option<&'static str> {
    let block_keywords = [
        "if", "unless", "case", "while", "until", "for", "begin", "def", "class", "module",
    ];

    for &kw in &block_keywords {
        if let Some(rest) = code.strip_prefix(kw) {
            let is_boundary = if rest.is_empty() {
                matches!(kw, "begin" | "case")
            } else {
                let first_char = rest.chars().next().unwrap();
                match kw {
                    "begin" => first_char.is_whitespace() || first_char == ';',
                    "if" | "unless" | "case" | "while" | "until" | "for" => {
                        first_char.is_whitespace() || matches!(first_char, '(' | ';')
                    }
                    "def" => first_char.is_whitespace() || first_char == '(',
                    "class" => first_char.is_whitespace() || matches!(first_char, '<' | ';'),
                    "module" => first_char.is_whitespace() || first_char == ';',
                    _ => false,
                }
            };

            if is_boundary {
                return Some(kw);
            }
        }
    }

    if code.ends_with(" do")
        || code.contains(" do |")
        || code.ends_with(" do|")
        || code.ends_with(" do;")
    {
        return Some("do");
    }

    None
}

fn is_block_ender(code: &str) -> bool {
    if let Some(rest) = code.strip_prefix("end") {
        rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ';')
    } else {
        false
    }
}

fn is_block_continuation(code: &str) -> bool {
    let continuation_keywords = ["else", "elsif", "when", "rescue", "ensure"];
    for &kw in &continuation_keywords {
        if let Some(rest) = code.strip_prefix(kw) {
            let is_boundary = if rest.is_empty() {
                true
            } else {
                let first_char = rest.chars().next().unwrap();
                first_char.is_whitespace() || matches!(first_char, '(' | ';' | ':')
            };

            if is_boundary {
                return true;
            }
        }
    }
    false
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
    fn test_trailing_conditional_if_and_unless_are_rejected() {
        let input = r#"
brew "wget", args: ["with-iri"] if OS.mac?
cask "docker" unless OS.linux?
brew "curl" if condition
"#;
        let res = parse_brewfile(input);
        assert!(res.formulae.is_empty());
        assert!(res.casks.is_empty());
        assert_eq!(res.unsupported_constructs.len(), 3);
        for u in &res.unsupported_constructs {
            assert_eq!(u.construct_type, "dynamic_ruby");
        }
        assert!(res
            .diagnostics
            .iter()
            .any(|d| d.message.contains("Conditional")));
    }

    #[test]
    fn test_packages_inside_ruby_control_flow_blocks_not_imported() {
        let input = r#"
brew "git"

if OS.mac?
  brew "coreutils"
  cask "docker"
end

unless OS.linux?
  brew "mac-only-tool"
end
"#;
        let res = parse_brewfile(input);
        // Only unconditional git must be imported
        assert_eq!(res.formulae, vec!["git"]);
        assert!(res.casks.is_empty());
        assert!(!res.formulae.contains(&"coreutils".to_string()));
        assert!(!res.formulae.contains(&"mac-only-tool".to_string()));
        assert!(!res.casks.contains(&"docker".to_string()));

        // Both warnings and unsupported constructs must be recorded
        assert!(!res.unsupported_constructs.is_empty());
        assert!(res
            .diagnostics
            .iter()
            .any(|d| d.message.contains("coreutils")));
        assert!(res.diagnostics.iter().any(|d| d.message.contains("docker")));
    }

    #[test]
    fn test_nested_ruby_blocks_handled_conservatively() {
        let input = r#"
if OS.mac?
  if Hardware::CPU.arm?
    brew "arm-pkg"
  end
  brew "intel-pkg"
end
"#;
        let res = parse_brewfile(input);
        assert!(res.formulae.is_empty());
        assert!(!res.unsupported_constructs.is_empty());
    }

    #[test]
    fn test_unclosed_block_at_eof() {
        let input = r#"
if OS.mac?
  brew "coreutils"
"#;
        let res = parse_brewfile(input);
        assert!(res.formulae.is_empty());
        assert!(res
            .unsupported_constructs
            .iter()
            .any(|u| u.reason.contains("Unclosed")));
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
        assert!(
            res.casks.is_empty(),
            "cask docker inside if block must not be imported"
        );
        assert_eq!(res.unsupported_constructs.len(), 6);
        assert_eq!(res.unsupported_constructs[0].construct_type, "tap");
        assert_eq!(res.unsupported_constructs[1].construct_type, "dynamic_ruby");
        assert_eq!(res.unsupported_constructs[2].construct_type, "dynamic_ruby");
        assert_eq!(res.unsupported_constructs[3].construct_type, "dynamic_ruby");
        assert_eq!(res.unsupported_constructs[4].construct_type, "mas");
        assert_eq!(
            res.unsupported_constructs[5].construct_type,
            "bundle_extension"
        );
    }

    #[test]
    fn test_keyword_prefix_variable_does_not_suppress_following_brew_declaration() {
        // 'beginner = true' must not match 'begin' block starter and suppress 'brew "git"'
        let input = r#"
beginner = true
definitely_true = 1
format = :json
brew "git"
"#;
        let res = parse_brewfile(input);
        assert_eq!(res.formulae, vec!["git"]);
        // Non-brew/cask statements are flagged as unrecognized, not dynamic_ruby block starters
        for u in &res.unsupported_constructs {
            assert_eq!(u.construct_type, "unrecognized");
        }
    }
}
