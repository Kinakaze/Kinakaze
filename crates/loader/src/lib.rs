//! Provider image loading, registration and native module lifetime.
//!
//! This crate has no RPC transport or session ownership. The embedding runtime
//! supplies its live API at load and after child adoption. It is linked once;
//! the canonical ELF linker, allocator and process registry remain shared.
mod handoff;
mod providers;

pub use handoff::install;
pub use providers::load;
use std::ffi::{c_char, c_void};

/// Native C ABI used by libc to preserve one storage location after ELF COPY.
pub type CopyRedirect = unsafe extern "C" fn(*const c_char, *mut c_void) -> i32;
