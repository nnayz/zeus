//! Fail-closed policy for facts obtained from an authenticated local transport.

use crate::wire::{BuildId, ProtocolVersion, Request, Response};

pub const MAX_SIGNING_IDENTITY_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerRole {
    Engine,
    Helper,
}

/// Facts must be populated by the platform IPC/code-signing seam. The core
/// never infers identity from a client-provided request payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedPeerFacts {
    pub local_transport_authenticated: bool,
    pub audit_token_validated: bool,
    pub code_signature_valid: bool,
    /// The platform seam evaluated the pinned designated requirement against
    /// the audit-token process, rather than trusting a payload claim.
    pub designated_requirement_satisfied: bool,
    pub role: PeerRole,
    pub uid: u32,
    pub executable_id: String,
    pub team_id: String,
    pub designated_requirement: String,
    pub executable_build_id: BuildId,
    pub protocol: ProtocolVersion,
    pub capabilities: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerPolicy {
    pub expected_role: PeerRole,
    pub expected_uid: u32,
    pub expected_executable_id: String,
    pub expected_team_id: String,
    pub expected_designated_requirement: String,
    pub allowed_build_ids: Vec<BuildId>,
    pub protocol_major: u16,
    pub minimum_protocol_minor: u16,
    pub maximum_protocol_minor: u16,
    pub required_capabilities: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerRejection {
    UnauthenticatedTransport,
    InvalidAuditToken,
    InvalidCodeSignature,
    DesignatedRequirementNotSatisfied,
    WrongRole,
    WrongUid,
    MissingExecutableId,
    WrongExecutableId,
    MissingTeamId,
    WrongTeamId,
    MissingDesignatedRequirement,
    WrongDesignatedRequirement,
    IdentityFactTooLong,
    BuildNotAllowed,
    WrongProtocolMajor,
    ProtocolMinorTooOld,
    ProtocolMinorTooNew,
    MissingCapability,
    InvalidPolicy,
}

impl PeerPolicy {
    /// Evaluate independently authenticated facts. Every required fact must
    /// match exactly; no ad-hoc, unsigned, downgraded, or partial identity is
    /// accepted.
    pub fn evaluate(&self, peer: &AuthenticatedPeerFacts) -> Result<(), PeerRejection> {
        if self.expected_executable_id.is_empty()
            || self.expected_team_id.is_empty()
            || self.expected_designated_requirement.is_empty()
            || self.allowed_build_ids.is_empty()
            || self.minimum_protocol_minor > self.maximum_protocol_minor
            || self.expected_executable_id.len() > MAX_SIGNING_IDENTITY_BYTES
            || self.expected_team_id.len() > MAX_SIGNING_IDENTITY_BYTES
            || self.expected_designated_requirement.len() > MAX_SIGNING_IDENTITY_BYTES
        {
            return Err(PeerRejection::InvalidPolicy);
        }
        if !peer.local_transport_authenticated {
            return Err(PeerRejection::UnauthenticatedTransport);
        }
        if !peer.audit_token_validated {
            return Err(PeerRejection::InvalidAuditToken);
        }
        if !peer.code_signature_valid {
            return Err(PeerRejection::InvalidCodeSignature);
        }
        if !peer.designated_requirement_satisfied {
            return Err(PeerRejection::DesignatedRequirementNotSatisfied);
        }
        if peer.role != self.expected_role {
            return Err(PeerRejection::WrongRole);
        }
        if peer.uid != self.expected_uid {
            return Err(PeerRejection::WrongUid);
        }
        if peer.executable_id.is_empty() {
            return Err(PeerRejection::MissingExecutableId);
        }
        if peer.team_id.is_empty() {
            return Err(PeerRejection::MissingTeamId);
        }
        if peer.designated_requirement.is_empty() {
            return Err(PeerRejection::MissingDesignatedRequirement);
        }
        if peer.executable_id.len() > MAX_SIGNING_IDENTITY_BYTES
            || peer.team_id.len() > MAX_SIGNING_IDENTITY_BYTES
            || peer.designated_requirement.len() > MAX_SIGNING_IDENTITY_BYTES
        {
            return Err(PeerRejection::IdentityFactTooLong);
        }
        if peer.executable_id != self.expected_executable_id {
            return Err(PeerRejection::WrongExecutableId);
        }
        if peer.team_id != self.expected_team_id {
            return Err(PeerRejection::WrongTeamId);
        }
        if peer.designated_requirement != self.expected_designated_requirement {
            return Err(PeerRejection::WrongDesignatedRequirement);
        }
        if !self.allowed_build_ids.contains(&peer.executable_build_id) {
            return Err(PeerRejection::BuildNotAllowed);
        }
        if peer.protocol.major != self.protocol_major {
            return Err(PeerRejection::WrongProtocolMajor);
        }
        if peer.protocol.minor < self.minimum_protocol_minor {
            return Err(PeerRejection::ProtocolMinorTooOld);
        }
        if peer.protocol.minor > self.maximum_protocol_minor {
            return Err(PeerRejection::ProtocolMinorTooNew);
        }
        if peer.capabilities & self.required_capabilities != self.required_capabilities {
            return Err(PeerRejection::MissingCapability);
        }
        Ok(())
    }
}

/// Connection-scoped proof produced only after the platform peer identity has
/// passed [`PeerPolicy`]. It binds requests to both live component
/// incarnations and rejects replay before policy code sees a command.
#[derive(Debug, Eq, PartialEq)]
pub struct ConnectionBinding {
    authenticated_build_id: BuildId,
    helper_boot_nonce: [u8; 16],
    engine_incarnation: [u8; 16],
    channel_nonce: [u8; 16],
    last_sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestRejection {
    InvalidBinding,
    WrongBuild,
    StaleHelperIncarnation,
    StaleEngineIncarnation,
    StaleChannel,
    Replay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseRejection {
    StaleHelperIncarnation,
    StaleEngineIncarnation,
    StaleChannel,
    WrongSequence,
}

impl ConnectionBinding {
    pub fn authenticate(
        policy: &PeerPolicy,
        peer: &AuthenticatedPeerFacts,
        helper_boot_nonce: [u8; 16],
        engine_incarnation: [u8; 16],
        channel_nonce: [u8; 16],
    ) -> Result<Self, PeerRejection> {
        policy.evaluate(peer)?;
        if helper_boot_nonce == [0; 16] || engine_incarnation == [0; 16] || channel_nonce == [0; 16]
        {
            return Err(PeerRejection::InvalidPolicy);
        }
        Ok(Self {
            authenticated_build_id: peer.executable_build_id,
            helper_boot_nonce,
            engine_incarnation,
            channel_nonce,
            last_sequence: 0,
        })
    }

    pub fn authorize(&mut self, request: &Request) -> Result<(), RequestRejection> {
        if self.helper_boot_nonce == [0; 16] || self.engine_incarnation == [0; 16] {
            return Err(RequestRejection::InvalidBinding);
        }
        if request.client_build_id != self.authenticated_build_id {
            return Err(RequestRejection::WrongBuild);
        }
        if request.helper_boot_nonce != self.helper_boot_nonce {
            return Err(RequestRejection::StaleHelperIncarnation);
        }
        if request.engine_incarnation != self.engine_incarnation {
            return Err(RequestRejection::StaleEngineIncarnation);
        }
        if request.channel_nonce != self.channel_nonce {
            return Err(RequestRejection::StaleChannel);
        }
        if request.sequence == 0 || request.sequence <= self.last_sequence {
            return Err(RequestRejection::Replay);
        }
        self.last_sequence = request.sequence;
        Ok(())
    }

    /// Client-side response binding. Construct this binding only after applying
    /// a Helper-role [`PeerPolicy`] to audit-token/code-signing facts.
    pub fn authorize_response(
        &self,
        response: &Response,
        expected_sequence: u64,
    ) -> Result<(), ResponseRejection> {
        if response.helper_boot_nonce != self.helper_boot_nonce {
            return Err(ResponseRejection::StaleHelperIncarnation);
        }
        if response.engine_incarnation != self.engine_incarnation {
            return Err(ResponseRejection::StaleEngineIncarnation);
        }
        if response.channel_nonce != self.channel_nonce {
            return Err(ResponseRejection::StaleChannel);
        }
        if expected_sequence == 0 || response.request_sequence != expected_sequence {
            return Err(ResponseRejection::WrongSequence);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(byte: u8) -> BuildId {
        BuildId([byte; 32])
    }

    fn policy() -> PeerPolicy {
        PeerPolicy {
            expected_role: PeerRole::Engine,
            expected_uid: 501,
            expected_executable_id: "com.zeus.engine".into(),
            expected_team_id: "TEAM".into(),
            expected_designated_requirement: "anchor apple generic and identifier zeus".into(),
            allowed_build_ids: vec![build(7)],
            protocol_major: 1,
            minimum_protocol_minor: 2,
            maximum_protocol_minor: 4,
            required_capabilities: 0b11,
        }
    }

    fn peer() -> AuthenticatedPeerFacts {
        AuthenticatedPeerFacts {
            local_transport_authenticated: true,
            audit_token_validated: true,
            code_signature_valid: true,
            designated_requirement_satisfied: true,
            role: PeerRole::Engine,
            uid: 501,
            executable_id: "com.zeus.engine".into(),
            team_id: "TEAM".into(),
            designated_requirement: "anchor apple generic and identifier zeus".into(),
            executable_build_id: build(7),
            protocol: ProtocolVersion { major: 1, minor: 3 },
            capabilities: 0b11,
        }
    }

    #[test]
    fn accepts_only_complete_exact_identity() {
        assert_eq!(policy().evaluate(&peer()), Ok(()));

        type PeerMutation = Box<dyn Fn(&mut AuthenticatedPeerFacts)>;
        let cases: Vec<(PeerMutation, PeerRejection)> = vec![
            (
                Box::new(|p| p.local_transport_authenticated = false),
                PeerRejection::UnauthenticatedTransport,
            ),
            (
                Box::new(|p| p.audit_token_validated = false),
                PeerRejection::InvalidAuditToken,
            ),
            (
                Box::new(|p| p.code_signature_valid = false),
                PeerRejection::InvalidCodeSignature,
            ),
            (
                Box::new(|p| p.designated_requirement_satisfied = false),
                PeerRejection::DesignatedRequirementNotSatisfied,
            ),
            (
                Box::new(|p| p.role = PeerRole::Helper),
                PeerRejection::WrongRole,
            ),
            (Box::new(|p| p.uid = 502), PeerRejection::WrongUid),
            (
                Box::new(|p| p.executable_id = "com.attacker".into()),
                PeerRejection::WrongExecutableId,
            ),
            (
                Box::new(|p| p.team_id = "OTHER".into()),
                PeerRejection::WrongTeamId,
            ),
            (
                Box::new(|p| p.designated_requirement = "other".into()),
                PeerRejection::WrongDesignatedRequirement,
            ),
            (
                Box::new(|p| p.executable_build_id = build(8)),
                PeerRejection::BuildNotAllowed,
            ),
            (
                Box::new(|p| p.protocol.major = 2),
                PeerRejection::WrongProtocolMajor,
            ),
            (
                Box::new(|p| p.protocol.minor = 1),
                PeerRejection::ProtocolMinorTooOld,
            ),
            (
                Box::new(|p| p.protocol.minor = 5),
                PeerRejection::ProtocolMinorTooNew,
            ),
            (
                Box::new(|p| p.capabilities = 0b01),
                PeerRejection::MissingCapability,
            ),
        ];

        for (mutate, expected) in cases {
            let mut candidate = peer();
            mutate(&mut candidate);
            assert_eq!(policy().evaluate(&candidate), Err(expected));
        }
    }

    #[test]
    fn missing_and_unbounded_signing_facts_fail_closed() {
        let mut candidate = peer();
        candidate.team_id.clear();
        assert_eq!(
            policy().evaluate(&candidate),
            Err(PeerRejection::MissingTeamId)
        );

        candidate = peer();
        candidate.designated_requirement = "x".repeat(MAX_SIGNING_IDENTITY_BYTES + 1);
        assert_eq!(
            policy().evaluate(&candidate),
            Err(PeerRejection::IdentityFactTooLong)
        );
    }

    fn request(sequence: u64) -> Request {
        Request {
            client_build_id: build(7),
            helper_boot_nonce: [3; 16],
            engine_incarnation: [4; 16],
            channel_nonce: [5; 16],
            sequence,
            command: crate::wire::RequestCommand::Status,
        }
    }

    #[test]
    fn connection_binding_rejects_replay_and_stale_incarnations() {
        let mut binding =
            ConnectionBinding::authenticate(&policy(), &peer(), [3; 16], [4; 16], [5; 16])
                .expect("exact peer binds");
        assert_eq!(binding.authorize(&request(1)), Ok(()));
        assert_eq!(
            binding.authorize(&request(1)),
            Err(RequestRejection::Replay)
        );

        let mut stale_helper = request(2);
        stale_helper.helper_boot_nonce = [9; 16];
        assert_eq!(
            binding.authorize(&stale_helper),
            Err(RequestRejection::StaleHelperIncarnation)
        );

        let mut stale_engine = request(2);
        stale_engine.engine_incarnation = [9; 16];
        assert_eq!(
            binding.authorize(&stale_engine),
            Err(RequestRejection::StaleEngineIncarnation)
        );

        let mut stale_channel = request(2);
        stale_channel.channel_nonce = [9; 16];
        assert_eq!(
            binding.authorize(&stale_channel),
            Err(RequestRejection::StaleChannel)
        );

        let mut wrong_build = request(2);
        wrong_build.client_build_id = build(8);
        assert_eq!(
            binding.authorize(&wrong_build),
            Err(RequestRejection::WrongBuild)
        );
        assert_eq!(binding.authorize(&request(2)), Ok(()));

        let response = Response {
            helper_boot_nonce: [3; 16],
            engine_incarnation: [4; 16],
            channel_nonce: [5; 16],
            request_sequence: 2,
            body: crate::wire::ResponseBody::Ack(crate::wire::AckKind::Renewed),
        };
        assert_eq!(binding.authorize_response(&response, 2), Ok(()));
        assert_eq!(
            binding.authorize_response(&response, 1),
            Err(ResponseRejection::WrongSequence)
        );
        let mut stale_response = response;
        stale_response.channel_nonce = [9; 16];
        assert_eq!(
            binding.authorize_response(&stale_response, 2),
            Err(ResponseRejection::StaleChannel)
        );
    }
}
