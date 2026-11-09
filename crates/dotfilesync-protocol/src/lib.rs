//! Local IPC protocol shared by `dotfilesyncd` and its clients (CLI today, GPUI later).
//!
//! The protocol runs only over a Unix-domain socket in a private per-user directory.
//! See `docs/daemon-architecture.md` for the full process, security and state model.

#[cfg(not(unix))]
compile_error!("dotfilesync-protocol requires a Unix platform (Unix-domain sockets).");

pub mod client;
pub mod frame;
pub mod messages;
pub mod paths;
pub mod peer;

pub use client::{Client, ClientError};
pub use frame::{read_frame, write_frame, FrameError};
pub use messages::*;
pub use paths::{DataLayout, PathError};
pub use peer::{current_euid, peer_euid};

/// Current protocol version. Bump on any incompatible wire change.
pub const PROTOCOL_VERSION: u32 = 1;
/// Versions this build can speak.
pub const SUPPORTED_VERSIONS: &[u32] = &[PROTOCOL_VERSION];

/// Maximum size of a frame sent by a client to the daemon.
pub const MAX_REQUEST_FRAME: usize = 64 * 1024;
/// Maximum size of a frame sent by the daemon to a client (reports can be large).
pub const MAX_RESPONSE_FRAME: usize = 16 * 1024 * 1024;
