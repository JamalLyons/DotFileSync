#[cfg(not(target_os = "macos"))]
compile_error!(
    "dotfilesyncd ships for macOS only in v1; the headless Linux service is tracked in #12."
);

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dotfilesync_daemon::{Daemon, DaemonConfig};
use dotfilesync_protocol::DataLayout;

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

const USAGE: &str = "\
dotfilesyncd - per-user dotfilesync daemon (local Unix socket only; no network listener)

USAGE:
    dotfilesyncd [--data-dir <DIR>] [--brew-bin <PATH>]

OPTIONS:
    --data-dir <DIR>   Data directory (default: $DOTFILESYNC_DATA_DIR or
                       ~/Library/Application Support/dotfilesync)
    --brew-bin <PATH>  Homebrew executable to use (default: $DOTFILESYNC_BREW_BIN, PATH,
                       /opt/homebrew/bin/brew, /usr/local/bin/brew)
    -V, --version      Print version
    -h, --help         Print help

Runs in the foreground. Stop with SIGTERM/SIGINT or `dotfilesync service stop`.";

fn main() -> ExitCode {
    let mut data_dir: Option<PathBuf> = None;
    let mut brew_bin: Option<PathBuf> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--data-dir") => data_dir = args.next().map(PathBuf::from),
            Some("--brew-bin") => brew_bin = args.next().map(PathBuf::from),
            Some("-V") | Some("--version") => {
                println!("dotfilesyncd {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Some("-h") | Some("--help") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("dotfilesyncd: unexpected argument {arg:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let layout = match DataLayout::resolve(data_dir.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dotfilesyncd: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut config = DaemonConfig::new(layout);
    config.brew_bin = brew_bin;

    // SAFETY: installing an async-signal-safe handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        // A client vanishing mid-write must surface as EPIPE, not kill the daemon.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let daemon = match Daemon::bind(config) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("dotfilesyncd: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "dotfilesyncd {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        daemon.socket_path().display()
    );
    if daemon.recovered_interrupted > 0 {
        eprintln!(
            "dotfilesyncd: {} job(s) from the previous run were interrupted and will not be replayed",
            daemon.recovered_interrupted
        );
    }

    let handle = daemon.shutdown_handle();
    std::thread::spawn(move || loop {
        if SIGNALLED.load(Ordering::SeqCst) {
            handle.shutdown();
            return;
        }
        if handle.is_shutdown() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    });

    match daemon.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dotfilesyncd: {e}");
            ExitCode::FAILURE
        }
    }
}
