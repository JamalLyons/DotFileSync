//! Transport, handshake, framing and security boundary tests over a temporary socket.

mod common;

use common::*;
use dotfilesync_daemon::{Daemon, DaemonError};
use dotfilesync_protocol::client::ClientOptions;
use dotfilesync_protocol::{
    current_euid, write_frame, Client, ClientError, ErrorCode, Outcome, Response, ServerFrame,
};
use serde_json::json;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

#[test]
fn client_handshake_ping_and_info_over_temp_socket() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut c = env.client();
    assert_eq!(c.daemon().protocol_version, 1);
    assert_eq!(c.daemon().pid, std::process::id());
    c.ping().unwrap();
    let info = c.daemon_info().unwrap();
    assert_eq!(info.data_dir, env.layout.root);
    assert_eq!(info.active_jobs, 0);
}

#[test]
fn incompatible_version_is_rejected_explicitly() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let err = Client::connect_with(
        &env.layout,
        ClientOptions {
            protocol_version: 999,
            ..Default::default()
        },
    )
    .err()
    .expect("must fail");
    match err {
        ClientError::IncompatibleVersion {
            supported,
            requested,
        } => {
            assert_eq!(supported, vec![1]);
            assert_eq!(requested, 999);
        }
        other => panic!("expected IncompatibleVersion, got {other:?}"),
    }
}

#[test]
fn wrong_uid_peer_is_rejected_before_handshake() {
    // Limitation: CI has no second OS user, so the foreign UID comes from an injected
    // authenticator. The production path uses getpeereid(2); see same_uid test below.
    let env = Env::new(Brew::Fast);
    let _d = Running::start(with_auth(env.config(), FixedUid(current_euid() + 1)));
    match Client::connect(&env.layout, "test") {
        Err(ClientError::Unauthorized(msg)) => assert!(msg.contains("only the user")),
        other => panic!("expected Unauthorized, got {:?}", other.err()),
    }
    // A raw client gets the error frame and then EOF, without the daemon reading its input.
    let mut s = raw(&env.layout);
    match recv(&mut s) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::Unauthorized),
        other => panic!("unexpected {other:?}"),
    }
    assert!(is_closed(&mut s));
    let log = fs::read_to_string(&env.layout.log_file).unwrap();
    assert!(log.contains(&format!(
        "rejected connection from uid {}",
        current_euid() + 1
    )));
}

#[test]
fn same_uid_peer_is_accepted_via_real_kernel_credentials() {
    // Default config uses OsPeerAuthenticator (getpeereid); the test process is the owner.
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut c = env.client();
    c.ping().unwrap();
}

#[test]
fn request_cannot_claim_a_uid() {
    // There is no uid field anywhere in the protocol; extra fields are ignored or rejected,
    // and a foreign peer stays rejected regardless of what it sends.
    let env = Env::new(Brew::Fast);
    let _d = Running::start(with_auth(env.config(), FixedUid(current_euid() + 7)));
    let mut s = raw(&env.layout);
    // The daemon rejects a foreign peer on accept and may close before our hello is written,
    // so the write can race to EPIPE; the queued Unauthorized frame is still readable.
    let hello = json!({"type":"hello","protocol_version":1,"client":"x","uid":current_euid()});
    let _ = write_frame(&mut s, &serde_json::to_vec(&hello).unwrap(), usize::MAX);
    match recv(&mut s) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::Unauthorized),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn first_frame_must_be_hello() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut s = raw(&env.layout);
    send_json(&mut s, &json!({"type":"request","id":1,"op":"ping"}));
    match recv(&mut s) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::HandshakeRequired),
        other => panic!("unexpected {other:?}"),
    }
    assert!(is_closed(&mut s));
}

#[test]
fn malformed_requests_get_typed_errors_and_daemon_stays_healthy() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut s = raw(&env.layout);
    hello(&mut s);

    // Valid JSON with an id but wrong shape: correlated error, connection stays open.
    send_json(&mut s, &json!({"type":"request","id":5,"op":7}));
    match recv(&mut s) {
        ServerFrame::Response {
            id: 5,
            outcome: Outcome::Err(e),
        } => assert_eq!(e.code, ErrorCode::MalformedRequest),
        other => panic!("unexpected {other:?}"),
    }
    // Unknown and reserved operations.
    send_json(
        &mut s,
        &json!({"type":"request","id":6,"op":"exec","params":{"cmd":"id"}}),
    );
    match recv(&mut s) {
        ServerFrame::Response {
            id: 6,
            outcome: Outcome::Err(e),
        } => assert_eq!(e.code, ErrorCode::UnknownOperation),
        other => panic!("unexpected {other:?}"),
    }
    send_json(
        &mut s,
        &json!({"type":"request","id":7,"op":"runtime.x","params":{}}),
    );
    match recv(&mut s) {
        ServerFrame::Response {
            id: 7,
            outcome: Outcome::Err(e),
        } => {
            assert_eq!(e.code, ErrorCode::UnsupportedOperation);
            assert!(e.message.contains("#5"));
        }
        other => panic!("unexpected {other:?}"),
    }
    // Relative paths are rejected by validation.
    send_json(
        &mut s,
        &json!({"type":"request","id":8,"op":"start_job","params":{"spec":{"kind":"scan","manifest":null,"home":"relative/home","repo_root":null}}}),
    );
    match recv(&mut s) {
        ServerFrame::Response {
            id: 8,
            outcome: Outcome::Err(e),
        } => assert_eq!(e.code, ErrorCode::InvalidParams),
        other => panic!("unexpected {other:?}"),
    }
    send_json(&mut s, &json!({"type":"request","id":9,"op":"ping"}));
    assert!(matches!(
        recv(&mut s),
        ServerFrame::Response {
            id: 9,
            outcome: Outcome::Ok(Response::Pong)
        }
    ));

    // Non-JSON garbage: connection-level error, then closed.
    write_frame(&mut s, b"\xff\xfe not json", usize::MAX).unwrap();
    match recv(&mut s) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::MalformedRequest),
        other => panic!("unexpected {other:?}"),
    }
    assert!(is_closed(&mut s));

    env.client().ping().unwrap();
}

#[test]
fn oversized_frame_is_rejected_without_allocation() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let mut s = raw(&env.layout);
    hello(&mut s);
    // Announce 1 GiB; send only the header.
    s.write_all(&(1u32 << 30).to_be_bytes()).unwrap();
    match recv(&mut s) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::FrameTooLarge),
        other => panic!("unexpected {other:?}"),
    }
    assert!(is_closed(&mut s));
    // An oversized handshake is rejected too.
    let mut s2 = raw(&env.layout);
    s2.write_all(&(70_000u32).to_be_bytes()).unwrap();
    match recv(&mut s2) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::FrameTooLarge),
        other => panic!("unexpected {other:?}"),
    }
    env.client().ping().unwrap();
}

#[test]
fn silent_and_half_sent_clients_are_dropped_and_do_not_block_others() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();

    // Slow-loris: connects and never sends a handshake.
    let mut silent = raw(&env.layout);
    // Half a frame header, then nothing.
    let mut partial = raw(&env.layout);
    partial.write_all(&[0, 0]).unwrap();
    // Disconnects mid-frame.
    {
        let mut gone = raw(&env.layout);
        gone.write_all(&100u32.to_be_bytes()).unwrap();
        gone.write_all(b"{\"type\"").unwrap();
    }

    let t = Instant::now();
    env.client().ping().unwrap();
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "other clients must stay responsive"
    );

    // The handshake timeout (500 ms in tests) closes stalled connections.
    let t = Instant::now();
    assert!(is_closed(&mut silent));
    assert!(is_closed(&mut partial));
    assert!(t.elapsed() < Duration::from_secs(4));
}

#[test]
fn connection_limit_returns_busy() {
    let env = Env::new(Brew::Fast);
    let mut cfg = env.config();
    cfg.connection.max_connections = 2;
    cfg.connection.handshake_timeout = Duration::from_secs(5);
    let _d = Running::start(cfg);
    let _a = env.client();
    let _b = env.client();
    let mut s = raw(&env.layout);
    match recv(&mut s) {
        ServerFrame::Error { error } => assert_eq!(error.code, ErrorCode::Busy),
        other => panic!("unexpected {other:?}"),
    }
    drop(_a);
    assert!(wait_until(Duration::from_secs(3), || Client::connect(
        &env.layout,
        "t"
    )
    .is_ok()));
}

#[test]
fn private_permissions_on_dirs_socket_and_state() {
    let env = Env::new(Brew::Fast);
    let _d = env.start();
    let l = &env.layout;
    for dir in [&l.root, &l.run_dir, &l.state_dir, &l.logs_dir] {
        assert_eq!(mode_of(dir), 0o700, "{}", dir.display());
    }
    for file in [&l.socket, &l.lock_file, &l.journal, &l.log_file] {
        assert_eq!(mode_of(file), 0o600, "{}", file.display());
    }
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(&l.socket)
        .unwrap()
        .file_type()
        .is_socket());
}

#[test]
fn refuses_permissive_or_symlinked_data_dir_and_client_refuses_too() {
    let env = Env::new(Brew::Fast);
    fs::create_dir_all(&env.layout.root).unwrap();
    fs::set_permissions(&env.layout.root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        Daemon::bind(env.config()),
        Err(DaemonError::Path(_))
    ));
    // Unchanged: we never silently repair permissions.
    assert_eq!(mode_of(&env.layout.root), 0o755);
    assert!(matches!(
        Client::connect(&env.layout, "t"),
        Err(ClientError::InsecureDataDir(_))
    ));

    let env2 = Env::new(Brew::Fast);
    let real = env2.tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&real, &env2.layout.root).unwrap();
    assert!(matches!(
        Daemon::bind(env2.config()),
        Err(DaemonError::Path(_))
    ));
}

#[test]
fn client_reports_not_running_without_creating_anything() {
    let env = Env::new(Brew::Fast);
    let err = Client::connect(&env.layout, "t").err().unwrap();
    assert!(err.is_not_running(), "{err:?}");
    assert!(
        !env.layout.root.exists(),
        "clients must not create the data dir"
    );
}

#[test]
fn stale_socket_is_replaced_but_live_daemon_is_never_displaced() {
    let env = Env::new(Brew::Fast);
    env.layout.ensure_private_dirs().unwrap();
    // A socket file left behind by a dead process (bound, then dropped without unlink).
    drop(UnixListener::bind(&env.layout.socket).unwrap());
    assert!(env.layout.socket.exists());
    assert!(Client::connect(&env.layout, "t")
        .err()
        .unwrap()
        .is_not_running());

    let first = env.start();
    env.client().ping().unwrap();

    // Duplicate startup: refused by the instance lock; first daemon's socket untouched.
    match Daemon::bind(env.config()) {
        Err(DaemonError::AlreadyRunning { .. }) => {}
        Err(e) => panic!("expected AlreadyRunning, got {e:?}"),
        Ok(_) => panic!("expected AlreadyRunning, got a second daemon"),
    }
    env.client().ping().unwrap();

    first.stop();
    assert!(
        !env.layout.socket.exists(),
        "clean shutdown removes its own socket"
    );
}

#[test]
fn non_socket_at_socket_path_is_refused_and_left_alone() {
    let env = Env::new(Brew::Fast);
    env.layout.ensure_private_dirs().unwrap();
    fs::write(&env.layout.socket, b"precious").unwrap();
    assert!(matches!(
        Daemon::bind(env.config()),
        Err(DaemonError::UnsafePath { .. })
    ));
    assert_eq!(fs::read(&env.layout.socket).unwrap(), b"precious");

    let env2 = Env::new(Brew::Fast);
    env2.layout.ensure_private_dirs().unwrap();
    let target = env2.tmp.path().join("target");
    fs::write(&target, b"x").unwrap();
    std::os::unix::fs::symlink(&target, &env2.layout.socket).unwrap();
    assert!(matches!(
        Daemon::bind(env2.config()),
        Err(DaemonError::UnsafePath { .. })
    ));
    assert!(fs::symlink_metadata(&env2.layout.socket)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn shutdown_does_not_unlink_a_socket_it_does_not_own() {
    let env = Env::new(Brew::Fast);
    let d = env.start();
    // Something replaces our socket path after startup (simulated).
    fs::remove_file(&env.layout.socket).unwrap();
    drop(UnixListener::bind(&env.layout.socket).unwrap());
    d.stop();
    assert!(
        env.layout.socket.exists(),
        "a socket inode created by someone else must not be removed"
    );
}

#[test]
fn source_has_no_network_listener_or_command_endpoint() {
    // Static guard complementing the lsof test: the daemon and protocol crates contain no
    // TCP/UDP sockets and no shell execution. (Subprocesses only run via core's brew runner.)
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for krate in ["dotfilesync-daemon/src", "dotfilesync-protocol/src"] {
        for entry in walk(&root.join(krate)) {
            let src = fs::read_to_string(&entry).unwrap();
            for banned in [
                "TcpListener",
                "TcpStream",
                "UdpSocket",
                "std::net::",
                "Command::new(\"sh\"",
                "\"/bin/sh\"",
            ] {
                assert!(
                    !src.contains(banned),
                    "{} contains {banned}",
                    entry.display()
                );
            }
        }
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}
