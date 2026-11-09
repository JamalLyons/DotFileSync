use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

    #[error("Command '{program}' was cancelled")]
    Cancelled { program: String },

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
    cancel: Option<Arc<AtomicBool>>,
}

impl Default for RealCommandRunner {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            brew_bin_override: None,
            cancel: None,
        }
    }
}

impl RealCommandRunner {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            brew_bin_override: None,
            cancel: None,
        }
    }

    pub fn with_brew_binary(mut self, path: PathBuf) -> Self {
        self.brew_bin_override = Some(path);
        self
    }

    /// Attach a cancellation flag. When the flag becomes `true` while a command is running,
    /// the runner terminates the processes it owns (same cleanup as the timeout path) and
    /// returns [`RunnerError::Cancelled`]. A command is not started if the flag is already set.
    pub fn with_cancellation(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
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
        let start_time = std::time::Instant::now();

        if self.is_cancelled() {
            return Err(RunnerError::Cancelled {
                program: program.display().to_string(),
            });
        }

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
        // Limits:
        // Descendants that explicitly detach from the session or process group (e.g. via setsid(2)
        // or setpgid(2)) are outside this group and cannot be safely discovered or killed without
        // fragile process-table heuristics or PID reuse hazards. However, output-reader threads
        // are bounded by the overall call deadline, so unkillable detached descendants will never
        // block the caller.
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

        // Drain stdout and stderr while process runs to prevent pipe buffer deadlocks.
        // We set non-blocking mode on pipes and use periodic polling so reader threads
        // can observe stop signals promptly rather than blocking indefinitely waiting for EOF.
        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| RunnerError::ExecutionFailed {
                program: program_display.clone(),
                message: "Failed to capture stdout pipe".to_string(),
            })?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| RunnerError::ExecutionFailed {
                program: program_display.clone(),
                message: "Failed to capture stderr pipe".to_string(),
            })?;

        let mut tracked_descendants: Vec<i32> = Vec::new();

        if let Err(e) = set_nonblocking(stdout_pipe.as_raw_fd()) {
            kill_owned_processes(pid, &tracked_descendants, &mut child);
            return Err(RunnerError::ExecutionFailed {
                program: program_display,
                message: format!("Failed setting non-blocking mode on stdout: {e}"),
            });
        }
        if let Err(e) = set_nonblocking(stderr_pipe.as_raw_fd()) {
            kill_owned_processes(pid, &tracked_descendants, &mut child);
            return Err(RunnerError::ExecutionFailed {
                program: program_display,
                message: format!("Failed setting non-blocking mode on stderr: {e}"),
            });
        }

        let stop_stdout = Arc::new(AtomicBool::new(false));
        let stop_stderr = Arc::new(AtomicBool::new(false));

        let stdout_handle = spawn_reader_thread(stdout_pipe, Arc::clone(&stop_stdout));
        let stderr_handle = spawn_reader_thread(stderr_pipe, Arc::clone(&stop_stderr));

        let mut status_opt = None;
        while start_time.elapsed() < self.timeout {
            if self.is_cancelled() {
                stop_stdout.store(true, Ordering::SeqCst);
                stop_stderr.store(true, Ordering::SeqCst);
                kill_owned_processes(pid, &tracked_descendants, &mut child);
                let _ = join_reader_bounded(stdout_handle, &stop_stdout, Duration::from_millis(50));
                let _ = join_reader_bounded(stderr_handle, &stop_stderr, Duration::from_millis(50));
                return Err(RunnerError::Cancelled {
                    program: program_display,
                });
            }
            collect_child_pids(pid as i32, &mut tracked_descendants);
            let remaining = self.timeout.saturating_sub(start_time.elapsed());
            let slice = Duration::from_millis(20).min(remaining);
            match child.wait_timeout(slice) {
                Ok(Some(status)) => {
                    status_opt = Some(status);
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    stop_stdout.store(true, Ordering::SeqCst);
                    stop_stderr.store(true, Ordering::SeqCst);
                    kill_owned_processes(pid, &tracked_descendants, &mut child);
                    let _ =
                        join_reader_bounded(stdout_handle, &stop_stdout, Duration::from_millis(50));
                    let _ =
                        join_reader_bounded(stderr_handle, &stop_stderr, Duration::from_millis(50));
                    return Err(RunnerError::ExecutionFailed {
                        program: program_display,
                        message: e.to_string(),
                    });
                }
            }
        }

        if let Some(status) = status_opt {
            // Direct child exited within timeout.
            // Wait for output-reader threads, bounded by overall call deadline to prevent
            // lingering descendants from keeping pipes open indefinitely.
            while !stdout_handle.is_finished() || !stderr_handle.is_finished() {
                if self.is_cancelled() || start_time.elapsed() >= self.timeout {
                    let cancelled = self.is_cancelled();
                    stop_stdout.store(true, Ordering::SeqCst);
                    stop_stderr.store(true, Ordering::SeqCst);
                    kill_owned_processes(pid, &tracked_descendants, &mut child);
                    let _ =
                        join_reader_bounded(stdout_handle, &stop_stdout, Duration::from_millis(50));
                    let _ =
                        join_reader_bounded(stderr_handle, &stop_stderr, Duration::from_millis(50));
                    if cancelled {
                        return Err(RunnerError::Cancelled {
                            program: program_display,
                        });
                    }
                    return Err(RunnerError::Timeout {
                        program: program_display,
                        timeout: self.timeout,
                    });
                }
                std::thread::sleep(Duration::from_millis(2));
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
        } else {
            // Command wait timed out: kill owned process group and all tracked descendants, reap child, and bound reader cleanup
            stop_stdout.store(true, Ordering::SeqCst);
            stop_stderr.store(true, Ordering::SeqCst);
            kill_owned_processes(pid, &tracked_descendants, &mut child);
            let _ = join_reader_bounded(stdout_handle, &stop_stdout, Duration::from_millis(50));
            let _ = join_reader_bounded(stderr_handle, &stop_stderr, Duration::from_millis(50));
            Err(RunnerError::Timeout {
                program: program_display,
                timeout: self.timeout,
            })
        }
    }
}

fn set_nonblocking(fd: std::os::unix::io::RawFd) -> std::io::Result<()> {
    #[cfg(unix)]
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

type ReaderOutput = (Vec<u8>, std::io::Result<()>);
type ReaderJoinResult = Result<ReaderOutput, RunnerError>;

fn spawn_reader_thread<R: Read + AsRawFd + Send + 'static>(
    mut pipe: R,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<ReaderOutput> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let fd = pipe.as_raw_fd();
        let mut buf = [0u8; 8192];

        while !stop.load(Ordering::Relaxed) {
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
                revents: 0,
            };
            // Poll for up to 15ms so stop notifications or incoming bytes are handled promptly
            let ret = unsafe { libc::poll(&mut pfd, 1, 15) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return (bytes, Err(err));
            }
            if ret > 0 {
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) => {
                            // EOF reached on pipe
                            return (bytes, Ok(()));
                        }
                        Ok(n) => {
                            bytes.extend_from_slice(&buf[..n]);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            break;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                            continue;
                        }
                        Err(e) => {
                            return (bytes, Err(e));
                        }
                    }
                }
            }
        }

        (bytes, Ok(()))
    })
}

fn join_reader_bounded(
    handle: std::thread::JoinHandle<ReaderOutput>,
    stop: &Arc<AtomicBool>,
    max_wait: Duration,
) -> Option<ReaderJoinResult> {
    stop.store(true, Ordering::SeqCst);
    let start = std::time::Instant::now();
    while !handle.is_finished() {
        if start.elapsed() >= max_wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Some(handle.join().map_err(|_| RunnerError::ExecutionFailed {
        program: String::new(),
        message: "Reader thread panicked".to_string(),
    }))
}

#[cfg(target_os = "macos")]
extern "C" {
    fn proc_listchildpids(
        ppid: libc::pid_t,
        buffer: *mut libc::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
}

/// Recursively discovers direct children and descendants of `root_pid`
/// using macOS libSystem's `proc_listchildpids`.
#[cfg(target_os = "macos")]
fn collect_child_pids(root_pid: i32, tracked: &mut Vec<i32>) {
    let mut to_check = vec![root_pid];
    let mut checked = std::collections::HashSet::new();
    let mut buf = [0i32; 128];

    while let Some(parent) = to_check.pop() {
        if !checked.insert(parent) {
            continue;
        }
        let count = unsafe {
            proc_listchildpids(
                parent,
                buf.as_mut_ptr() as *mut libc::c_void,
                std::mem::size_of_val(&buf) as libc::c_int,
            )
        };
        if count > 0 {
            for &child_pid in &buf[..count as usize] {
                if child_pid > 0 && !tracked.contains(&child_pid) {
                    tracked.push(child_pid);
                    to_check.push(child_pid);
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn collect_child_pids(_root_pid: i32, _tracked: &mut Vec<i32>) {}

/// Terminates processes that the runner provably owns: the direct child, the process group
/// created during spawn (PGID = pid), and all tracked descendants discovered via `proc_listchildpids`
/// (including those that detached into a new session via setsid(2)).
fn kill_owned_processes(pid: u32, tracked_descendants: &[i32], child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
        for &descendant_pid in tracked_descendants {
            if descendant_pid > 0 && descendant_pid != pid as i32 {
                libc::kill(-descendant_pid, libc::SIGKILL);
                libc::kill(descendant_pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
