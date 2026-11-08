use serde_json::Value;
use std::path::Path;

use super::model::{BrewDiscoveryResult, ObservedPackage, PackageKind};
use super::runner::{CommandOutput, CommandRunner, RunnerError};
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

    // 2. Query brew version (using native Path without lossy conversion)
    match run_brew_command(runner, &brew_path, &["--version"]) {
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
    match query_packages_json(runner, &brew_path) {
        Ok((formulae, casks)) => {
            result.packages_enumerated = true;
            result.formulae = formulae;
            result.casks = casks;
            return result;
        }
        Err(json_err) => {
            // Attempt fallback to `brew list --versions`
            match query_packages_list(runner, &brew_path) {
                Ok((formulae, casks)) => {
                    result.packages_enumerated = true;
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
                    result.packages_enumerated = false;
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

fn run_brew_command(
    runner: &dyn CommandRunner,
    brew_path: &Path,
    args: &[&str],
) -> Result<CommandOutput, RunnerError> {
    if let Some(prog) = brew_path.to_str() {
        runner.run(prog, args)
    } else {
        let os_args: Vec<&std::ffi::OsStr> =
            args.iter().copied().map(std::ffi::OsStr::new).collect();
        runner.run_path(brew_path, &os_args)
    }
}

fn query_packages_json(
    runner: &dyn CommandRunner,
    brew_path: &Path,
) -> Result<(Vec<ObservedPackage>, Vec<ObservedPackage>), String> {
    let out = run_brew_command(runner, brew_path, &["info", "--json=v2", "--installed"])
        .map_err(|e| e.to_string())?;

    let v: Value = serde_json::from_str(&out.stdout)
        .map_err(|e| format!("Failed to parse 'brew info' JSON: {e}"))?;

    let obj = v
        .as_object()
        .ok_or_else(|| "Root JSON response is not an object".to_string())?;

    let formulae_arr = obj
        .get("formulae")
        .ok_or_else(|| "JSON response missing 'formulae' field".to_string())?
        .as_array()
        .ok_or_else(|| "'formulae' field is not an array".to_string())?;

    let mut formulae = Vec::new();
    for item in formulae_arr {
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

    let casks_arr = obj
        .get("casks")
        .ok_or_else(|| "JSON response missing 'casks' field".to_string())?
        .as_array()
        .ok_or_else(|| "'casks' field is not an array".to_string())?;

    let mut casks = Vec::new();
    for item in casks_arr {
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

    formulae.sort_by(|a, b| a.name.cmp(&b.name));
    casks.sort_by(|a, b| a.name.cmp(&b.name));

    Ok((formulae, casks))
}

fn query_packages_list(
    runner: &dyn CommandRunner,
    brew_path: &Path,
) -> Result<(Vec<ObservedPackage>, Vec<ObservedPackage>), String> {
    let formula_out = run_brew_command(runner, brew_path, &["list", "--versions", "--formula"])
        .map_err(|e| format!("'brew list --versions --formula' failed: {e}"))?;

    let mut formulae = parse_list_versions_output(&formula_out.stdout, PackageKind::Formula);

    let cask_out = run_brew_command(runner, brew_path, &["list", "--versions", "--cask"])
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
