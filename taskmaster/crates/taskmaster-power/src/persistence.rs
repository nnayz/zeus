//! Exclusive Helper lifetime and durable root-state primitives.
//!
//! This module has no service-registration or host-power operation. Production
//! callers must use one fixed root-owned `0700` directory and expected UID 0.
//! Tests may pass their own UID and an isolated `0700` temporary directory.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::Path;

use rustix::fs::{
    AtFlags, FileType, FlockOperation, Mode, OFlags, flock, fstat, fsync, open, openat, renameat,
    unlinkat,
};
use sha2::{Digest, Sha256};

use crate::recovery::{
    JOURNAL_SCHEMA_VERSION, JournalObservation, JournalPhase, RecoveryRecord, SleepObservation,
};

const LOCK_NAME: &str = "helper.lock";
const JOURNAL_NAME: &str = "recovery.journal";
const JOURNAL_MAGIC: [u8; 4] = *b"ZPJ1";
const JOURNAL_BYTES: usize = 144;
const MAX_JOURNAL_BYTES: u64 = 512;

#[derive(Debug)]
pub enum PersistenceError {
    Io(std::io::Error),
    AlreadyLocked,
    InsecureState(&'static str),
    InvalidRecord,
    InvalidTemporaryNonce,
}

impl From<std::io::Error> for PersistenceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// An open, validated state directory plus the process-lifetime kernel lock.
///
/// The lock file descriptor is `CLOEXEC` and remains owned by this value. No
/// journal method is available until the nonblocking exclusive lock succeeds.
pub struct LockedStateDirectory {
    directory: File,
    _lock: File,
    helper_instance: [u8; 16],
    expected_uid: u32,
}

impl LockedStateDirectory {
    pub fn acquire(
        path: &Path,
        expected_uid: u32,
        helper_instance: [u8; 16],
    ) -> Result<Self, PersistenceError> {
        if helper_instance == [0; 16] {
            return Err(PersistenceError::InvalidRecord);
        }
        let directory_fd = open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        validate_node(
            directory_fd.as_fd(),
            FileType::Directory,
            expected_uid,
            0o700,
            false,
        )?;
        let directory = File::from(directory_fd);

        let lock_fd = openat(
            &directory,
            LOCK_NAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io_error)?;
        validate_node(
            lock_fd.as_fd(),
            FileType::RegularFile,
            expected_uid,
            0o600,
            true,
        )?;
        match flock(&lock_fd, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                return Err(PersistenceError::AlreadyLocked);
            }
            Err(error) => return Err(PersistenceError::Io(io_error(error))),
        }
        let lock = File::from(lock_fd);

        Ok(Self {
            directory,
            _lock: lock,
            helper_instance,
            expected_uid,
        })
    }

    pub fn helper_instance(&self) -> [u8; 16] {
        self.helper_instance
    }

    pub fn read_journal(&self) -> Result<JournalObservation, PersistenceError> {
        let fd = match openat(
            &self.directory,
            JOURNAL_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(error) if error == rustix::io::Errno::NOENT => {
                return Ok(JournalObservation::Missing);
            }
            Err(error) => return Err(PersistenceError::Io(io_error(error))),
        };
        let stat = validate_node(
            fd.as_fd(),
            FileType::RegularFile,
            self.expected_uid,
            0o600,
            true,
        )?;
        if stat.st_size < 0 || stat.st_size as u64 > MAX_JOURNAL_BYTES {
            return Ok(JournalObservation::Corrupt);
        }
        let mut file = File::from(fd);
        let mut bytes = Vec::with_capacity(stat.st_size as usize);
        file.read_to_end(&mut bytes)?;
        Ok(decode_record(&bytes))
    }

    /// Atomically and durably replace the journal inside the already-open state
    /// directory. `temporary_nonce` must be a new random value for this attempt.
    pub fn write_journal(
        &self,
        record: RecoveryRecord,
        temporary_nonce: [u8; 16],
    ) -> Result<(), PersistenceError> {
        if record.helper_instance != self.helper_instance || !record_is_valid(record) {
            return Err(PersistenceError::InvalidRecord);
        }
        if temporary_nonce == [0; 16] {
            return Err(PersistenceError::InvalidTemporaryNonce);
        }
        let temporary_name = format!("recovery.tmp.{}", hex(&temporary_nonce));
        let temporary_fd = openat(
            &self.directory,
            temporary_name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io_error)?;
        validate_node(
            temporary_fd.as_fd(),
            FileType::RegularFile,
            self.expected_uid,
            0o600,
            true,
        )?;
        let mut temporary = File::from(temporary_fd);
        let result = (|| {
            temporary.write_all(&encode_record(record))?;
            temporary.sync_all()?;
            renameat(
                &self.directory,
                temporary_name.as_str(),
                &self.directory,
                JOURNAL_NAME,
            )
            .map_err(io_error)?;
            fsync(&self.directory).map_err(io_error)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = unlinkat(&self.directory, temporary_name.as_str(), AtFlags::empty());
        }
        result
    }

    /// Durably remove a validated journal. Missing is an error: callers must
    /// not turn lost ownership evidence into a successful restoration claim.
    pub fn clear_journal(&self) -> Result<(), PersistenceError> {
        match self.read_journal()? {
            JournalObservation::Valid(_) => {}
            _ => return Err(PersistenceError::InvalidRecord),
        }
        unlinkat(&self.directory, JOURNAL_NAME, AtFlags::empty()).map_err(io_error)?;
        fsync(&self.directory).map_err(io_error)?;
        Ok(())
    }
}

fn validate_node<Fd: AsFd>(
    fd: Fd,
    file_type: FileType,
    expected_uid: u32,
    expected_mode: u32,
    require_single_link: bool,
) -> Result<rustix::fs::Stat, PersistenceError> {
    let stat = fstat(fd).map_err(io_error)?;
    if FileType::from_raw_mode(stat.st_mode) != file_type {
        return Err(PersistenceError::InsecureState("wrong file type"));
    }
    if stat.st_uid != expected_uid {
        return Err(PersistenceError::InsecureState("wrong owner"));
    }
    if u32::from(stat.st_mode & 0o777) != expected_mode {
        return Err(PersistenceError::InsecureState("wrong permissions"));
    }
    if require_single_link && stat.st_nlink != 1 {
        return Err(PersistenceError::InsecureState("unexpected hard links"));
    }
    Ok(stat)
}

fn io_error(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

fn record_is_valid(record: RecoveryRecord) -> bool {
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

fn encode_record(record: RecoveryRecord) -> [u8; JOURNAL_BYTES] {
    let mut payload = Vec::with_capacity(JOURNAL_BYTES - 32);
    payload.extend_from_slice(&JOURNAL_MAGIC);
    payload.extend_from_slice(&record.schema_version.to_be_bytes());
    payload.extend_from_slice(&record.owner);
    payload.extend_from_slice(&record.host_boot_id);
    payload.extend_from_slice(&record.helper_instance);
    payload.extend_from_slice(&record.engine_incarnation);
    payload.extend_from_slice(&record.lease_id);
    payload.extend_from_slice(&record.consent_generation.to_be_bytes());
    payload.extend_from_slice(&record.hard_deadline_millis.to_be_bytes());
    payload.extend_from_slice(&record.generation.to_be_bytes());
    payload.push(match record.prior_state {
        SleepObservation::Enabled => 1,
        SleepObservation::Disabled => 2,
        SleepObservation::Unknown => 3,
    });
    payload.push(match record.phase {
        JournalPhase::Prepared => 1,
        JournalPhase::Owned => 2,
        JournalPhase::Restoring => 3,
    });
    debug_assert_eq!(payload.len(), JOURNAL_BYTES - 32);
    let digest = Sha256::digest(&payload);
    payload.extend_from_slice(&digest);
    payload.try_into().expect("fixed journal length")
}

fn decode_record(bytes: &[u8]) -> JournalObservation {
    if bytes.len() != JOURNAL_BYTES || bytes[..4] != JOURNAL_MAGIC {
        return JournalObservation::Corrupt;
    }
    let (payload, recorded_digest) = bytes.split_at(JOURNAL_BYTES - 32);
    if Sha256::digest(payload).as_slice() != recorded_digest {
        return JournalObservation::Corrupt;
    }
    let schema_version = u16::from_be_bytes([bytes[4], bytes[5]]);
    if schema_version != JOURNAL_SCHEMA_VERSION {
        return JournalObservation::UnsupportedSchema;
    }
    let mut at = 6;
    let mut array = || {
        let value = bytes[at..at + 16].try_into().expect("fixed checked length");
        at += 16;
        value
    };
    let owner = array();
    let host_boot_id = array();
    let helper_instance = array();
    let engine_incarnation = array();
    let lease_id = array();
    let consent_generation = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap());
    at += 8;
    let hard_deadline_millis = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap());
    at += 8;
    let generation = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap());
    at += 8;
    let prior_state = match bytes[at] {
        1 => SleepObservation::Enabled,
        2 => SleepObservation::Disabled,
        3 => SleepObservation::Unknown,
        _ => return JournalObservation::Corrupt,
    };
    at += 1;
    let phase = match bytes[at] {
        1 => JournalPhase::Prepared,
        2 => JournalPhase::Owned,
        3 => JournalPhase::Restoring,
        _ => return JournalObservation::Corrupt,
    };
    let record = RecoveryRecord {
        schema_version,
        owner,
        host_boot_id,
        helper_instance,
        engine_incarnation,
        lease_id,
        consent_generation,
        hard_deadline_millis,
        generation,
        prior_state,
        phase,
    };
    if record_is_valid(record) {
        JournalObservation::Valid(record)
    } else {
        JournalObservation::Corrupt
    }
}

fn hex(bytes: &[u8; 16]) -> String {
    let mut value = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("String write");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    fn directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    fn record(helper_instance: [u8; 16], phase: JournalPhase) -> RecoveryRecord {
        RecoveryRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            owner: [1; 16],
            host_boot_id: [2; 16],
            helper_instance,
            engine_incarnation: [4; 16],
            lease_id: [5; 16],
            consent_generation: 6,
            hard_deadline_millis: 10_000,
            generation: 7,
            prior_state: SleepObservation::Enabled,
            phase,
        }
    }

    #[test]
    fn exclusive_lock_rejects_a_second_live_helper() {
        let directory = directory();
        let first = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
        assert!(matches!(
            LockedStateDirectory::acquire(directory.path(), uid(), [4; 16]),
            Err(PersistenceError::AlreadyLocked)
        ));
        drop(first);
        LockedStateDirectory::acquire(directory.path(), uid(), [4; 16])
            .expect("lock is available only after the first lifetime ends");
    }

    #[test]
    fn journal_round_trip_replace_and_clear_are_bounded() {
        let directory = directory();
        let locked = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
        assert_eq!(locked.read_journal().unwrap(), JournalObservation::Missing);
        locked
            .write_journal(record([3; 16], JournalPhase::Prepared), [8; 16])
            .unwrap();
        assert_eq!(
            locked.read_journal().unwrap(),
            JournalObservation::Valid(record([3; 16], JournalPhase::Prepared))
        );
        locked
            .write_journal(record([3; 16], JournalPhase::Owned), [9; 16])
            .unwrap();
        assert_eq!(
            locked.read_journal().unwrap(),
            JournalObservation::Valid(record([3; 16], JournalPhase::Owned))
        );
        locked.clear_journal().unwrap();
        assert_eq!(locked.read_journal().unwrap(), JournalObservation::Missing);
    }

    #[test]
    fn linked_wrong_mode_and_corrupt_state_fail_closed() {
        let directory = directory();
        let locked = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
        symlink("elsewhere", directory.path().join(JOURNAL_NAME)).unwrap();
        assert!(matches!(
            locked.read_journal(),
            Err(PersistenceError::Io(_))
        ));
        fs::remove_file(directory.path().join(JOURNAL_NAME)).unwrap();
        fs::write(directory.path().join(JOURNAL_NAME), b"corrupt").unwrap();
        fs::set_permissions(
            directory.path().join(JOURNAL_NAME),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(locked.read_journal().unwrap(), JournalObservation::Corrupt);
        fs::set_permissions(
            directory.path().join(JOURNAL_NAME),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(matches!(
            locked.read_journal(),
            Err(PersistenceError::InsecureState("wrong permissions"))
        ));
    }

    #[test]
    fn writer_identity_and_temporary_nonce_are_mandatory() {
        let directory = directory();
        let locked = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
        assert!(matches!(
            locked.write_journal(record([4; 16], JournalPhase::Prepared), [8; 16]),
            Err(PersistenceError::InvalidRecord)
        ));
        assert!(matches!(
            locked.write_journal(record([3; 16], JournalPhase::Prepared), [0; 16]),
            Err(PersistenceError::InvalidTemporaryNonce)
        ));
        assert_eq!(locked.read_journal().unwrap(), JournalObservation::Missing);
    }

    #[test]
    fn insecure_directory_is_rejected_before_lock_or_journal_access() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]),
            Err(PersistenceError::InsecureState("wrong permissions"))
        ));
        assert!(!directory.path().join(LOCK_NAME).exists());
    }

    #[test]
    fn checksum_detects_tampering() {
        let original = record([3; 16], JournalPhase::Owned);
        let mut encoded = encode_record(original);
        encoded[20] ^= 1;
        assert_eq!(decode_record(&encoded), JournalObservation::Corrupt);
    }
}
