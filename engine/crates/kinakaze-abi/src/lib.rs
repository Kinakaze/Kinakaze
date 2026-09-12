#![no_std]

//! Scalar types as the *guest* defines them.
//!
//! Every type here exists because `core::ffi`'s equivalent describes the wrong
//! platform. `core::ffi::c_long` is the host's `long`, which on Windows MSVC is 32
//! bits; guest code is Linux x86_64, where `long` is 64. A libc entry point
//! returning `c_long` therefore returns half a value, sign-extended or truncated,
//! and the compiler is perfectly happy about it.
//!
//! That is not a hypothetical: two entry points were written that way — `gethostid`
//! and `random`, in separate modules by separate hands — before it was noticed. Both
//! compiled, and both would have returned a 32-bit result into a 64-bit register
//! with the top half whatever was there before. Naming the guest's types once, here,
//! is what stops the third one.
//!
//! The rule for this project: **inside a `kinakaze_abi_*` signature, use these types
//! or plain sized integers, never `core::ffi::c_long`/`c_ulong`.** The other
//! `core::ffi` types (`c_int`, `c_char`, `c_void`, `c_uint`) agree between the two
//! platforms and are fine.

/// The guest's `long`: 64-bit on Linux x86_64.
///
/// Use in place of `core::ffi::c_long`, which is 32-bit on Windows.
pub type Long = i64;

/// The guest's `unsigned long`: 64-bit on Linux x86_64.
pub type ULong = u64;

/// The guest's `time_t`, `off_t`, `ssize_t`, `clock_t` and `suseconds_t`, all of
/// which are `long` on Linux x86_64.
pub type Time = i64;
/// The guest's `off_t`, spelled separately because file offsets read better named.
pub type Off = i64;
/// The guest's `size_t`.
pub type Size = u64;
/// The guest's `ssize_t`.
pub type SSize = i64;

/// Linux-compatible process identifier used at the public ABI boundary.
pub type Pid = i32;

/// The current prototype only targets the native 64-bit Windows ABI.
pub const SUPPORTED_WINDOWS_MACHINE: &str = "x86_64-pc-windows-msvc";

/// Negative return value used by the C facade when a runtime operation fails.
pub const ABI_ERROR: Pid = -1;
