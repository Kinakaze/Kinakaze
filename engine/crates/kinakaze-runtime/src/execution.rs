//! Borrowed image handoff to the execution engine. No loader types or module
//! allocations cross this boundary; the caller owns the view and result storage.
use std::sync::OnceLock;

#[repr(C)]
pub struct CodeImage {
    pub bytes: *const u8,
    pub length: usize,
    pub base: usize,
    pub mapped_length: usize,
    pub load_bias: i128,
}

#[repr(C)]
pub struct CodeConfig {
    pub canary_slot: u32,
    pub thread_pointer_slot: u32,
    pub transition_slot: u32,
    pub scratch_slot: u32,
    pub syscall_dispatcher: usize,
}

#[repr(C)]
pub struct CodeResult {
    pub canary_sites: usize,
    pub syscall_sites: usize,
    pub thread_pointer_sites: usize,
    pub error: [u8; 256],
}

impl Default for CodeResult {
    fn default() -> Self {
        Self {
            canary_sites: 0,
            syscall_sites: 0,
            thread_pointer_sites: 0,
            error: [0; 256],
        }
    }
}

pub type PrepareCode =
    unsafe extern "C" fn(*const CodeImage, *const CodeConfig, *mut CodeResult) -> i32;
static PROCESSOR: OnceLock<PrepareCode> = OnceLock::new();

pub fn install(processor: PrepareCode) {
    assert!(
        PROCESSOR.set(processor).is_ok(),
        "code processor already installed"
    );
}

/// The engine is installed afresh during native worker bootstrap, including
/// fork children. An absent engine is an error, never unpatched guest execution.
///
/// # Safety
/// The file bytes and writable mapped image must stay valid for this call.
pub unsafe fn prepare(image: &CodeImage, config: &CodeConfig, result: &mut CodeResult) -> i32 {
    match PROCESSOR.get() {
        Some(processor) => unsafe { processor(image, config, result) },
        None => {
            let message = b"execution engine is not installed";
            result.error[..message.len()].copy_from_slice(message);
            38
        }
    }
}
