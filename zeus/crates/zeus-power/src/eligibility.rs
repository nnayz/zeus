//! Exact execution identity and the engine-owned eligibility reducer.

/// A stable Zeus identifier. It is intentionally opaque to the power helper.
pub type OpaqueId = [u8; 16];

/// OS process identity that remains exact across PID reuse.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Platform-provided process birth token (for example, process start time).
    pub birth_token: u64,
}

impl ProcessIdentity {
    fn is_valid(self) -> bool {
        self.pid != 0 && self.birth_token != 0
    }
}

/// An exact, local execution selected by the Engine.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LocalExecutionIdentity {
    /// Work executes directly in the identified process.
    Direct {
        session_id: OpaqueId,
        incarnation: OpaqueId,
        host_boot_id: OpaqueId,
        execution_generation: u64,
        process: ProcessIdentity,
    },
    /// Work is held by a local Holder and runs in its identified child.
    Held {
        session_id: OpaqueId,
        incarnation: OpaqueId,
        host_boot_id: OpaqueId,
        execution_generation: u64,
        holder: ProcessIdentity,
        child: ProcessIdentity,
    },
}

impl LocalExecutionIdentity {
    pub fn session_id(&self) -> OpaqueId {
        match self {
            Self::Direct { session_id, .. } | Self::Held { session_id, .. } => *session_id,
        }
    }

    pub fn incarnation(&self) -> OpaqueId {
        match self {
            Self::Direct { incarnation, .. } | Self::Held { incarnation, .. } => *incarnation,
        }
    }

    fn is_valid(&self) -> bool {
        let (host_boot_id, execution_generation) = match self {
            Self::Direct {
                host_boot_id,
                execution_generation,
                ..
            }
            | Self::Held {
                host_boot_id,
                execution_generation,
                ..
            } => (*host_boot_id, *execution_generation),
        };
        self.session_id() != [0; 16]
            && self.incarnation() != [0; 16]
            && host_boot_id != [0; 16]
            && execution_generation != 0
            && match self {
                Self::Direct { process, .. } => process.is_valid(),
                Self::Held { holder, child, .. } => holder.is_valid() && child.is_valid(),
            }
    }
}

/// Where, or whether, a session currently executes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionRoute {
    Remote,
    /// Selected work exists, but no local execution has started.
    Deferred,
    Direct(LocalExecutionIdentity),
    Held(LocalExecutionIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HibernationState {
    Awake,
    Hibernating,
    Hibernated,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Liveness {
    Alive,
    Dead,
    Unknown,
}

/// UI/status reduction is diagnostic input, not execution identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReducedStatus {
    Working,
    Idle,
    WaitingForUser,
    Finished,
    Failed,
    Unknown,
}

/// Engine facts for one selected or unselected session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionFacts {
    pub explicitly_selected: bool,
    pub route: ExecutionRoute,
    pub hibernation: HibernationState,
    pub primary_liveness: Liveness,
    /// Required for `Held`; ignored for other routes.
    pub child_liveness: Liveness,
    pub reduced_status: ReducedStatus,
}

/// A capability produced only after all identity and liveness checks pass.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EligibleExecution(LocalExecutionIdentity);

impl EligibleExecution {
    pub fn identity(&self) -> &LocalExecutionIdentity {
        &self.0
    }

    pub fn into_identity(self) -> LocalExecutionIdentity {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IneligibleReason {
    NotSelected,
    Remote,
    Deferred,
    DirectUnsupported,
    Hibernating,
    Hibernated,
    UnknownHibernation,
    InvalidIdentity,
    PrimaryDead,
    PrimaryLivenessUnknown,
    ChildDead,
    ChildLivenessUnknown,
    RouteIdentityMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EligibilityDecision {
    pub result: Result<EligibleExecution, IneligibleReason>,
    /// Preserved for explanation only. It never proves execution or liveness.
    pub observed_status: ReducedStatus,
}

/// Reduce Engine-owned facts to an exact local execution.
///
/// Status is deliberately not a gate. An Idle/Waiting status can cover a long
/// child command, while Finished can race with process-exit observation. Exact
/// process liveness and incarnation are the authority.
pub fn reduce_eligibility(facts: ExecutionFacts) -> EligibilityDecision {
    let result = if !facts.explicitly_selected {
        Err(IneligibleReason::NotSelected)
    } else {
        match facts.hibernation {
            HibernationState::Awake => match facts.route {
                ExecutionRoute::Remote => Err(IneligibleReason::Remote),
                ExecutionRoute::Deferred => Err(IneligibleReason::Deferred),
                ExecutionRoute::Direct(identity) => {
                    if !matches!(identity, LocalExecutionIdentity::Direct { .. }) {
                        Err(IneligibleReason::RouteIdentityMismatch)
                    } else {
                        // Proposed v1 requires a Holder/child identity so the
                        // Engine can prove stable execution across GUI lifetime.
                        Err(IneligibleReason::DirectUnsupported)
                    }
                }
                ExecutionRoute::Held(identity) => {
                    if !matches!(identity, LocalExecutionIdentity::Held { .. }) {
                        Err(IneligibleReason::RouteIdentityMismatch)
                    } else if !identity.is_valid() {
                        Err(IneligibleReason::InvalidIdentity)
                    } else {
                        require_alive(facts.primary_liveness, false)
                            .and_then(|()| require_alive(facts.child_liveness, true))
                            .map(|()| EligibleExecution(identity))
                    }
                }
            },
            HibernationState::Hibernating => Err(IneligibleReason::Hibernating),
            HibernationState::Hibernated => Err(IneligibleReason::Hibernated),
            HibernationState::Unknown => Err(IneligibleReason::UnknownHibernation),
        }
    };

    EligibilityDecision {
        result,
        observed_status: facts.reduced_status,
    }
}

fn require_alive(liveness: Liveness, child: bool) -> Result<(), IneligibleReason> {
    match (liveness, child) {
        (Liveness::Alive, _) => Ok(()),
        (Liveness::Dead, false) => Err(IneligibleReason::PrimaryDead),
        (Liveness::Unknown, false) => Err(IneligibleReason::PrimaryLivenessUnknown),
        (Liveness::Dead, true) => Err(IneligibleReason::ChildDead),
        (Liveness::Unknown, true) => Err(IneligibleReason::ChildLivenessUnknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> LocalExecutionIdentity {
        LocalExecutionIdentity::Held {
            session_id: [1; 16],
            incarnation: [2; 16],
            host_boot_id: [3; 16],
            execution_generation: 1,
            holder: ProcessIdentity {
                pid: 10,
                birth_token: 100,
            },
            child: ProcessIdentity {
                pid: 11,
                birth_token: 101,
            },
        }
    }

    fn facts() -> ExecutionFacts {
        ExecutionFacts {
            explicitly_selected: true,
            route: ExecutionRoute::Held(id()),
            hibernation: HibernationState::Awake,
            primary_liveness: Liveness::Alive,
            child_liveness: Liveness::Alive,
            reduced_status: ReducedStatus::Working,
        }
    }

    #[test]
    fn only_exact_live_local_execution_is_eligible() {
        assert!(reduce_eligibility(facts()).result.is_ok());

        let mut remote = facts();
        remote.route = ExecutionRoute::Remote;
        assert_eq!(
            reduce_eligibility(remote).result,
            Err(IneligibleReason::Remote)
        );

        let mut deferred = facts();
        deferred.route = ExecutionRoute::Deferred;
        assert_eq!(
            reduce_eligibility(deferred).result,
            Err(IneligibleReason::Deferred)
        );

        let mut dead = facts();
        dead.child_liveness = Liveness::Dead;
        assert_eq!(
            reduce_eligibility(dead).result,
            Err(IneligibleReason::ChildDead)
        );
    }

    #[test]
    fn hibernation_and_unknown_state_fail_closed() {
        for (state, reason) in [
            (HibernationState::Hibernating, IneligibleReason::Hibernating),
            (HibernationState::Hibernated, IneligibleReason::Hibernated),
            (
                HibernationState::Unknown,
                IneligibleReason::UnknownHibernation,
            ),
        ] {
            let mut input = facts();
            input.hibernation = state;
            assert_eq!(reduce_eligibility(input).result, Err(reason));
        }
    }

    #[test]
    fn status_neither_proves_nor_disproves_execution() {
        for status in [
            ReducedStatus::Working,
            ReducedStatus::Idle,
            ReducedStatus::WaitingForUser,
            ReducedStatus::Finished,
            ReducedStatus::Failed,
            ReducedStatus::Unknown,
        ] {
            let mut input = facts();
            input.reduced_status = status;
            let decision = reduce_eligibility(input);
            assert!(decision.result.is_ok());
            assert_eq!(decision.observed_status, status);
        }

        let mut dead_but_working = facts();
        dead_but_working.primary_liveness = Liveness::Dead;
        assert_eq!(
            reduce_eligibility(dead_but_working).result,
            Err(IneligibleReason::PrimaryDead)
        );
    }

    #[test]
    fn rejects_pid_reuse_unsafe_and_mismatched_identity() {
        let mut invalid = facts();
        invalid.route = ExecutionRoute::Held(LocalExecutionIdentity::Held {
            session_id: [1; 16],
            incarnation: [2; 16],
            host_boot_id: [3; 16],
            execution_generation: 1,
            holder: ProcessIdentity {
                pid: 10,
                birth_token: 0,
            },
            child: ProcessIdentity {
                pid: 11,
                birth_token: 101,
            },
        });
        assert_eq!(
            reduce_eligibility(invalid).result,
            Err(IneligibleReason::InvalidIdentity)
        );

        let mut mismatch = facts();
        mismatch.route = ExecutionRoute::Direct(id());
        assert_eq!(
            reduce_eligibility(mismatch).result,
            Err(IneligibleReason::RouteIdentityMismatch)
        );
    }
}
