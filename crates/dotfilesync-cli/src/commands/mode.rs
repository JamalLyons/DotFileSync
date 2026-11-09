//! Choosing between the daemon and standalone execution, and explaining the choice.

use anyhow::{anyhow, bail, Result};
use clap::ValueEnum;
use dotfilesync_protocol::{Client, ClientError, DataLayout};
use std::path::{Path, PathBuf};

pub const CLIENT_NAME: &str = concat!("dotfilesync-cli/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Mode {
    /// Use dotfilesyncd if it is running, otherwise run in this process
    #[default]
    Auto,
    /// Require dotfilesyncd; fail with an explanation if it is not running
    Daemon,
    /// Always run in this process without contacting the daemon
    Standalone,
}

pub enum Route {
    Daemon(Box<Client>),
    /// `note` explains why auto mode fell back (printed to stderr).
    Standalone {
        note: Option<String>,
    },
}

pub fn layout(data_dir: Option<&Path>) -> Result<DataLayout> {
    DataLayout::resolve(data_dir).map_err(|e| anyhow!(e))
}

fn not_running_hint() -> &'static str {
    "Start it with `dotfilesync service start` (install it first with `dotfilesync service install`), or run `dotfilesyncd` in a terminal."
}

/// Connect to the daemon, turning errors into user-facing explanations.
pub fn connect(data_dir: Option<&Path>) -> Result<Client> {
    let layout = layout(data_dir)?;
    Client::connect(&layout, CLIENT_NAME).map_err(|e| explain(e, &layout))
}

pub fn explain(e: ClientError, layout: &DataLayout) -> anyhow::Error {
    match e {
        ClientError::NotRunning { .. } => anyhow!("{e}. {}", not_running_hint()),
        ClientError::IncompatibleVersion { .. } => anyhow!(
            "{e}. Restart the daemon after upgrading (`dotfilesync service stop && dotfilesync service start`), or use --mode standalone."
        ),
        ClientError::InsecureDataDir(_) => anyhow!(
            "{e}. The data directory {} must be owned by you with mode 0700.",
            layout.root.display()
        ),
        other => anyhow!("{other}. Use --mode standalone to bypass the daemon."),
    }
}

/// Decide where `command` runs. `standalone_only` names a flag that the daemon cannot honor.
pub fn route(
    mode: Mode,
    data_dir: Option<&Path>,
    command: &str,
    standalone_only: Option<&str>,
) -> Result<Route> {
    if mode == Mode::Standalone {
        return Ok(Route::Standalone { note: None });
    }
    if let Some(flag) = standalone_only {
        let why = format!(
            "{flag} only applies to standalone mode (the daemon uses its own configured Homebrew)"
        );
        if mode == Mode::Daemon {
            bail!("{why}; drop {flag} or use --mode standalone");
        }
        return Ok(Route::Standalone {
            note: Some(format!(
                "note: {why}; running {command} standalone in this process."
            )),
        });
    }
    let layout = match layout(data_dir) {
        Ok(l) => l,
        Err(e) if mode == Mode::Auto => {
            return Ok(Route::Standalone {
                note: Some(format!(
                    "note: cannot locate the dotfilesyncd data directory ({e}); running {command} standalone in this process."
                )),
            })
        }
        Err(e) => return Err(e),
    };
    match Client::connect(&layout, CLIENT_NAME) {
        Ok(c) => Ok(Route::Daemon(Box::new(c))),
        Err(e) if e.is_not_running() && mode == Mode::Auto => Ok(Route::Standalone {
            note: Some(format!(
                "note: dotfilesyncd is not running; running {command} standalone in this process. {}",
                not_running_hint()
            )),
        }),
        Err(e) => Err(explain(e, &layout)),
    }
}

pub fn absolute(p: &Path) -> Result<PathBuf> {
    std::path::absolute(p).map_err(|e| anyhow!("cannot resolve path '{}': {e}", p.display()))
}
