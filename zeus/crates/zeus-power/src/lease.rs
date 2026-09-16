//! Monotonic lease aggregation, explicit arming, and fail-closed safety policy.

use std::collections::BTreeSet;

use crate::consent::ConsentGrant;
use crate::eligibility::LocalExecutionIdentity;

/// Milliseconds from one boot-scoped continuous monotonic clock that advances
/// through system sleep (for example, `mach_continuous_time` on macOS).
/// Values from another boot or clock domain must never enter one coordinator.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicTime(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observed<T> {
    pub value: T,
    pub sampled_at: MonotonicTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerSource {
    Ac,
    Battery,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LidState {
    Open,
    Closed,
}

/// Each signal has independent freshness. `None` is unreadable and fails closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafetyObservations {
    pub power_source: Option<Observed<PowerSource>>,
    pub battery_percent: Option<Observed<u8>>,
    pub thermal: Option<Observed<ThermalState>>,
    pub lid: Option<Observed<LidState>>,
    pub helper_healthy: Option<Observed<bool>>,
    pub emergency_sleep: Option<Observed<bool>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafetyFailure {
    MissingPowerSource,
    MissingBattery,
    MissingThermal,
    MissingLid,
    MissingHelperHealth,
    MissingEmergencyState,
    FutureSample,
    StaleSample,
    BatteryPower,
    InvalidBatteryPercent,
    ThermalUnsafe,
    HelperUnhealthy,
    EmergencySleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeasePolicy {
    pub maximum_signal_age: u64,
    pub maximum_renew_window: u64,
    pub maximum_total_duration: u64,
    pub maximum_selected_executions: usize,
}

impl Default for LeasePolicy {
    fn default() -> Self {
        Self {
            maximum_signal_age: 30_000,
            maximum_renew_window: 90_000,
            maximum_total_duration: 8 * 60 * 60 * 1_000,
            maximum_selected_executions: crate::wire::MAX_EXECUTIONS_PER_REQUEST,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseIntent {
    /// Capability issued only by the Engine consent coordinator after an
    /// explicit user action for this exact execution generation.
    pub grant: ConsentGrant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InactiveReason {
    NotArmed,
    NoEligibleExecutions,
    Expired,
    Safety(SafetyFailure),
    InvalidInput,
}

/// This is desired helper policy, not proof that host power state changed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseState {
    Inactive(InactiveReason),
    DesiredActive {
        executions: BTreeSet<LocalExecutionIdentity>,
        engine_incarnation: [u8; 16],
        consent_generation: u64,
        consent_nonce: [u8; 16],
        /// One atomic selection has one immutable original consent deadline.
        deadline: MonotonicTime,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinatorError {
    InvalidPolicy,
    ClockRegression,
    ArmEpochNotIncreasing,
    DeadlineNotFuture,
    DeadlineTooFar,
    TooManyExecutions,
    ConsentGenerationMismatch,
    MixedConsentSelection,
}

#[derive(Clone, Debug)]
pub struct LeaseCoordinator {
    policy: LeasePolicy,
    last_now: MonotonicTime,
    arm_epoch: Option<u64>,
    armed_at: Option<MonotonicTime>,
    allow_sleep_now_latched: bool,
    safety_trip_latched: bool,
    last_safety: Option<SafetyObservations>,
    state: LeaseState,
}

impl LeaseCoordinator {
    pub fn new(policy: LeasePolicy, now: MonotonicTime) -> Result<Self, CoordinatorError> {
        if policy.maximum_signal_age == 0
            || policy.maximum_renew_window == 0
            || policy.maximum_total_duration == 0
            || policy.maximum_selected_executions == 0
        {
            return Err(CoordinatorError::InvalidPolicy);
        }
        Ok(Self {
            policy,
            last_now: now,
            arm_epoch: None,
            armed_at: None,
            allow_sleep_now_latched: false,
            safety_trip_latched: false,
            last_safety: None,
            state: LeaseState::Inactive(InactiveReason::NotArmed),
        })
    }

    /// Explicit user arm. Epochs prevent queued/stale messages from clearing a latch.
    pub fn arm(&mut self, now: MonotonicTime, epoch: u64) -> Result<(), CoordinatorError> {
        self.advance(now)?;
        if self.arm_epoch.is_some_and(|current| epoch <= current) {
            return Err(CoordinatorError::ArmEpochNotIncreasing);
        }
        self.arm_epoch = Some(epoch);
        self.armed_at = Some(now);
        self.allow_sleep_now_latched = false;
        self.safety_trip_latched = false;
        self.state = LeaseState::Inactive(InactiveReason::NoEligibleExecutions);
        Ok(())
    }

    /// Immediate sticky release. Renewals cannot clear this latch.
    pub fn allow_sleep_now(&mut self) {
        self.allow_sleep_now_latched = true;
        self.state = LeaseState::Inactive(InactiveReason::NotArmed);
    }

    pub fn allow_sleep_now_latched(&self) -> bool {
        self.allow_sleep_now_latched
    }
    pub fn safety_trip_latched(&self) -> bool {
        self.safety_trip_latched
    }
    pub fn state(&self) -> &LeaseState {
        &self.state
    }

    /// Replaces the complete selected set and derives its maximum deadline.
    pub fn reconcile(
        &mut self,
        now: MonotonicTime,
        intents: &[LeaseIntent],
        safety: SafetyObservations,
    ) -> Result<&LeaseState, CoordinatorError> {
        if let Err(error) = self.advance(now) {
            self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
            return Err(error);
        }
        self.last_safety = Some(safety.clone());
        if self.arm_epoch.is_none() || self.allow_sleep_now_latched || self.safety_trip_latched {
            self.state = LeaseState::Inactive(InactiveReason::NotArmed);
            return Ok(&self.state);
        }
        if let Err(failure) = evaluate_safety(&safety, now, self.policy.maximum_signal_age) {
            self.safety_trip_latched = true;
            self.state = LeaseState::Inactive(InactiveReason::Safety(failure));
            return Ok(&self.state);
        }
        if intents.len() > self.policy.maximum_selected_executions {
            self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
            return Err(CoordinatorError::TooManyExecutions);
        }
        if intents.is_empty() {
            self.state = LeaseState::Inactive(InactiveReason::NoEligibleExecutions);
            return Ok(&self.state);
        }

        let renew_limit = now
            .0
            .checked_add(self.policy.maximum_renew_window)
            .ok_or(CoordinatorError::DeadlineTooFar)?;
        let total_limit = self
            .armed_at
            .expect("armed checked")
            .0
            .checked_add(self.policy.maximum_total_duration)
            .ok_or(CoordinatorError::DeadlineTooFar)?;
        let allowed_limit = renew_limit.min(total_limit);
        let expected_generation = self.arm_epoch.expect("armed checked");
        let first = &intents[0].grant;
        if first.consent_generation() != expected_generation {
            self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
            return Err(CoordinatorError::ConsentGenerationMismatch);
        }
        let engine_incarnation = first.engine_incarnation();
        let consent_nonce = first.consent_nonce();
        let deadline = first.deadline();
        if deadline <= now {
            self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
            return Err(CoordinatorError::DeadlineNotFuture);
        }
        if deadline.0 > allowed_limit {
            self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
            return Err(CoordinatorError::DeadlineTooFar);
        }

        let mut selected = BTreeSet::new();
        for intent in intents {
            let grant = &intent.grant;
            if grant.engine_incarnation() != engine_incarnation
                || grant.consent_generation() != expected_generation
                || grant.consent_nonce() != consent_nonce
                || grant.deadline() != deadline
            {
                self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
                return Err(CoordinatorError::MixedConsentSelection);
            }
            selected.insert(grant.execution().clone());
        }
        self.state = LeaseState::DesiredActive {
            executions: selected,
            engine_incarnation,
            consent_generation: expected_generation,
            consent_nonce,
            deadline,
        };
        Ok(&self.state)
    }

    /// Advances expiry/freshness without polling or renewing anything.
    pub fn tick(&mut self, now: MonotonicTime) -> Result<&LeaseState, CoordinatorError> {
        if let Err(error) = self.advance(now) {
            self.state = LeaseState::Inactive(InactiveReason::InvalidInput);
            return Err(error);
        }
        if self.allow_sleep_now_latched || self.safety_trip_latched {
            self.state = LeaseState::Inactive(InactiveReason::NotArmed);
            return Ok(&self.state);
        }
        if let Some(safety) = &self.last_safety
            && let Err(failure) = evaluate_safety(safety, now, self.policy.maximum_signal_age)
        {
            self.safety_trip_latched = true;
            self.state = LeaseState::Inactive(InactiveReason::Safety(failure));
            return Ok(&self.state);
        }
        if matches!(&self.state, LeaseState::DesiredActive { deadline, .. } if *deadline <= now) {
            self.state = LeaseState::Inactive(InactiveReason::Expired);
        }
        Ok(&self.state)
    }

    fn advance(&mut self, now: MonotonicTime) -> Result<(), CoordinatorError> {
        if now < self.last_now {
            return Err(CoordinatorError::ClockRegression);
        }
        self.last_now = now;
        Ok(())
    }
}

pub fn evaluate_safety(
    safety: &SafetyObservations,
    now: MonotonicTime,
    maximum_age: u64,
) -> Result<(), SafetyFailure> {
    let power = checked(
        safety.power_source,
        now,
        maximum_age,
        SafetyFailure::MissingPowerSource,
    )?;
    let battery = checked(
        safety.battery_percent,
        now,
        maximum_age,
        SafetyFailure::MissingBattery,
    )?;
    let thermal = checked(
        safety.thermal,
        now,
        maximum_age,
        SafetyFailure::MissingThermal,
    )?;
    let _lid = checked(safety.lid, now, maximum_age, SafetyFailure::MissingLid)?;
    let helper = checked(
        safety.helper_healthy,
        now,
        maximum_age,
        SafetyFailure::MissingHelperHealth,
    )?;
    let emergency = checked(
        safety.emergency_sleep,
        now,
        maximum_age,
        SafetyFailure::MissingEmergencyState,
    )?;

    if power != PowerSource::Ac {
        return Err(SafetyFailure::BatteryPower);
    }
    if battery > 100 {
        return Err(SafetyFailure::InvalidBatteryPercent);
    }
    if matches!(thermal, ThermalState::Serious | ThermalState::Critical) {
        return Err(SafetyFailure::ThermalUnsafe);
    }
    if !helper {
        return Err(SafetyFailure::HelperUnhealthy);
    }
    if emergency {
        return Err(SafetyFailure::EmergencySleep);
    }
    Ok(())
}

fn checked<T: Copy>(
    signal: Option<Observed<T>>,
    now: MonotonicTime,
    maximum_age: u64,
    missing: SafetyFailure,
) -> Result<T, SafetyFailure> {
    let signal = signal.ok_or(missing)?;
    if signal.sampled_at > now {
        return Err(SafetyFailure::FutureSample);
    }
    if now.0 - signal.sampled_at.0 > maximum_age {
        return Err(SafetyFailure::StaleSample);
    }
    Ok(signal.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{ConsentCoordinator, ConsentPolicy};
    use crate::eligibility::*;

    fn safety(at: u64) -> SafetyObservations {
        SafetyObservations {
            power_source: Some(Observed {
                value: PowerSource::Ac,
                sampled_at: MonotonicTime(at),
            }),
            battery_percent: Some(Observed {
                value: 80,
                sampled_at: MonotonicTime(at),
            }),
            thermal: Some(Observed {
                value: ThermalState::Nominal,
                sampled_at: MonotonicTime(at),
            }),
            lid: Some(Observed {
                value: LidState::Closed,
                sampled_at: MonotonicTime(at),
            }),
            helper_healthy: Some(Observed {
                value: true,
                sampled_at: MonotonicTime(at),
            }),
            emergency_sleep: Some(Observed {
                value: false,
                sampled_at: MonotonicTime(at),
            }),
        }
    }

    fn eligible(n: u8) -> EligibleExecution {
        reduce_eligibility(ExecutionFacts {
            explicitly_selected: true,
            route: ExecutionRoute::Held(LocalExecutionIdentity::Held {
                session_id: [n; 16],
                incarnation: [n + 1; 16],
                host_boot_id: [9; 16],
                execution_generation: u64::from(n),
                holder: ProcessIdentity {
                    pid: u32::from(n),
                    birth_token: 1,
                },
                child: ProcessIdentity {
                    pid: u32::from(n) + 100,
                    birth_token: 2,
                },
            }),
            hibernation: HibernationState::Awake,
            primary_liveness: Liveness::Alive,
            child_liveness: Liveness::Alive,
            reduced_status: ReducedStatus::Idle,
        })
        .result
        .unwrap()
    }

    fn intents(executions: &[u8], generation: u64, deadline: u64) -> Vec<LeaseIntent> {
        let mut consent =
            ConsentCoordinator::new(ConsentPolicy::default(), [8; 16], MonotonicTime(0)).unwrap();
        consent
            .authorize_selection(
                MonotonicTime(1),
                generation,
                [generation as u8; 16],
                MonotonicTime(deadline),
                executions.iter().copied().map(eligible),
            )
            .unwrap();
        consent
            .grants()
            .cloned()
            .map(|grant| LeaseIntent { grant })
            .collect()
    }

    #[test]
    fn activates_one_atomic_consent_selection() {
        let mut coordinator =
            LeaseCoordinator::new(LeasePolicy::default(), MonotonicTime(0)).unwrap();
        coordinator.arm(MonotonicTime(1), 1).unwrap();
        let intents = intents(&[1, 2], 1, 30);
        let state = coordinator
            .reconcile(MonotonicTime(10), &intents, safety(10))
            .unwrap();
        match state {
            LeaseState::DesiredActive {
                executions,
                engine_incarnation,
                consent_generation,
                consent_nonce,
                deadline,
            } => {
                assert_eq!(executions.len(), 2);
                assert_eq!(*engine_incarnation, [8; 16]);
                assert_eq!(*consent_generation, 1);
                assert_eq!(*consent_nonce, [1; 16]);
                assert_eq!(*deadline, MonotonicTime(30));
            }
            _ => panic!("not active"),
        }
    }

    #[test]
    fn expiry_clock_regression_and_excessive_deadline_fail_closed() {
        let policy = LeasePolicy {
            maximum_renew_window: 10,
            maximum_total_duration: 100,
            ..LeasePolicy::default()
        };
        let mut coordinator = LeaseCoordinator::new(policy, MonotonicTime(0)).unwrap();
        coordinator.arm(MonotonicTime(1), 1).unwrap();
        let intent = intents(&[1], 1, 12).pop().unwrap();
        assert!(matches!(
            coordinator.reconcile(MonotonicTime(2), &[intent], safety(2)),
            Ok(LeaseState::DesiredActive { .. })
        ));
        coordinator.tick(MonotonicTime(12)).unwrap();
        assert_eq!(
            coordinator.state(),
            &LeaseState::Inactive(InactiveReason::Expired)
        );
        assert_eq!(
            coordinator.tick(MonotonicTime(11)),
            Err(CoordinatorError::ClockRegression)
        );
        assert_eq!(
            coordinator.state(),
            &LeaseState::Inactive(InactiveReason::InvalidInput)
        );
    }

    #[test]
    fn allow_sleep_now_is_sticky_against_queued_renew() {
        let mut coordinator =
            LeaseCoordinator::new(LeasePolicy::default(), MonotonicTime(0)).unwrap();
        coordinator.arm(MonotonicTime(1), 7).unwrap();
        coordinator.allow_sleep_now();
        let intent = intents(&[1], 7, 20).pop().unwrap();
        assert_eq!(
            coordinator
                .reconcile(MonotonicTime(2), std::slice::from_ref(&intent), safety(2))
                .unwrap(),
            &LeaseState::Inactive(InactiveReason::NotArmed)
        );
        assert_eq!(
            coordinator.arm(MonotonicTime(3), 7),
            Err(CoordinatorError::ArmEpochNotIncreasing)
        );
        coordinator.arm(MonotonicTime(3), 8).unwrap();
        assert_eq!(
            coordinator.reconcile(MonotonicTime(4), &[intent], safety(4)),
            Err(CoordinatorError::ConsentGenerationMismatch)
        );
        let fresh = intents(&[1], 8, 20).pop().unwrap();
        assert!(matches!(
            coordinator.reconcile(MonotonicTime(4), &[fresh], safety(4)),
            Ok(LeaseState::DesiredActive { .. })
        ));
    }

    #[test]
    fn every_missing_stale_or_future_safety_signal_fails_closed() {
        let now = MonotonicTime(100);
        let mut facts = safety(100);
        facts.thermal = None;
        assert_eq!(
            evaluate_safety(&facts, now, 10),
            Err(SafetyFailure::MissingThermal)
        );
        facts = safety(89);
        assert_eq!(
            evaluate_safety(&facts, now, 10),
            Err(SafetyFailure::StaleSample)
        );
        facts = safety(101);
        assert_eq!(
            evaluate_safety(&facts, now, 10),
            Err(SafetyFailure::FutureSample)
        );
        facts = safety(100);
        facts.power_source.as_mut().unwrap().value = PowerSource::Battery;
        assert_eq!(
            evaluate_safety(&facts, now, 10),
            Err(SafetyFailure::BatteryPower)
        );
        facts = safety(100);
        facts.thermal.as_mut().unwrap().value = ThermalState::Critical;
        assert_eq!(
            evaluate_safety(&facts, now, 10),
            Err(SafetyFailure::ThermalUnsafe)
        );
    }

    #[test]
    fn safety_trip_requires_a_new_explicit_arm_epoch() {
        let mut coordinator =
            LeaseCoordinator::new(LeasePolicy::default(), MonotonicTime(0)).unwrap();
        coordinator.arm(MonotonicTime(1), 1).unwrap();
        let intent = intents(&[1], 1, 50).pop().unwrap();
        let mut unsafe_facts = safety(2);
        unsafe_facts.helper_healthy.as_mut().unwrap().value = false;
        assert!(matches!(
            coordinator.reconcile(
                MonotonicTime(2),
                std::slice::from_ref(&intent),
                unsafe_facts
            ),
            Ok(LeaseState::Inactive(InactiveReason::Safety(_)))
        ));
        assert_eq!(
            coordinator
                .reconcile(MonotonicTime(3), std::slice::from_ref(&intent), safety(3))
                .unwrap(),
            &LeaseState::Inactive(InactiveReason::NotArmed)
        );
        coordinator.arm(MonotonicTime(4), 2).unwrap();
        assert_eq!(
            coordinator.reconcile(MonotonicTime(5), &[intent], safety(5)),
            Err(CoordinatorError::ConsentGenerationMismatch)
        );
        let fresh = intents(&[1], 2, 50).pop().unwrap();
        assert!(matches!(
            coordinator.reconcile(MonotonicTime(5), &[fresh], safety(5)),
            Ok(LeaseState::DesiredActive { .. })
        ));
    }
}
