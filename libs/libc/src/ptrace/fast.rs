//! The untraced hot path. This module has no allocation, lock, or TLS access.

/// Read Windows x64 TEB->PEB->BeingDebugged on every call, so an attachment
/// becomes visible without rebuilding or patching any generated syscall site.
#[inline(always)]
pub(crate) fn debugger_present() -> bool {
    let peb: usize;
    let debugged: u8;
    unsafe {
        core::arch::asm!(
            "mov {peb}, gs:[0x60]",
            "mov {debugged}, byte ptr [{peb} + 2]",
            peb = out(reg) peb,
            debugged = out(reg_byte) debugged,
            options(readonly, nostack, preserves_flags),
        );
    }
    let _ = peb;
    debugged != 0
}
