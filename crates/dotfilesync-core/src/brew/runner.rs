use std::collections::HashMap;
use std::ffi::OsStr;
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

    #[error("Command '{program}' timed out after {}", format_timeout(*.timeout))]
    Timeout { program: String, timeout: Duration },

    #[error("Command '{program}' exited with code {code}: {stderr}")]
    NonZeroExit {
        program: String,
        code: i32,
        stderr: String,
    },
}

pub fn format_timeout(timeout: Duration) -> String {
    let millis = timeout.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else if timeout.subsec_millis() == 0 {
        format!("{}s", timeout.as_secs())
    } else {
        format!("{:.2}s", timeout.as_secs_f64())
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RunnerError> {
        let os_args: Vec<&OsStr> = args.iter().copied().map(OsStr::new).collect();
        self.run_path(Path::new(program), &os_args)
    }

    fn run_path(&self, program: &Path, args: &[&OsStr]) -> Result<CommandOutput, RunnerError>;

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
            if let Some(env_path) = std::env::var_os("DOTFILESYNC_BREW_BIN") {
                let p = PathBuf::from(env_path);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // Check standard PATH lookup, preserving native non-UTF-8 paths
        if let Some(path_var) = std::env::var_os("PATH") {
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
        let os_args: Vec<&OsStr> = args.iter().copied().map(OsStr::new).collect();
        self.run_path(Path::new(program), &os_args)
    }

    fn run_path(&self, program: &Path, args: &[&OsStr]) -> Result<CommandOutput, RunnerError> {
        let exec_path = if let Some(prog_str) = program.to_str() {
            self.find_executable(prog_str)
                .unwrap_or_else(|| program.to_path_buf())
        } else {
            program.to_path_buf()
        };

        let mut cmd = Command::new(&exec_path);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

        // On Unix, spawn in a separate process group so timeout cleanup terminates
        // the spawned command and all its descendants without affecting unrelated processes.
        // Limitation: descendants that explicitly detach via setsid()/setpgid() are outside
        // this process group and cannot be reached by group signal termination.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunnerError::ExecutableNotFound(program.display().to_string())
            } else {
                RunnerError::ExecutionFailed {
                    program: program.display().to_string(),
                    message: e.to_string(),
                }
            }
        })?;

        let pid = child.id();
        let program_display = program.display().to_string();

        let kill_all = |child: &mut std::process::Child| {
            kill_process_tree(pid);
            let _ = child.kill();
        };

        // Drain stdout and stderr while process runs to prevent pipe buffer deadlocks
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| RunnerError::ExecutionFailed {
                program: program_display.clone(),
                message: "Failed to capture stdout pipe".to_string(),
            })?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| RunnerError::ExecutionFailed {
                program: program_display.clone(),
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

        let start_time = std::time::Instant::now();

        match child.wait_timeout(self.timeout) {
            Ok(Some(status)) => {
                // Direct child exited within timeout.
                // Wait for output-reader threads, bounded by overall call deadline to prevent
                // lingering descendants from keeping pipes open indefinitely.
                let elapsed = start_time.elapsed();
                if elapsed >= self.timeout {
                    kill_all(&mut child);
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    return Err(RunnerError::Timeout {
                        program: program_display,
                        timeout: self.timeout,
                    });
                }

                let remaining = self.timeout - elapsed;
                let reader_start = std::time::Instant::now();
                while !stdout_handle.is_finished() || !stderr_handle.is_finished() {
                    if reader_start.elapsed() >= remaining {
                        kill_all(&mut child);
                        let _ = child.wait();
                        let _ = stdout_handle.join();
                        let _ = stderr_handle.join();
                        return Err(RunnerError::Timeout {
                            program: program_display,
                            timeout: self.timeout,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }

                let (stdout_bytes, stdout_res) =
                    stdout_handle
                        .join()
                        .map_err(|_| RunnerError::ExecutionFailed {
                            program: program_display.clone(),
                            message: "Stdout reader thread panicked".to_string(),
                        })?;
                stdout_res.map_err(|e| RunnerError::ExecutionFailed {
                    program: program_display.clone(),
                    message: format!("Failed reading stdout: {e}"),
                })?;

                let (stderr_bytes, stderr_res) =
                    stderr_handle
                        .join()
                        .map_err(|_| RunnerError::ExecutionFailed {
                            program: program_display.clone(),
                            message: "Stderr reader thread panicked".to_string(),
                        })?;
                stderr_res.map_err(|e| RunnerError::ExecutionFailed {
                    program: program_display.clone(),
                    message: format!("Failed reading stderr: {e}"),
                })?;

                let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
                let code = status.code().unwrap_or(-1);

                if !status.success() {
                    Err(RunnerError::NonZeroExit {
                        program: program_display,
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
                // Command wait timed out: kill process group, reap child, and join reader threads
                kill_all(&mut child);
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                Err(RunnerError::Timeout {
                    program: program_display,
                    timeout: self.timeout,
                })
            }
            Err(e) => {
                kill_all(&mut child);
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                Err(RunnerError::ExecutionFailed {
                    program: program_display,
                    message: e.to_string(),
                })
            }
        }
    }
}

/// Recursively terminates a process and any descendants, including descendants
/// that detached into their own process groups or sessions (via setsid/setpgid).
fn kill_process_tree(pid: u32) {
    // 1. Discover descendants via pgrep -P WHILE parent is still alive
    // (once parent terminates, children are immediately re-parented to PID 1 by the kernel).
    if let Ok(output) = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            for line in s.lines() {
                if let Ok(child_pid) = line.trim().parse::<i32>() {
                    kill_process_tree(child_pid as u32);
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-child_pid, libc::SIGKILL);
                        libc::kill(child_pid, libc::SIGKILL);
                    }
                }
            }
        }
    }

    // 2. Terminate the parent process group and process
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
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

    fn run_path(&self, program: &Path, args: &[&OsStr]) -> Result<CommandOutput, RunnerError> {
        let prog_str = program.to_string_lossy().to_string();
        let args_vec: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let key = (prog_str, args_vec);
        let map = self.canned_responses.lock().unwrap();
        if let Some(res) = map.get(&key) {
            res.clone()
        } else {
            Err(RunnerError::ExecutableNotFound(
                program.display().to_string(),
            ))
        }
    }
}
