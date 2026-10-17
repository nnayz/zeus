//! Small, versioned, bounded binary IPC models.
//!
//! The protocol contains no path, command, argument, environment, or power
//! setting value. Authentication facts come from the transport, not this wire.

use crate::eligibility::{LocalExecutionIdentity, ProcessIdentity};

pub const MAGIC: [u8; 4] = *b"ZPWR";
pub const CURRENT_PROTOCOL: ProtocolVersion = ProtocolVersion { major: 1, minor: 1 };
pub const MAX_FRAME_BYTES: usize = 4096;
pub const MAX_EXECUTIONS_PER_REQUEST: usize = 48;
pub const MAX_LEASE_TTL_MILLIS: u32 = 90_000;
pub const MAX_CONSENT_DURATION_MILLIS: u32 = 8 * 60 * 60 * 1_000;
const HEADER_BYTES: usize = 14;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BuildId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub client_build_id: BuildId,
    /// Helper boot identity learned during the mutually authenticated handshake.
    pub helper_boot_nonce: [u8; 16],
    /// Fresh Engine identity. It must not be reused after Engine restart.
    pub engine_incarnation: [u8; 16],
    /// Fresh challenge for this authenticated channel. Reconnect creates a new
    /// value, so replay state never silently resets for captured frames.
    pub channel_nonce: [u8; 16],
    /// Strictly increasing within an authenticated connection/incarnation.
    pub sequence: u64,
    pub command: RequestCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestCommand {
    Status,
    Acquire {
        lease_id: [u8; 16],
        consent_generation: u64,
        consent_nonce: [u8; 16],
        /// Bounded duration, never a caller-clock absolute deadline. The Helper
        /// converts it using its boot-scoped continuous monotonic clock.
        ttl_millis: u32,
        /// Immutable first-acquire horizon enforced by the Helper. Renewal can
        /// shorten but never reset this bound.
        maximum_total_duration_millis: u32,
        executions: Vec<LocalExecutionIdentity>,
    },
    Renew {
        lease_id: [u8; 16],
        consent_generation: u64,
        consent_nonce: [u8; 16],
        ttl_millis: u32,
        executions: Vec<LocalExecutionIdentity>,
    },
    Release {
        lease_id: [u8; 16],
        consent_generation: u64,
    },
    PrepareUninstall {
        transaction_id: [u8; 16],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    /// Echoes the negotiated identities so a response cannot cross helper or
    /// Engine incarnations unnoticed.
    pub helper_boot_nonce: [u8; 16],
    pub engine_incarnation: [u8; 16],
    pub channel_nonce: [u8; 16],
    pub request_sequence: u64,
    pub body: ResponseBody,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseBody {
    Status(StatusResponse),
    Ack(AckKind),
    Error(WireErrorCode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AckKind {
    Acquired,
    Renewed,
    Released,
    UninstallPrepared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusResponse {
    pub active: bool,
    pub allow_sleep_now_latched: bool,
    pub selected_execution_count: u16,
    pub remaining_ttl_millis: Option<u32>,
    pub safety: WireSafetyState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireSafetyState {
    Safe,
    Unknown,
    Stale,
    BatteryPower,
    BatteryUnreadable,
    ThermalUnreadable,
    ThermalUnsafe,
    Emergency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireErrorCode {
    Unauthorized,
    Malformed,
    Replay,
    StaleIncarnation,
    IncompatibleProtocol,
    IncompatibleBuild,
    Unsafe,
    Conflict,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    FrameTooLarge,
    Truncated,
    BadMagic,
    LengthMismatch,
    UnsupportedProtocol,
    WrongFrameKind,
    UnknownTag,
    TooManyExecutions,
    InvalidBoolean,
    InvalidIdentity,
    InvalidLeaseId,
    InvalidConsent,
    EmptyExecutions,
    DuplicateExecution,
    MixedBootIdentity,
    InvalidTtl,
    InvalidTotalDuration,
    TrailingBytes,
}

impl Request {
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut body = Vec::with_capacity(256);
        body.extend_from_slice(&self.client_build_id.0);
        body.extend_from_slice(&self.helper_boot_nonce);
        body.extend_from_slice(&self.engine_incarnation);
        body.extend_from_slice(&self.channel_nonce);
        put_u64(&mut body, self.sequence);
        match &self.command {
            RequestCommand::Status => body.push(0),
            RequestCommand::Acquire {
                lease_id,
                consent_generation,
                consent_nonce,
                ttl_millis,
                maximum_total_duration_millis,
                executions,
            } => {
                body.push(1);
                encode_lease(
                    &mut body,
                    lease_id,
                    *consent_generation,
                    consent_nonce,
                    *ttl_millis,
                    Some(*maximum_total_duration_millis),
                    executions,
                )?;
            }
            RequestCommand::Renew {
                lease_id,
                consent_generation,
                consent_nonce,
                ttl_millis,
                executions,
            } => {
                body.push(2);
                encode_lease(
                    &mut body,
                    lease_id,
                    *consent_generation,
                    consent_nonce,
                    *ttl_millis,
                    None,
                    executions,
                )?;
            }
            RequestCommand::Release {
                lease_id,
                consent_generation,
            } => {
                if *lease_id == [0; 16] {
                    return Err(CodecError::InvalidLeaseId);
                }
                if *consent_generation == 0 {
                    return Err(CodecError::InvalidConsent);
                }
                body.push(3);
                body.extend_from_slice(lease_id);
                put_u64(&mut body, *consent_generation);
            }
            RequestCommand::PrepareUninstall { transaction_id } => {
                if *transaction_id == [0; 16] {
                    return Err(CodecError::InvalidLeaseId);
                }
                body.push(4);
                body.extend_from_slice(transaction_id);
            }
        }
        encode_frame(1, &body)
    }

    pub fn decode(frame: &[u8]) -> Result<Self, CodecError> {
        let body = decode_frame(frame, 1)?;
        let mut cursor = Cursor::new(body);
        let client_build_id = BuildId(cursor.array()?);
        let helper_boot_nonce = cursor.array()?;
        let engine_incarnation = cursor.array()?;
        let channel_nonce = cursor.array()?;
        let sequence = cursor.u64()?;
        let command = match cursor.u8()? {
            0 => RequestCommand::Status,
            1 => {
                let lease = decode_lease(&mut cursor, true)?;
                RequestCommand::Acquire {
                    lease_id: lease.lease_id,
                    consent_generation: lease.consent_generation,
                    consent_nonce: lease.consent_nonce,
                    ttl_millis: lease.ttl_millis,
                    maximum_total_duration_millis: lease
                        .maximum_total_duration_millis
                        .expect("acquire includes total duration"),
                    executions: lease.executions,
                }
            }
            2 => {
                let lease = decode_lease(&mut cursor, false)?;
                RequestCommand::Renew {
                    lease_id: lease.lease_id,
                    consent_generation: lease.consent_generation,
                    consent_nonce: lease.consent_nonce,
                    ttl_millis: lease.ttl_millis,
                    executions: lease.executions,
                }
            }
            3 => {
                let lease_id = cursor.array()?;
                let consent_generation = cursor.u64()?;
                if lease_id == [0; 16] {
                    return Err(CodecError::InvalidLeaseId);
                }
                if consent_generation == 0 {
                    return Err(CodecError::InvalidConsent);
                }
                RequestCommand::Release {
                    lease_id,
                    consent_generation,
                }
            }
            4 => {
                let transaction_id = cursor.array()?;
                if transaction_id == [0; 16] {
                    return Err(CodecError::InvalidLeaseId);
                }
                RequestCommand::PrepareUninstall { transaction_id }
            }
            _ => return Err(CodecError::UnknownTag),
        };
        cursor.finish()?;
        Ok(Self {
            client_build_id,
            helper_boot_nonce,
            engine_incarnation,
            channel_nonce,
            sequence,
            command,
        })
    }
}

impl Response {
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut body = Vec::with_capacity(64);
        body.extend_from_slice(&self.helper_boot_nonce);
        body.extend_from_slice(&self.engine_incarnation);
        body.extend_from_slice(&self.channel_nonce);
        put_u64(&mut body, self.request_sequence);
        match self.body {
            ResponseBody::Status(status) => {
                body.push(0);
                body.push(status.active.into());
                body.push(status.allow_sleep_now_latched.into());
                if usize::from(status.selected_execution_count) > MAX_EXECUTIONS_PER_REQUEST {
                    return Err(CodecError::TooManyExecutions);
                }
                put_u16(&mut body, status.selected_execution_count);
                match status.remaining_ttl_millis {
                    Some(ttl_millis) => {
                        if ttl_millis == 0 || ttl_millis > MAX_LEASE_TTL_MILLIS {
                            return Err(CodecError::InvalidTtl);
                        }
                        body.push(1);
                        put_u32(&mut body, ttl_millis);
                    }
                    None => body.push(0),
                }
                body.push(safety_tag(status.safety));
            }
            ResponseBody::Ack(ack) => {
                body.push(1);
                body.push(ack_tag(ack));
            }
            ResponseBody::Error(error) => {
                body.push(2);
                body.push(error_tag(error));
            }
        }
        encode_frame(2, &body)
    }

    pub fn decode(frame: &[u8]) -> Result<Self, CodecError> {
        let body = decode_frame(frame, 2)?;
        let mut cursor = Cursor::new(body);
        let helper_boot_nonce = cursor.array()?;
        let engine_incarnation = cursor.array()?;
        let channel_nonce = cursor.array()?;
        let request_sequence = cursor.u64()?;
        let response_body = match cursor.u8()? {
            0 => {
                let active = cursor.boolean()?;
                let allow_sleep_now_latched = cursor.boolean()?;
                let selected_execution_count = cursor.u16()?;
                if usize::from(selected_execution_count) > MAX_EXECUTIONS_PER_REQUEST {
                    return Err(CodecError::TooManyExecutions);
                }
                let remaining_ttl_millis = if cursor.boolean()? {
                    let ttl = cursor.u32()?;
                    if ttl == 0 || ttl > MAX_LEASE_TTL_MILLIS {
                        return Err(CodecError::InvalidTtl);
                    }
                    Some(ttl)
                } else {
                    None
                };
                let safety = decode_safety(cursor.u8()?)?;
                ResponseBody::Status(StatusResponse {
                    active,
                    allow_sleep_now_latched,
                    selected_execution_count,
                    remaining_ttl_millis,
                    safety,
                })
            }
            1 => ResponseBody::Ack(decode_ack(cursor.u8()?)?),
            2 => ResponseBody::Error(decode_error(cursor.u8()?)?),
            _ => return Err(CodecError::UnknownTag),
        };
        cursor.finish()?;
        Ok(Self {
            helper_boot_nonce,
            engine_incarnation,
            channel_nonce,
            request_sequence,
            body: response_body,
        })
    }
}

fn encode_lease(
    body: &mut Vec<u8>,
    lease_id: &[u8; 16],
    consent_generation: u64,
    consent_nonce: &[u8; 16],
    ttl_millis: u32,
    maximum_total_duration_millis: Option<u32>,
    executions: &[LocalExecutionIdentity],
) -> Result<(), CodecError> {
    validate_lease(
        lease_id,
        consent_generation,
        consent_nonce,
        ttl_millis,
        maximum_total_duration_millis,
        executions,
    )?;
    body.extend_from_slice(lease_id);
    put_u64(body, consent_generation);
    body.extend_from_slice(consent_nonce);
    put_u32(body, ttl_millis);
    if let Some(duration) = maximum_total_duration_millis {
        put_u32(body, duration);
    }
    put_u16(body, executions.len() as u16);
    for execution in executions {
        encode_execution(body, execution);
    }
    Ok(())
}

struct DecodedLease {
    lease_id: [u8; 16],
    consent_generation: u64,
    consent_nonce: [u8; 16],
    ttl_millis: u32,
    maximum_total_duration_millis: Option<u32>,
    executions: Vec<LocalExecutionIdentity>,
}

fn decode_lease(cursor: &mut Cursor<'_>, acquire: bool) -> Result<DecodedLease, CodecError> {
    let lease_id = cursor.array()?;
    let consent_generation = cursor.u64()?;
    let consent_nonce = cursor.array()?;
    let ttl_millis = cursor.u32()?;
    let maximum_total_duration_millis = if acquire { Some(cursor.u32()?) } else { None };
    let count = usize::from(cursor.u16()?);
    if count > MAX_EXECUTIONS_PER_REQUEST {
        return Err(CodecError::TooManyExecutions);
    }
    let mut executions = Vec::with_capacity(count);
    for _ in 0..count {
        executions.push(decode_execution(cursor)?);
    }
    validate_lease(
        &lease_id,
        consent_generation,
        &consent_nonce,
        ttl_millis,
        maximum_total_duration_millis,
        &executions,
    )?;
    Ok(DecodedLease {
        lease_id,
        consent_generation,
        consent_nonce,
        ttl_millis,
        maximum_total_duration_millis,
        executions,
    })
}

fn validate_lease(
    lease_id: &[u8; 16],
    consent_generation: u64,
    consent_nonce: &[u8; 16],
    ttl_millis: u32,
    maximum_total_duration_millis: Option<u32>,
    executions: &[LocalExecutionIdentity],
) -> Result<(), CodecError> {
    if *lease_id == [0; 16] {
        return Err(CodecError::InvalidLeaseId);
    }
    if consent_generation == 0 || *consent_nonce == [0; 16] {
        return Err(CodecError::InvalidConsent);
    }
    if executions.is_empty() {
        return Err(CodecError::EmptyExecutions);
    }
    if executions.len() > MAX_EXECUTIONS_PER_REQUEST {
        return Err(CodecError::TooManyExecutions);
    }
    if ttl_millis == 0 || ttl_millis > MAX_LEASE_TTL_MILLIS {
        return Err(CodecError::InvalidTtl);
    }
    if let Some(duration) = maximum_total_duration_millis
        && (duration == 0 || duration > MAX_CONSENT_DURATION_MILLIS)
    {
        return Err(CodecError::InvalidTotalDuration);
    }
    let unique: std::collections::BTreeSet<_> = executions.iter().collect();
    if unique.len() != executions.len() {
        return Err(CodecError::DuplicateExecution);
    }
    let first_boot = execution_boot_id(&executions[0]);
    if executions
        .iter()
        .any(|execution| execution_boot_id(execution) != first_boot)
    {
        return Err(CodecError::MixedBootIdentity);
    }
    Ok(())
}

fn execution_boot_id(execution: &LocalExecutionIdentity) -> [u8; 16] {
    match execution {
        LocalExecutionIdentity::Direct { host_boot_id, .. }
        | LocalExecutionIdentity::Held { host_boot_id, .. } => *host_boot_id,
    }
}

fn encode_execution(body: &mut Vec<u8>, execution: &LocalExecutionIdentity) {
    match execution {
        LocalExecutionIdentity::Direct {
            session_id,
            incarnation,
            host_boot_id,
            execution_generation,
            process,
        } => {
            body.push(0);
            body.extend_from_slice(session_id);
            body.extend_from_slice(incarnation);
            body.extend_from_slice(host_boot_id);
            put_u64(body, *execution_generation);
            encode_process(body, *process);
        }
        LocalExecutionIdentity::Held {
            session_id,
            incarnation,
            host_boot_id,
            execution_generation,
            holder,
            child,
        } => {
            body.push(1);
            body.extend_from_slice(session_id);
            body.extend_from_slice(incarnation);
            body.extend_from_slice(host_boot_id);
            put_u64(body, *execution_generation);
            encode_process(body, *holder);
            encode_process(body, *child);
        }
    }
}

fn decode_execution(cursor: &mut Cursor<'_>) -> Result<LocalExecutionIdentity, CodecError> {
    let tag = cursor.u8()?;
    let session_id = cursor.array()?;
    let incarnation = cursor.array()?;
    let host_boot_id = cursor.array()?;
    let execution_generation = cursor.u64()?;
    if session_id == [0; 16]
        || incarnation == [0; 16]
        || host_boot_id == [0; 16]
        || execution_generation == 0
    {
        return Err(CodecError::InvalidIdentity);
    }
    match tag {
        0 => Ok(LocalExecutionIdentity::Direct {
            session_id,
            incarnation,
            host_boot_id,
            execution_generation,
            process: decode_process(cursor)?,
        }),
        1 => Ok(LocalExecutionIdentity::Held {
            session_id,
            incarnation,
            host_boot_id,
            execution_generation,
            holder: decode_process(cursor)?,
            child: decode_process(cursor)?,
        }),
        _ => Err(CodecError::UnknownTag),
    }
}

fn encode_process(body: &mut Vec<u8>, process: ProcessIdentity) {
    put_u32(body, process.pid);
    put_u64(body, process.birth_token);
}

fn decode_process(cursor: &mut Cursor<'_>) -> Result<ProcessIdentity, CodecError> {
    let process = ProcessIdentity {
        pid: cursor.u32()?,
        birth_token: cursor.u64()?,
    };
    if process.pid == 0 || process.birth_token == 0 {
        return Err(CodecError::InvalidIdentity);
    }
    Ok(process)
}

fn encode_frame(kind: u8, body: &[u8]) -> Result<Vec<u8>, CodecError> {
    let total = HEADER_BYTES
        .checked_add(body.len())
        .ok_or(CodecError::FrameTooLarge)?;
    if total > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge);
    }
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&MAGIC);
    put_u32(&mut frame, total as u32);
    put_u16(&mut frame, CURRENT_PROTOCOL.major);
    put_u16(&mut frame, CURRENT_PROTOCOL.minor);
    frame.push(kind);
    frame.push(0);
    frame.extend_from_slice(body);
    Ok(frame)
}

fn decode_frame(frame: &[u8], expected_kind: u8) -> Result<&[u8], CodecError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge);
    }
    if frame.len() < HEADER_BYTES {
        return Err(CodecError::Truncated);
    }
    if frame[0..4] != MAGIC {
        return Err(CodecError::BadMagic);
    }
    let declared = u32::from_be_bytes(frame[4..8].try_into().expect("fixed slice")) as usize;
    if declared != frame.len() {
        return Err(CodecError::LengthMismatch);
    }
    let version = ProtocolVersion {
        major: u16::from_be_bytes(frame[8..10].try_into().expect("fixed slice")),
        minor: u16::from_be_bytes(frame[10..12].try_into().expect("fixed slice")),
    };
    if version != CURRENT_PROTOCOL {
        return Err(CodecError::UnsupportedProtocol);
    }
    if frame[12] != expected_kind {
        return Err(CodecError::WrongFrameKind);
    }
    if frame[13] != 0 {
        return Err(CodecError::UnknownTag);
    }
    Ok(&frame[HEADER_BYTES..])
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(CodecError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(CodecError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        self.take(N)?.try_into().map_err(|_| CodecError::Truncated)
    }
    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool, CodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(CodecError::InvalidBoolean),
        }
    }
    fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_be_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn finish(self) -> Result<(), CodecError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(CodecError::TrailingBytes)
        }
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn ack_tag(value: AckKind) -> u8 {
    match value {
        AckKind::Acquired => 0,
        AckKind::Renewed => 1,
        AckKind::Released => 2,
        AckKind::UninstallPrepared => 3,
    }
}
fn decode_ack(tag: u8) -> Result<AckKind, CodecError> {
    match tag {
        0 => Ok(AckKind::Acquired),
        1 => Ok(AckKind::Renewed),
        2 => Ok(AckKind::Released),
        3 => Ok(AckKind::UninstallPrepared),
        _ => Err(CodecError::UnknownTag),
    }
}
fn safety_tag(value: WireSafetyState) -> u8 {
    value as u8
}
fn decode_safety(tag: u8) -> Result<WireSafetyState, CodecError> {
    match tag {
        0 => Ok(WireSafetyState::Safe),
        1 => Ok(WireSafetyState::Unknown),
        2 => Ok(WireSafetyState::Stale),
        3 => Ok(WireSafetyState::BatteryPower),
        4 => Ok(WireSafetyState::BatteryUnreadable),
        5 => Ok(WireSafetyState::ThermalUnreadable),
        6 => Ok(WireSafetyState::ThermalUnsafe),
        7 => Ok(WireSafetyState::Emergency),
        _ => Err(CodecError::UnknownTag),
    }
}
fn error_tag(value: WireErrorCode) -> u8 {
    value as u8
}
fn decode_error(tag: u8) -> Result<WireErrorCode, CodecError> {
    match tag {
        0 => Ok(WireErrorCode::Unauthorized),
        1 => Ok(WireErrorCode::Malformed),
        2 => Ok(WireErrorCode::Replay),
        3 => Ok(WireErrorCode::StaleIncarnation),
        4 => Ok(WireErrorCode::IncompatibleProtocol),
        5 => Ok(WireErrorCode::IncompatibleBuild),
        6 => Ok(WireErrorCode::Unsafe),
        7 => Ok(WireErrorCode::Conflict),
        8 => Ok(WireErrorCode::Internal),
        _ => Err(CodecError::UnknownTag),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held() -> LocalExecutionIdentity {
        held_with(1)
    }

    fn held_with(value: u8) -> LocalExecutionIdentity {
        LocalExecutionIdentity::Held {
            session_id: [value; 16],
            incarnation: [value.wrapping_add(1); 16],
            host_boot_id: [3; 16],
            execution_generation: u64::from(value),
            holder: ProcessIdentity {
                pid: u32::from(value) + 1,
                birth_token: u64::from(value) + 40,
            },
            child: ProcessIdentity {
                pid: u32::from(value) + 100,
                birth_token: u64::from(value) + 50,
            },
        }
    }

    #[test]
    fn request_and_response_round_trip() {
        let request = Request {
            client_build_id: BuildId([9; 32]),
            helper_boot_nonce: [8; 16],
            engine_incarnation: [6; 16],
            channel_nonce: [5; 16],
            sequence: 42,
            command: RequestCommand::Acquire {
                lease_id: [7; 16],
                consent_generation: 6,
                consent_nonce: [6; 16],
                ttl_millis: 99,
                maximum_total_duration_millis: 1_000,
                executions: vec![held()],
            },
        };
        assert_eq!(Request::decode(&request.encode().unwrap()), Ok(request));

        let response = Response {
            helper_boot_nonce: [8; 16],
            engine_incarnation: [6; 16],
            channel_nonce: [5; 16],
            request_sequence: 42,
            body: ResponseBody::Status(StatusResponse {
                active: true,
                allow_sleep_now_latched: false,
                selected_execution_count: 1,
                remaining_ttl_millis: Some(99),
                safety: WireSafetyState::Safe,
            }),
        };
        assert_eq!(Response::decode(&response.encode().unwrap()), Ok(response));
    }

    #[test]
    fn decoder_rejects_length_version_kind_and_trailing_bytes() {
        let request = Request {
            client_build_id: BuildId([0; 32]),
            helper_boot_nonce: [0; 16],
            engine_incarnation: [1; 16],
            channel_nonce: [2; 16],
            sequence: 1,
            command: RequestCommand::Status,
        };
        let valid = request.encode().unwrap();

        let mut bad = valid.clone();
        bad[4..8].copy_from_slice(&1_u32.to_be_bytes());
        assert_eq!(Request::decode(&bad), Err(CodecError::LengthMismatch));
        bad = valid.clone();
        bad[8..10].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(Request::decode(&bad), Err(CodecError::UnsupportedProtocol));
        assert_eq!(Response::decode(&valid), Err(CodecError::WrongFrameKind));

        bad = valid;
        bad.push(0);
        let len = bad.len() as u32;
        bad[4..8].copy_from_slice(&len.to_be_bytes());
        assert_eq!(Request::decode(&bad), Err(CodecError::TrailingBytes));
    }

    #[test]
    fn encoder_and_decoder_enforce_execution_bound() {
        let request = Request {
            client_build_id: BuildId([0; 32]),
            helper_boot_nonce: [0; 16],
            engine_incarnation: [1; 16],
            channel_nonce: [2; 16],
            sequence: 1,
            command: RequestCommand::Renew {
                lease_id: [1; 16],
                consent_generation: 6,
                consent_nonce: [6; 16],
                ttl_millis: 2,
                executions: vec![held(); MAX_EXECUTIONS_PER_REQUEST + 1],
            },
        };
        assert_eq!(request.encode(), Err(CodecError::TooManyExecutions));

        let maximum = Request {
            client_build_id: BuildId([0; 32]),
            helper_boot_nonce: [1; 16],
            engine_incarnation: [2; 16],
            channel_nonce: [3; 16],
            sequence: 1,
            command: RequestCommand::Acquire {
                lease_id: [4; 16],
                consent_generation: 6,
                consent_nonce: [6; 16],
                ttl_millis: MAX_LEASE_TTL_MILLIS,
                maximum_total_duration_millis: 1_000,
                executions: (1..=MAX_EXECUTIONS_PER_REQUEST as u8)
                    .map(held_with)
                    .collect(),
            },
        };
        let encoded = maximum.encode().expect("documented maximum fits frame");
        assert!(encoded.len() <= MAX_FRAME_BYTES);
        assert_eq!(Request::decode(&encoded), Ok(maximum));

        let mut frame = Request {
            client_build_id: BuildId([0; 32]),
            helper_boot_nonce: [0; 16],
            engine_incarnation: [1; 16],
            channel_nonce: [2; 16],
            sequence: 1,
            command: RequestCommand::Acquire {
                lease_id: [1; 16],
                consent_generation: 6,
                consent_nonce: [6; 16],
                ttl_millis: 2,
                maximum_total_duration_millis: 1_000,
                executions: vec![held()],
            },
        }
        .encode()
        .unwrap();
        // Count follows the fixed request identity, command tag, lease,
        // consent identity, TTL, and original total-duration bound.
        let count_offset = HEADER_BYTES + 32 + 16 + 16 + 16 + 8 + 1 + 16 + 8 + 16 + 4 + 4;
        frame[count_offset..count_offset + 2]
            .copy_from_slice(&((MAX_EXECUTIONS_PER_REQUEST + 1) as u16).to_be_bytes());
        assert_eq!(Request::decode(&frame), Err(CodecError::TooManyExecutions));
    }

    #[test]
    fn ttl_and_status_counts_are_bounded() {
        for ttl_millis in [0, MAX_LEASE_TTL_MILLIS + 1] {
            let request = Request {
                client_build_id: BuildId([1; 32]),
                helper_boot_nonce: [2; 16],
                engine_incarnation: [3; 16],
                channel_nonce: [4; 16],
                sequence: 1,
                command: RequestCommand::Acquire {
                    lease_id: [5; 16],
                    consent_generation: 6,
                    consent_nonce: [6; 16],
                    ttl_millis,
                    maximum_total_duration_millis: 1_000,
                    executions: vec![held()],
                },
            };
            assert_eq!(request.encode(), Err(CodecError::InvalidTtl));
        }

        let response = Response {
            helper_boot_nonce: [2; 16],
            engine_incarnation: [3; 16],
            channel_nonce: [4; 16],
            request_sequence: 1,
            body: ResponseBody::Status(StatusResponse {
                active: true,
                allow_sleep_now_latched: false,
                selected_execution_count: (MAX_EXECUTIONS_PER_REQUEST + 1) as u16,
                remaining_ttl_millis: Some(1),
                safety: WireSafetyState::Safe,
            }),
        };
        assert_eq!(response.encode(), Err(CodecError::TooManyExecutions));
    }

    #[test]
    fn lease_identity_and_selection_fail_closed() {
        let request = |command| Request {
            client_build_id: BuildId([1; 32]),
            helper_boot_nonce: [2; 16],
            engine_incarnation: [3; 16],
            channel_nonce: [4; 16],
            sequence: 1,
            command,
        };
        let acquire = |lease_id, consent_generation, consent_nonce, duration, executions| {
            RequestCommand::Acquire {
                lease_id,
                consent_generation,
                consent_nonce,
                ttl_millis: 1,
                maximum_total_duration_millis: duration,
                executions,
            }
        };
        assert_eq!(
            request(acquire([0; 16], 1, [1; 16], 1, vec![held()])).encode(),
            Err(CodecError::InvalidLeaseId)
        );
        assert_eq!(
            request(acquire([1; 16], 0, [1; 16], 1, vec![held()])).encode(),
            Err(CodecError::InvalidConsent)
        );
        assert_eq!(
            request(acquire([1; 16], 1, [0; 16], 1, vec![held()])).encode(),
            Err(CodecError::InvalidConsent)
        );
        assert_eq!(
            request(acquire([1; 16], 1, [1; 16], 1, vec![])).encode(),
            Err(CodecError::EmptyExecutions)
        );
        assert_eq!(
            request(acquire([1; 16], 1, [1; 16], 1, vec![held(), held()],)).encode(),
            Err(CodecError::DuplicateExecution)
        );
        let mut other_boot = held_with(2);
        if let LocalExecutionIdentity::Held { host_boot_id, .. } = &mut other_boot {
            *host_boot_id = [9; 16];
        }
        assert_eq!(
            request(acquire([1; 16], 1, [1; 16], 1, vec![held(), other_boot],)).encode(),
            Err(CodecError::MixedBootIdentity)
        );
        assert_eq!(
            request(acquire(
                [1; 16],
                1,
                [1; 16],
                MAX_CONSENT_DURATION_MILLIS + 1,
                vec![held()],
            ))
            .encode(),
            Err(CodecError::InvalidTotalDuration)
        );
    }

    #[test]
    fn oversized_input_is_rejected_before_parsing() {
        let frame = vec![0; MAX_FRAME_BYTES + 1];
        assert_eq!(Request::decode(&frame), Err(CodecError::FrameTooLarge));
    }
}
