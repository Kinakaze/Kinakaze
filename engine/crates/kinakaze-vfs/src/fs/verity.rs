//! Persistent fs-verity metadata belongs to the native inode, not its pathname.
//!
//! A native EA stores a versioned transaction state and the Linux descriptor;
//! Merkle blocks occupy a hidden, block-aligned tail of the same data stream.
//! Thus hard links, rename, unlink, handle duplication and fork retain one
//! identity without a broker, process-local registry, or pinned sidecar stream.
//! PREPARING is flushed before extending native EOF. Its recorded logical EOF
//! remains authoritative until either an ENABLED descriptor is published or
//! recovery has truncated and flushed the unpublished tail. Guest reads, stat,
//! seeks and mappings must never use native EOF without checking this record.

use super::{ea, object::Object};
use crate::{
    EBADF, EBUSY, EEXIST, EINVAL, EIO, EISDIR, ENOTTY, EOPNOTSUPP, EPERM, FdFlags, FdKind,
};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_READ_EA, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA,
    GetFileSizeEx,
};

pub(crate) mod merkle;
pub use merkle::Descriptor;
mod transaction;
pub use transaction::{Publication, Transaction, TransactionId, TransactionState, publication};

const EA_NAME: &[u8] = b"KINAKAZE.LINUX.VERITY";
const MAGIC: &[u8; 8] = b"CYVERIT2";
const PREPARING: u8 = 1;
const ENABLED: u8 = 2;
const BUILT: u8 = 3;
const ETXTBSY: i32 = 26;
const ENODATA: i32 = 61;
const QUERY_ACCESS: u32 = FILE_READ_ATTRIBUTES | FILE_READ_EA;

/// Cross-process inode transaction guard. It owns only a temporary kernel
/// mutex handle; no mutable process state or resident process is introduced.
pub struct Guard {
    _native: crate::xattr::InodeLock,
}

/// Hold across metadata absence checks and native EOF/section creation. Native
/// mutex ownership is recursive for same-thread helper calls while held.
pub fn lock(handle: HANDLE) -> Result<Guard, i32> {
    // Linux write-only descriptors can participate in inode transactions.
    // GENERIC_WRITE need not include native FILE_READ_ATTRIBUTES, which the
    // inode identity query requires. Select a metadata-only query handle from
    // the granted rights, without retrying a failed lock or acquiring data read
    // access on behalf of the caller.
    let query;
    let identity = if Object::granted_access(handle)? & FILE_READ_ATTRIBUTES == 0 {
        query = Object::reopen(handle, QUERY_ACCESS)?;
        query.raw()
    } else {
        handle
    };
    crate::xattr::InodeLock::acquire(identity).map(|native| Guard { _native: native })
}

#[derive(Clone, Debug)]
struct Record {
    state: u8,
    transaction: TransactionId,
    descriptor: Descriptor,
    tree_offset: u64,
    tree_size: u64,
}

impl Record {
    fn new(state: u8, descriptor: Descriptor) -> Result<Self, i32> {
        Self::with_transaction(state, descriptor, TransactionId::new()?)
    }

    fn with_transaction(
        state: u8,
        descriptor: Descriptor,
        transaction: TransactionId,
    ) -> Result<Self, i32> {
        let block = descriptor.block_size() as u64;
        let tree_offset = descriptor
            .data_size()
            .checked_add(block - 1)
            .ok_or(crate::EOVERFLOW)?
            & !(block - 1);
        let tree_size = descriptor.tree_size()?;
        let end = tree_offset.checked_add(tree_size).ok_or(crate::EOVERFLOW)?;
        if end > i64::MAX as u64 {
            return Err(crate::EOVERFLOW);
        }
        Ok(Self {
            state,
            transaction,
            descriptor,
            tree_offset,
            tree_size,
        })
    }

    fn encode(&self) -> [u8; 320] {
        let mut bytes = [0u8; 320];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8] = self.state;
        bytes[16..24].copy_from_slice(&self.tree_offset.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.tree_size.to_le_bytes());
        bytes[32..48].copy_from_slice(self.transaction.as_bytes());
        bytes[64..].copy_from_slice(self.descriptor.bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() != 320
            || &bytes[..8] != MAGIC
            || !matches!(bytes[8], PREPARING | BUILT | ENABLED)
            || bytes[9..16] != [0; 7]
            || bytes[48..64] != [0; 16]
        {
            return Err(EIO);
        }
        let descriptor = Descriptor::decode(&bytes[64..]).map_err(|_| EIO)?;
        let transaction =
            TransactionId::from_bytes(bytes[32..48].try_into().unwrap()).map_err(|_| EIO)?;
        let record = Self::with_transaction(bytes[8], descriptor, transaction).map_err(|_| EIO)?;
        if u64::from_le_bytes(bytes[16..24].try_into().unwrap()) != record.tree_offset
            || u64::from_le_bytes(bytes[24..32].try_into().unwrap()) != record.tree_size
        {
            return Err(EIO);
        }
        Ok(record)
    }
}

fn read_record(object: &Object) -> Result<Option<Record>, i32> {
    match ea::read(object, EA_NAME) {
        Ok(record) => record.map(|bytes| Record::decode(&bytes)).transpose(),
        // A filesystem without EAs cannot contain this backend's verity state.
        // This is absence of the filesystem feature, not an I/O-error fallback.
        Err(EOPNOTSUPP) => Ok(None),
        Err(error) => Err(error),
    }
}

fn native_size(object: &Object) -> Result<u64, i32> {
    let mut size = 0i64;
    if unsafe { GetFileSizeEx(object.raw(), &mut size) } == 0 {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    u64::try_from(size).map_err(|_| EIO)
}

fn exact_read(object: &Object, offset: u64, bytes: &mut [u8]) -> Result<(), i32> {
    let mut done = 0;
    while done < bytes.len() {
        let count = object.read_at(
            offset.checked_add(done as u64).ok_or(EINVAL)?,
            &mut bytes[done..],
        )?;
        if count == 0 {
            return Err(EIO);
        }
        done += count;
    }
    Ok(())
}

fn exact_write(object: &Object, offset: u64, bytes: &[u8]) -> Result<(), i32> {
    let mut done = 0;
    while done < bytes.len() {
        let count = object.write_at(
            offset.checked_add(done as u64).ok_or(EINVAL)?,
            &bytes[done..],
        )?;
        if count == 0 {
            return Err(EIO);
        }
        done += count;
    }
    Ok(())
}

fn fatal_interruption() -> Result<(), i32> {
    if crate::signal::fatal_pending()? {
        Err(crate::EINTR)
    } else {
        Ok(())
    }
}

/// Caller holds the inode mutex and a writable private inode handle. The marker
/// must survive any failed cleanup, so a later process cannot expose the tail.
fn recover(object: &Object, record: &Record) -> Result<(), i32> {
    recover_with(object, record, |_| Ok(()))
}

fn recover_with(
    object: &Object,
    record: &Record,
    checkpoint: impl Fn(transaction::Checkpoint) -> Result<(), i32>,
) -> Result<(), i32> {
    use transaction::Checkpoint;
    if !matches!(record.state, PREPARING | BUILT) {
        return Err(EPERM);
    }
    object.suppress_data_times()?;
    checkpoint(Checkpoint::RollbackTruncate)?;
    object.set_length(record.descriptor.data_size())?;
    checkpoint(Checkpoint::RollbackFlush)?;
    object.flush()?;
    checkpoint(Checkpoint::RollbackDelete)?;
    ea::write(object, EA_NAME, &[])?;
    checkpoint(Checkpoint::RollbackFinalFlush)?;
    object.flush()
}

/// Read the committed Linux descriptor. A malformed record is an I/O error;
/// an interrupted enable is not represented as a successful verity file.
/// The caller keeps `handle` live through this call.
pub fn descriptor(handle: HANDLE) -> Result<Option<Descriptor>, i32> {
    let object = Object::reopen(handle, QUERY_ACCESS)?;
    Ok(read_record(&object)?
        .filter(|record| record.state == ENABLED)
        .map(|record| record.descriptor))
}

/// Includes PREPARING: unpublished tree bytes must never change guest st_size.
/// The caller keeps `handle` live through this call.
pub fn logical_size(handle: HANDLE) -> Result<Option<u64>, i32> {
    let object = Object::reopen(handle, QUERY_ACCESS)?;
    Ok(read_record(&object)?.map(|record| record.descriptor.data_size()))
}

/// Atomic logical-size query for a regular file, including ordinary files that
/// may begin an enable transaction concurrently with this query.
pub fn authoritative_size(handle: HANDLE) -> Result<u64, i32> {
    let object = Object::reopen(handle, QUERY_ACCESS)?;
    authoritative_size_object(&object)
}

/// Reuse a private metadata open with QUERY_ACCESS. Its EA query/cancellation
/// cannot affect a borrowed descriptor's concurrent data I/O.
pub(crate) fn authoritative_size_object(object: &Object) -> Result<u64, i32> {
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    match read_record(object)? {
        Some(record) => Ok(record.descriptor.data_size()),
        None => native_size(object),
    }
}

/// Call after acquiring native write access, and before truncate/write/map.
/// Acquisition races with enable are closed by the enable handle's deny-write
/// share reservation. An orphan PREPARING transaction is recovered under the
/// same cross-process inode mutex used by enable; no process liveness guesses.
pub fn ensure_writable(handle: HANDLE) -> Result<(), i32> {
    let query = Object::reopen(handle, QUERY_ACCESS)?;
    let Some(record) = read_record(&query)? else {
        return Ok(());
    };
    if record.state == ENABLED {
        return Err(EPERM);
    }
    let _lock = crate::xattr::InodeLock::acquire(query.raw())?;
    let Some(record) = read_record(&query)? else {
        return Ok(());
    };
    if record.state == ENABLED {
        return Err(EPERM);
    }
    let writer = Object::reopen(handle, QUERY_ACCESS | GENERIC_WRITE | FILE_WRITE_EA)?;
    recover(&writer, &record)
}

/// A short-lived pinned filesystem description for one ioctl or mmap operation.
/// Validation, descriptor generation and native handle duplication observe one
/// fd-table entry. Closing/reusing the fd never changes this object's inode.
/// No native Object ownership or mutable fd-table entry escapes this interface.
#[derive(Debug)]
pub struct Opened {
    object: Object,
    directory: bool,
    generation: u32,
    overlay: Option<crate::mount::overlay::Description>,
    noatime: bool,
    native: Option<crate::mount::native::Description>,
}

impl Opened {
    pub fn from_fd(fd: i32) -> Result<Self, i32> {
        let mut overlay = None;
        let mut native = None;
        let (object, entry) = Object::from_fd_checked(fd, |entry| {
            if entry.flags.contains(FdFlags::PATH_ONLY) {
                return Err(EBADF);
            }
            if !matches!(entry.kind, FdKind::File | FdKind::Directory) || entry.raw == 0 {
                return Err(ENOTTY);
            }
            overlay = crate::mount::overlay::reference(entry)?;
            native = crate::mount::native::reference(entry)?;
            Ok(())
        })?;
        let noatime = entry.flags.contains(FdFlags::NOATIME);
        let object = if overlay.is_some() || noatime {
            let object = Object::reopen(
                object.raw(),
                Object::granted_access(object.raw())? | FILE_WRITE_ATTRIBUTES,
            )?;
            object.suppress_atime()?;
            object
        } else {
            object
        };
        Ok(Self {
            noatime,
            native,
            object,
            directory: entry.kind == FdKind::Directory,
            generation: entry.generation,
            overlay,
        })
    }

    pub fn accessed(&self) {
        if !self.noatime {
            if let Some(d) = &self.overlay {
                d.accessed();
            }
        }
    }

    pub fn volatile_state(&self) -> Result<Option<(u64, u64)>, i32> {
        self.overlay
            .as_ref()
            .map(|d| d.volatile_state())
            .transpose()
            .map(Option::flatten)
    }

    pub fn is_directory(&self) -> bool {
        self.directory
    }

    /// Borrow for the lifetime of this pin. Never close or retain the returned
    /// handle; a surviving VMA must own its section or independently pin it.
    pub fn borrowed_handle(&self) -> HANDLE {
        self.object.raw()
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn mount_noexec(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(|d| d.mount_flags() & 8 != 0)
            || self
                .native
                .as_ref()
                .is_some_and(|d| d.policy.flags() & 8 != 0)
    }

    /// Independently retain the native mount-writer open for a surviving VMA.
    pub fn mount_writer(&self) -> Result<Vec<std::os::windows::io::OwnedHandle>, i32> {
        use std::os::windows::io::FromRawHandle;
        let mut writers = self
            .overlay
            .as_ref()
            .map(|d| d.mapping_writer())
            .transpose()?
            .unwrap_or_default();
        if let Some(writer) = self.native.as_ref().and_then(|d| d.writer.as_ref()) {
            writers.push(unsafe {
                std::os::windows::io::OwnedHandle::from_raw_handle(
                    Object::duplicate(writer.raw())?.into_raw(),
                )
            });
        }
        Ok(writers)
    }

    pub fn descriptor(&self) -> Result<Option<Descriptor>, i32> {
        descriptor(self.object.raw())
    }

    pub fn measure(&self) -> Result<(u32, Vec<u8>), i32> {
        if self.directory {
            return Err(ENODATA);
        }
        let descriptor = self.descriptor()?.ok_or(ENODATA)?;
        Ok((descriptor.algorithm(), descriptor.digest()))
    }

    pub fn read_metadata(&self, kind: u64, offset: u64, bytes: &mut [u8]) -> Result<usize, i32> {
        offset.checked_add(bytes.len() as u64).ok_or(EINVAL)?;
        if self.directory {
            return Err(ENODATA);
        }
        read_metadata_object(&self.object, kind, offset, bytes)
    }
}

/// Storage-only convenience entry used by native backend tests. This is NOT a
/// production VMA barrier: production callers must retain an explicit
/// Transaction across global participant parking, staging and publication.
/// The libc ENABLE gate must stay until all mapping/fork paths participate.
pub fn enable(fd: i32, algorithm: u32, block_size: u32, salt: &[u8]) -> Result<(), i32> {
    Descriptor::new(algorithm, block_size, salt, 0)?;
    let mut transaction = Opened::from_fd(fd)?.prepare(algorithm, block_size, salt)?;
    if let Err(error) = transaction.build().and_then(|()| transaction.commit()) {
        if !matches!(
            transaction.state(),
            TransactionState::Published | TransactionState::Committed
        ) {
            transaction.abort()?;
        }
        return Err(error);
    }
    Ok(())
}

/// Read through the pinned native inode, verifying every returned data block.
/// All regular-file reads take the inode lock even before a verity record
/// exists: otherwise absence-check + native read could race the first hidden
/// tail append. None is reserved for callers' dispatch API; this backend returns
/// Some even for an ordinary file and never exposes unverified metadata bytes.
pub fn verified_read(handle: HANDLE, offset: u64, bytes: &mut [u8]) -> Result<Option<usize>, i32> {
    if Object::granted_access(handle)? & FILE_READ_DATA == 0 {
        return Err(EBADF);
    }
    let object = Object::reopen(handle, GENERIC_READ | QUERY_ACCESS)?;
    read_object(&object, offset, bytes)
}

/// Internal copy-up reads have the same integrity boundary as guest reads, but
/// must not change the lower inode's atime. Each private read handle suppresses
/// its own automatic access timestamp; unrelated real readers remain unchanged.
pub(crate) fn verified_read_preserving_atime(
    handle: HANDLE,
    offset: u64,
    bytes: &mut [u8],
) -> Result<Option<usize>, i32> {
    if Object::granted_access(handle)? & FILE_READ_DATA == 0 {
        return Err(EBADF);
    }
    let object = Object::reopen(handle, GENERIC_READ | QUERY_ACCESS | FILE_WRITE_ATTRIBUTES)?;
    object.suppress_atime()?;
    read_object(&object, offset, bytes)
}

/// The caller owns a private asynchronous handle with data/EA read access.
/// Borrowed guest descriptors must use `verified_read` to isolate cancellation.
pub(crate) fn read_object(
    object: &Object,
    offset: u64,
    bytes: &mut [u8],
) -> Result<Option<usize>, i32> {
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    let record = read_record(&object)?;
    let size = match &record {
        Some(record) => record.descriptor.data_size(),
        None => native_size(&object)?,
    };
    let length = bytes
        .len()
        .min(size.saturating_sub(offset).min(usize::MAX as u64) as usize);
    if length == 0 {
        return Ok(Some(0));
    }
    let Some(record) = record.filter(|record| record.state == ENABLED) else {
        return object.read_at(offset, &mut bytes[..length]).map(Some);
    };
    read_verified_record(object, &record, offset, &mut bytes[..length]).map(Some)
}

/// Read a completed but not necessarily published descriptor using a retained
/// native inode and serialized transaction identity. No process-local owner is
/// required. A stale identity is ESTALE; PREPARING is EBUSY, never a fake root.
pub fn staged_descriptor(handle: HANDLE, transaction: TransactionId) -> Result<Descriptor, i32> {
    let object = Object::reopen(handle, QUERY_ACCESS)?;
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    Ok(staged_record(&object, transaction)?.descriptor)
}

fn staged_record(object: &Object, transaction: TransactionId) -> Result<Record, i32> {
    let record = read_record(object)?.ok_or(116)?; // ESTALE
    if record.transaction != transaction {
        return Err(116);
    }
    if record.state == PREPARING {
        return Err(EBUSY);
    }
    Ok(record)
}

/// Authenticate staged data before any VMA participant exposes it. The lock
/// covers record validation, Merkle reads and copying so abort/replacement can
/// never mix roots or leak hidden bytes. Each call owns a separate asynchronous
/// read handle; peers can read concurrently without sharing an I/O status block.
/// This is an internal mapping transition, so it preserves the inode's atime.
pub fn staged_verified_read(
    handle: HANDLE,
    transaction: TransactionId,
    offset: u64,
    bytes: &mut [u8],
) -> Result<usize, i32> {
    if Object::granted_access(handle)? & FILE_READ_DATA == 0 {
        return Err(EBADF);
    }
    let object = Object::reopen(handle, GENERIC_READ | QUERY_ACCESS | FILE_WRITE_ATTRIBUTES)?;
    object.suppress_atime()?;
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    let record = staged_record(&object, transaction)?;
    read_verified_record(&object, &record, offset, bytes)
}

fn read_verified_record(
    object: &Object,
    record: &Record,
    offset: u64,
    bytes: &mut [u8],
) -> Result<usize, i32> {
    let size = record.descriptor.data_size();
    let length = bytes
        .len()
        .min(size.saturating_sub(offset).min(usize::MAX as u64) as usize);
    let block_size = record.descriptor.block_size();
    let mut block = vec![0u8; block_size];
    let mut done = 0usize;
    while done < length {
        let position = offset.checked_add(done as u64).ok_or(EINVAL)?;
        let index = position / block_size as u64;
        let start = index * block_size as u64;
        let valid = (size - start).min(block_size as u64) as usize;
        block.fill(0);
        let result = exact_read(&object, start, &mut block[..valid]).and_then(|()| {
            record
                .descriptor
                .verify_block(index, &block, |tree_position, bytes| {
                    exact_read(
                        &object,
                        record.tree_offset.checked_add(tree_position).ok_or(EIO)?,
                        bytes,
                    )
                })
        });
        if let Err(error) = result {
            return if done == 0 { Err(error) } else { Ok(done) };
        }
        let within = (position - start) as usize;
        let count = (valid - within).min(length - done);
        bytes[done..done + count].copy_from_slice(&block[within..within + count]);
        done += count;
    }
    Ok(done)
}

pub fn measure(fd: i32) -> Result<(u32, Vec<u8>), i32> {
    Opened::from_fd(fd)?.measure()
}

pub fn read_metadata(fd: i32, kind: u64, offset: u64, bytes: &mut [u8]) -> Result<usize, i32> {
    Opened::from_fd(fd)?.read_metadata(kind, offset, bytes)
}

fn read_metadata_object(
    pin: &Object,
    kind: u64,
    offset: u64,
    bytes: &mut [u8],
) -> Result<usize, i32> {
    offset.checked_add(bytes.len() as u64).ok_or(EINVAL)?;
    let object = Object::reopen(pin.raw(), GENERIC_READ | QUERY_ACCESS)?;
    let record = read_record(&object)?
        .filter(|record| record.state == ENABLED)
        .ok_or(ENODATA)?;
    match kind {
        1 => {
            let length = bytes.len().min(
                record
                    .tree_size
                    .saturating_sub(offset)
                    .min(usize::MAX as u64) as usize,
            );
            if length == 0 {
                return Ok(0);
            }
            object.read_at(
                record.tree_offset.checked_add(offset).ok_or(EINVAL)?,
                &mut bytes[..length],
            )
        }
        2 => {
            let descriptor = record.descriptor.bytes();
            let length = bytes
                .len()
                .min((descriptor.len() as u64).saturating_sub(offset) as usize);
            if length > 0 {
                bytes[..length]
                    .copy_from_slice(&descriptor[offset as usize..offset as usize + length]);
            }
            Ok(length)
        }
        3 => Err(ENODATA), // Builtin signatures are not configured, never forged.
        _ => Err(EINVAL),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ads_probe;
