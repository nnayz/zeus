//! Engine-owned cross-client control, independent of the PTY transport.
//!
//! Callers serialize lease changes and effects with the Registry lock. Neither
//! snapshots nor ownership add a Holder attachment, queue, or background task.

use std::io;

use zeus_proto::terminal::*;
use zeus_proto::{ClientRole, ControlError};

use crate::session::{GridSignature, Session};

pub(crate) struct TerminalControl {
    state: ControlState,
    snapshot_sequence: u64,
    snapshot_signature: Option<(GridSignature, zeus_proto::frames::TerminalModes)>,
}

impl TerminalControl {
    pub(crate) fn new() -> io::Result<Self> {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        Ok(Self {
            state: ControlState {
                epoch: ControlEpoch {
                    incarnation: nonce.iter().map(|b| format!("{b:02x}")).collect(),
                    generation: 0,
                },
                owner: None,
                command_seq: 0,
            },
            snapshot_sequence: 0,
            snapshot_signature: None,
        })
    }

    fn check_epoch(&self, expected: &ControlEpoch) -> Result<(), ControlError> {
        if *expected != self.state.epoch {
            return Err(ControlError::new(
                "stale_controller_epoch",
                "refresh controller state before acting",
            ));
        }
        Ok(())
    }

    fn check_owner(&self, expected: &ControlEpoch, owner_id: &str) -> Result<(), ControlError> {
        self.check_epoch(expected)?;
        if !self
            .state
            .owner
            .as_ref()
            .is_some_and(|owner| owner.id == owner_id)
        {
            return Err(ControlError::new(
                "not_controller",
                "explicitly acquire control before acting",
            ));
        }
        Ok(())
    }

    fn advance(&mut self, owner: Option<Controller>) -> Result<(), ControlError> {
        self.state.epoch.generation =
            self.state.epoch.generation.checked_add(1).ok_or_else(|| {
                ControlError::new("epoch_exhausted", "recreate the Engine session")
            })?;
        self.state.owner = owner;
        self.state.command_seq = 0;
        Ok(())
    }
}

impl Session {
    pub fn terminal_control_state(&self) -> ControlState {
        let exited = self.view().exited;
        let mut control = self.terminal_control.lock().expect("terminal control");
        if exited && control.state.owner.is_some() {
            // Even at counter exhaustion, an exited process has no controller.
            if control.advance(None).is_err() {
                control.state.owner = None;
            }
        }
        control.state.clone()
    }

    /// Must be called under the Registry lock held through the subsequent
    /// lifecycle effect; this check alone is not a transferable authorization.
    pub fn validate_terminal_control(
        &self,
        expected: &ControlEpoch,
        owner_id: &str,
    ) -> Result<(), ControlError> {
        self.terminal_control_state();
        self.terminal_control
            .lock()
            .expect("terminal control")
            .check_owner(expected, owner_id)
    }

    pub fn acquire_terminal_control(
        &self,
        expected: &ControlEpoch,
        owner: Controller,
        takeover: bool,
    ) -> Result<ControlState, ControlError> {
        owner.validate()?;
        if self.view().exited {
            return Err(ControlError::new("session_exited", "session has exited"));
        }
        let mut control = self.terminal_control.lock().expect("terminal control");
        control.check_epoch(expected)?;
        if !self.ready_for_control_transfer() {
            return Err(ControlError::new(
                "terminal_unavailable",
                "wait for remote input recovery before taking control",
            ));
        }
        if control.state.owner.is_some() && !takeover {
            return Err(ControlError::new(
                "controller_busy",
                "explicit takeover is required",
            ));
        }
        control.advance(Some(owner))?;
        let state = control.state.clone();
        drop(control);
        self.grid_wake().notify();
        Ok(state)
    }

    pub fn release_terminal_control(
        &self,
        expected: &ControlEpoch,
        owner_id: &str,
    ) -> Result<ControlState, ControlError> {
        let mut control = self.terminal_control.lock().expect("terminal control");
        control.check_owner(expected, owner_id)?;
        control.advance(None)?;
        let state = control.state.clone();
        drop(control);
        self.grid_wake().notify();
        Ok(state)
    }

    /// Legacy IPC has no device/epoch. It must never bypass an explicit lease.
    pub(crate) fn require_unowned_terminal(&self) -> Result<(), ControlError> {
        if self
            .terminal_control_state()
            .owner
            .as_ref()
            .is_some_and(|owner| owner.role == ClientRole::Mobile)
        {
            return Err(ControlError::new(
                "controller_busy",
                "Companion owns this terminal; take control first",
            ));
        }
        Ok(())
    }

    pub fn terminal_send_text(
        &self,
        params: &TerminalSendTextParams,
    ) -> Result<ControlState, ControlError> {
        self.terminal_control_state();
        if params.text.len() > MAX_TEXT_BYTES
            || params
                .text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(ControlError::bad_request(
                "text exceeds bounds or contains terminal controls",
            ));
        }
        let mut control = self.terminal_control.lock().expect("terminal control");
        control.check_owner(&params.expected, &params.owner_id)?;
        if control.state.command_seq.checked_add(1) != Some(params.command_seq) {
            return Err(ControlError::new(
                "command_sequence",
                "command was already consumed or is out of order; refresh state",
            ));
        }
        // Consume before I/O. No payload cache and no replay after a lost reply,
        // transport failure, gateway crash, or later takeover.
        control.state.command_seq = params.command_seq;
        let result = self.send_text_once(&params.text, params.submit);
        let state = control.state.clone();
        drop(control);
        result.map_err(|_| {
            ControlError::new(
                "input_unconfirmed",
                "delivery is unconfirmed; refresh state, do not retry this command",
            )
        })?;
        Ok(state)
    }

    pub fn terminal_snapshot(
        &self,
        params: &TerminalSnapshotParams,
    ) -> Result<TerminalSnapshot, ControlError> {
        if params.protocol != TERMINAL_PROTOCOL {
            return Err(ControlError::version_mismatch(
                "unsupported Engine terminal protocol",
            ));
        }
        self.terminal_control_state();
        let (grid, modes, signature) = self.terminal_snapshot_sample()?;
        validate_snapshot(&grid)?;
        let mut control = self.terminal_control.lock().expect("terminal control");
        if control.snapshot_signature != Some((signature, modes)) {
            control.snapshot_sequence =
                control.snapshot_sequence.checked_add(1).ok_or_else(|| {
                    ControlError::new("sequence_exhausted", "recreate the Engine session")
                })?;
            control.snapshot_signature = Some((signature, modes));
        }
        let cursor = SnapshotCursor {
            incarnation: control.state.epoch.incarnation.clone(),
            sequence: control.snapshot_sequence,
        };
        let bytes = if params.since.as_ref() == Some(&cursor) {
            None
        } else {
            let bytes = grid
                .encode()
                .map_err(|_| ControlError::internal("invalid Engine grid"))?;
            if bytes.len() > MAX_SNAPSHOT_BYTES {
                return Err(ControlError::new(
                    "terminal_geometry",
                    "snapshot exceeds byte bound",
                ));
            }
            Some(bytes)
        };
        Ok(TerminalSnapshot {
            protocol: TERMINAL_PROTOCOL,
            session_id: params.session_id.clone(),
            cursor,
            control: control.state.clone(),
            grid: bytes,
            modes,
            capabilities: ["snapshot", "send_text", "control_lease", "scrollback"]
                .map(String::from)
                .to_vec(),
            exited: self.view().exited,
        })
    }
}

pub(crate) fn dispatch(
    registry: &std::sync::Mutex<crate::registry::Registry>,
    events: &crate::events::EventBus,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ControlError> {
    fn decode<T: serde::de::DeserializeOwned>(
        params: Option<serde_json::Value>,
    ) -> Result<T, ControlError> {
        serde_json::from_value(params.unwrap_or(serde_json::Value::Null))
            .map_err(|_| ControlError::bad_request("invalid terminal request"))
    }
    fn encode<T: serde::Serialize>(value: T) -> Result<serde_json::Value, ControlError> {
        serde_json::to_value(value)
            .map_err(|_| ControlError::internal("terminal response encoding failed"))
    }
    let registry = registry
        .lock()
        .map_err(|_| ControlError::internal("registry unavailable"))?;
    let session = |id: &zeus_proto::SessionId| {
        registry
            .get(&id.0)
            .ok_or_else(|| ControlError::not_found("session unavailable"))
    };
    let (id, state) = match method {
        SNAPSHOT => {
            let p: TerminalSnapshotParams = decode(params)?;
            return encode(session(&p.session_id)?.terminal_snapshot(&p)?);
        }
        SCROLLBACK => {
            let p: TerminalScrollbackParams = decode(params)?;
            if p.protocol != TERMINAL_PROTOCOL {
                return Err(ControlError::version_mismatch(
                    "unsupported terminal protocol",
                ));
            }
            if !(1..=MAX_SCROLL_ROWS).contains(&p.max_rows) || p.first_row < 0 {
                return Err(ControlError::bad_request("invalid scrollback bounds"));
            }
            let session = session(&p.session_id)?;
            let (cols, rows) = session.screen_size();
            validate_geometry(
                u16::try_from(cols).unwrap_or(u16::MAX),
                u16::try_from(rows).unwrap_or(u16::MAX),
            )?;
            if cols.saturating_mul(p.max_rows as usize) > MAX_SNAPSHOT_CELLS {
                return Err(ControlError::bad_request("scrollback exceeds cell bound"));
            }
            let result = session.read_scrollback_cells(p.first_row, p.max_rows);
            let encoded = encode(result)?;
            if serde_json::to_vec(&encoded)
                .map_err(|_| ControlError::internal("scrollback encoding"))?
                .len()
                > MAX_SNAPSHOT_BYTES
            {
                return Err(ControlError::new(
                    "terminal_geometry",
                    "scrollback exceeds byte bound",
                ));
            }
            return Ok(encoded);
        }
        ACQUIRE_CONTROL => {
            let p: AcquireControlParams = decode(params)?;
            let state = session(&p.session_id)?.acquire_terminal_control(
                &p.expected,
                p.owner,
                p.takeover,
            )?;
            (p.session_id, state)
        }
        RELEASE_CONTROL => {
            let p: ReleaseControlParams = decode(params)?;
            let state =
                session(&p.session_id)?.release_terminal_control(&p.expected, &p.owner_id)?;
            (p.session_id, state)
        }
        SEND_TEXT => {
            let p: TerminalSendTextParams = decode(params)?;
            let state = session(&p.session_id)?.terminal_send_text(&p)?;
            (p.session_id, state)
        }
        _ => return Err(ControlError::bad_request("unsupported terminal operation")),
    };
    events.publish(
        "terminal.control_changed",
        serde_json::json!({"sessionID": id, "control": state}),
        Some(&id.0),
    );
    encode(state)
}
