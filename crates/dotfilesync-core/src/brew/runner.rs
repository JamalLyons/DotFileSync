use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;
use thiserror::Error;
use wait_timeout::ChildExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Error)]
pub enum RunnerError {
    #[error("Executable '{0}' was not found")]
    ExecutableNotFound(String),

    #[error("Command '{program}' failed to execute: {message}")]
    ExecutionFailed { program: String, message: String },

    #[error("Command '{program}' timed out after {timeout_secs}s")]
    Timeout { program: String, timeout_secs: u64 },

    #[error("Command '{program}' exited with code {code}: {stderr}")]
    NonZeroExit {
        program: String,
        code: i32,
        stderr: String,
    },
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RunnerError>;
    fn find_executable(&self, name: &str) -> Option<PathBuf>;
}

pub struct RealCommandRunner {
    timeout: Duration,
    brew_bin_override: Option<PathBuf>,
}

impl Default for RealCommandRunner {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            brew_bin_override: None,
        }
    }
}

impl RealCommandRunner {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            brew_bin_override: None,
        }
    }

    pub fn with_brew_binary(mut self, path: PathBuf) -> Self {
        self.brew_bin_override = Some(path);
        self
    }
}

impl CommandRunner for RealCommandRunner {
    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        // Check explicit override first
        if name == "brew" {
            if let Some(ref override_path) = self.brew_bin_override {
                if override_path.is_file() {
                    return Some(override_path.clone());
                }
            }
            if let Ok(env_path) = std::env::var("DOTFILESYNC_BREW_BIN") {
                let p = PathBuf::from(env_path);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // First check standard PATH lookup
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }

        // On macOS, Homebrew may be in standard locations even if PATH is minimal
        if name == "brew" {
            let mac_locations = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
            for loc in mac_locations {
                let p = Path::new(loc);
                if p.is_file() {
                    return Some(p.to_path_buf());
                }
            }
        }

        None
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RunnerError> {
        let exec_path = self
            .find_executable(program)
            .unwrap_or_else(|| PathBuf::from(program));

        let mut child = Command::new(&exec_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RunnerError::ExecutableNotFound(program.to_string())
                } else {
                    RunnerError::ExecutionFailed {
                        program: program.to_string(),
                        message: e.to_string(),
                    }
                }
            })?;

        // Drain stdout and stderr while process runs to prevent pipe buffer deadlocks
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| RunnerError::ExecutionFailed {
                program: program.to_string(),
                message: "Failed to capture stdout pipe".to_string(),
            })?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| RunnerError::ExecutionFailed {
                program: program.to_string(),
                message: "Failed to capture stderr pipe".to_string(),
            })?;

        let stdout_handle = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let res = stdout_pipe.read_to_end(&mut bytes);
            (bytes, res)
        });

        let stderr_handle = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let res = stderr_pipe.read_to_end(&mut bytes);
            (bytes, res)
        });

        match child.wait_timeout(self.timeout) {
            Ok(Some(status)) => {
                let (stdout_bytes, stdout_res) =
                    stdout_handle
                        .join()
                        .map_err(|_| RunnerError::ExecutionFailed {
                            program: program.to_string(),
                            message: "Stdout reader thread panicked".to_string(),
                        })?;
                stdout_res.map_err(|e| RunnerError::ExecutionFailed {
                    program: program.to_string(),
                    message: format!("Failed reading stdout: {e}"),
                })?;

                let (stderr_bytes, stderr_res) =
                    stderr_handle
                        .join()
                        .map_err(|_| RunnerError::ExecutionFailed {
                            program: program.to_string(),
                            message: "Stderr reader thread panicked".to_string(),
                        })?;
                stderr_res.map_err(|e| RunnerError::ExecutionFailed {
                    program: program.to_string(),
                    message: format!("Failed reading stderr: {e}"),
                })?;

                let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
                let code = status.code().unwrap_or(-1);

                if !status.success() {
                    Err(RunnerError::NonZeroExit {
                        program: program.to_string(),
                        code,
                        stderr,
                    })
                } else {
                    Ok(CommandOutput {
                        status_code: code,
                        stdout,
                        stderr,
                    })
                }
            }
            Ok(None) => {
                // Timed out, terminate child process
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                Err(RunnerError::Timeout {
                    program: program.to_string(),
                    timeout_secs: self.timeout.as_secs(),
                })
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                Err(RunnerError::ExecutionFailed {
                    program: program.to_string(),
                    message: e.to_string(),
                })
            }
        }
    }
}

type CommandKey = (String, Vec<String>);
type CommandResult = Result<CommandOutput, RunnerError>;

pub struct MockCommandRunner {
    executables: Mutex<HashMap<String, Option<PathBuf>>>,
    canned_responses: Mutex<HashMap<CommandKey, CommandResult>>,
}

impl Default for MockCommandRunner {
    fn default() -> Self {
        Self {
            executables: Mutex::new(HashMap::new()),
            canned_responses: Mutex::new(HashMap::new()),
        }
    }
}

impl MockCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_executable(self, name: impl Into<String>, path: Option<PathBuf>) -> Self {
        self.executables.lock().unwrap().insert(name.into(), path);
        self
    }

    pub fn with_response(
        self,
        program: impl Into<String>,
        args: &[&str],
        response: Result<CommandOutput, RunnerError>,
    ) -> Self {
        let key = (program.into(), args.iter().map(|s| s.to_string()).collect());
        self.canned_responses.lock().unwrap().insert(key, response);
        self
    }
}

impl CommandRunner for MockCommandRunner {
    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        let map = self.executables.lock().unwrap();
        map.get(name).cloned().flatten()
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RunnerError> {
        let key = (
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        let map = self.canned_responses.lock().unwrap();
        if let Some(res) = map.get(&key) {
            res.clone()
        } else {
            Err(RunnerError::ExecutableNotFound(program.to_string()))
        }
    }
}
