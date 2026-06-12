//! Placeholder PTY for platforms without a Unix pseudo-terminal.
//!
//! Windows is the real target here, and it is a genuine implementation task
//! rather than a shim: ConPTY replaces the fd model entirely.
//!
//! What a Windows implementation needs to do:
//!
//! - `CreatePseudoConsole` with a `COORD` size and two anonymous pipes, giving
//!   back an `HPCON` plus read/write ends that stand in for the master fd.
//! - Spawn through `CreateProcessW` with an
//!   `EXTENDED_STARTUPINFO_PRESENT` attribute list carrying
//!   `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`. There is no fork, so the
//!   pre-exec work in the Unix path — signal reset, `setsid`, `TIOCSCTTY`,
//!   closing inherited descriptors — has no analogue and simply drops away.
//! - `ResizePseudoConsole` for `resize`. There is no `TIOCGWINSZ`, so `size`
//!   has to track the last requested size rather than ask the kernel.
//! - Job objects for `kill_group`: assign the child to a job with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so descendants die with it, since
//!   Windows has no process groups in the POSIX sense.
//!
//! Until then these calls fail at runtime rather than at compile time, so the
//! rest of the engine still builds and tests on any platform.

use std::io;
use std::time::Duration;

use super::{Exit, PtySpec};

pub struct Pty {
    _private: (),
}

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "PTY support for this platform is not implemented yet; see pty::unsupported",
    ))
}

impl Pty {
    pub fn spawn(_spec: &PtySpec) -> io::Result<Self> {
        unsupported()
    }
    pub fn pid(&self) -> u32 {
        0
    }
    pub fn resize(&self, _cols: u16, _rows: u16) -> io::Result<()> {
        unsupported()
    }
    pub fn size(&self) -> io::Result<(u16, u16)> {
        unsupported()
    }
    pub fn wait(&mut self) -> io::Result<Exit> {
        unsupported()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        unsupported()
    }
    pub fn kill_group(&self, _signal: i32) -> io::Result<()> {
        unsupported()
    }
    pub fn terminate(&mut self, _grace: Duration) -> io::Result<Exit> {
        unsupported()
    }
}
