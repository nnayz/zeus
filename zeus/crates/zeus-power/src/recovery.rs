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
    pub owner: [u8; 16],
    pub generation: u64,
    pub prior_state: SleepObservation,
    pub phase: JournalPhase,
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

/// Semantic effects only. There is no arbitrary command, path, or boolean setter.
pub trait RecoveryBackend {
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
    RestorationNotVerified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryState {
    Recovering,
    Ready,
    Owned(RecoveryRecord),
    RecoveryRequired,
    Conflict(ConflictReason),
    Ambiguous(AmbiguousReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryError<E> {
    Backend(E),
    NotReady,
    Conflict(ConflictReason),
    Ambiguous(AmbiguousReason),
}

pub struct RecoveryMachine<B: RecoveryBackend> {
    backend: B,
    owner: [u8; 16],
    state: RecoveryState,
}

impl<B: RecoveryBackend> RecoveryMachine<B> {
    pub fn new(backend: B, owner: [u8; 16]) -> Self {
        Self {
            backend,
            owner,
            state: RecoveryState::Recovering,
        }
    }

    pub fn state(&self) -> RecoveryState {
        self.state
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn into_backend(self) -> B {
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
                let observed = self.call(|b| b.observe_sleep())?;
                match (record.phase, observed) {
                    (JournalPhase::Prepared, SleepObservation::Enabled) => {
                        self.call(|b| b.clear_journal())?;
                        self.state = RecoveryState::Ready;
                        Ok(())
                    }
                    (JournalPhase::Prepared, SleepObservation::Disabled)
                    | (JournalPhase::Owned, SleepObservation::Disabled)
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

    pub fn acquire(&mut self, generation: u64) -> Result<(), RecoveryError<B::Error>> {
        if self.state != RecoveryState::Ready {
            return Err(RecoveryError::NotReady);
        }
        match self.call(|b| b.read_journal())? {
            JournalObservation::Missing => {}
            JournalObservation::Valid(record) if record.owner != self.owner => {
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
            owner: self.owner,
            generation,
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
            JournalObservation::Valid(record) if record.owner != self.owner => {
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
        Ok(())
    }

    fn restore(&mut self, mut record: RecoveryRecord) -> Result<(), RecoveryError<B::Error>> {
        if record.prior_state != SleepObservation::Enabled {
            return self.ambiguous(AmbiguousReason::InvalidRecord);
        }
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
    }

    impl FakeBackend {
        pub fn normal() -> Self {
            Self {
                journal: JournalObservation::Missing,
                sleep: SleepObservation::Enabled,
                operations: 0,
                fail_before: None,
            }
        }
        pub fn fail_before_operation(&mut self, operation: usize) {
            self.operations = 0;
            self.fail_before = Some(operation);
        }
        pub fn clear_fault(&mut self) {
            self.fail_before = None;
            self.operations = 0;
        }
        pub fn operation_count(&self) -> usize {
            self.operations
        }
        fn step(&mut self) -> Result<(), FakeError> {
            self.operations += 1;
            if self.fail_before == Some(self.operations) {
                Err(FakeError::InjectedCrash)
            } else {
                Ok(())
            }
        }
    }

    impl RecoveryBackend for FakeBackend {
        type Error = FakeError;
        fn read_journal(&mut self) -> Result<JournalObservation, Self::Error> {
            self.step()?;
            Ok(self.journal)
        }
        fn write_journal(&mut self, record: RecoveryRecord) -> Result<(), Self::Error> {
            self.step()?;
            self.journal = JournalObservation::Valid(record);
            Ok(())
        }
        fn clear_journal(&mut self) -> Result<(), Self::Error> {
            self.step()?;
            self.journal = JournalObservation::Missing;
            Ok(())
        }
        fn observe_sleep(&mut self) -> Result<SleepObservation, Self::Error> {
            self.step()?;
            Ok(self.sleep)
        }
        fn disable_sleep(&mut self) -> Result<(), Self::Error> {
            self.step()?;
            self.sleep = SleepObservation::Disabled;
            Ok(())
        }
        fn restore_normal_sleep(&mut self) -> Result<(), Self::Error> {
            self.step()?;
            self.sleep = SleepObservation::Enabled;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    const OWNER: [u8; 16] = [7; 16];

    fn restart_and_recover(mut backend: FakeBackend) -> RecoveryMachine<FakeBackend> {
        backend.clear_fault();
        let mut restarted = RecoveryMachine::new(backend, OWNER);
        restarted.startup_recover().unwrap();
        assert_eq!(restarted.state(), RecoveryState::Ready);
        assert_eq!(restarted.backend().sleep, SleepObservation::Enabled);
        assert_eq!(restarted.backend().journal, JournalObservation::Missing);
        restarted
    }

    #[test]
    fn acquire_release_and_uninstall_are_verified() {
        let mut machine = RecoveryMachine::new(FakeBackend::normal(), OWNER);
        machine.startup_recover().unwrap();
        machine.acquire(1).unwrap();
        assert!(matches!(machine.state(), RecoveryState::Owned(_)));
        assert_eq!(machine.backend().sleep, SleepObservation::Disabled);
        machine.release().unwrap();
        machine.prepare_uninstall().unwrap();
        assert_eq!(machine.state(), RecoveryState::Ready);
    }

    #[test]
    fn acquisition_crash_points_recover_deterministically() {
        // Operations: journal read, sleep read, journal prepare, disable,
        // readback, journal owned. A crash before each operation is recoverable.
        for crash_at in 1..=6 {
            let mut backend = FakeBackend::normal();
            let mut initial = RecoveryMachine::new(backend.clone(), OWNER);
            initial.startup_recover().unwrap();
            backend = initial.into_backend();
            backend.fail_before_operation(crash_at);
            let mut machine = RecoveryMachine::new(backend, OWNER);
            machine.state = RecoveryState::Ready;
            assert!(machine.acquire(1).is_err(), "crash point {crash_at}");
            restart_and_recover(machine.into_backend());
        }

        // Crash immediately after the final durable commit.
        let mut machine = RecoveryMachine::new(FakeBackend::normal(), OWNER);
        machine.startup_recover().unwrap();
        machine.acquire(1).unwrap();
        restart_and_recover(machine.into_backend());
    }

    #[test]
    fn release_crash_points_are_idempotently_recovered() {
        for crash_at in 1..=6 {
            let mut machine = RecoveryMachine::new(FakeBackend::normal(), OWNER);
            machine.startup_recover().unwrap();
            machine.acquire(1).unwrap();
            let mut backend = machine.into_backend();
            backend.fail_before_operation(crash_at);
            let owned = match backend.journal {
                JournalObservation::Valid(record) => record,
                _ => unreachable!(),
            };
            let mut releasing = RecoveryMachine::new(backend, OWNER);
            releasing.state = RecoveryState::Owned(owned);
            assert!(releasing.release().is_err(), "crash point {crash_at}");
            restart_and_recover(releasing.into_backend());
        }
    }

    #[test]
    fn preexisting_external_state_and_corrupt_journal_are_not_clobbered() {
        let mut external = FakeBackend::normal();
        external.sleep = SleepObservation::Disabled;
        let mut machine = RecoveryMachine::new(external, OWNER);
        assert_eq!(
            machine.startup_recover(),
            Err(RecoveryError::Conflict(ConflictReason::PreExistingDisabled))
        );
        assert_eq!(machine.backend().sleep, SleepObservation::Disabled);

        let mut corrupt = FakeBackend::normal();
        corrupt.journal = JournalObservation::Corrupt;
        let mut machine = RecoveryMachine::new(corrupt, OWNER);
        assert_eq!(
            machine.startup_recover(),
            Err(RecoveryError::Ambiguous(AmbiguousReason::CorruptJournal))
        );
        assert_eq!(machine.backend().sleep, SleepObservation::Enabled);
    }

    #[test]
    fn externally_changed_owned_state_becomes_ambiguous() {
        let mut machine = RecoveryMachine::new(FakeBackend::normal(), OWNER);
        machine.startup_recover().unwrap();
        machine.acquire(1).unwrap();
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
    fn different_owner_conflicts_without_mutation() {
        let record = RecoveryRecord {
            schema_version: 1,
            owner: [9; 16],
            generation: 1,
            prior_state: SleepObservation::Enabled,
            phase: JournalPhase::Owned,
        };
        let mut backend = FakeBackend::normal();
        backend.journal = JournalObservation::Valid(record);
        backend.sleep = SleepObservation::Disabled;
        let mut machine = RecoveryMachine::new(backend, OWNER);
        // Startup is recovery mode and may clean a stale valid journal regardless
        // of build owner. Live acquisition never takes over another owner.
        machine.state = RecoveryState::Ready;
        assert_eq!(
            machine.acquire(2),
            Err(RecoveryError::Conflict(ConflictReason::DifferentOwner))
        );
        assert_eq!(machine.backend().sleep, SleepObservation::Disabled);
    }
}
