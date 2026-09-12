//! The rtld_fini hook handed to ELF _start in RDX.
use super::{dynamic_loader_lock, loaded_linker};
use crate::LinkError;

/// Run pending destructors without retaining a loader borrow or lock across
/// guest code. A destructor may dlopen, finalize recursively, or fork.
///
/// # Safety
/// The active linker's guest mappings and callback ABI must remain valid.
pub unsafe fn run_finalizers() -> Result<(), LinkError> {
    loop {
        let next = {
            let _guard = dynamic_loader_lock();
            let linker = loaded_linker();
            if linker.is_null() {
                return Err(LinkError::Execution(
                    "finalization without active linker".into(),
                ));
            }
            unsafe { (&mut *linker).next_finalizer()? }
        };
        let Some(address) = next else {
            return Ok(());
        };
        let function: unsafe extern "sysv64" fn() = unsafe { core::mem::transmute(address) };
        unsafe { function() };
        // Reacquire ACTIVE_LINKER each time: fork rebuilds its private owner.
    }
}

pub(crate) unsafe extern "sysv64" fn rtld_fini() {
    if let Err(error) = unsafe { run_finalizers() } {
        eprintln!("kinakaze: ELF finalization failed: {error}");
        // The void callback cannot return an error to libc. Never turn a
        // malformed destructor table into a successful process exit.
        kinakaze_vfs::job::publish_exit(127);
        unsafe {
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            TerminateProcess(GetCurrentProcess(), 127);
        }
        std::process::abort();
    }
}
