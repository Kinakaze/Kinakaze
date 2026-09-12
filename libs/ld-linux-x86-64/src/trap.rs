//! Trap stubs for symbols nothing defines.
//!
//! This linker binds every relocation eagerly, so by default one missing symbol
//! makes the whole program unloadable. For a real binary that is the wrong
//! failure: BusyBox references the union of everything all its applets need, so
//! `ls` cannot start because `nslookup` might have wanted a resolver.
//!
//! glibc's lazy PLT binding does not have this problem — an unused symbol is never
//! resolved, so its absence costs nothing until something calls it. Binding to a
//! trap reproduces that property without implementing lazy binding: the program
//! loads, and a missing symbol reports itself by name at the moment it is actually
//! used.
//!
//! The distinction between code and data is load-bearing. A missing *function*
//! becomes a stub that prints its own name and aborts. A missing *data* object
//! cannot do that — reading through it is a plain memory load with nowhere to put a
//! diagnostic — so it points into a guard page instead, and the access faults
//! immediately rather than silently yielding zeros.

use std::sync::{Mutex, OnceLock};

use crate::LinkError;

/// How the linker treats a symbol nothing defines.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UnresolvedPolicy {
    /// Fail the load. Correct for a self-contained object where any gap is a bug.
    #[default]
    Fail,
    /// Bind to a trap and keep going, reporting the name if it is ever used.
    Trap,
}

/// Bytes reserved per stub. The generated sequence is 17 bytes; the rest is
/// padding so each stub starts at a predictable offset.
const STUB_SIZE: usize = 32;

/// How many stubs one page-run can hold before another is allocated.
const STUBS_PER_RUN: usize = 512;

struct TrapTable {
    /// Names, indexed by the id baked into each stub.
    names: Vec<String>,
    /// Executable runs holding the generated stubs.
    runs: Vec<usize>,
    /// A single page with no access, for missing data objects.
    guard_page: Option<usize>,
}

fn table() -> &'static Mutex<TrapTable> {
    static TABLE: OnceLock<Mutex<TrapTable>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(TrapTable {
            names: Vec::new(),
            runs: Vec::new(),
            guard_page: None,
        })
    })
}

/// Records a missing symbol and returns an address that traps when used.
///
/// `is_function` selects between an executable stub and the guard page.
pub fn trap_for(name: &str, is_function: bool) -> Result<usize, LinkError> {
    let mut table = table().lock().map_err(|_| LinkError::AddressOverflow)?;
    if !is_function {
        return guard_page(&mut table);
    }

    let id = table.names.len();
    static LOG_TRAPS: OnceLock<bool> = OnceLock::new();
    if *LOG_TRAPS.get_or_init(|| std::env::var_os("KINAKAZE_LOG_TRAPS").is_some()) {
        eprintln!("kinakaze: trapping unresolved symbol: {name} (id {id})");
    }
    table.names.push(name.to_owned());

    // Allocate a new run when the current one is full, or when there is none.
    if id % STUBS_PER_RUN == 0 {
        let run = allocate_executable(STUB_SIZE * STUBS_PER_RUN)?;
        table.runs.push(run);
    }
    let run = *table.runs.last().ok_or(LinkError::AddressOverflow)?;
    let address = run + (id % STUBS_PER_RUN) * STUB_SIZE;

    // SAFETY: the address is inside a run this module allocated as writable and
    // executable, with STUB_SIZE bytes available.
    unsafe { write_stub(address, id as u32) };
    Ok(address)
}

/// Reports how many symbols were trapped, and their names.
pub fn trapped_symbols() -> Vec<String> {
    table()
        .lock()
        .map(|table| table.names.clone())
        .unwrap_or_default()
}

fn guard_page(table: &mut TrapTable) -> Result<usize, LinkError> {
    if let Some(page) = table.guard_page {
        return Ok(page);
    }
    let page = allocate_no_access()?;
    table.guard_page = Some(page);
    Ok(page)
}

/// Emits `mov edi, id; mov rax, handler; jmp rax`.
///
/// The id is passed in `edi` because that is the first integer argument under the
/// SysV convention the guest is using, so the handler receives it as an ordinary
/// parameter.
///
/// # Safety
///
/// `address` must be writable and executable for `STUB_SIZE` bytes.
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn write_stub(address: usize, id: u32) {
    let handler = report_unimplemented as *const () as usize;
    let mut code = [0u8; STUB_SIZE];
    let mut at = 0;

    // bf <imm32>  : mov edi, id
    code[at] = 0xbf;
    at += 1;
    code[at..at + 4].copy_from_slice(&id.to_le_bytes());
    at += 4;

    // 48 b8 <imm64> : mov rax, handler
    code[at] = 0x48;
    code[at + 1] = 0xb8;
    at += 2;
    code[at..at + 8].copy_from_slice(&(handler as u64).to_le_bytes());
    at += 8;

    // ff e0 : jmp rax
    code[at] = 0xff;
    code[at + 1] = 0xe0;

    // SAFETY: the caller guarantees a writable, executable range of STUB_SIZE.
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u8, STUB_SIZE) };
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
unsafe fn write_stub(_address: usize, _id: u32) {}

/// Called by a stub when the guest uses a symbol nothing defined.
///
/// Aborting is the only defensible response: returning would let the caller act on
/// a value that was never produced, and the whole point of the trap is that the
/// failure is attributed to the right symbol instead of surfacing later as
/// corruption somewhere unrelated.
extern "sysv64" fn report_unimplemented(id: u32) -> ! {
    let name = table()
        .lock()
        .ok()
        .and_then(|table| table.names.get(id as usize).cloned())
        .unwrap_or_else(|| format!("<trap id {id}>"));
    eprintln!("kinakaze: guest called unimplemented symbol: {name}");
    let publish = TERMINATION_PUBLISHER.load(std::sync::atomic::Ordering::Acquire);
    if publish != 0 {
        // SAFETY: the loader installed a retained canonical provider export.
        let publish: unsafe extern "system" fn(i32) = unsafe { core::mem::transmute(publish) };
        unsafe { publish(6) }; // Linux SIGABRT, before the native process vanishes.
    }
    std::process::abort();
}

static TERMINATION_PUBLISHER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Installs the canonical libc's process-status publisher, outside loader locks.
/// The provider must remain loaded for this process's lifetime.
pub fn set_termination_publisher(publish: unsafe extern "system" fn(i32)) {
    TERMINATION_PUBLISHER.store(publish as usize, std::sync::atomic::Ordering::Release);
}

#[cfg(windows)]
fn allocate_executable(len: usize) -> Result<usize, LinkError> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
    };
    let _fork_mapping_transaction =
        kinakaze_runtime::begin_fork_mapping_transaction().ok_or(LinkError::MappingFailed {
            object: "<trap stubs>".to_owned(),
            len,
        })?;
    // SAFETY: requests a fresh private commit; a null base lets the OS choose.
    let block = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            len,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_EXECUTE_READWRITE,
        )
    };
    if block.is_null() {
        return Err(LinkError::MappingFailed {
            object: "<trap stubs>".to_owned(),
            len,
        });
    }
    if !kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
        base: block as usize,
        len,
        behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
        storage: kinakaze_runtime::ForkMappingStorage::Ordinary,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: 0,
        domain: kinakaze_runtime::ForkMappingDomain::HostPrivate,
    }) {
        // SAFETY: registration failed before the address was published.
        unsafe {
            windows_sys::Win32::System::Memory::VirtualFree(
                block,
                0,
                windows_sys::Win32::System::Memory::MEM_RELEASE,
            )
        };
        return Err(LinkError::MappingFailed {
            object: "<trap stubs>".to_owned(),
            len,
        });
    }
    Ok(block as usize)
}

/// Reserves one page that faults on any access.
#[cfg(windows)]
fn allocate_no_access() -> Result<usize, LinkError> {
    use windows_sys::Win32::System::Memory::{MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc};
    let len = crate::object::PAGE_SIZE as usize;
    let _fork_mapping_transaction =
        kinakaze_runtime::begin_fork_mapping_transaction().ok_or(LinkError::MappingFailed {
            object: "<trap guard page>".to_owned(),
            len,
        })?;
    // Reserved but uncommitted with no access: any read or write faults at once,
    // which is the closest thing to a diagnostic a data load can produce.
    // SAFETY: requests a reservation only; nothing is written here.
    let block = unsafe { VirtualAlloc(std::ptr::null(), len, MEM_RESERVE, PAGE_NOACCESS) };
    if block.is_null() {
        return Err(LinkError::MappingFailed {
            object: "<trap guard page>".to_owned(),
            len,
        });
    }
    if !kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
        base: block as usize,
        len,
        behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
        storage: kinakaze_runtime::ForkMappingStorage::Ordinary,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: 0,
        domain: kinakaze_runtime::ForkMappingDomain::HostPrivate,
    }) {
        // SAFETY: registration failed before the address was published.
        unsafe {
            windows_sys::Win32::System::Memory::VirtualFree(
                block,
                0,
                windows_sys::Win32::System::Memory::MEM_RELEASE,
            )
        };
        return Err(LinkError::MappingFailed {
            object: "<trap guard page>".to_owned(),
            len,
        });
    }
    Ok(block as usize)
}

#[cfg(not(windows))]
fn allocate_executable(len: usize) -> Result<usize, LinkError> {
    Err(LinkError::MappingFailed {
        object: "<trap stubs>".to_owned(),
        len,
    })
}

#[cfg(not(windows))]
fn allocate_no_access() -> Result<usize, LinkError> {
    Err(LinkError::MappingFailed {
        object: "<trap guard page>".to_owned(),
        len: 4096,
    })
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn stubs_are_distinct_and_carry_their_own_identity() {
        // Each missing symbol needs its own address, or the diagnostic would name
        // whichever symbol happened to be recorded last.
        let first = trap_for("first_missing_symbol", true).unwrap();
        let second = trap_for("second_missing_symbol", true).unwrap();
        assert_ne!(first, second);
        assert_eq!(second - first, STUB_SIZE);

        let names = trapped_symbols();
        assert!(names.contains(&"first_missing_symbol".to_owned()));
        assert!(names.contains(&"second_missing_symbol".to_owned()));
    }

    #[test]
    fn the_generated_stub_loads_its_own_id() {
        let address = trap_for("identity_check", true).unwrap();
        // The id is whatever index this symbol landed at.
        let expected = trapped_symbols()
            .iter()
            .position(|name| name == "identity_check")
            .unwrap() as u32;
        // SAFETY: the stub was just written at this address by this module.
        let bytes = unsafe { std::slice::from_raw_parts(address as *const u8, 5) };
        assert_eq!(bytes[0], 0xbf, "first instruction should be mov edi, imm32");
        let encoded = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        assert_eq!(encoded, expected);
    }

    #[test]
    fn missing_data_objects_share_one_inaccessible_page() {
        // A data symbol cannot trap by executing, so it must point somewhere that
        // faults on access rather than reading as zero.
        let first = trap_for("missing_data_one", false).unwrap();
        let second = trap_for("missing_data_two", false).unwrap();
        assert_eq!(
            first, second,
            "one guard page is enough for every data symbol"
        );
        assert_ne!(first, 0, "a null address would read as zero, not fault");
    }
}
