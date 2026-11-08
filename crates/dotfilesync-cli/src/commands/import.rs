use anyhow::{Context, Result};
use clap::Args;
use dotfilesync_core::{
    parse_brewfile, serialize_manifest, HomebrewSection, Manifest, ManifestMetadata,
    PackageManagersSection, CURRENT_SCHEMA_VERSION,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::output::{human, json};

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Path to Brewfile to statically import
    #[arg(short, long)]
    pub brewfile: PathBuf,

    /// Optional destination path to write a new dotfilesync.toml manifest skeleton (refuses to overwrite existing files)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Output parse results in JSON format
    #[arg(long)]
    pub json: bool,
}

pub fn execute(args: ImportArgs) -> Result<()> {
    let content = fs::read_to_string(&args.brewfile)
        .with_context(|| format!("Failed to read Brewfile at '{}'", args.brewfile.display()))?;

    let parse_result = parse_brewfile(&content);

    if let Some(output_path) = &args.output {
        let manifest = Manifest {
            schema_version: CURRENT_SCHEMA_VERSION,
            metadata: Some(ManifestMetadata {
                name: Some("imported-brewfile".to_string()),
                description: Some(format!(
                    "Imported statically from '{}'",
                    args.brewfile.display()
                )),
            }),
            package_managers: PackageManagersSection {
                homebrew: HomebrewSection {
                    formulae: parse_result.formulae.clone(),
                    casks: parse_result.casks.clone(),
                },
            },
            files: Vec::new(),
            exclusions: Default::default(),
            overrides: Vec::new(),
        };

        let toml_str = serialize_manifest(&manifest)?;

        // Atomic create_new: refuses to replace an existing file, leaving its bytes completely untouched.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::anyhow!(
                        "Output manifest file '{}' already exists; refusing to overwrite to preserve existing configuration and exclusions.",
                        output_path.display()
                    )
                } else {
                    anyhow::anyhow!(
                        "Failed to create new output file '{}': {}",
                        output_path.display(),
                        e
                    )
                }
            })?;

        file.write_all(toml_str.as_bytes()).with_context(|| {
            format!(
                "Failed to write manifest contents to '{}'",
                output_path.display()
            )
        })?;
    }

    if args.json {
        println!("{}", json::render_json(&parse_result)?);
    } else {
        println!(
            "{}",
            human::render_import_result(
                &parse_result,
                args.output.as_ref().and_then(|p| p.to_str())
            )
        );
    }

    Ok(())
}
