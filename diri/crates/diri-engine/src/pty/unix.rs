//! Unix PTY implementation.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use super::{Exit, PtySpec};

/// One past the highest signal to reset in the child.
///
/// The libc crate does not export `NSIG` uniformly, so it is spelled out per
/// platform. Darwin stops at 32; Linux reserves 33..=64 for realtime signals,
/// which nothing here uses but which cost nothing to clear.
#[cfg(target_os = "macos")]
const MAX_SIGNAL: libc::c_int = 32;
#[cfg(not(target_os = "macos"))]
const MAX_SIGNAL: libc::c_int = 65;

pub struct Pty {
    master: OwnedFd,
    child: Child,
}

impl Pty {
    /// Spawns `spec` on a new pseudo-terminal.
    ///
    /// Four details in the child are load-bearing, all inherited from the
    /// Swift shim this replaces:
    ///
    /// 1. **Signal state is reset.** A daemon built on an async runtime blocks
    ///    or ignores signals, and a child inherits both the mask and any
    ///    `SIG_IGN` dispositions across `exec`. Leave `SIGWINCH` ignored and
    ///    the agent never learns the terminal resized, so it never repaints.
    /// 2. **`setsid` then `TIOCSCTTY`.** The child must lead its own session
    ///    and own the slave as its controlling terminal, or job control and
    ///    signal delivery to the foreground group do not work.
    /// 3. **Descriptors above stderr are closed.** Otherwise the child holds
    ///    the daemon's control socket open, and closing that socket no longer
    ///    signals a disconnect to the far end.
    /// 4. **The environment is replaced, not extended** — handled by the
    ///    caller through `PtySpec::env`.
    pub fn spawn(spec: &PtySpec) -> io::Result<Self> {
        let program = spec
            .argv
            .first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "argv is empty"))?;

        let mut winsize = libc::winsize {
            ws_row: spec.rows,
            ws_col: spec.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;
        // SAFETY: both out-params are valid locals; winsize is fully initialized.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut winsize,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty succeeded, so both are fresh owned descriptors.
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };

        let mut command = Command::new(OsStr::new(program));
        command.args(&spec.argv[1..]);
        command.env_clear();
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command.current_dir(&spec.cwd);

        // The runtime dup2s these onto 0/1/2 in the child.
        command.stdin(Stdio::from(slave.try_clone()?));
        command.stdout(Stdio::from(slave.try_clone()?));
        command.stderr(Stdio::from(slave.try_clone()?));

        let slave_fd = slave.as_raw_fd();
        // SAFETY: the closure runs between fork and exec and uses only
        // async-signal-safe syscalls.
        unsafe {
            command.pre_exec(move || {
                let mut empty: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut empty);
                libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
                for signal in 1..MAX_SIGNAL {
                    libc::signal(signal, libc::SIG_DFL);
                }

                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }

                // Close everything the daemon left open above stderr. The
                // runtime has already placed the slave on 0/1/2.
                let max = libc::getdtablesize();
                for fd in 3..max {
                    libc::close(fd);
                }
                Ok(())
            });
        }

        let child = command.spawn()?;
        drop(slave); // the parent must not hold the slave open
        Ok(Self { master, child })
    }

    /// The child's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the fd is owned and winsize is initialized.
        let rc = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &winsize) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The kernel's current window size, which is the authority — not whatever
    /// the caller last asked for.
    pub fn size(&self) -> io::Result<(u16, u16)> {
        // SAFETY: the fd is owned; the kernel fills the struct.
        let mut winsize: libc::winsize = unsafe { std::mem::zeroed() };
        let rc =
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCGWINSZ as _, &mut winsize) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((winsize.ws_col, winsize.ws_row))
    }

    /// A reader over the master side. Reads return `EIO` on Linux once the
    /// child exits, which callers should treat as end of stream.
    pub fn reader(&self) -> io::Result<PtyStream> {
        Ok(PtyStream(File::from(self.master.try_clone()?)))
    }

    pub fn writer(&self) -> io::Result<PtyStream> {
        Ok(PtyStream(File::from(self.master.try_clone()?)))
    }

    /// Blocks until the child exits.
    pub fn wait(&mut self) -> io::Result<Exit> {
        let status = self.child.wait()?;
        Ok(exit_from(status))
    }

    /// Reaps the child if it has already exited, without blocking.
    pub fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        Ok(self.child.try_wait()?.map(exit_from))
    }

    /// Signals the child's whole process group, so backgrounded grandchildren
    /// die with it rather than being reparented to init.
    pub fn kill_group(&self, signal: i32) -> io::Result<()> {
        let pid = self.child.id() as i32;
        // The child called setsid, so its pid is its process-group id.
        // SAFETY: plain kill(2) on a group we created.
        let rc = unsafe { libc::kill(-pid, signal) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            // ESRCH means it is already gone, which is the desired end state.
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }

    /// SIGTERM the group, and SIGKILL anything still alive after `grace`.
    pub fn terminate(&mut self, grace: std::time::Duration) -> io::Result<Exit> {
        self.kill_group(libc::SIGTERM)?;
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline {
            if let Some(exit) = self.try_wait()? {
                return Ok(exit);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        self.kill_group(libc::SIGKILL)?;
        self.wait()
    }
}

fn exit_from(status: std::process::ExitStatus) -> Exit {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(signal) => Exit::Signal(signal),
        None => Exit::Code(status.code().unwrap_or(-1)),
    }
}

/// Read/write handle on the PTY master.
pub struct PtyStream(File);

impl PtyStream {
    pub fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// Waits until the terminal has output, or `timeout` elapses.
    ///
    /// Everything that reads a PTY should go through this rather than blocking
    /// in `read`: a child that never speaks again would otherwise wedge the
    /// caller forever, with no way to interrupt it.
    pub fn wait_readable(&self, timeout: std::time::Duration) -> io::Result<bool> {
        let mut poll_fd = libc::pollfd {
            fd: self.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd with a millisecond timeout.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout.as_millis() as libc::c_int) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(poll_fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0)
    }

    pub fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl Read for PtyStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer) {
            // Linux reports EIO on the master once the last slave closes;
            // macOS returns 0. Normalize to end of stream.
            Err(error) if error.raw_os_error() == Some(libc::EIO) => Ok(0),
            other => other,
        }
    }
}

impl Write for PtyStream {
    /// Writes the whole buffer, retrying on `EAGAIN`.
    ///
    /// A master fd that anything has switched to non-blocking will accept a
    /// short write and return `EAGAIN` for the rest; dropping those bytes
    /// truncates injected prompts, which is exactly the class of bug that made
    /// pasted input go missing in the Swift daemon.
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            match self.0.write(buffer) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::Interrupted =>
                {
                    std::thread::yield_now();
                    continue;
                }
                other => return other,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
