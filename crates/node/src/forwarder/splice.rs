//! v1.0.8: Linux splice(2) zero-copy bidirectional TCP forwarding.
//!
//! For an UNLIMITED (non-rate-limited) rule, the node forwards bytes with the
//! `splice(2)` syscall instead of a userspace read/write copy. splice moves
//! data *inside the kernel* through a pipe — the bytes are never copied into
//! this process's address space — which removes the two userspace copies (and
//! the CPU + memory-bandwidth cost) that a plain relay pays per byte. This is
//! the same technique realm and other high-performance relays use.
//!
//! Structure (per direction): a non-blocking pipe is the kernel intermediary.
//! Step 1 splices `socket → pipe` (pull up to a pipe-full from the source);
//! step 2 splices `pipe → socket` (push them to the destination, draining
//! fully). Readiness is driven by tokio (`readable()`/`writable()` + `try_io`),
//! so the task parks instead of busy-looping on EAGAIN. Because the pipe is
//! fully drained between reads, an EAGAIN in step 1 always means "source socket
//! not readable" and in step 2 "destination socket not writable" — never the
//! pipe — so the readiness we wait on is always the right one.
//!
//! Rate limiting is NOT possible here (the bytes never reach userspace to be
//! throttled) — the caller only takes this path for unlimited rules. Byte
//! counts ARE available (splice returns the count moved), so the two totals are
//! returned for traffic accounting / billing, exactly like the userspace path.
//!
//! This whole module is Linux-only; the caller falls back to the userspace copy
//! on other targets.

use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use tokio::io::Interest;
use tokio::net::TcpStream;

/// Pipe capacity we try to set (64 KiB = 16 × 4 KiB pages), matching realm's
/// default. Best-effort: if `F_SETPIPE_SZ` fails we keep the kernel default.
const PIPE_SIZE: libc::c_int = 16 * 4096;

/// A non-blocking pipe pair that closes both ends on drop.
struct Pipe {
    r: RawFd,
    w: RawFd,
}

impl Pipe {
    fn new() -> io::Result<Pipe> {
        let mut fds = [0 as libc::c_int; 2];
        // pipe2 with O_NONBLOCK so both ends never block a runtime worker.
        let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // Best-effort enlarge — a bigger pipe means fewer splice syscalls under
        // load. Ignore failure (capped by /proc/sys/fs/pipe-max-size, or the
        // caller may lack permission); the kernel default still works.
        unsafe { libc::fcntl(fds[1], libc::F_SETPIPE_SZ, PIPE_SIZE) };
        Ok(Pipe {
            r: fds[0],
            w: fds[1],
        })
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.r);
            libc::close(self.w);
        }
    }
}

/// One raw `splice` call. Returns Ok(0) on EOF, Ok(n) on success, or an
/// `io::Error` (WouldBlock is surfaced so the caller can wait for readiness).
fn splice_raw(from: RawFd, to: RawFd, len: usize) -> io::Result<usize> {
    let n = unsafe {
        libc::splice(
            from,
            std::ptr::null_mut(),
            to,
            std::ptr::null_mut(),
            len,
            (libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK) as libc::c_uint,
        )
    };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Shut down the write half of a socket (SHUT_WR), signalling EOF to the peer.
/// Best-effort; errors (e.g. the peer already closed) are ignored.
fn shutdown_write(fd: RawFd) {
    unsafe {
        libc::shutdown(fd, libc::SHUT_WR);
    }
}

/// Pump one direction, `src → dst`, with splice via a private pipe. Returns the
/// total bytes moved. On return (EOF or error) the destination's write half is
/// shut down so the peer sees EOF and the opposite pump can finish too.
///
/// v1.2.8: `on_bytes` fires on every successful splice INTO the destination,
/// so bytes are reportable as they move. The return value used to be the only
/// output, which meant a caller could not bill a connection until it ended — a
/// long-lived tunnel moved unlimited traffic while its owner's quota read zero.
/// With the callback the unreported amount is never more than one in-flight
/// chunk.
async fn pump(src: &TcpStream, dst: &TcpStream, on_bytes: impl Fn(u64)) -> io::Result<u64> {
    let pipe = Pipe::new()?;
    let src_fd = src.as_raw_fd();
    let dst_fd = dst.as_raw_fd();
    let mut total: u64 = 0;

    let result = async {
        loop {
            // Step 1: socket → pipe. The pipe is empty here (fully drained
            // below), so EAGAIN can only mean the source is not readable.
            let n = loop {
                src.readable().await?;
                match src.try_io(Interest::READABLE, || {
                    splice_raw(src_fd, pipe.w, PIPE_SIZE as usize)
                }) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e),
                }
            };
            if n == 0 {
                break; // source EOF — clean end of stream
            }

            // Step 2: pipe → socket, drained fully. The pipe is non-empty, so
            // EAGAIN can only mean the destination is not writable.
            let mut left = n;
            while left > 0 {
                dst.writable().await?;
                match dst.try_io(Interest::WRITABLE, || splice_raw(pipe.r, dst_fd, left)) {
                    Ok(0) => return Ok(()), // destination closed for writing
                    Ok(m) => {
                        left -= m;
                        total += m as u64;
                        on_bytes(m as u64);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }
    .await;

    // Whether we ended on EOF or an error, signal EOF to the destination's peer
    // so the other direction's pump unblocks and the connection tears down.
    shutdown_write(dst_fd);
    result.map(|()| total)
}

/// Forward bytes both ways between `a` and `b` with splice zero-copy until both
/// directions reach EOF. Returns `(a→b bytes, b→a bytes)`.
///
/// The streams are wrapped in `Arc` so both pump tasks can drive readiness on
/// them concurrently (`readable`/`writable`/`try_io` take `&self`); the raw fds
/// stay valid for the whole operation because the `Arc`s outlive both pumps.
pub async fn zero_copy_bidirectional(
    a: TcpStream,
    b: TcpStream,
    on_up: impl Fn(u64),
    on_down: impl Fn(u64),
) -> io::Result<(u64, u64)> {
    let a = Arc::new(a);
    let b = Arc::new(b);
    let (a_up, b_up) = (a.clone(), b.clone());
    let ab = pump(&a_up, &b_up, on_up); // a → b
    let ba = pump(&b, &a, on_down); // b → a
    let (r_ab, r_ba) = tokio::join!(ab, ba);
    Ok((r_ab?, r_ba?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// End-to-end: client → [splice relay] → echo target. The payload must
    /// round-trip and the returned byte counts must be exact.
    #[tokio::test]
    async fn splice_roundtrips_and_counts_bytes() {
        // Echo target: read once, echo back, then close.
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = target.accept().await.unwrap();
            let mut b = vec![0u8; 1024];
            let n = s.read(&mut b).await.unwrap();
            s.write_all(&b[..n]).await.unwrap();
            s.shutdown().await.unwrap();
        });

        // Relay: accept a client, connect to target, splice both ways.
        let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_addr = relay.local_addr().unwrap();
        let relay_task = tokio::spawn(async move {
            let (client, _) = relay.accept().await.unwrap();
            let upstream = TcpStream::connect(target_addr).await.unwrap();
            zero_copy_bidirectional(client, upstream, |_| {}, |_| {})
                .await
                .unwrap()
        });

        // Client: send, receive the echo, close.
        let mut client = TcpStream::connect(relay_addr).await.unwrap();
        let msg = b"hello-splice-zero-copy";
        client.write_all(msg).await.unwrap();
        client.shutdown().await.unwrap(); // half-close: signals EOF upstream
        let mut got = Vec::new();
        client.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, msg, "payload must round-trip through the splice relay");

        let (up, down) = relay_task.await.unwrap();
        assert_eq!(up, msg.len() as u64, "client→target byte count");
        assert_eq!(down, msg.len() as u64, "target→client byte count");
    }

    /// A larger transfer (bigger than one pipe-full) must move all bytes.
    #[tokio::test]
    async fn splice_moves_large_payload() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        // Sink target: drain everything the relay sends.
        let recv = tokio::spawn(async move {
            let (mut s, _) = target.accept().await.unwrap();
            let mut total = 0u64;
            let mut b = vec![0u8; 64 * 1024];
            loop {
                match s.read(&mut b).await.unwrap() {
                    0 => break,
                    n => total += n as u64,
                }
            }
            total
        });

        let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_addr = relay.local_addr().unwrap();
        // v1.2.8: record every callback so we can prove reporting is
        // INCREMENTAL. A payload this size spans several pipe-fulls, so a
        // correct implementation reports more than once; the old
        // count-once-at-the-end shape would show exactly one call.
        let ticks = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let ticks_up = ticks.clone();
        let relay_task = tokio::spawn(async move {
            let (client, _) = relay.accept().await.unwrap();
            let upstream = TcpStream::connect(target_addr).await.unwrap();
            zero_copy_bidirectional(
                client,
                upstream,
                move |n| ticks_up.lock().unwrap().push(n),
                |_| {},
            )
            .await
            .unwrap()
        });

        let mut client = TcpStream::connect(relay_addr).await.unwrap();
        let payload = vec![0x5au8; 1_000_000]; // ~1 MiB, several pipe-fulls
        client.write_all(&payload).await.unwrap();
        client.shutdown().await.unwrap();
        // Drain any (empty) reverse traffic so the relay's b→a pump ends.
        let mut sink = Vec::new();
        client.read_to_end(&mut sink).await.unwrap();

        let (up, _down) = relay_task.await.unwrap();
        let received = recv.await.unwrap();
        assert_eq!(
            received,
            payload.len() as u64,
            "target must receive all bytes"
        );
        let reported = ticks.lock().unwrap().clone();
        assert!(
            reported.len() > 1,
            "bytes must be reported as they move, not once at the end (got {} call(s))",
            reported.len()
        );
        assert_eq!(
            reported.iter().sum::<u64>(),
            payload.len() as u64,
            "the reported chunks must add up to exactly what was forwarded"
        );
        assert_eq!(up, payload.len() as u64, "up count must equal payload size");
    }
}
