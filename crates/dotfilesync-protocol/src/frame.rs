//! Length-prefixed frames: a 4-byte big-endian length followed by a UTF-8 JSON body.

use std::io::{self, Read, Write};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrameError {
    /// The peer closed the connection cleanly before a new frame started.
    #[error("connection closed")]
    Closed,
    /// The announced length exceeds the receiver's limit. Nothing was allocated or read.
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    TooLarge { len: usize, max: usize },
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
}

impl FrameError {
    pub fn is_timeout(&self) -> bool {
        matches!(self, FrameError::Io(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
    }
}

pub fn write_frame<W: Write>(w: &mut W, body: &[u8], max: usize) -> Result<(), FrameError> {
    if body.len() > max {
        return Err(FrameError::TooLarge {
            len: body.len(),
            max,
        });
    }
    let len = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge {
        len: body.len(),
        max,
    })?;
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(body);
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

/// Read one frame, rejecting anything larger than `max` before allocating its body.
pub fn read_frame<R: Read>(r: &mut R, max: usize) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < header.len() {
        match r.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Err(FrameError::Closed),
            Ok(0) => {
                return Err(FrameError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed inside frame header",
                )))
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let len = u32::from_be_bytes(header) as usize;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{\"a\":1}", 1024).unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur, 1024).unwrap(), b"{\"a\":1}");
        assert!(matches!(
            read_frame(&mut cur, 1024),
            Err(FrameError::Closed)
        ));
    }

    #[test]
    fn oversized_header_rejected_without_reading_body() {
        let mut data = (u32::MAX).to_be_bytes().to_vec();
        data.extend_from_slice(b"xx");
        let mut cur = Cursor::new(data);
        match read_frame(&mut cur, 64) {
            Err(FrameError::TooLarge { len, max }) => {
                assert_eq!(len, u32::MAX as usize);
                assert_eq!(max, 64);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        // Only the 4-byte header was consumed.
        assert_eq!(cur.position(), 4);
    }

    #[test]
    fn truncated_frames_are_errors_not_hangs() {
        let mut cur = Cursor::new(vec![0u8, 0]);
        assert!(matches!(read_frame(&mut cur, 64), Err(FrameError::Io(_))));
        let mut data = 10u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"abc");
        let mut cur = Cursor::new(data);
        assert!(matches!(read_frame(&mut cur, 64), Err(FrameError::Io(_))));
    }

    #[test]
    fn writer_refuses_oversized_body() {
        let mut buf = Vec::new();
        assert!(matches!(
            write_frame(&mut buf, &[0u8; 65], 64),
            Err(FrameError::TooLarge { .. })
        ));
        assert!(buf.is_empty());
    }
}
