//! A live session: one child process on a PTY, watched.
//!
//! This is where the previous layers meet. A session owns the PTY, appends
//! everything the child writes to its [`OutputLog`], feeds the same bytes to a
//! [`HeadlessScreen`], evaluates the screen against the agent's manifest, and
//! folds the result through a [`StatusReducer`]. The current status and the
//! output log are what everything else in the product reads.
//!
//! The pump runs on its own thread rather than the async runtime, because the
//! PTY read is a blocking syscall — the same reasoning that moved the test
//! servers off the cooperative pool earlier tonight.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use diri_proto::{NeedsInputDetail, SessionStatus};

use crate::detect::ManifestEngine;
use crate::log::OutputLog;
use crate::pty::{Exit, Pty, PtySpec};
use crate::screen::HeadlessScreen;
use crate::status::{Authority, ClaudeHook, ReducerOutcome, StatusReducer, StatusSignal};

/// How often the pump ticks when the child is quiet, so debounce timers still
/// advance and staleness is noticed.
const TICK_INTERVAL: Duration = Duration::from_millis(100);

/// What a session looks like from the outside.
#[derive(Clone, Debug)]
pub struct SessionView {
    pub id: String,
    pub status: SessionStatus,
    pub needs_input: Option<NeedsInputDetail>,
    pub title: Option<String>,
    pub tail_offset: u64,
    pub exited: bool,
}

/// The state the pump thread and the outside world share.
struct Shared {
    id: String,
    status: Mutex<SessionStatus>,
    needs_input: Mutex<Option<NeedsInputDetail>>,
    title: Mutex<Option<String>>,
    log: Mutex<OutputLog>,
    screen: Mutex<HeadlessScreen>,
    reducer: Mutex<StatusReducer>,
    exited: AtomicBool,
    stop: AtomicBool,
}

pub struct Session {
    shared: Arc<Shared>,
    pty: Arc<Mutex<Pty>>,
    pump: Option<JoinHandle<()>>,
    manifest_id: String,
}

/// How to start a session.
pub struct SessionSpec {
    pub id: String,
    pub pty: PtySpec,
    /// Which manifest drives detection ("claude-code", "codex", …).
    pub manifest_id: String,
    pub authority: Authority,
    pub logs_dir: PathBuf,
}

impl Session {
    /// Spawns the child and starts watching it.
    pub fn spawn(spec: SessionSpec, engine: Arc<ManifestEngine>) -> std::io::Result<Self> {
        let pty = Pty::spawn(&spec.pty)?;
        let log = OutputLog::writer(&spec.logs_dir, &spec.id)?;
        let screen = HeadlessScreen::new(spec.pty.cols as usize, spec.pty.rows as usize);
        let reducer = StatusReducer::new(spec.authority, SystemTime::now());

        let shared = Arc::new(Shared {
            id: spec.id.clone(),
            status: Mutex::new(SessionStatus::Starting),
            needs_input: Mutex::new(None),
            title: Mutex::new(None),
            log: Mutex::new(log),
            screen: Mutex::new(screen),
            reducer: Mutex::new(reducer),
            exited: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });

        let reader = pty.reader()?;
        let pty = Arc::new(Mutex::new(pty));

        let pump = {
            let shared = Arc::clone(&shared);
            let engine = Arc::clone(&engine);
            let pty = Arc::clone(&pty);
            let manifest_id = spec.manifest_id.clone();
            std::thread::Builder::new()
                .name(format!("diri-session-{}", spec.id))
                .spawn(move || pump(shared, engine, pty, reader, manifest_id))?
        };

        Ok(Self {
            shared,
            pty,
            pump: Some(pump),
            manifest_id: spec.manifest_id,
        })
    }

    pub fn id(&self) -> &str {
        &self.shared.id
    }

    pub fn manifest_id(&self) -> &str {
        &self.manifest_id
    }

    pub fn view(&self) -> SessionView {
        SessionView {
            id: self.shared.id.clone(),
            status: self.shared.status.lock().expect("status").clone(),
            needs_input: self.shared.needs_input.lock().expect("needs input").clone(),
            title: self.shared.title.lock().expect("title").clone(),
            tail_offset: self.shared.log.lock().expect("log").tail_offset(),
            exited: self.shared.exited.load(Ordering::SeqCst),
        }
    }

    pub fn status(&self) -> SessionStatus {
        self.shared.status.lock().expect("status").clone()
    }

    /// Reads recorded output by absolute stream offset, for attach and replay.
    pub fn read_output(&self, from_offset: u64, max_bytes: usize) -> (u64, Vec<u8>) {
        self.shared
            .log
            .lock()
            .expect("log")
            .read(from_offset, max_bytes)
    }

    /// The visible screen, as detection sees it.
    pub fn screen_lines(&self) -> Vec<String> {
        self.shared.screen.lock().expect("screen").lines()
    }

    /// Sends bytes to the child, as if typed.
    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let mut writer = self.pty.lock().expect("pty").writer()?;
        writer.write_all(bytes)?;
        writer.flush()?;
        self.feed_signal(StatusSignal::UserKeystroke);
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.pty.lock().expect("pty").resize(cols, rows)?;
        self.shared
            .screen
            .lock()
            .expect("screen")
            .resize(cols as usize, rows as usize);
        Ok(())
    }

    /// Feeds an out-of-band signal — a hook callback, a notify — into the
    /// reducer.
    pub fn feed_signal(&self, signal: StatusSignal) -> ReducerOutcome {
        let outcome = self
            .shared
            .reducer
            .lock()
            .expect("reducer")
            .reduce(signal, SystemTime::now());
        apply(&self.shared, &outcome);
        outcome
    }

    pub fn claude_hook(&self, hook: ClaudeHook, is_subagent: bool) -> ReducerOutcome {
        self.feed_signal(StatusSignal::ClaudeHook { hook, is_subagent })
    }

    /// Ends the session, killing the child's whole process group.
    pub fn terminate(&mut self, grace: Duration) -> std::io::Result<Exit> {
        let exit = self.pty.lock().expect("pty").terminate(grace)?;
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        Ok(exit)
    }
}

impl Drop for Session {
    /// Dropping a session ends it.
    ///
    /// The child has to go, not merely be forgotten: the pump thread cannot be
    /// reclaimed while the terminal has a writer, and a forgotten child would
    /// keep running with nothing watching or reaping it. Surviving the owner is
    /// the holder's job — a separate process, not yet ported — and not
    /// something this type can honestly offer.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if !self.shared.exited.load(Ordering::SeqCst)
            && let Ok(pty) = self.pty.lock()
        {
            let _ = pty.kill_group(libc::SIGKILL);
        }
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

/// Applies a reducer outcome to the shared state.
fn apply(shared: &Shared, outcome: &ReducerOutcome) {
    if let Some(status) = &outcome.status_change {
        *shared.status.lock().expect("status") = status.clone();
        if matches!(status, SessionStatus::Exited(_)) {
            shared.exited.store(true, Ordering::SeqCst);
        }
    }
    if let Some(detail) = &outcome.needs_input {
        *shared.needs_input.lock().expect("needs input") = Some(detail.clone());
    }
    // Leaving a needs-input state clears the pending detail, so the UI does not
    // keep showing a prompt that has been answered.
    if matches!(
        outcome.status_change,
        Some(SessionStatus::Working) | Some(SessionStatus::Idle)
    ) {
        *shared.needs_input.lock().expect("needs input") = None;
    }
}

/// The read/evaluate/reduce loop.
///
/// Waits on the terminal with a timeout rather than blocking in `read`. Two
/// reasons, both of which a blocking read got wrong: the debounce timers must
/// keep advancing while the child is *quiet* — that is exactly when staleness
/// and idle confirmation matter — and a blocking read cannot be interrupted, so
/// stopping a session would hang until the child happened to say something.
fn pump(
    shared: Arc<Shared>,
    engine: Arc<ManifestEngine>,
    pty: Arc<Mutex<Pty>>,
    mut reader: crate::pty::PtyStream,
    manifest_id: String,
) {
    let mut buffer = [0u8; 8192];
    let mut last_tick = SystemTime::now();
    let fd = reader.as_raw_fd();

    loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }

        // Wait for output, but never longer than a tick.
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, a millisecond timeout.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, TICK_INTERVAL.as_millis() as i32) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        let hung_up = poll_fd.revents & (libc::POLLHUP | libc::POLLERR) != 0;
        let readable = poll_fd.revents & libc::POLLIN != 0;

        let read_result = if readable || hung_up {
            reader.read(&mut buffer)
        } else {
            Ok(usize::MAX) // nothing to read; fall through to the tick
        };

        match read_result {
            Ok(usize::MAX) => {}
            Ok(0) => break, // the child closed the terminal
            Ok(n) => {
                let chunk = &buffer[..n];
                {
                    let mut log = shared.log.lock().expect("log");
                    // A failed disk write must not stop the session: the child
                    // is still running and its status still matters.
                    let _ = log.append(chunk);
                }
                let observation = {
                    let mut screen = shared.screen.lock().expect("screen");
                    screen.feed(chunk);
                    *shared.title.lock().expect("title") = screen.title().map(str::to_string);
                    engine.evaluate(&screen.snapshot(), &manifest_id)
                };

                let now = SystemTime::now();
                let mut reducer = shared.reducer.lock().expect("reducer");
                let outcome = reducer.reduce(StatusSignal::PtyOutputActivity, now);
                apply(&shared, &outcome);
                if let Some(observation) = observation {
                    let outcome = reducer.reduce(StatusSignal::Screen(observation), now);
                    drop(reducer);
                    apply(&shared, &outcome);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }

        // Ticks drive the debounce timers even when the child is quiet.
        if last_tick.elapsed().unwrap_or_default() >= TICK_INTERVAL {
            last_tick = SystemTime::now();
            let outcome = shared
                .reducer
                .lock()
                .expect("reducer")
                .reduce(StatusSignal::Tick, last_tick);
            apply(&shared, &outcome);
        }
    }

    // The stream ended: reap the child and record how it died.
    let exit = pty.lock().expect("pty").wait().ok();
    let (code, signal) = match exit {
        Some(Exit::Code(code)) => (Some(code), None),
        Some(Exit::Signal(signal)) => (None, Some(signal)),
        None => (None, None),
    };
    let outcome = shared.reducer.lock().expect("reducer").reduce(
        StatusSignal::ProcessExit { code, signal },
        SystemTime::now(),
    );
    apply(&shared, &outcome);
    shared.exited.store(true, Ordering::SeqCst);
    let _ = shared.log.lock().expect("log").flush();
}

/// Convenience for tests and callers that just want the shipped rules.
pub fn load_engine(manifests: &Path) -> std::io::Result<(Arc<ManifestEngine>, Vec<String>)> {
    let (engine, failed) = ManifestEngine::load_dir(manifests)?;
    Ok((Arc::new(engine), failed))
}

/// Maps a manifest's status model onto the reducer authority the daemon uses.
pub fn authority_for(manifest_id: &str, engine: &ManifestEngine) -> Authority {
    match engine.manifest(manifest_id).map(|m| m.status_model) {
        Some(crate::detect::StatusModel::ProcessOnly) | None => Authority::ProcessOnly,
        // Claude drives from hooks and arbitrates with the screen; everything
        // else with a full status model is screen-led.
        Some(crate::detect::StatusModel::Full) if manifest_id == "claude-code" => {
            Authority::HooksPrimary
        }
        Some(crate::detect::StatusModel::Full) => Authority::ScreenPrimary,
    }
}
