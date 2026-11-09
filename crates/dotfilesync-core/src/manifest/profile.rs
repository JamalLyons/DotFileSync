//! Profile selection and layer merge.
//!
//! Merge order is base, then each `extends` profile depth-first in listed order, then the
//! selected profile. Within a layer, package names are added and then `exclude_*` removes them
//! (an exclude wins over an add from an earlier layer; a later layer can add the name again).
//! Files are keyed by manifest target path: a later layer replaces the earlier entry wholesale,
//! and `exclude_files` removes by that same target. Scalar `setup` keys use last-layer-wins when
//! the key is present. Exclusions and overrides are unioned. Lists are sorted and deduplicated.
//! Entries whose `os` does not match the requested [`TargetOs`] are dropped. A cycle in `extends`
//! is an error.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::model::{
    ExclusionsSection, HomebrewSection, MachineOverride, Manifest, Profile, SetupOverlay,
    SetupSection, TargetOs, TrackedFile,
};
use super::ManifestError;
use crate::manifest::ValidationError;

/// How a caller chooses which profile to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSelection {
    /// `select.hostnames[hostname]`, else `select.default`, else the base layer only.
    Auto,
    /// A named profile. It must exist.
    Explicit(String),
    /// Ignore `select` and every profile.
    BaseOnly,
}

/// The manifest after profile selection, OS filtering, and layer merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedManifest {
    pub profile: Option<String>,
    pub os: TargetOs,
    pub homebrew: HomebrewSection,
    pub files: Vec<TrackedFile>,
    pub setup: SetupSection,
    pub exclusions: ExclusionsSection,
    pub overrides: Vec<MachineOverride>,
}

/// Hostname from `libc::gethostname`, with a trailing `.local` removed and ASCII lowercased.
pub fn current_hostname() -> String {
    normalize_hostname(&raw_hostname())
}

/// The only implemented target OS.
pub fn current_os() -> TargetOs {
    TargetOs::Macos
}

pub fn normalize_hostname(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    trimmed
        .strip_suffix(".local")
        .unwrap_or(&trimmed)
        .to_string()
}

fn raw_hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a writable zeroed array of `buf.len()` bytes, which is the contract of
    // `gethostname`. The returned length is bounded by the first NUL or the buffer size.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

pub(crate) fn profile_validation_errors(manifest: &Manifest) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for (name, profile) in &manifest.profiles {
        if name.is_empty() {
            errors.push(ValidationError::UnknownProfile {
                name: name.clone(),
                context: "profiles".into(),
            });
        }
        if let Some(os) = &profile.os {
            if os.trim().is_empty() {
                errors.push(ValidationError::InvalidOsCondition { value: os.clone() });
            }
        }
        for extended in &profile.extends {
            if !manifest.profiles.contains_key(extended) {
                errors.push(ValidationError::UnknownProfile {
                    name: extended.clone(),
                    context: format!("profiles.{name}.extends"),
                });
            }
        }
    }

    for (host, profile) in &manifest.select.hostnames {
        if !manifest.profiles.contains_key(profile) {
            errors.push(ValidationError::UnknownProfile {
                name: profile.clone(),
                context: format!("select.hostnames.{host}"),
            });
        }
    }
    if let Some(default) = &manifest.select.default {
        if !manifest.profiles.contains_key(default) {
            errors.push(ValidationError::UnknownProfile {
                name: default.clone(),
                context: "select.default".into(),
            });
        }
    }

    if let Err(ManifestError::ProfileCycle { cycle }) =
        extend_order(manifest, manifest.profiles.keys().cloned().collect())
    {
        errors.push(ValidationError::ProfileCycle { cycle });
    }

    errors
}

/// Resolve `sel` against `manifest` for `hostname` and `os`.
pub fn resolve_profile(
    manifest: &Manifest,
    sel: &ProfileSelection,
    hostname: &str,
    os: TargetOs,
) -> Result<ResolvedManifest, ManifestError> {
    let profile_name = select_name(manifest, sel, hostname)?;
    let order = match &profile_name {
        Some(name) => extend_order(manifest, vec![name.clone()])?,
        None => Vec::new(),
    };

    let mut formulae = BTreeSet::new();
    let mut casks = BTreeSet::new();
    for name in &manifest.package_managers.homebrew.formulae {
        formulae.insert(name.clone());
    }
    for name in &manifest.package_managers.homebrew.casks {
        casks.insert(name.clone());
    }

    let mut files: BTreeMap<String, TrackedFile> = BTreeMap::new();
    for file in &manifest.files {
        if file_os_matches(file, os) {
            files.insert(file.target.clone(), file.clone());
        }
    }

    let mut setup = manifest.setup.clone();
    let mut exclusion_paths = BTreeSet::new();
    let mut exclusion_patterns = BTreeSet::new();
    for path in &manifest.exclusions.paths {
        exclusion_paths.insert(path.clone());
    }
    for pattern in &manifest.exclusions.patterns {
        exclusion_patterns.insert(pattern.clone());
    }
    let mut overrides: BTreeMap<String, MachineOverride> = BTreeMap::new();
    for ov in &manifest.overrides {
        overrides.insert(ov.target.clone(), ov.clone());
    }

    for name in &order {
        let profile = manifest
            .profiles
            .get(name)
            .ok_or_else(|| ManifestError::UnknownProfile { name: name.clone() })?;
        if !profile_os_matches(profile, os) {
            continue;
        }
        apply_packages(&mut formulae, &mut casks, profile);
        apply_files(&mut files, profile, os);
        apply_setup(&mut setup, &profile.setup);
        for path in &profile.exclusions.paths {
            exclusion_paths.insert(path.clone());
        }
        for pattern in &profile.exclusions.patterns {
            exclusion_patterns.insert(pattern.clone());
        }
        for ov in &profile.overrides {
            overrides.insert(ov.target.clone(), ov.clone());
        }
    }

    Ok(ResolvedManifest {
        profile: profile_name,
        os,
        homebrew: HomebrewSection {
            formulae: formulae.into_iter().collect(),
            casks: casks.into_iter().collect(),
        },
        files: files.into_values().collect(),
        setup,
        exclusions: ExclusionsSection {
            paths: exclusion_paths.into_iter().collect(),
            patterns: exclusion_patterns.into_iter().collect(),
        },
        overrides: overrides.into_values().collect(),
    })
}

fn select_name(
    manifest: &Manifest,
    sel: &ProfileSelection,
    hostname: &str,
) -> Result<Option<String>, ManifestError> {
    match sel {
        ProfileSelection::BaseOnly => Ok(None),
        ProfileSelection::Explicit(name) => {
            if manifest.profiles.contains_key(name) {
                Ok(Some(name.clone()))
            } else {
                Err(ManifestError::UnknownProfile { name: name.clone() })
            }
        }
        ProfileSelection::Auto => {
            let host = normalize_hostname(hostname);
            let mapped = manifest
                .select
                .hostnames
                .iter()
                .find(|(key, _)| normalize_hostname(key) == host)
                .map(|(_, value)| value.clone())
                .or_else(|| manifest.select.default.clone());
            match mapped {
                Some(name) if manifest.profiles.contains_key(&name) => Ok(Some(name)),
                Some(name) => Err(ManifestError::UnknownProfile { name }),
                None => Ok(None),
            }
        }
    }
}

/// Depth-first `extends` order, each profile emitted after the profiles it extends.
fn extend_order(manifest: &Manifest, roots: Vec<String>) -> Result<Vec<String>, ManifestError> {
    let mut order = Vec::new();
    let mut visiting = Vec::new();
    let mut done = HashSet::new();
    for root in roots {
        walk(manifest, &root, &mut visiting, &mut done, &mut order)?;
    }
    Ok(order)
}

fn walk(
    manifest: &Manifest,
    name: &str,
    visiting: &mut Vec<String>,
    done: &mut HashSet<String>,
    order: &mut Vec<String>,
) -> Result<(), ManifestError> {
    if done.contains(name) {
        return Ok(());
    }
    if visiting.iter().any(|seen| seen == name) {
        let mut cycle = visiting.clone();
        cycle.push(name.to_string());
        return Err(ManifestError::ProfileCycle {
            cycle: cycle.join(" -> "),
        });
    }
    let extends = manifest
        .profiles
        .get(name)
        .ok_or_else(|| ManifestError::UnknownProfile {
            name: name.to_string(),
        })?
        .extends
        .clone();
    visiting.push(name.to_string());
    for extended in &extends {
        walk(manifest, extended, visiting, done, order)?;
    }
    visiting.pop();
    done.insert(name.to_string());
    order.push(name.to_string());
    Ok(())
}

fn profile_os_matches(profile: &Profile, os: TargetOs) -> bool {
    match &profile.os {
        None => true,
        Some(declared) => os.matches(declared),
    }
}

fn file_os_matches(file: &TrackedFile, os: TargetOs) -> bool {
    match &file.os {
        None => true,
        Some(declared) => os.matches(declared),
    }
}

fn apply_packages(
    formulae: &mut BTreeSet<String>,
    casks: &mut BTreeSet<String>,
    profile: &Profile,
) {
    let brew = &profile.package_managers.homebrew;
    for name in &brew.formulae {
        formulae.insert(name.clone());
    }
    for name in &brew.casks {
        casks.insert(name.clone());
    }
    for name in &brew.exclude_formulae {
        formulae.remove(name);
    }
    for name in &brew.exclude_casks {
        casks.remove(name);
    }
}

fn apply_files(files: &mut BTreeMap<String, TrackedFile>, profile: &Profile, os: TargetOs) {
    for file in &profile.files {
        if file_os_matches(file, os) {
            files.insert(file.target.clone(), file.clone());
        }
    }
    for target in &profile.exclude_files {
        files.remove(target);
    }
}

fn apply_setup(setup: &mut SetupSection, overlay: &SetupOverlay) {
    if let Some(shell) = &overlay.login_shell {
        setup.login_shell = Some(shell.clone());
    }
    if let Some(fisher) = overlay.fisher {
        setup.fisher = fisher;
    }
}
