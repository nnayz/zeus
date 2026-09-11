//! Additive Engine terminal API. This is local IPC, never the external
//! Companion API or the Holder protocol. The gateway supplies authenticated
//! device identities and enforces its own allowlist/scopes.

use serde::{Deserialize, Serialize};

use crate::{ClientRole, ControlError, SessionId, frames::TerminalModes, grid::GridUpdate};

pub const TERMINAL_PROTOCOL: u32 = 1;
pub const MAX_SNAPSHOT_CELLS: usize = 32_768;
pub const MAX_SNAPSHOT_DIMENSION: u16 = 512;
pub const MAX_SNAPSHOT_BYTES: usize = 1_048_576;
pub const MAX_TEXT_BYTES: usize = 16_384;
pub const MAX_SCROLL_ROWS: i64 = 128;
pub const SNAPSHOT: &str = "terminal.snapshot";
pub const ACQUIRE_CONTROL: &str = "terminal.acquire_control";
pub const RELEASE_CONTROL: &str = "terminal.release_control";
pub const SEND_TEXT: &str = "terminal.send_text";
pub const SCROLLBACK: &str = "terminal.scrollback";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlEpoch {
    /// Fresh for every Engine Session creation/adoption, including restart.
    /// This is not the process's Holder incarnation or an authorization token.
    pub incarnation: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Controller {
    pub id: String,
    pub label: String,
    pub role: ClientRole,
}

impl Controller {
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
            || self.label.is_empty()
            || self.label.len() > 128
            || self.label.chars().any(char::is_control)
            || self.role == ClientRole::Unknown
        {
            return Err(ControlError::bad_request("invalid controller identity"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    pub epoch: ControlEpoch,
    pub owner: Option<Controller>,
    /// Last consumed command. A failed/ambiguous write is consumed too.
    pub command_seq: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCursor {
    pub incarnation: String,
    pub sequence: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotParams {
    #[serde(rename = "sessionID")]
    pub session_id: SessionId,
    pub protocol: u32,
    pub since: Option<SnapshotCursor>,
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub protocol: u32,
    #[serde(rename = "sessionID")]
    pub session_id: SessionId,
    pub cursor: SnapshotCursor,
    pub control: ControlState,
    /// Complete visible RLE grid; absent only when `since` is current.
    /// No diffs/replay, no scrollback, and no PTY resize on viewing.
    #[serde(with = "optional_grid_bytes")]
    pub grid: Option<Vec<u8>>,
    pub modes: TerminalModes,
    pub capabilities: Vec<String>,
    pub exited: bool,
}

impl TerminalSnapshot {
    pub fn decode_grid(&self) -> Result<Option<GridUpdate>, ControlError> {
        let Some(bytes) = &self.grid else {
            return Ok(None);
        };
        // Check the fixed header BEFORE the RLE decoder can allocate rows.
        if bytes.len() < 11 || bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(ControlError::bad_request("invalid snapshot size"));
        }
        let cols = u16::from_be_bytes([bytes[0], bytes[1]]);
        let rows = u16::from_be_bytes([bytes[2], bytes[3]]);
        validate_geometry(cols, rows)?;
        // RLE can expand a tiny payload into many cells. Validate every run
        // before delegating to the existing compatibility decoder.
        let invalid = || ControlError::bad_request("invalid snapshot RLE geometry");
        let u16_at = |offset: usize| -> Result<u16, ControlError> {
            let pair = bytes.get(offset..offset + 2).ok_or_else(invalid)?;
            Ok(u16::from_be_bytes([pair[0], pair[1]]))
        };
        if u16_at(9)? != rows || bytes[8] & 2 == 0 || u16_at(4)? >= cols || u16_at(6)? >= rows {
            return Err(invalid());
        }
        let mut offset = 11;
        for y in 0..rows {
            if u16_at(offset)? != y {
                return Err(invalid());
            }
            let runs = u16_at(offset + 2)?;
            if runs == 0 || runs > cols {
                return Err(invalid());
            }
            offset += 4;
            let mut cells = 0usize;
            for _ in 0..runs {
                let repeat = u16_at(offset)?;
                cells += usize::from(repeat);
                if repeat == 0
                    || cells > usize::from(cols)
                    || bytes.get(offset..offset + 16).is_none()
                {
                    return Err(invalid());
                }
                offset += 16;
            }
            if cells != usize::from(cols) {
                return Err(invalid());
            }
        }
        if offset != bytes.len() {
            return Err(invalid());
        }
        let grid = GridUpdate::decode(bytes)
            .map_err(|_| ControlError::bad_request("invalid snapshot grid"))?;
        validate_snapshot(&grid)?;
        Ok(Some(grid))
    }
}

pub fn validate_geometry(cols: u16, rows: u16) -> Result<(), ControlError> {
    if cols == 0
        || rows == 0
        || cols > MAX_SNAPSHOT_DIMENSION
        || rows > MAX_SNAPSHOT_DIMENSION
        || usize::from(cols) * usize::from(rows) > MAX_SNAPSHOT_CELLS
    {
        return Err(ControlError::new(
            "terminal_geometry",
            "terminal exceeds Companion geometry bounds",
        ));
    }
    Ok(())
}

pub fn validate_snapshot(grid: &GridUpdate) -> Result<(), ControlError> {
    validate_geometry(grid.cols, grid.rows)?;
    if !grid.is_full_snapshot
        || grid.cursor_col >= grid.cols
        || grid.cursor_row >= grid.rows
        || grid.changed_rows.len() != usize::from(grid.rows)
        || grid
            .changed_rows
            .iter()
            .enumerate()
            .any(|(y, row)| usize::from(row.y) != y || row.cells.len() != usize::from(grid.cols))
    {
        return Err(ControlError::bad_request("invalid full snapshot geometry"));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireControlParams {
    #[serde(rename = "sessionID")]
    pub session_id: SessionId,
    pub expected: ControlEpoch,
    pub owner: Controller,
    pub takeover: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseControlParams {
    #[serde(rename = "sessionID")]
    pub session_id: SessionId,
    pub expected: ControlEpoch,
    pub owner_id: String,
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSendTextParams {
    #[serde(rename = "sessionID")]
    pub session_id: SessionId,
    pub expected: ControlEpoch,
    pub owner_id: String,
    pub command_seq: u64,
    pub text: String,
    pub submit: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalScrollbackParams {
    #[serde(rename = "sessionID")]
    pub session_id: SessionId,
    pub protocol: u32,
    pub first_row: i64,
    pub max_rows: i64,
}

/// Negotiated desktop attachment state. `client_id` is Engine-assigned.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentControlState {
    pub client_id: String,
    pub control: ControlState,
    pub error: Option<ControlError>,
}

/// Only local desktop attachments negotiate this envelope. The Companion
/// v1 allowlist never exposes raw input, resize, scroll, or this binary channel.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlledFrame {
    pub expected: ControlEpoch,
    pub action: AttachmentAction,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttachmentAction {
    TakeControl,
    ReleaseControl,
    Input {
        bytes: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Scroll {
        direction: u8,
        lines: u16,
        col: u16,
        row: u16,
    },
}

mod optional_grid_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Bytes(#[serde(with = "crate::methods::base64_bytes")] Vec<u8>);

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => crate::methods::base64_bytes::serialize(bytes, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<Bytes>::deserialize(deserializer).map(|bytes| bytes.map(|bytes| bytes.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{ChangedRow, GridCell};

    fn snapshot() -> TerminalSnapshot {
        let grid = GridUpdate {
            cols: 2,
            rows: 1,
            cursor_col: 1,
            cursor_row: 0,
            cursor_visible: true,
            is_full_snapshot: true,
            changed_rows: vec![ChangedRow::new(0, vec![GridCell::BLANK; 2])],
        };
        TerminalSnapshot {
            protocol: 1,
            session_id: SessionId("test".into()),
            cursor: SnapshotCursor {
                incarnation: "engine-session".into(),
                sequence: 1,
            },
            control: ControlState {
                epoch: ControlEpoch {
                    incarnation: "engine-session".into(),
                    generation: 0,
                },
                owner: None,
                command_seq: 0,
            },
            grid: Some(grid.encode().unwrap()),
            modes: TerminalModes {
                focus_reporting: true,
                mouse_utf8: true,
                ..Default::default()
            },
            capabilities: vec!["snapshot".into()],
            exited: false,
        }
    }

    #[test]
    fn snapshot_roundtrip_preserves_grid_modes_and_lease_without_payload_debug() {
        let original = snapshot();
        let wire = serde_json::to_value(&original).unwrap();
        assert!(wire["grid"].is_string());
        let decoded: TerminalSnapshot = serde_json::from_value(wire).unwrap();
        assert!(original == decoded);
        assert!(decoded.decode_grid().unwrap().unwrap().is_full_snapshot);
        let mut unchanged = decoded;
        unchanged.grid = None;
        let wire = serde_json::to_value(&unchanged).unwrap();
        assert!(wire["grid"].is_null());
        assert!(
            serde_json::from_value::<TerminalSnapshot>(wire)
                .unwrap()
                .grid
                .is_none()
        );
    }

    #[test]
    fn malformed_geometry_is_rejected_before_row_allocation() {
        let mut snapshot = snapshot();
        let bytes = snapshot.grid.as_mut().unwrap();
        bytes[..4].copy_from_slice(&[255, 255, 255, 255]);
        assert_eq!(
            snapshot.decode_grid().unwrap_err().code,
            "terminal_geometry"
        );
        assert!(validate_geometry(512, 512).is_err());
        assert!(validate_geometry(0, 1).is_err());
        let mut grid = self::snapshot().decode_grid().unwrap().unwrap();
        grid.cursor_col = 2;
        assert!(validate_snapshot(&grid).is_err());
        grid.cursor_col = 0;
        grid.changed_rows[0].y = 1;
        assert!(validate_snapshot(&grid).is_err());
    }

    #[test]
    fn malicious_rle_expansion_and_trailing_bytes_are_rejected() {
        let mut value = snapshot();
        value.grid.as_mut().unwrap()[9..11].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(value.decode_grid().is_err());
        let mut value = snapshot();
        value.grid.as_mut().unwrap()[15..17].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(value.decode_grid().is_err());
        let mut value = snapshot();
        value.grid.as_mut().unwrap().push(0);
        assert!(value.decode_grid().is_err());
        let mut value = snapshot();
        value.grid.as_mut().unwrap()[15..17].copy_from_slice(&0u16.to_be_bytes());
        assert!(value.decode_grid().is_err());
    }
}
