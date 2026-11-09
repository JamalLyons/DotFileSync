//! `dotfilesync service install|start|stop|status|uninstall` (per-user launchd LaunchAgent).

use anyhow::{anyhow, bail, Result};
use clap::Subcommand;
use dotfilesync_core::RealCommandRunner;
use dotfilesync_daemon::service::launchd::LaunchdAgent;
use dotfilesync_daemon::service::{
    InstallOptions, InstallOutcome, ServiceManager, ServiceStatus, StopOutcome,
};
use dotfilesync_protocol::{paths::DATA_DIR_ENV, Client, ClientError, DataLayout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::mode::{self, CLIENT_NAME};

/// Test seam: run a fake `launchctl` instead of `/bin/launchctl`.
const LAUNCHCTL_ENV: &str = "DOTFILESYNC_LAUNCHCTL";

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Write the per-user LaunchAgent definition. Does NOT load or start the daemon.
    Install {
        /// Opt in to starting dotfilesyncd automatically at login (default: off)
        #[arg(long)]
        start_at_login: bool,
        /// dotfilesyncd binary to run (default: next to this executable)
        #[arg(long)]
        daemon_path: Option<PathBuf>,
        /// Replace an existing definition with different contents
        #[arg(long)]
        force: bool,
    },
    /// Load the LaunchAgent (if needed) and start dotfilesyncd now
    Start,
    /// Stop dotfilesyncd and unload the LaunchAgent for this session
    Stop,
    /// Show whether the service is installed, loaded, and reachable
    Status,
    /// Stop the service and remove its definition. Keeps your data, configs and repositories.
    Uninstall,
}

fn agent() -> Result<LaunchdAgent> {
    let runner = Arc::new(RealCommandRunner::new(Duration::from_secs(15)));
    let mut agent = LaunchdAgent::for_current_user(runner)?;
    if let Some(fake) = std::env::var_os(LAUNCHCTL_ENV).filter(|v| !v.is_empty()) {
        let fake = fake
            .into_string()
            .map_err(|_| anyhow!("{LAUNCHCTL_ENV} must be UTF-8"))?;
        agent = agent.with_launchctl(fake);
    }
    Ok(agent)
}

fn default_daemon_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("cannot locate the directory of {}", exe.display()))?;
    let candidate = dir.join("dotfilesyncd");
    if !candidate.is_file() {
        bail!(
            "dotfilesyncd was not found next to this executable ({}); pass --daemon-path",
            candidate.display()
        );
    }
    Ok(std::fs::canonicalize(candidate)?)
}

enum Reach {
    Up(u32, String),
    Down(String),
    Error(String),
}

fn reach(layout: &DataLayout) -> Reach {
    match Client::connect(layout, CLIENT_NAME) {
        Ok(mut c) => match c.daemon_info() {
            Ok(info) => Reach::Up(info.pid, info.daemon_version),
            Err(e) => Reach::Error(e.to_string()),
        },
        Err(e @ ClientError::NotRunning { .. }) => Reach::Down(e.to_string()),
        Err(e) => Reach::Error(mode::explain(e, layout).to_string()),
    }
}

fn wait_for(layout: &DataLayout, up: bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let is_up = matches!(reach(layout), Reach::Up(..));
        if is_up == up {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn print_status(st: &ServiceStatus, layout: &DataLayout) {
    println!("LaunchAgent definition: {}", st.definition_path.display());
    if st.installed {
        let login = if st.start_at_login == Some(true) {
            "starts at login"
        } else {
            "does not start at login"
        };
        println!("  installed: yes ({login})");
    } else {
        println!("  installed: no  (run `dotfilesync service install`)");
    }
    println!(
        "  loaded in launchd: {}{}",
        if st.loaded { "yes" } else { "no" },
        match (st.running, st.pid) {
            (true, Some(pid)) => format!(" (running, pid {pid})"),
            (true, None) => " (running)".into(),
            _ => String::new(),
        }
    );
    match reach(layout) {
        Reach::Up(pid, version) => println!(
            "Daemon: reachable at {} (dotfilesyncd {version}, pid {pid})",
            layout.socket.display()
        ),
        Reach::Down(why) => {
            println!("Daemon: not running ({why})");
            println!("  scan/status will run standalone in the CLI process until it is started.");
        }
        Reach::Error(why) => println!("Daemon: unreachable: {why}"),
    }
}

pub fn execute(cmd: ServiceCommand, data_dir: Option<&Path>) -> Result<()> {
    let layout = mode::layout(data_dir)?;
    let agent = agent()?;
    match cmd {
        ServiceCommand::Install {
            start_at_login,
            daemon_path,
            force,
        } => {
            let daemon_path = match daemon_path {
                Some(p) => mode::absolute(&p)?,
                None => default_daemon_path()?,
            };
            // Bake a non-default data dir into the agent so daemon and CLI agree.
            let explicit_dir = data_dir.is_some() || std::env::var_os(DATA_DIR_ENV).is_some();
            let opts = InstallOptions {
                daemon_path,
                start_at_login,
                data_dir: explicit_dir.then(|| layout.root.clone()),
                force,
            };
            let outcome = agent.install(&opts)?;
            let verb = match outcome {
                InstallOutcome::Installed => "Installed",
                InstallOutcome::Replaced => "Replaced",
                InstallOutcome::Unchanged => "Already installed (unchanged)",
            };
            println!("{verb}: {}", agent.plist_path().display());
            println!(
                "Start at login: {}",
                if start_at_login {
                    "enabled"
                } else {
                    "disabled (opt in with `dotfilesync service install --start-at-login --force`)"
                }
            );
            println!(
                "The daemon was not started. Run `dotfilesync service start` to start it now."
            );
        }
        ServiceCommand::Start => {
            if let Reach::Up(pid, _) = reach(&layout) {
                println!("dotfilesyncd is already running (pid {pid}).");
                return Ok(());
            }
            agent.start()?;
            if wait_for(&layout, true, Duration::from_secs(5)) {
                if let Reach::Up(pid, version) = reach(&layout) {
                    println!("dotfilesyncd {version} is running (pid {pid}).");
                }
            } else {
                bail!(
                    "launchd accepted the agent but the daemon is not answering on {}; check {}",
                    layout.socket.display(),
                    layout.log_file.display()
                );
            }
        }
        ServiceCommand::Stop => match agent.stop()? {
            StopOutcome::Unloaded => {
                if !wait_for(&layout, false, Duration::from_secs(10)) {
                    bail!("launchd unloaded the agent but the daemon is still answering");
                }
                println!("Stopped dotfilesyncd and unloaded the LaunchAgent for this session.");
                println!("It stays installed; it will start at next login only if start-at-login is enabled.");
            }
            StopOutcome::NotLoaded => match Client::connect(&layout, CLIENT_NAME) {
                Ok(mut c) => {
                    c.shutdown()?;
                    if !wait_for(&layout, false, Duration::from_secs(10)) {
                        bail!("asked dotfilesyncd to shut down but it is still answering");
                    }
                    println!("Stopped a dotfilesyncd that was not managed by launchd.");
                }
                Err(ClientError::NotRunning { .. }) => println!("dotfilesyncd is not running."),
                Err(e) => return Err(mode::explain(e, &layout)),
            },
        },
        ServiceCommand::Status => {
            let st = agent.status()?;
            print_status(&st, &layout);
        }
        ServiceCommand::Uninstall => {
            let out = agent.uninstall()?;
            if out.unloaded {
                println!("Stopped and unloaded the LaunchAgent.");
            }
            if out.removed_definition {
                println!("Removed {}", agent.plist_path().display());
            } else {
                println!("No LaunchAgent definition was installed.");
            }
            println!(
                "Kept daemon state and logs in {} (remove manually if you no longer need them).",
                layout.root.display()
            );
            println!("Your configuration files and any sync repositories were not touched.");
        }
    }
    Ok(())
}
