//! Explicit storage transaction ownership for the cross-process VMA barrier.
//! No method returns with the inode mutex held, and errors do not release the
//! deny-write reservation. The coordinator owns this object until every VMA has
//! installed verified storage or completed rollback. Drop only closes a handle;
//! it never performs blocking I/O or dispatches guest signal handlers.

use super::*;

fn built_storage_present(writer: &Object, record: &Record) -> Result<bool, i32> {
    let required = if record.tree_size == 0 {
        record.descriptor.data_size()
    } else {
        record
            .tree_offset
            .checked_add(record.tree_size)
            .ok_or(EIO)?
    };
    Ok(native_size(writer)? >= required)
}

/// Opaque 128-bit native random identity, retained in every phase of one EA.
/// Serialize these bytes, not a Rust owner or a process-local handle, to peers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionId([u8; 16]);

impl TransactionId {
    pub(super) fn new() -> Result<Self, i32> {
        let mut bytes = [0u8; 16];
        let status =
            unsafe { crate::BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 16, 2) };
        if status < 0 || bytes == [0; 16] {
            return Err(EIO);
        }
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Result<Self, i32> {
        if bytes == [0; 16] {
            Err(EINVAL)
        } else {
            Ok(Self(bytes))
        }
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// A snapshot of the authoritative EA, not proof that a failed flush reached
/// stable storage. A recovery owner must reconcile this while excluding a live
/// coordinator through the global epoch protocol and retaining the native inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    Absent,
    Preparing(TransactionId),
    Built(TransactionId),
    Enabled(TransactionId),
}

pub fn publication(handle: HANDLE) -> Result<Publication, i32> {
    let query = Object::reopen(handle, QUERY_ACCESS)?;
    let _lock = crate::xattr::InodeLock::acquire(query.raw())?;
    Ok(match read_record(&query)? {
        None => Publication::Absent,
        Some(record) => match record.state {
            PREPARING => Publication::Preparing(record.transaction),
            BUILT => Publication::Built(record.transaction),
            ENABLED => Publication::Enabled(record.transaction),
            _ => return Err(EIO),
        },
    })
}

/// Local completion knowledge. Published means ENABLED is visible, but its
/// final flush has not succeeded; it must never be treated as an abortable tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionState {
    Prepared,
    Built,
    Published,
    Committed,
    Aborted,
    /// A failed metadata operation needs authoritative EA inspection. Neither
    /// successful rollback nor durable publication may be inferred from errno.
    Indeterminate,
}

/// Owns the deny-write native file object across prepare, participant parking,
/// Merkle construction, staged verification, publication and participant resume.
/// Methods require exclusive access; peers reopen their own asynchronous read
/// objects and use the inode + TransactionId staged-read API instead.
#[derive(Debug)]
pub struct Transaction {
    writer: Object,
    record: Record,
    state: TransactionState,
    #[cfg(test)]
    fault: std::cell::Cell<Option<(Checkpoint, i32)>>,
}

impl Opened {
    pub fn prepare(
        &self,
        algorithm: u32,
        block_size: u32,
        salt: &[u8],
    ) -> Result<Transaction, i32> {
        Descriptor::new(algorithm, block_size, salt, 0)?;
        if self.is_directory() {
            return Err(EISDIR);
        }
        let query = Object::reopen(self.borrowed_handle(), QUERY_ACCESS)?;
        let before = read_record(&query)?;
        if before
            .as_ref()
            .is_some_and(|record| record.state == ENABLED)
        {
            return Err(EEXIST);
        }
        let writer = match unsafe {
            Object::reopen_deny_write(
                self.borrowed_handle(),
                GENERIC_READ | GENERIC_WRITE | QUERY_ACCESS | FILE_WRITE_EA,
            )
        } {
            Err(ETXTBSY) if before.is_some() => return Err(EBUSY),
            result => result?,
        };
        writer.suppress_data_times()?;
        let guard = crate::xattr::InodeLock::acquire(writer.raw())?;
        if let Some(record) = read_record(&writer)? {
            if record.state == ENABLED {
                return Err(EEXIST);
            }
            // Acquiring deny-write proves no live transaction/recovery writer
            // owns this inode. No PID or abandoned-mutex heuristic is required.
            recover(&writer, &record)?;
        }
        fatal_interruption()?;
        let descriptor = Descriptor::new(algorithm, block_size, salt, native_size(&writer)?)?;
        let record = Record::new(PREPARING, descriptor)?;
        ea::write(&writer, EA_NAME, &record.encode())?;
        writer.flush()?;
        drop(guard);
        Ok(Transaction {
            writer,
            record,
            state: TransactionState::Prepared,
            #[cfg(test)]
            fault: std::cell::Cell::new(None),
        })
    }

    /// Reacquire a dead/dropped coordinator's storage reservation without
    /// guessing its publication outcome or generating a replacement identity.
    /// The caller must first own recovery in the global epoch protocol. Native
    /// sharing excludes a still-live storage owner even if its thread is idle.
    /// An ENABLED record returns Published, not Committed: commit must flush it
    /// before the coordinator can claim durable completion to participants.
    pub fn resume(&self, expected: TransactionId) -> Result<Transaction, i32> {
        if self.is_directory() {
            return Err(EISDIR);
        }
        let writer = unsafe {
            Object::reopen_deny_write(
                self.borrowed_handle(),
                GENERIC_READ | GENERIC_WRITE | QUERY_ACCESS | FILE_WRITE_EA,
            )
        }
        .map_err(|error| if error == ETXTBSY { EBUSY } else { error })?;
        writer.suppress_data_times()?;
        let guard = crate::xattr::InodeLock::acquire(writer.raw())?;
        let record = read_record(&writer)?.ok_or(116)?;
        if record.transaction != expected {
            return Err(116);
        }
        fatal_interruption()?;
        let state = match record.state {
            PREPARING => {
                // A predecessor may have died between the marker write and
                // its flush. No resumed build may append before this flush.
                writer.flush()?;
                TransactionState::Prepared
            }
            BUILT => {
                // Even if the prior owner died after writing BUILT but before
                // its flush completed, the tree itself was flushed first.
                if built_storage_present(&writer, &record)? {
                    writer.flush()?;
                    TransactionState::Built
                } else {
                    // A failed abort can leave BUILT after truncating the tree.
                    // Retain an owner so recovery can finish abort, not commit.
                    TransactionState::Indeterminate
                }
            }
            ENABLED => TransactionState::Published,
            _ => return Err(EIO),
        };
        drop(guard);
        Ok(Transaction {
            writer,
            record,
            state,
            #[cfg(test)]
            fault: std::cell::Cell::new(None),
        })
    }
}

impl Transaction {
    /// Borrowed only while this transaction is alive. Never close this handle
    /// or pass its numeric value to another process; transfer/pin inode identity.
    pub fn borrowed_handle(&self) -> HANDLE {
        self.writer.raw()
    }

    pub fn id(&self) -> TransactionId {
        self.record.transaction
    }

    pub fn state(&self) -> TransactionState {
        self.state
    }

    /// Reconcile an indeterminate metadata error without dropping the native
    /// reservation. A visible ENABLED record becomes Published, never a claim
    /// that the failed flush was durable. A visible BUILT record is flushed
    /// before returning Built. This method does not truncate or publish an EA.
    pub fn reconcile(&mut self) -> Result<TransactionState, i32> {
        let _lock = crate::xattr::InodeLock::acquire(self.writer.raw())?;
        let Some(record) = read_record(&self.writer)? else {
            if !matches!(
                self.state,
                TransactionState::Indeterminate | TransactionState::Aborted
            ) || native_size(&self.writer)? != self.record.descriptor.data_size()
            {
                return Err(EIO);
            }
            self.writer.flush()?;
            self.state = TransactionState::Aborted;
            return Ok(self.state);
        };
        if record.transaction != self.id() {
            return Err(116);
        }
        self.record = record;
        self.state = TransactionState::Indeterminate;
        self.state = match self.record.state {
            PREPARING => {
                self.writer.flush()?;
                TransactionState::Prepared
            }
            BUILT => {
                if !built_storage_present(&self.writer, &self.record)? {
                    return Err(EIO);
                }
                self.writer.flush()?;
                TransactionState::Built
            }
            ENABLED => TransactionState::Published,
            _ => return Err(EIO),
        };
        Ok(self.state)
    }

    pub fn descriptor(&self) -> Result<&Descriptor, i32> {
        match self.state {
            TransactionState::Built | TransactionState::Published | TransactionState::Committed => {
                Ok(&self.record.descriptor)
            }
            _ => Err(EBUSY),
        }
    }

    /// Call only after the global PARKED barrier: for small Merkle block sizes
    /// hidden tail bytes may occupy an original native VMA's EOF-padding page.
    /// The tree is built with bounded positional I/O while only the native
    /// deny-write reservation is held. Ordinary reads/stat and VMA participants
    /// can take the inode mutex throughout this potentially long operation.
    /// Failure retains PREPARING and this owner; explicitly abort or drop for
    /// later crash recovery. No signal handler executes inside this operation.
    pub fn build(&mut self) -> Result<(), i32> {
        if self.state != TransactionState::Prepared {
            return Err(EINVAL);
        }
        {
            let _lock = crate::xattr::InodeLock::acquire(self.writer.raw())?;
            self.require_record(PREPARING)?;
        }
        fatal_interruption()?;
        let tree_offset = self.record.tree_offset;
        let descriptor = merkle::build(
            self.record.descriptor.clone(),
            |offset, bytes| {
                fatal_interruption()?;
                self.check(Checkpoint::Read)?;
                exact_read(&self.writer, offset, bytes)
            },
            |offset, bytes| {
                fatal_interruption()?;
                self.check(Checkpoint::Write)?;
                exact_write(
                    &self.writer,
                    tree_offset.checked_add(offset).ok_or(EINVAL)?,
                    bytes,
                )
            },
        )?;
        self.check(Checkpoint::TreeFlush)?;
        self.writer.flush()?;
        fatal_interruption()?;
        let _lock = crate::xattr::InodeLock::acquire(self.writer.raw())?;
        self.require_record(PREPARING)?;
        let mut built = self.record.clone();
        built.state = BUILT;
        built.descriptor = descriptor;
        self.check(Checkpoint::BuiltWrite)?;
        self.state = TransactionState::Indeterminate;
        ea::write(&self.writer, EA_NAME, &built.encode())?;
        self.record = built;
        self.check(Checkpoint::BuiltFlush)?;
        self.writer.flush()?;
        self.state = TransactionState::Built;
        Ok(())
    }

    /// Publish only after the caller has established the global PARKED barrier
    /// and staged all required verified mappings. On final-flush failure the EA
    /// remains ENABLED and state=Published; keep this reservation, reconcile the
    /// epoch and retry flush via commit. Never issue abort for that condition.
    /// Success does not drop the reservation or resume any participant.
    pub fn commit(&mut self) -> Result<(), i32> {
        if !matches!(
            self.state,
            TransactionState::Built | TransactionState::Published
        ) {
            return Err(EINVAL);
        }
        fatal_interruption()?;
        let _lock = crate::xattr::InodeLock::acquire(self.writer.raw())?;
        if self.state == TransactionState::Built {
            self.require_record(BUILT)?;
            if !built_storage_present(&self.writer, &self.record)? {
                self.state = TransactionState::Indeterminate;
                return Err(EIO);
            }
            self.check(Checkpoint::CommitWrite)?;
            let mut enabled = self.record.clone();
            enabled.state = ENABLED;
            self.state = TransactionState::Indeterminate;
            if let Err(error) = ea::write(&self.writer, EA_NAME, &enabled.encode())
                .and_then(|()| self.check(Checkpoint::CommitAfterWrite))
            {
                // A cancelled native request has been retired, but the syscall
                // error alone need not say whether its EA replacement landed.
                if let Ok(Some(record)) = read_record(&self.writer) {
                    if record.transaction == self.id()
                        && record.descriptor == self.record.descriptor
                    {
                        self.state = match record.state {
                            BUILT => TransactionState::Built,
                            ENABLED => TransactionState::Published,
                            _ => TransactionState::Indeterminate,
                        };
                        self.record = record;
                    }
                }
                return Err(error);
            }
            self.record = enabled;
            self.state = TransactionState::Published;
        } else {
            self.require_record(ENABLED)?;
        }
        self.check(Checkpoint::CommitFlush)?;
        self.writer.flush()?;
        self.state = TransactionState::Committed;
        Ok(())
    }

    /// Roll back only this unpublished identity. Failure retains the owner and
    /// the durable masking record until cleanup can finish; Drop will not retry
    /// under a caller's locks or deliver handlers. ENABLED is never truncated.
    pub fn abort(&mut self) -> Result<(), i32> {
        if matches!(
            self.state,
            TransactionState::Published | TransactionState::Committed
        ) {
            return Err(EPERM);
        }
        if self.state == TransactionState::Aborted {
            return Ok(());
        }
        let _lock = crate::xattr::InodeLock::acquire(self.writer.raw())?;
        let Some(record) = read_record(&self.writer)? else {
            // A previous abort may have cleared the EA after the truncation
            // flush, then failed its final flush. Retain the lease and finish
            // that flush, but never accept an exposed tail as completed cleanup.
            if self.state != TransactionState::Indeterminate
                || native_size(&self.writer)? != self.record.descriptor.data_size()
            {
                return Err(EIO);
            }
            self.writer.flush()?;
            self.state = TransactionState::Aborted;
            return Ok(());
        };
        if record.transaction != self.id() {
            return Err(116); // ESTALE: cannot clear a different enable attempt.
        }
        if record.state == ENABLED {
            self.record = record;
            self.state = TransactionState::Published;
            return Err(EPERM);
        }
        self.state = TransactionState::Indeterminate;
        recover_with(&self.writer, &record, |point| self.check(point))?;
        self.state = TransactionState::Aborted;
        Ok(())
    }

    fn require_record(&self, state: u8) -> Result<(), i32> {
        let record = read_record(&self.writer)?.ok_or(EIO)?;
        if record.transaction != self.id() {
            return Err(116);
        }
        if record.state != state || record.descriptor != self.record.descriptor {
            return Err(EIO);
        }
        Ok(())
    }

    #[inline]
    pub(super) fn check(&self, point: Checkpoint) -> Result<(), i32> {
        #[cfg(test)]
        if let Some((expected, error)) = self.fault.get() {
            if expected == point {
                self.fault.set(None);
                return Err(error);
            }
        }
        let _ = point;
        Ok(())
    }
}

// Per-owner test injection, never a global process setting. The underlying
// file, EA, flushing and recovery are real native operations in every test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Checkpoint {
    Read,
    Write,
    TreeFlush,
    BuiltWrite,
    BuiltFlush,
    CommitWrite,
    CommitAfterWrite,
    CommitFlush,
    RollbackTruncate,
    RollbackFlush,
    RollbackDelete,
    RollbackFinalFlush,
}

#[cfg(test)]
mod tests;
