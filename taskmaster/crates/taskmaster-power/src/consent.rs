//! Engine-owned explicit consent bound to exact local execution generations.
//!
//! This reducer is deliberately platform neutral. It does not contact the power
//! Helper and it does not persist consent. A new Engine incarnation starts with
//! no grants. Reconciliation can only remove grants; adding or replacing a grant
//! requires a new explicit user-authorized generation.

use std::collections::{BTreeMap, BTreeSet};

use crate::eligibility::{EligibleExecution, LocalExecutionIdentity};
use crate::lease::MonotonicTime;

/// One explicit authorization for one exact local execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsentGrant {
    engine_incarnation: [u8; 16],
    consent_generation: u64,
    consent_nonce: [u8; 16],
    execution: LocalExecutionIdentity,
    granted_at: MonotonicTime,
    deadline: MonotonicTime,
}

impl ConsentGrant {
    pub fn engine_incarnation(&self) -> [u8; 16] {
        self.engine_incarnation
    }

    pub fn consent_generation(&self) -> u64 {
        self.consent_generation
    }

    pub fn consent_nonce(&self) -> [u8; 16] {
        self.consent_nonce
    }

    pub fn execution(&self) -> &LocalExecutionIdentity {
        &self.execution
    }

    pub fn granted_at(&self) -> MonotonicTime {
        self.granted_at
    }

    pub fn deadline(&self) -> MonotonicTime {
        self.deadline
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConsentPolicy {
    pub maximum_duration_millis: u64,
    pub maximum_selected_executions: usize,
}

impl Default for ConsentPolicy {
    fn default() -> Self {
        Self {
            maximum_duration_millis: 8 * 60 * 60 * 1_000,
            maximum_selected_executions: crate::wire::MAX_EXECUTIONS_PER_REQUEST,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsentError {
    InvalidPolicy,
    InvalidEngineIncarnation,
    ClockRegression,
    GenerationNotIncreasing,
    InvalidConsentNonce,
    EmptySelection,
    TooManyExecutions,
    DeadlineNotFuture,
    DeadlineTooFar,
    DuplicateExecution,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsentRemovalReason {
    UserRevoked,
    ExecutionChanged,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsentReconciliation {
    pub active: Vec<ConsentGrant>,
    pub removed: Vec<(ConsentGrant, ConsentRemovalReason)>,
}

/// In-memory authority for the current Engine process.
///
/// Consent is intentionally not serializable. Engine restart, adoption, wake,
/// respawn, and queued input cannot recreate it. The caller must construct a new
/// coordinator with a fresh incarnation and obtain a new explicit generation.
#[derive(Clone, Debug)]
pub struct ConsentCoordinator {
    policy: ConsentPolicy,
    engine_incarnation: [u8; 16],
    last_now: MonotonicTime,
    last_consent_generation: u64,
    grants: BTreeMap<LocalExecutionIdentity, ConsentGrant>,
}

impl ConsentCoordinator {
    pub fn new(
        policy: ConsentPolicy,
        engine_incarnation: [u8; 16],
        now: MonotonicTime,
    ) -> Result<Self, ConsentError> {
        if policy.maximum_duration_millis == 0 || policy.maximum_selected_executions == 0 {
            return Err(ConsentError::InvalidPolicy);
        }
        if engine_incarnation == [0; 16] {
            return Err(ConsentError::InvalidEngineIncarnation);
        }
        Ok(Self {
            policy,
            engine_incarnation,
            last_now: now,
            last_consent_generation: 0,
            grants: BTreeMap::new(),
        })
    }

    pub fn engine_incarnation(&self) -> [u8; 16] {
        self.engine_incarnation
    }

    #[cfg(test)]
    fn grant_count(&self) -> usize {
        self.grants.len()
    }

    /// Replaces the complete selection after a fresh explicit user action.
    ///
    /// The generation is issued by the genuine desktop-to-Engine authorization
    /// path. It must increase even after revocation, so a delayed request cannot
    /// silently re-arm a cleared selection.
    pub fn authorize_selection(
        &mut self,
        now: MonotonicTime,
        consent_generation: u64,
        consent_nonce: [u8; 16],
        deadline: MonotonicTime,
        executions: impl IntoIterator<Item = EligibleExecution>,
    ) -> Result<(), ConsentError> {
        self.advance(now)?;
        if consent_generation == 0 || consent_generation <= self.last_consent_generation {
            return Err(ConsentError::GenerationNotIncreasing);
        }
        if consent_nonce == [0; 16] {
            return Err(ConsentError::InvalidConsentNonce);
        }
        if deadline <= now {
            return Err(ConsentError::DeadlineNotFuture);
        }
        let maximum_deadline = now
            .0
            .checked_add(self.policy.maximum_duration_millis)
            .ok_or(ConsentError::DeadlineTooFar)?;
        if deadline.0 > maximum_deadline {
            return Err(ConsentError::DeadlineTooFar);
        }

        let identities: Vec<_> = executions
            .into_iter()
            .map(EligibleExecution::into_identity)
            .collect();
        if identities.is_empty() {
            return Err(ConsentError::EmptySelection);
        }
        if identities.len() > self.policy.maximum_selected_executions {
            return Err(ConsentError::TooManyExecutions);
        }
        let unique: BTreeSet<_> = identities.iter().cloned().collect();
        if unique.len() != identities.len() {
            return Err(ConsentError::DuplicateExecution);
        }

        // Consume the generation only after the full request validates. From
        // this point the replacement is one in-memory atomic state transition.
        self.last_consent_generation = consent_generation;
        self.grants = unique
            .into_iter()
            .map(|execution| {
                let grant = ConsentGrant {
                    engine_incarnation: self.engine_incarnation,
                    consent_generation,
                    consent_nonce,
                    execution: execution.clone(),
                    granted_at: now,
                    deadline,
                };
                (execution, grant)
            })
            .collect();
        Ok(())
    }

    /// Revokes all grants and consumes an authenticated action generation.
    ///
    /// The genuine desktop-to-Engine action path must allocate the generation
    /// in order. Consuming it fences delayed authorize messages at or below the
    /// revocation action, so Allow Sleep Now cannot be undone by queued work.
    pub fn revoke_all(
        &mut self,
        now: MonotonicTime,
        revocation_generation: u64,
    ) -> Result<Vec<(ConsentGrant, ConsentRemovalReason)>, ConsentError> {
        self.advance(now)?;
        if revocation_generation == 0 || revocation_generation <= self.last_consent_generation {
            return Err(ConsentError::GenerationNotIncreasing);
        }
        self.last_consent_generation = revocation_generation;
        Ok(std::mem::take(&mut self.grants)
            .into_values()
            .map(|grant| (grant, ConsentRemovalReason::UserRevoked))
            .collect())
    }

    /// Retains only still-eligible exact identities and unexpired grants.
    ///
    /// This method can never add consent. A changed Holder incarnation, process
    /// birth token, execution generation, or session identity removes the old
    /// grant and requires another explicit authorization.
    pub fn reconcile(
        &mut self,
        now: MonotonicTime,
        eligible: impl IntoIterator<Item = EligibleExecution>,
    ) -> Result<ConsentReconciliation, ConsentError> {
        self.advance(now)?;
        let eligible: BTreeSet<_> = eligible
            .into_iter()
            .map(EligibleExecution::into_identity)
            .collect();
        let mut removed = Vec::new();
        self.grants.retain(|identity, grant| {
            let reason = if grant.deadline <= now {
                Some(ConsentRemovalReason::Expired)
            } else if !eligible.contains(identity) {
                Some(ConsentRemovalReason::ExecutionChanged)
            } else {
                None
            };
            if let Some(reason) = reason {
                removed.push((grant.clone(), reason));
                false
            } else {
                true
            }
        });
        Ok(ConsentReconciliation {
            active: self.grants.values().cloned().collect(),
            removed,
        })
    }

    fn advance(&mut self, now: MonotonicTime) -> Result<(), ConsentError> {
        if now < self.last_now {
            // A clock-domain failure invalidates every deadline. Do not expose
            // the prior grants after returning an error.
            self.grants.clear();
            return Err(ConsentError::ClockRegression);
        }
        self.last_now = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eligibility::{
        ExecutionFacts, ExecutionRoute, HibernationState, Liveness, ProcessIdentity, ReducedStatus,
        reduce_eligibility,
    };

    fn eligible(execution_generation: u64, child_birth_token: u64) -> EligibleExecution {
        let identity = LocalExecutionIdentity::Held {
            session_id: [1; 16],
            incarnation: [2; 16],
            host_boot_id: [3; 16],
            execution_generation,
            holder: ProcessIdentity {
                pid: 10,
                birth_token: 100,
            },
            child: ProcessIdentity {
                pid: 11,
                birth_token: child_birth_token,
            },
        };
        reduce_eligibility(ExecutionFacts {
            explicitly_selected: true,
            route: ExecutionRoute::Held(identity),
            hibernation: HibernationState::Awake,
            primary_liveness: Liveness::Alive,
            child_liveness: Liveness::Alive,
            reduced_status: ReducedStatus::Working,
        })
        .result
        .expect("exact live held execution")
    }

    fn coordinator() -> ConsentCoordinator {
        ConsentCoordinator::new(
            ConsentPolicy {
                maximum_duration_millis: 1_000,
                maximum_selected_executions: 2,
            },
            [9; 16],
            MonotonicTime(100),
        )
        .unwrap()
    }

    #[test]
    fn explicit_generation_is_bound_to_exact_execution() {
        let mut coordinator = coordinator();
        coordinator
            .authorize_selection(
                MonotonicTime(100),
                1,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();
        let grant = coordinator
            .reconcile(MonotonicTime(100), [eligible(7, 101)])
            .unwrap()
            .active
            .pop()
            .unwrap();
        assert_eq!(grant.engine_incarnation(), [9; 16]);
        assert_eq!(grant.consent_generation(), 1);

        let result = coordinator
            .reconcile(MonotonicTime(200), [eligible(8, 101)])
            .unwrap();
        assert_eq!(result.removed.len(), 1);
        assert_eq!(result.removed[0].1, ConsentRemovalReason::ExecutionChanged);
        assert!(result.active.is_empty());
    }

    #[test]
    fn pid_reuse_and_respawn_cannot_inherit_consent() {
        let mut coordinator = coordinator();
        coordinator
            .authorize_selection(
                MonotonicTime(100),
                1,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();
        let result = coordinator
            .reconcile(MonotonicTime(200), [eligible(7, 202)])
            .unwrap();
        assert_eq!(result.removed[0].1, ConsentRemovalReason::ExecutionChanged);
        assert!(result.active.is_empty());
    }

    #[test]
    fn revoke_and_restart_require_fresh_explicit_consent() {
        let mut coordinator = coordinator();
        coordinator
            .authorize_selection(
                MonotonicTime(100),
                4,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();
        assert_eq!(
            coordinator.revoke_all(MonotonicTime(101), 5).unwrap().len(),
            1
        );
        assert_eq!(
            coordinator.authorize_selection(
                MonotonicTime(101),
                5,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            ),
            Err(ConsentError::GenerationNotIncreasing)
        );
        coordinator
            .authorize_selection(
                MonotonicTime(101),
                6,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();

        let restarted =
            ConsentCoordinator::new(ConsentPolicy::default(), [8; 16], MonotonicTime(101)).unwrap();
        assert_eq!(restarted.grant_count(), 0);
        assert_ne!(
            restarted.engine_incarnation(),
            coordinator.engine_incarnation()
        );
    }

    #[test]
    fn expiry_and_bounds_fail_closed() {
        let mut coordinator = coordinator();
        assert_eq!(
            coordinator.authorize_selection(
                MonotonicTime(100),
                1,
                [6; 16],
                MonotonicTime(1_101),
                [eligible(7, 101)],
            ),
            Err(ConsentError::DeadlineTooFar)
        );
        coordinator
            .authorize_selection(
                MonotonicTime(100),
                1,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();
        let result = coordinator
            .reconcile(MonotonicTime(500), [eligible(7, 101)])
            .unwrap();
        assert_eq!(result.removed[0].1, ConsentRemovalReason::Expired);
        assert!(result.active.is_empty());
    }

    #[test]
    fn clock_regression_clears_every_grant() {
        let mut coordinator = coordinator();
        coordinator
            .authorize_selection(
                MonotonicTime(100),
                1,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();
        assert_eq!(
            coordinator.reconcile(MonotonicTime(99), [eligible(7, 101)]),
            Err(ConsentError::ClockRegression)
        );
        assert_eq!(coordinator.grant_count(), 0);
    }

    #[test]
    fn invalid_requests_do_not_replace_existing_selection() {
        let mut coordinator = coordinator();
        coordinator
            .authorize_selection(
                MonotonicTime(100),
                1,
                [6; 16],
                MonotonicTime(500),
                [eligible(7, 101)],
            )
            .unwrap();
        assert_eq!(
            coordinator.authorize_selection(
                MonotonicTime(101),
                2,
                [6; 16],
                MonotonicTime(500),
                [eligible(8, 101), eligible(8, 101)],
            ),
            Err(ConsentError::DuplicateExecution)
        );
        assert_eq!(coordinator.grant_count(), 1);
        let active = coordinator
            .reconcile(MonotonicTime(101), [eligible(7, 101)])
            .unwrap()
            .active;
        assert_eq!(active[0].consent_generation(), 1);
    }
}
