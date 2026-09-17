//! Root-recovery protocol abstractions. No backend here mutates real host power.
//!
//! The real system setting is a global boolean without an ownership token. Even
//! journaled readback cannot detect another utility writing the same value while
//! Zeus owns it. Callers must surface that coexistence limit; this model never
//! treats pre-existing or ambiguous disabled state as Zeus ownership.

pub const JOURNAL_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalPhase {
    Prepared,
    Owned,
    Restoring,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryRecord {
    pub schema_version: u16,
    /// Stable installation identity stored separately in root-owned state.
    pub owner: [u8; 16],
    /// Boot identity observed by the privileged Helper.
    pub host_boot_id: [u8; 16],
    /// Random nonce for the Helper process that created the mutation intent.
    pub helper_instance: [u8; 16],
    pub engine_incarnation: [u8; 16],
    pub lease_id: [u8; 16],
    pub consent_generation: u64,
    /// Immutable deadline in the Helper's boot-scoped continuous clock domain.
    pub hard_deadline_millis: u64,
    pub generation: u64,
    pub prior_state: SleepObservation,
    pub phase: JournalPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryContext {
    pub owner: [u8; 16],
    pub host_boot_id: [u8; 16],
    pub helper_instance: [u8; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcquisitionContext {
    pub generation: u64,
    pub engine_incarnation: [u8; 16],
    pub lease_id: [u8; 16],
    pub consent_generation: u64,
    pub hard_deadline_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalObservation {
    Missing,
    Valid(RecoveryRecord),
    Corrupt,
    UnsupportedSchema,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepObservation {
    Enabled,
    Disabled,
    Unknown,
}

mod sealed {
    pub trait Sealed {}
}

/// Semantic effects only. There is no arbitrary command, path, or boolean setter.
///
/// Production implementations must make journal replacement atomic and durable
/// through file and parent-directory synchronization. A successful write must
/// survive restart as the complete old or complete new record, never torn data.
/// The trait is sealed so callers cannot bypass [`RecoveryMachine`] with a
/// separately implemented mutating backend.
pub trait RecoveryBackend: sealed::Sealed {
    type Error;
    fn read_journal(&mut self) -> Result<JournalObservation, Self::Error>;
    fn write_journal(&mut self, record: RecoveryRecord) -> Result<(), Self::Error>;
    fn clear_journal(&mut self) -> Result<(), Self::Error>;
    fn observe_sleep(&mut self) -> Result<SleepObservation, Self::Error>;
    fn disable_sleep(&mut self) -> Result<(), Self::Error>;
    fn restore_normal_sleep(&mut self) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictReason {
    PreExistingDisabled,
    ExistingJournal,
    DifferentOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmbiguousReason {
    CorruptJournal,
    UnsupportedJournal,
    UnknownSystemState,
    UnexpectedSystemState,
    InvalidRecord,
    MutationNotVerified,
    PreparedOwnershipUnproven,
    RestorationNotVerified,
    DifferentBoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryState {
    Recovering,
    Ready,
    Owned(RecoveryRecord),
    RemovalPrepared,
    RecoveryRequired,
    Conflict(ConflictReason),
    Ambiguous(AmbiguousReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryError<E> {
    Backend(E),
    NotReady,
    InvalidGeneration,
    InvalidIdentity,
    Conflict(ConflictReason),
    Ambiguous(AmbiguousReason),
}

pub struct RecoveryMachine<B: RecoveryBackend> {
    backend: B,
    context: RecoveryContext,
    last_generation: u64,
    state: RecoveryState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryConfigError {
    ZeroIdentity,
}

impl<B: RecoveryBackend> RecoveryMachine<B> {
    pub fn new(backend: B, context: RecoveryContext) -> Result<Self, RecoveryConfigError> {
        if context.owner == [0; 16]
            || context.host_boot_id == [0; 16]
            || context.helper_instance == [0; 16]
        {
            return Err(RecoveryConfigError::ZeroIdentity);
        }
        Ok(Self {
            backend,
            context,
            last_generation: 0,
            state: RecoveryState::Recovering,
        })
    }

    pub fn state(&self) -> RecoveryState {
        self.state
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    #[cfg(test)]
    fn into_backend(self) -> B {
        self.backend
    }

    /// Must complete before acquisition. A valid stale journal is recovered;
    /// missing/corrupt ownership evidence never authorizes a blind restoration.
    pub fn startup_recover(&mut self) -> Result<(), RecoveryError<B::Error>> {
        self.state = RecoveryState::Recovering;
        let journal = self.call(|b| b.read_journal())?;
        match journal {
            JournalObservation::Missing => match self.call(|b| b.observe_sleep())? {
                SleepObservation::Enabled => {
                    self.state = RecoveryState::Ready;
                    Ok(())
                }
                SleepObservation::Disabled => self.conflict(ConflictReason::PreExistingDisabled),
                SleepObservation::Unknown => self.ambiguous(AmbiguousReason::UnknownSystemState),
            },
            JournalObservation::Corrupt => self.ambiguous(AmbiguousReason::CorruptJournal),
            JournalObservation::UnsupportedSchema => {
                self.ambiguous(AmbiguousReason::UnsupportedJournal)
            }
            JournalObservation::Valid(record) => {
                if !valid_record(record) {
                    return self.ambiguous(AmbiguousReason::InvalidRecord);
                }
                if record.owner != self.context.owner {
                    return self.conflict(ConflictReason::DifferentOwner);
                }
                if record.host_boot_id != self.context.host_boot_id {
                    return self.ambiguous(AmbiguousReason::DifferentBoot);
                }
                self.last_generation = self.last_generation.max(record.generation);
                let observed = self.call(|b| b.observe_sleep())?;
                match (record.phase, observed) {
                    (JournalPhase::Prepared, SleepObservation::Enabled) => {
                        self.call(|b| b.clear_journal())?;
                        self.state = RecoveryState::Ready;
                        Ok(())
                    }
                    // Prepared proves durable intent, not that Zeus performed
                    // the global mutation. Another utility could have written
                    // the same boolean before Zeus did. Never clear it blindly.
                    (JournalPhase::Prepared, SleepObservation::Disabled) => {
                        self.ambiguous(AmbiguousReason::PreparedOwnershipUnproven)
                    }
                    (JournalPhase::Owned, SleepObservation::Disabled)
                    | (JournalPhase::Restoring, SleepObservation::Disabled) => self.restore(record),
                    (JournalPhase::Restoring, SleepObservation::Enabled) => {
                        self.call(|b| b.clear_journal())?;
                        self.state = RecoveryState::Ready;
                        Ok(())
                    }
                    (JournalPhase::Owned, SleepObservation::Enabled) => {
                        self.ambiguous(AmbiguousReason::UnexpectedSystemState)
                    }
                    (_, SleepObservation::Unknown) => {
                        self.ambiguous(AmbiguousReason::UnknownSystemState)
                    }
                }
            }
        }
    }

    pub fn acquire(
        &mut self,
        acquisition: AcquisitionContext,
    ) -> Result<(), RecoveryError<B::Error>> {
        if self.state != RecoveryState::Ready {
            return Err(RecoveryError::NotReady);
        }
        if acquisition.generation == 0 || acquisition.generation <= self.last_generation {
            return Err(RecoveryError::InvalidGeneration);
        }
        if acquisition.engine_incarnation == [0; 16]
            || acquisition.lease_id == [0; 16]
            || acquisition.consent_generation == 0
            || acquisition.hard_deadline_millis == 0
        {
            return Err(RecoveryError::InvalidIdentity);
        }
        // A failed attempt consumes its generation so an in-process replay can
        // never retry the same intent after an ambiguous side effect.
        self.last_generation = acquisition.generation;
        match self.call(|b| b.read_journal())? {
            JournalObservation::Missing => {}
            JournalObservation::Valid(record) if record.owner != self.context.owner => {
                return self.conflict(ConflictReason::DifferentOwner);
            }
            _ => return self.conflict(ConflictReason::ExistingJournal),
        }
        match self.call(|b| b.observe_sleep())? {
            SleepObservation::Enabled => {}
            SleepObservation::Disabled => {
                return self.conflict(ConflictReason::PreExistingDisabled);
            }
            SleepObservation::Unknown => {
                return self.ambiguous(AmbiguousReason::UnknownSystemState);
            }
        }
        let mut record = RecoveryRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            owner: self.context.owner,
            host_boot_id: self.context.host_boot_id,
            helper_instance: self.context.helper_instance,
            engine_incarnation: acquisition.engine_incarnation,
            lease_id: acquisition.lease_id,
            consent_generation: acquisition.consent_generation,
            hard_deadline_millis: acquisition.hard_deadline_millis,
            generation: acquisition.generation,
            prior_state: SleepObservation::Enabled,
            phase: JournalPhase::Prepared,
        };
        self.call(|b| b.write_journal(record))?;
        self.call(|b| b.disable_sleep())?;
        if self.call(|b| b.observe_sleep())? != SleepObservation::Disabled {
            return self.ambiguous(AmbiguousReason::MutationNotVerified);
        }
        record.phase = JournalPhase::Owned;
        self.call(|b| b.write_journal(record))?;
        self.state = RecoveryState::Owned(record);
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), RecoveryError<B::Error>> {
        let owned = match self.state {
            RecoveryState::Owned(record) => record,
            _ => return Err(RecoveryError::NotReady),
        };
        match self.call(|b| b.read_journal())? {
            JournalObservation::Valid(record) if record == owned => {}
            JournalObservation::Valid(record) if record.owner != self.context.owner => {
                return self.conflict(ConflictReason::DifferentOwner);
            }
            JournalObservation::Corrupt | JournalObservation::UnsupportedSchema => {
                return self.ambiguous(AmbiguousReason::CorruptJournal);
            }
            _ => return self.ambiguous(AmbiguousReason::UnexpectedSystemState),
        }
        match self.call(|b| b.observe_sleep())? {
            SleepObservation::Disabled => self.restore(owned),
            SleepObservation::Enabled => self.ambiguous(AmbiguousReason::UnexpectedSystemState),
            SleepObservation::Unknown => self.ambiguous(AmbiguousReason::UnknownSystemState),
        }
    }

    /// Uninstall is safe only after verified normal sleep and journal removal.
    pub fn prepare_uninstall(&mut self) -> Result<(), RecoveryError<B::Error>> {
        if matches!(self.state, RecoveryState::Owned(_)) {
            self.release()?;
        }
        if self.state != RecoveryState::Ready {
            return Err(RecoveryError::NotReady);
        }
        if self.call(|b| b.read_journal())? != JournalObservation::Missing {
            return self.ambiguous(AmbiguousReason::UnexpectedSystemState);
        }
        if self.call(|b| b.observe_sleep())? != SleepObservation::Enabled {
            return self.ambiguous(AmbiguousReason::RestorationNotVerified);
        }
        self.state = RecoveryState::RemovalPrepared;
        Ok(())
    }

    fn restore(&mut self, mut record: RecoveryRecord) -> Result<(), RecoveryError<B::Error>> {
        if record.prior_state != SleepObservation::Enabled {
            return self.ambiguous(AmbiguousReason::InvalidRecord);
        }
        // The process-lifetime lock must already prove that no prior Helper is
        // live. Record the current writer incarnation before any restoration so
        // a crash cannot make a successor confuse the stale writer with itself.
        record.helper_instance = self.context.helper_instance;
        record.phase = JournalPhase::Restoring;
        self.call(|b| b.write_journal(record))?;
        self.call(|b| b.restore_normal_sleep())?;
        if self.call(|b| b.observe_sleep())? != SleepObservation::Enabled {
            return self.ambiguous(AmbiguousReason::RestorationNotVerified);
        }
        self.call(|b| b.clear_journal())?;
        self.state = RecoveryState::Ready;
        Ok(())
    }

    fn call<T>(
        &mut self,
        operation: impl FnOnce(&mut B) -> Result<T, B::Error>,
    ) -> Result<T, RecoveryError<B::Error>> {
        match operation(&mut self.backend) {
            Ok(value) => Ok(value),
            Err(error) => {
                self.state = RecoveryState::RecoveryRequired;
                Err(RecoveryError::Backend(error))
            }
        }
    }

    fn conflict<T>(&mut self, reason: ConflictReason) -> Result<T, RecoveryError<B::Error>> {
        self.state = RecoveryState::Conflict(reason);
        Err(RecoveryError::Conflict(reason))
    }

    fn ambiguous<T>(&mut self, reason: AmbiguousReason) -> Result<T, RecoveryError<B::Error>> {
        self.state = RecoveryState::Ambiguous(reason);
        Err(RecoveryError::Ambiguous(reason))
    }
}

fn valid_record(record: RecoveryRecord) -> bool {
    record.schema_version == JOURNAL_SCHEMA_VERSION
        && record.owner != [0; 16]
        && record.host_boot_id != [0; 16]
        && record.helper_instance != [0; 16]
        && record.engine_incarnation != [0; 16]
        && record.lease_id != [0; 16]
        && record.consent_generation != 0
        && record.hard_deadline_millis != 0
        && record.generation != 0
        && record.prior_state == SleepObservation::Enabled
}

/// Deterministic, in-memory test backend. It never calls host APIs.
pub mod fake {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum FakeError {
        InjectedCrash,
    }

    #[derive(Clone, Debug)]
    pub struct FakeBackend {
        pub journal: JournalObservation,
        pub sleep: SleepObservation,
        operations: usize,
        fail_before: Option<usize>,
        fail_after: Option<usize>,
    }

    impl FakeBackend {
        pub fn normal() -> Self {
            Self {
                journal: JournalObservation::Missing,
                sleep: SleepObservation::Enabled,
                operations: 0,
                fail_before: None,
                fail_after: None,
            }
        }
        pub fn fail_before_operation(&mut self, operation: usize) {
            self.operations = 0;
            self.fail_before = Some(operation);
        }
        pub fn fail_after_operation(&mut self, operation: usize) {
            self.operations = 0;
            self.fail_after = Some(operation);
        }
        pub fn clear_fault(&mut self) {
            self.fail_before = None;
            self.fail_after = None;
            self.operations = 0;
        }
        pub fn operation_count(&self) -> usize {
            self.operations
        }
        fn begin(&mut self) -> Result<(), FakeError> {
            self.operations += 1;
            if self.fail_before == Some(self.operations) {
                Err(FakeError::InjectedCrash)
            } else {
                Ok(())
            }
        }
        fn end(&mut self) -> Result<(), FakeError> {
            if self.fail_after == Some(self.operations) {
                Err(FakeError::InjectedCrash)
            } else {
                Ok(())
            }
        }
    }

    impl sealed::Sealed for FakeBackend {}

    impl RecoveryBackend for FakeBackend {
        type Error = FakeError;
        fn read_journal(&mut self) -> Result<JournalObservation, Self::Error> {
            self.begin()?;
            let value = self.journal;
            self.end()?;
            Ok(value)
        }
        fn write_journal(&mut self, record: RecoveryRecord) -> Result<(), Self::Error> {
            self.begin()?;
            self.journal = JournalObservation::Valid(record);
            self.end()
        }
        fn clear_journal(&mut self) -> Result<(), Self::Error> {
            self.begin()?;
            self.journal = JournalObservation::Missing;
            self.end()
        }
        fn observe_sleep(&mut self) -> Result<SleepObservation, Self::Error> {
            self.begin()?;
            let value = self.sleep;
            self.end()?;
            Ok(value)
        }
        fn disable_sleep(&mut self) -> Result<(), Self::Error> {
            self.begin()?;
            self.sleep = SleepObservation::Disabled;
            self.end()
        }
        fn restore_normal_sleep(&mut self) -> Result<(), Self::Error> {
            self.begin()?;
            self.sleep = SleepObservation::Enabled;
            self.end()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    const OWNER: [u8; 16] = [7; 16];

    fn context() -> RecoveryContext {
        RecoveryContext {
            owner: OWNER,
            host_boot_id: [6; 16],
            helper_instance: [5; 16],
        }
    }

    fn acquisition(generation: u64) -> AcquisitionContext {
        AcquisitionContext {
            generation,
            engine_incarnation: [4; 16],
            lease_id: [3; 16],
            consent_generation: 1,
            hard_deadline_millis: 1_000,
        }
    }

    fn new_machine(backend: FakeBackend) -> RecoveryMachine<FakeBackend> {
        RecoveryMachine::new(backend, context()).expect("valid recovery identities")
    }

    fn restart_and_recover(mut backend: FakeBackend) -> RecoveryMachine<FakeBackend> {
        backend.clear_fault();
        let mut restarted = new_machine(backend);
        restarted.startup_recover().unwrap();
        assert_eq!(restarted.state(), RecoveryState::Ready);
        assert_eq!(restarted.backend().sleep, SleepObservation::Enabled);
        assert_eq!(restarted.backend().journal, JournalObservation::Missing);
        restarted
    }

    #[test]
    fn acquire_release_and_uninstall_are_verified() {
        let mut machine = new_machine(FakeBackend::normal());
        machine.startup_recover().unwrap();
        machine.acquire(acquisition(1)).unwrap();
        assert!(matches!(machine.state(), RecoveryState::Owned(_)));
        assert_eq!(machine.backend().sleep, SleepObservation::Disabled);
        machine.release().unwrap();
        machine.prepare_uninstall().unwrap();
        assert_eq!(machine.state(), RecoveryState::RemovalPrepared);
        assert_eq!(
            machine.acquire(acquisition(2)),
            Err(RecoveryError::NotReady)
        );
    }

    #[test]
    fn acquisition_crash_points_recover_or_remain_ambiguous_without_clobbering() {
        // Operations: journal read, sleep read, journal prepare, disable,
        // readback, journal owned. Before mutation, restart safely returns Ready.
        for crash_at in 1..=4 {
            let mut backend = FakeBackend::normal();
            let mut initial = new_machine(backend.clone());
            initial.startup_recover().unwrap();
            backend = initial.into_backend();
            backend.fail_before_operation(crash_at);
            let mut attempt = new_machine(backend);
            attempt.state = RecoveryState::Ready;
            assert!(
                attempt.acquire(acquisition(1)).is_err(),
                "crash point {crash_at}"
            );
            restart_and_recover(attempt.into_backend());
        }

        // Once the global bit changed but only Prepared is durable, restart
        // cannot prove Zeus performed the mutation. It leaves the bit alone and
        // reports the explicit no-go ambiguity instead of blindly restoring.
        for crash_at in 5..=6 {
            let mut backend = FakeBackend::normal();
            let mut initial = new_machine(backend.clone());
            initial.startup_recover().unwrap();
            backend = initial.into_backend();
            backend.fail_before_operation(crash_at);
            let mut attempt = new_machine(backend);
            attempt.state = RecoveryState::Ready;
            assert!(
                attempt.acquire(acquisition(1)).is_err(),
                "crash point {crash_at}"
            );
            let mut restarted = new_machine(attempt.into_backend());
            assert_eq!(
                restarted.startup_recover(),
                Err(RecoveryError::Ambiguous(
                    AmbiguousReason::PreparedOwnershipUnproven
                ))
            );
            assert_eq!(restarted.backend().sleep, SleepObservation::Disabled);
        }

        // Crash immediately after the final durable Owned commit is recoverable.
        let mut machine = new_machine(FakeBackend::normal());
        machine.startup_recover().unwrap();
        machine.acquire(acquisition(1)).unwrap();
        restart_and_recover(machine.into_backend());
    }

    #[test]
    fn acquisition_failures_after_side_effect_never_blindly_restore_prepared_state() {
        for crash_at in [1, 2, 3, 6] {
            let mut backend = FakeBackend::normal();
            let mut initial = new_machine(backend.clone());
            initial.startup_recover().unwrap();
            backend = initial.into_backend();
            backend.fail_after_operation(crash_at);
            let mut attempt = new_machine(backend);
            attempt.state = RecoveryState::Ready;
            assert!(
                attempt.acquire(acquisition(1)).is_err(),
                "after operation {crash_at}"
            );
            restart_and_recover(attempt.into_backend());
        }
        for crash_at in [4, 5] {
            let mut backend = FakeBackend::normal();
            let mut initial = new_machine(backend.clone());
            initial.startup_recover().unwrap();
            backend = initial.into_backend();
            backend.fail_after_operation(crash_at);
            let mut attempt = new_machine(backend);
            attempt.state = RecoveryState::Ready;
            assert!(
                attempt.acquire(acquisition(1)).is_err(),
                "after operation {crash_at}"
            );
            let mut restarted = new_machine(attempt.into_backend());
            assert_eq!(
                restarted.startup_recover(),
                Err(RecoveryError::Ambiguous(
                    AmbiguousReason::PreparedOwnershipUnproven
                ))
            );
            assert_eq!(restarted.backend().sleep, SleepObservation::Disabled);
        }
    }

    #[test]
    fn release_crash_points_are_idempotently_recovered() {
        for crash_at in 1..=6 {
            let mut machine = new_machine(FakeBackend::normal());
            machine.startup_recover().unwrap();
            machine.acquire(acquisition(1)).unwrap();
            let mut backend = machine.into_backend();
            backend.fail_before_operation(crash_at);
            let owned = match backend.journal {
                JournalObservation::Valid(record) => record,
                _ => unreachable!(),
            };
            let mut releasing = new_machine(backend);
            releasing.state = RecoveryState::Owned(owned);
            assert!(releasing.release().is_err(), "crash point {crash_at}");
            restart_and_recover(releasing.into_backend());
        }
    }

    #[test]
    fn preexisting_external_state_and_corrupt_journal_are_not_clobbered() {
        let mut external = FakeBackend::normal();
        external.sleep = SleepObservation::Disabled;
        let mut machine = new_machine(external);
        assert_eq!(
            machine.startup_recover(),
            Err(RecoveryError::Conflict(ConflictReason::PreExistingDisabled))
        );
        assert_eq!(machine.backend().sleep, SleepObservation::Disabled);

        let mut corrupt = FakeBackend::normal();
        corrupt.journal = JournalObservation::Corrupt;
        let mut machine = new_machine(corrupt);
        assert_eq!(
            machine.startup_recover(),
            Err(RecoveryError::Ambiguous(AmbiguousReason::CorruptJournal))
        );
        assert_eq!(machine.backend().sleep, SleepObservation::Enabled);
    }

    #[test]
    fn externally_changed_owned_state_becomes_ambiguous() {
        let mut machine = new_machine(FakeBackend::normal());
        machine.startup_recover().unwrap();
        machine.acquire(acquisition(1)).unwrap();
        machine.backend.sleep = SleepObservation::Enabled;
        assert_eq!(
            machine.release(),
            Err(RecoveryError::Ambiguous(
                AmbiguousReason::UnexpectedSystemState
            ))
        );
        assert!(matches!(
            machine.backend().journal,
            JournalObservation::Valid(_)
        ));
    }

    #[test]
    fn exclusive_successor_records_its_incarnation_before_restoring() {
        let mut original = new_machine(FakeBackend::normal());
        original.startup_recover().unwrap();
        original.acquire(acquisition(1)).unwrap();
        let mut backend = original.into_backend();
        let old_helper = match backend.journal {
            JournalObservation::Valid(record) => record.helper_instance,
            _ => unreachable!(),
        };
        assert_eq!(old_helper, [5; 16]);

        // Simulate a successor that already acquired the external lifetime lock.
        let mut successor_context = context();
        successor_context.helper_instance = [8; 16];
        backend.fail_after_operation(3); // restoring journal write completed
        let mut successor = RecoveryMachine::new(backend, successor_context).unwrap();
        assert!(successor.startup_recover().is_err());
        let record = match successor.backend().journal {
            JournalObservation::Valid(record) => record,
            _ => unreachable!(),
        };
        assert_eq!(record.phase, JournalPhase::Restoring);
        assert_eq!(record.helper_instance, [8; 16]);
    }

    #[test]
    fn different_owner_conflicts_without_mutation() {
        let record = RecoveryRecord {
            schema_version: 1,
            owner: [9; 16],
            host_boot_id: [6; 16],
            helper_instance: [5; 16],
            engine_incarnation: [4; 16],
            lease_id: [3; 16],
            consent_generation: 1,
            hard_deadline_millis: 1_000,
            generation: 1,
            prior_state: SleepObservation::Enabled,
            phase: JournalPhase::Owned,
        };
        let mut backend = FakeBackend::normal();
        backend.journal = JournalObservation::Valid(record);
        backend.sleep = SleepObservation::Disabled;
        let mut machine = new_machine(backend);
        assert_eq!(
            machine.startup_recover(),
            Err(RecoveryError::Conflict(ConflictReason::DifferentOwner))
        );
        assert_eq!(machine.backend().sleep, SleepObservation::Disabled);
    }

    #[test]
    fn zero_owner_and_replayed_generations_fail_before_mutation() {
        assert!(matches!(
            RecoveryMachine::new(
                FakeBackend::normal(),
                RecoveryContext {
                    owner: [0; 16],
                    ..context()
                }
            ),
            Err(RecoveryConfigError::ZeroIdentity)
        ));
        let mut machine = new_machine(FakeBackend::normal());
        machine.startup_recover().unwrap();
        assert_eq!(
            machine.acquire(acquisition(0)),
            Err(RecoveryError::InvalidGeneration)
        );
        assert_eq!(machine.backend().journal, JournalObservation::Missing);
        assert_eq!(machine.backend().sleep, SleepObservation::Enabled);
        machine.acquire(acquisition(1)).unwrap();
        machine.release().unwrap();
        assert_eq!(
            machine.acquire(acquisition(1)),
            Err(RecoveryError::InvalidGeneration)
        );
    }
}
