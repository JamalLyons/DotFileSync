//! Kernel-provided peer credentials for Unix-domain sockets.
//!
//! The UID always comes from the OS (`getpeereid(2)` on macOS/BSD, `SO_PEERCRED` on Linux),
//! never from message contents.

use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

pub fn current_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Effective UID of the process on the other end of `stream`.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
pub fn peer_euid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: valid fd and out-pointers to stack locals.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn peer_euid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid fd, correctly sized out-buffer.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socketpair_peer_is_current_user() {
        let (a, b) = UnixStream::pair().unwrap();
        assert_eq!(peer_euid(&a).unwrap(), current_euid());
        assert_eq!(peer_euid(&b).unwrap(), current_euid());
    }
}
