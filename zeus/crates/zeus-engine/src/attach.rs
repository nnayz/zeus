//! The per-session binary data channel: the app's terminal rendering path.
//!
//! A client connects to the daemon socket and sends one JSON
//! [`AttachRequest`] line instead of a control handshake; from then on the
//! connection carries binary [`Frame`]s both ways. The server side owns the
//! authoritative emulator: it seeds a fresh sink with a full grid snapshot
//! plus current modes (no byte replay, no reattach-mangle — the mosh model),
//! then streams paced grid diffs while output flows. The client sends input,
//! resize, scroll, and ping frames back on the same socket.
//!
//! One pump thread per session broadcasts to every sink attached to it, so
//! the grid walk and diff are done once regardless of sink count — the same
//! shape as the Swift daemon's coalesced `flushGrid`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeus_proto::frames::{Frame, FrameCodec, FrameType};
use zeus_proto::terminal::{
    AttachmentAction, AttachmentControlState, ControlEpoch, ControlState, ControlledFrame,
    Controller,
};
use zeus_proto::{AttachRequest, ClientRole, ControlError};

use crate::registry::Registry;
use crate::session::{AttachmentSeed, GridSignature};

/// Background-output ceiling for grid emission, matching the client pacer and
/// the Swift daemon's flush interval. The first frame after quiet and the
/// bounded response frames after interactive input go immediately.
const GRID_FLUSH_INTERVAL: Duration = Duration::from_millis(16);

/// One attached client's write half.
struct Sink {
    id: u64,
    writer: Arc<Mutex<UnixStream>>,
    controlled: bool,
}

struct Peer {
    client_id: String,
    initial_epoch: ControlEpoch,
    controlled: bool,
}

/// All live sinks for one session, plus whether a pump is serving them.
#[derive(Default)]
struct SessionSinks {
    sinks: Vec<Sink>,
    pump_running: bool,
}

/// Routes attach connections to per-session pumps.
#[derive(Clone, Default)]
pub struct AttachHub {
    sessions: Arc<Mutex<HashMap<String, SessionSinks>>>,
    next_sink: Arc<AtomicU64>,
}

impl AttachHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs one attach connection to completion: seeds the sink, registers it
    /// with the session's pump, then loops on incoming frames until the peer
    /// leaves. `reader` may hold bytes buffered past the attach line; they are
    /// fed to the frame codec first.
    pub fn serve(
        &self,
        registry: &Arc<Mutex<Registry>>,
        request: &AttachRequest,
        mut reader: impl Read,
        buffered: Vec<u8>,
        writer: Arc<Mutex<UnixStream>>,
    ) {
        let session_id = &request.attach.0;
        let controlled = request.control_protocol == Some(zeus_proto::terminal::TERMINAL_PROTOCOL);
        if request.role != ClientRole::Desktop || request.control_protocol.is_some() && !controlled
        {
            return;
        }
        let sink_id = self.next_sink.fetch_add(1, Ordering::SeqCst);
        let client_id = format!("desktop-{sink_id}");
        if let Ok(stream) = writer.lock() {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
        }
        // Capture under Registry, then release it before any socket write.
        let (seed, state) = {
            let Ok(mut guard) = registry.lock() else {
                return;
            };
            let _ = guard.wake_session(session_id);
            let Some(session) = guard.get(session_id) else {
                return;
            };
            let mut state = session.terminal_control_state();
            if state
                .owner
                .as_ref()
                .is_none_or(|owner| owner.role == ClientRole::Desktop)
            {
                let Ok(granted) = session.acquire_terminal_control(
                    &state.epoch,
                    desktop_owner(&client_id),
                    state.owner.is_some(),
                ) else {
                    return;
                };
                state = granted;
            }
            (session.attachment_seed(), state)
        };
        let peer = Peer {
            client_id,
            initial_epoch: state.epoch.clone(),
            controlled,
        };
        let seeded = (|| {
            if controlled {
                write_frame(&writer, &control_frame(&peer.client_id, state, None)?)?;
            }
            let grid = Frame::grid(&seed.grid).map_err(std::io::Error::other)?;
            write_frame(&writer, &grid)?;
            write_frame(&writer, &Frame::modes(seed.modes))
        })();
        if seeded.is_err() {
            release_peer(registry, session_id, &peer.client_id);
            return;
        }
        self.register(
            registry,
            session_id,
            Sink {
                id: sink_id,
                writer: Arc::clone(&writer),
                controlled,
            },
            seed,
        );

        // The read loop is this connection's thread. A feed error means a
        // corrupt stream; a false from handle_frame means the peer's write
        // half died — both end the whole serve.
        let mut codec = FrameCodec::new();
        let mut chunk = [0u8; 64 << 10];
        let mut pending = buffered;
        'serve: while let Ok(frames) = codec.feed(&pending) {
            pending.clear();
            for frame in frames {
                if !self.handle_frame(registry, session_id, &peer, &writer, &frame) {
                    break 'serve;
                }
            }
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => pending.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        self.deregister(session_id, sink_id);
        release_peer(registry, session_id, &peer.client_id);
    }

    fn handle_frame(
        &self,
        registry: &Arc<Mutex<Registry>>,
        session_id: &str,
        peer: &Peer,
        writer: &Arc<Mutex<UnixStream>>,
        frame: &Frame,
    ) -> bool {
        if frame.frame_type == FrameType::Ping {
            return write_frame(writer, &Frame::pong()).is_ok();
        }
        let Ok(mut guard) = registry.lock() else {
            return false;
        };
        let action = (|| -> Result<(ControlEpoch, AttachmentAction), ControlError> {
            if peer.controlled {
                if frame.frame_type != FrameType::Controlled || frame.payload.len() > 256 * 1024 {
                    return Err(ControlError::bad_request("epoch envelope required"));
                }
                let framed: ControlledFrame = serde_json::from_slice(&frame.payload)
                    .map_err(|_| ControlError::bad_request("invalid controlled frame"))?;
                return Ok((framed.expected, framed.action));
            }
            let action = match frame.frame_type {
                FrameType::Input => AttachmentAction::Input {
                    bytes: frame.payload.clone(),
                },
                FrameType::Resize => {
                    let (cols, rows) = frame
                        .resize_payload()
                        .ok_or_else(|| ControlError::bad_request("invalid resize"))?;
                    AttachmentAction::Resize { cols, rows }
                }
                FrameType::Scroll => {
                    let (direction, lines, col, row) = frame
                        .scroll_payload()
                        .ok_or_else(|| ControlError::bad_request("invalid scroll"))?;
                    AttachmentAction::Scroll {
                        direction,
                        lines,
                        col,
                        row,
                    }
                }
                _ => return Err(ControlError::bad_request("unsupported attachment frame")),
            };
            Ok((peer.initial_epoch.clone(), action))
        })();
        let result = action.and_then(|(expected, action)| {
            let session = guard
                .get(session_id)
                .ok_or_else(|| ControlError::not_found("session unavailable"))?;
            if matches!(action, AttachmentAction::TakeControl) {
                return session
                    .acquire_terminal_control(&expected, desktop_owner(&peer.client_id), true)
                    .map(|_| ());
            }
            session.validate_terminal_control(&expected, &peer.client_id)?;
            match action {
                AttachmentAction::ReleaseControl => session
                    .release_terminal_control(&expected, &peer.client_id)
                    .map(|_| ()),
                AttachmentAction::Resize { cols, rows } => {
                    zeus_proto::remote_pty::validate_terminal_dimensions(cols, rows)
                        .map_err(|_| ControlError::bad_request("invalid resize geometry"))?;
                    session
                        .resize(cols.max(2), rows.max(2))
                        .map_err(|_| ControlError::internal("resize failed"))
                }
                AttachmentAction::Scroll {
                    direction,
                    lines,
                    col,
                    row,
                } => session
                    .scroll(
                        direction == 0,
                        usize::from(lines),
                        usize::from(col),
                        usize::from(row),
                    )
                    .map_err(|_| ControlError::internal("scroll failed")),
                AttachmentAction::Input { bytes } => {
                    if bytes.len() > 64 * 1024 {
                        return Err(ControlError::bad_request("input too large"));
                    }
                    let _ = guard.wake_session(session_id);
                    guard
                        .get(session_id)
                        .ok_or_else(|| ControlError::not_found("session unavailable"))?
                        .write_input(&bytes)
                        .map_err(|_| ControlError::internal("input failed"))
                }
                AttachmentAction::TakeControl => unreachable!(),
            }
        });
        let state = guard.get(session_id).map(|s| s.terminal_control_state());
        drop(guard);
        if let Err(error) = result {
            if !peer.controlled {
                return false;
            }
            let Some(state) = state else { return false };
            return control_frame(&peer.client_id, state, Some(error))
                .is_ok_and(|f| write_frame(writer, &f).is_ok());
        }
        true
    }

    fn register(
        &self,
        registry: &Arc<Mutex<Registry>>,
        session_id: &str,
        sink: Sink,
        seed: AttachmentSeed,
    ) {
        let mut sessions = self.sessions.lock().expect("attach hub");
        let entry = sessions.entry(session_id.to_string()).or_default();
        entry.sinks.push(sink);
        if !entry.pump_running {
            entry.pump_running = true;
            let hub = self.clone();
            let registry = Arc::clone(registry);
            let session_id = session_id.to_string();
            let _ = std::thread::Builder::new()
                .name(format!("zeus-attach-{session_id}"))
                .spawn(move || hub.pump(&registry, &session_id, seed));
        }
    }

    /// Whether any client is currently attached to `session_id` — the
    /// governor's "someone is looking at this" signal.
    pub fn has_sinks(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .expect("attach hub")
            .get(session_id)
            .is_some_and(|entry| !entry.sinks.is_empty())
    }

    fn deregister(&self, session_id: &str, sink_id: u64) {
        let mut sessions = self.sessions.lock().expect("attach hub");
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.sinks.retain(|sink| sink.id != sink_id);
        }
    }

    /// The per-session broadcast loop. Grid writers wake it on change;
    /// background bursts coalesce to 16 ms while interactive responses bypass
    /// that wait. A quiet attached terminal performs no Registry or Screen
    /// polling. Ends within one bounded wait after the last sink.
    fn pump(&self, registry: &Arc<Mutex<Registry>>, session_id: &str, seed: AttachmentSeed) {
        let mut signature = seed.signature;
        let mut last_modes = Some(seed.modes);
        let mut last_control: Option<ControlState> = None;
        let mut wake = seed.wake;
        let mut wake_generation = seed.wake_generation;
        let mut last_emission = Instant::now()
            .checked_sub(GRID_FLUSH_INTERVAL)
            .unwrap_or_else(Instant::now);
        let stop = AtomicBool::new(false);
        loop {
            let observed_generation = wake_generation;
            let event = wake.wait_for_change(wake_generation, Duration::from_secs(1));
            let mut changed = event.generation != wake_generation;
            let mut interactive = event.interactive;
            wake_generation = event.generation;

            // A restart can replace the Session (and therefore its wake
            // source) while sinks remain connected. The bounded wait above is
            // the recovery ceiling; re-seed from the replacement immediately.
            let replacement_wake = {
                let Ok(guard) = registry.lock() else { break };
                guard.get(session_id).map(|session| session.grid_wake())
            };
            if let Some(replacement) = replacement_wake
                && !wake.same_source(&replacement)
            {
                wake = replacement;
                wake_generation = wake.generation();
                signature = GridSignature::default();
                last_modes = None;
                changed = true;
                interactive = true;
            }

            if changed && !interactive {
                let elapsed = last_emission.elapsed();
                if elapsed < GRID_FLUSH_INTERVAL {
                    let event = wake.wait_for_priority_or_timeout(
                        observed_generation,
                        GRID_FLUSH_INTERVAL - elapsed,
                    );
                    wake_generation = event.generation;
                }
            }
            // The session may be briefly absent mid-restart adoption: keep
            // the sinks, send nothing until it is back.
            let observed = if changed {
                let Ok(guard) = registry.lock() else { break };
                guard.get(session_id).map(|session| {
                    (
                        session.grid_update_if_changed(&mut signature),
                        session.modes(),
                        session.terminal_control_state(),
                    )
                })
            } else {
                None
            };

            let mut frames: Vec<Frame> = Vec::with_capacity(2);
            let mut control_update = None;
            if let Some((grid, modes, control)) = observed {
                if last_control.as_ref() != Some(&control) {
                    control_update = Some(control.clone());
                    last_control = Some(control);
                }
                if let Some(update) = grid
                    && let Ok(frame) = Frame::grid(&update)
                {
                    frames.push(frame);
                }
                // Fresh sinks get their initial modes at seed time; the pump
                // only broadcasts changes.
                if let Some(previous) = last_modes
                    && previous != modes
                {
                    frames.push(Frame::modes(modes));
                }
                last_modes = Some(modes);
            }

            if !frames.is_empty() || control_update.is_some() {
                // Two publications per input may bypass coalescing: one can
                // be a trailing change already in flight, and the next is the
                // actual terminal response. The bounded budget prevents a
                // keystroke from unthrottling sustained output indefinitely.
                wake.consume_interactive_priority();
                last_emission = Instant::now();
                let sinks: Vec<(u64, Arc<Mutex<UnixStream>>, bool)> = {
                    let sessions = self.sessions.lock().expect("attach hub");
                    match sessions.get(session_id) {
                        Some(entry) => entry
                            .sinks
                            .iter()
                            .map(|sink| (sink.id, Arc::clone(&sink.writer), sink.controlled))
                            .collect(),
                        None => Vec::new(),
                    }
                };
                for (sink_id, writer, controlled) in sinks {
                    if controlled
                        && let Some(control) = &control_update
                        && control_frame(&format!("desktop-{sink_id}"), control.clone(), None)
                            .map_or(true, |frame| write_frame(&writer, &frame).is_err())
                    {
                        self.deregister(session_id, sink_id);
                        continue;
                    }
                    for frame in &frames {
                        if write_frame(&writer, frame).is_err() {
                            // The peer is gone; its serve loop will also
                            // notice, but don't keep writing meanwhile.
                            self.deregister(session_id, sink_id);
                            break;
                        }
                    }
                }
            }

            {
                let mut sessions = self.sessions.lock().expect("attach hub");
                if let Some(entry) = sessions.get_mut(session_id)
                    && entry.sinks.is_empty()
                {
                    entry.pump_running = false;
                    sessions.remove(session_id);
                    stop.store(true, Ordering::SeqCst);
                }
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
        }
    }
}

fn write_frame(writer: &Arc<Mutex<UnixStream>>, frame: &Frame) -> std::io::Result<()> {
    let bytes =
        FrameCodec::encode(frame).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut stream = writer
        .lock()
        .map_err(|_| std::io::Error::other("writer poisoned"))?;
    if let Err(error) = stream.write_all(&bytes).and_then(|_| stream.flush()) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return Err(error);
    }
    Ok(())
}

fn desktop_owner(id: &str) -> Controller {
    Controller {
        id: id.into(),
        label: "Desktop".into(),
        role: ClientRole::Desktop,
    }
}

fn control_frame(
    client_id: &str,
    control: ControlState,
    error: Option<ControlError>,
) -> std::io::Result<Frame> {
    let payload = serde_json::to_vec(&AttachmentControlState {
        client_id: client_id.into(),
        control,
        error,
    })
    .map_err(std::io::Error::other)?;
    Ok(Frame::new(FrameType::Controller, payload))
}

fn release_peer(registry: &Arc<Mutex<Registry>>, session_id: &str, client_id: &str) {
    if let Ok(guard) = registry.lock()
        && let Some(session) = guard.get(session_id)
    {
        let state = session.terminal_control_state();
        if state
            .owner
            .as_ref()
            .is_some_and(|owner| owner.id == client_id)
        {
            let _ = session.release_terminal_control(&state.epoch, client_id);
        }
    }
}
