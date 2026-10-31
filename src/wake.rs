//! Main-loop wakeups for the command channel.
//!
//! The UI, D-Bus, tray and watcher threads all send commands through
//! `CommandSender`. A byte written to a pipe turns the GTK main loop from a
//! 25 Hz poll into an event-driven loop; the pipe is drained in one callback.

use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicI32, Ordering};

static WAKE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// Create the wakeup pipe. Returns the read end for `unix_fd_add_local`.
pub fn init() -> Option<RawFd> {
    let mut fds = [0; 2];
    // Non-blocking writes keep a stalled main loop from blocking a sender.
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if result != 0 {
        return None;
    }
    WAKE_WRITE_FD.store(fds[1], Ordering::SeqCst);
    Some(fds[0])
}

/// Wake the main loop. Safe to call from any thread; a full pipe only means
/// a wakeup is already pending.
pub fn wake() {
    let fd = WAKE_WRITE_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    let byte = 1u8;
    unsafe {
        libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
    }
}

/// Drain all pending wakeup bytes. The pipe is non-blocking, so this never
/// stalls the GTK thread.
pub fn drain(fd: RawFd) {
    let mut buffer = [0u8; 64];
    loop {
        let read =
            unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, buffer.len()) };
        if read <= 0 {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakeups_are_coalesced_and_drained() {
        let fd = init().expect("wakeup pipe");
        wake();
        wake();
        drain(fd);
        let mut byte = 0u8;
        let left = unsafe { libc::read(fd, &mut byte as *mut u8 as *mut libc::c_void, 1) };
        assert!(left <= 0, "pipe should be empty after drain");
        unsafe { libc::close(fd) };
    }
}
