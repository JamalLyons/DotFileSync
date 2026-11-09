//! Per-user launchd LaunchAgent management (`gui/<uid>` domain; never a system daemon).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dotfilesync_core::{CommandRunner, RunnerError};

use super::{
    InstallOptions, InstallOutcome, ServiceError, ServiceManager, ServiceStatus, StopOutcome,
    UninstallOutcome,
};

pub const LABEL: &str = "io.github.jamallyons.dotfilesyncd";
pub const LAUNCHCTL: &str = "/bin/launchctl";

pub struct LaunchdAgent {
    label: String,
    plist_path: PathBuf,
    uid: u32,
    launchctl: String,
    runner: Arc<dyn CommandRunner>,
}

impl LaunchdAgent {
    /// `launch_agents_dir` is normally `~/Library/LaunchAgents`; tests pass a temp dir and a
    /// recording runner so the real launchd is never touched.
    pub fn new(launch_agents_dir: &Path, uid: u32, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            label: LABEL.into(),
            plist_path: launch_agents_dir.join(format!("{LABEL}.plist")),
            uid,
            launchctl: LAUNCHCTL.into(),
            runner,
        }
    }

    /// Use a different `launchctl` executable (tests use a fake that records its arguments).
    pub fn with_launchctl(mut self, path: impl Into<String>) -> Self {
        self.launchctl = path.into();
        self
    }

    pub fn for_current_user(runner: Arc<dyn CommandRunner>) -> Result<Self, ServiceError> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or_else(|| ServiceError::Io {
                path: PathBuf::from("~"),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"),
            })?;
        Ok(Self::new(
            &PathBuf::from(home).join("Library/LaunchAgents"),
            dotfilesync_protocol::current_euid(),
            runner,
        ))
    }

    pub fn plist_path(&self) -> &Path {
        &self.plist_path
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    fn target(&self) -> String {
        format!("gui/{}/{}", self.uid, self.label)
    }

    fn launchctl(&self, args: &[&str]) -> Result<String, ServiceError> {
        self.runner
            .run(&self.launchctl, args)
            .map(|o| o.stdout)
            .map_err(|e| ServiceError::Command {
                command: format!("launchctl {}", args.join(" ")),
                message: e.to_string(),
            })
    }

    /// `Some(print output)` if launchd knows the agent.
    fn print(&self) -> Result<Option<String>, ServiceError> {
        let target = self.target();
        match self.runner.run(&self.launchctl, &["print", &target]) {
            Ok(out) => Ok(Some(out.stdout)),
            Err(RunnerError::NonZeroExit { .. }) => Ok(None),
            Err(e) => Err(ServiceError::Command {
                command: format!("launchctl print {target}"),
                message: e.to_string(),
            }),
        }
    }

    pub fn render_plist(&self, opts: &InstallOptions) -> Result<String, ServiceError> {
        let daemon = path_str(&opts.daemon_path)?;
        let mut args = vec![xml_escape(daemon)];
        if let Some(dir) = &opts.data_dir {
            args.push("--data-dir".into());
            args.push(xml_escape(path_str(dir)?));
        }
        let args_xml: String = args
            .iter()
            .map(|a| format!("        <string>{a}</string>\n"))
            .collect();
        let run_at_load = if opts.start_at_login { "true" } else { "false" };
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Managed by `dotfilesync service`. Per-user LaunchAgent; never a root daemon. -->
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}    </array>
    <key>RunAtLoad</key>
    <{run_at_load}/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>/dev/null</string>
    <key>StandardErrorPath</key>
    <string>/dev/null</string>
</dict>
</plist>
"#,
            label = xml_escape(&self.label),
        ))
    }

    fn read_ours(&self) -> Result<Option<String>, ServiceError> {
        let meta = match fs::symlink_metadata(&self.plist_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(self.io(e)),
        };
        if !meta.file_type().is_file() {
            return Err(ServiceError::NotOurs {
                path: self.plist_path.clone(),
            });
        }
        let content = fs::read_to_string(&self.plist_path).map_err(|e| self.io(e))?;
        if !content.contains(&format!("<string>{}</string>", xml_escape(&self.label))) {
            return Err(ServiceError::NotOurs {
                path: self.plist_path.clone(),
            });
        }
        Ok(Some(content))
    }

    fn io(&self, source: std::io::Error) -> ServiceError {
        ServiceError::Io {
            path: self.plist_path.clone(),
            source,
        }
    }
}

impl ServiceManager for LaunchdAgent {
    fn install(&self, opts: &InstallOptions) -> Result<InstallOutcome, ServiceError> {
        if !opts.daemon_path.is_absolute() {
            return Err(ServiceError::InvalidDaemonPath {
                path: opts.daemon_path.clone(),
                reason: "must be absolute".into(),
            });
        }
        if !opts.daemon_path.is_file() {
            return Err(ServiceError::InvalidDaemonPath {
                path: opts.daemon_path.clone(),
                reason: "file does not exist".into(),
            });
        }
        let content = self.render_plist(opts)?;
        let outcome = match self.read_ours() {
            Ok(Some(existing)) if existing == content => return Ok(InstallOutcome::Unchanged),
            Ok(Some(_)) if !opts.force => {
                return Err(ServiceError::DefinitionExists {
                    path: self.plist_path.clone(),
                })
            }
            Ok(Some(_)) => InstallOutcome::Replaced,
            Ok(None) => InstallOutcome::Installed,
            Err(ServiceError::NotOurs { .. }) if opts.force && self.plist_path.is_file() => {
                InstallOutcome::Replaced
            }
            Err(e) => return Err(e),
        };
        if let Some(dir) = self.plist_path.parent() {
            fs::create_dir_all(dir).map_err(|e| self.io(e))?;
        }
        let tmp = self.plist_path.with_extension("plist.tmp");
        let _ = fs::remove_file(&tmp);
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(&tmp)
                .map_err(|e| self.io(e))?;
            f.write_all(content.as_bytes()).map_err(|e| self.io(e))?;
        }
        fs::rename(&tmp, &self.plist_path).map_err(|e| self.io(e))?;
        Ok(outcome)
    }

    fn start(&self) -> Result<(), ServiceError> {
        if self.read_ours()?.is_none() {
            return Err(ServiceError::NotInstalled {
                path: self.plist_path.clone(),
            });
        }
        if self.print()?.is_none() {
            let plist = path_str(&self.plist_path)?.to_string();
            self.launchctl(&["bootstrap", &self.domain(), &plist])?;
        }
        self.launchctl(&["kickstart", &self.target()])?;
        Ok(())
    }

    fn stop(&self) -> Result<StopOutcome, ServiceError> {
        if self.print()?.is_none() {
            return Ok(StopOutcome::NotLoaded);
        }
        self.launchctl(&["bootout", &self.target()])?;
        Ok(StopOutcome::Unloaded)
    }

    fn status(&self) -> Result<ServiceStatus, ServiceError> {
        let content = self.read_ours()?;
        let printed = self.print()?;
        let pid = printed.as_deref().and_then(parse_pid);
        let running = printed
            .as_deref()
            .is_some_and(|p| p.lines().any(|l| l.trim() == "state = running"));
        Ok(ServiceStatus {
            definition_path: self.plist_path.clone(),
            installed: content.is_some(),
            start_at_login: content.as_deref().map(parse_run_at_load),
            loaded: printed.is_some(),
            running,
            pid,
        })
    }

    fn uninstall(&self) -> Result<UninstallOutcome, ServiceError> {
        // Validate ownership of the definition before touching launchd.
        let ours = self.read_ours()?;
        let unloaded = self.stop()? == StopOutcome::Unloaded;
        let removed_definition = if ours.is_some() {
            fs::remove_file(&self.plist_path).map_err(|e| self.io(e))?;
            true
        } else {
            false
        };
        Ok(UninstallOutcome {
            unloaded,
            removed_definition,
        })
    }
}

fn path_str(p: &Path) -> Result<&str, ServiceError> {
    p.to_str().ok_or_else(|| ServiceError::InvalidDaemonPath {
        path: p.to_path_buf(),
        reason: "launchd property lists require UTF-8 paths".into(),
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_run_at_load(plist: &str) -> bool {
    let compact: String = plist.split_whitespace().collect();
    compact.contains("<key>RunAtLoad</key><true/>")
}

fn parse_pid(print: &str) -> Option<u32> {
    print.lines().find_map(|l| {
        l.trim()
            .strip_prefix("pid = ")
            .and_then(|v| v.trim().parse().ok())
    })
}
