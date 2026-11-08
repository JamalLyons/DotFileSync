use serde_json::Value;

use super::model::{BrewDiscoveryResult, ObservedPackage, PackageKind};
use super::runner::CommandRunner;
use crate::diagnostic::Diagnostic;

pub fn discover_homebrew(runner: &dyn CommandRunner) -> BrewDiscoveryResult {
    let mut result = BrewDiscoveryResult::default();

    // 1. Locate brew executable
    let brew_path = match runner.find_executable("brew") {
        Some(path) => path,
        None => {
            result.diagnostics.push(Diagnostic::warning(
                "Homebrew executable ('brew') not found on PATH or standard macOS locations (/opt/homebrew/bin/brew, /usr/local/bin/brew)."
            ));
            return result;
        }
    };

    result.available = true;
    result.brew_path = Some(brew_path.clone());

    // 2. Query brew version
    let brew_prog = brew_path.to_string_lossy();
    match runner.run(&brew_prog, &["--version"]) {
        Ok(out) => {
            let first_line = out.stdout.lines().next().unwrap_or("").trim().to_string();
            if !first_line.is_empty() {
                result.brew_version = Some(first_line);
            }
        }
        Err(e) => {
            result.diagnostics.push(
                Diagnostic::warning("Failed to determine Homebrew version")
                    .with_detail(e.to_string()),
            );
        }
    }

    // 3. Query installed packages
    // Prefer `brew info --json=v2 --installed`, with fallback to `brew list --versions`
    match query_packages_json(runner, &brew_prog) {
        Ok((formulae, casks)) => {
            result.formulae = formulae;
            result.casks = casks;
            return result;
        }
        Err(json_err) => {
            // Attempt fallback to `brew list --versions`
            match query_packages_list(runner, &brew_prog) {
                Ok((formulae, casks)) => {
                    result.formulae = formulae;
                    result.casks = casks;
                    result.diagnostics.push(
                        Diagnostic::info(
                            "Used fallback 'brew list --versions' after JSON inspection was unavailable",
                        )
                        .with_detail(json_err),
                    );
                    return result;
                }
                Err(list_err) => {
                    result.diagnostics.push(
                        Diagnostic::error("Failed to query installed Homebrew packages")
                            .with_detail(format!(
                                "JSON query error: {json_err}; list fallback error: {list_err}"
                            )),
                    );
                }
            }
        }
    }

    result
}

fn query_packages_json(
    runner: &dyn CommandRunner,
    brew_prog: &str,
) -> Result<(Vec<ObservedPackage>, Vec<ObservedPackage>), String> {
    let out = runner
        .run(brew_prog, &["info", "--json=v2", "--installed"])
        .map_err(|e| e.to_string())?;

    let v: Value = serde_json::from_str(&out.stdout)
        .map_err(|e| format!("Failed to parse 'brew info' JSON: {e}"))?;

    let mut formulae = Vec::new();
    if let Some(arr) = v.get("formulae").and_then(|f| f.as_array()) {
        for item in arr {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                let version = item
                    .get("installed")
                    .and_then(|inst| inst.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|obj| obj.get("version"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                formulae.push(ObservedPackage {
                    name: name.to_string(),
                    kind: PackageKind::Formula,
                    version,
                    installed: true,
                });
            }
        }
    }

    let mut casks = Vec::new();
    if let Some(arr) = v.get("casks").and_then(|c| c.as_array()) {
        for item in arr {
            let name = item
                .get("token")
                .or_else(|| item.get("name"))
                .and_then(|n| n.as_str());

            if let Some(token) = name {
                let version = item
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                casks.push(ObservedPackage {
                    name: token.to_string(),
                    kind: PackageKind::Cask,
                    version,
                    installed: true,
                });
            }
        }
    }

    formulae.sort_by(|a, b| a.name.cmp(&b.name));
    casks.sort_by(|a, b| a.name.cmp(&b.name));

    Ok((formulae, casks))
}

fn query_packages_list(
    runner: &dyn CommandRunner,
    brew_prog: &str,
) -> Result<(Vec<ObservedPackage>, Vec<ObservedPackage>), String> {
    let formula_out = runner
        .run(brew_prog, &["list", "--versions", "--formula"])
        .map_err(|e| format!("'brew list --versions --formula' failed: {e}"))?;

    let mut formulae = parse_list_versions_output(&formula_out.stdout, PackageKind::Formula);

    let cask_out = runner
        .run(brew_prog, &["list", "--versions", "--cask"])
        .map_err(|e| format!("'brew list --versions --cask' failed: {e}"))?;

    let mut casks = parse_list_versions_output(&cask_out.stdout, PackageKind::Cask);

    formulae.sort_by(|a, b| a.name.cmp(&b.name));
    casks.sort_by(|a, b| a.name.cmp(&b.name));

    Ok((formulae, casks))
}

pub fn parse_list_versions_output(output: &str, kind: PackageKind) -> Vec<ObservedPackage> {
    let mut packages = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if let Some(&name) = parts.first() {
            let version = if parts.len() > 1 {
                Some(parts[1..].join(" "))
            } else {
                None
            };
            packages.push(ObservedPackage {
                name: name.to_string(),
                kind,
                version,
                installed: true,
            });
        }
    }
    packages
}
