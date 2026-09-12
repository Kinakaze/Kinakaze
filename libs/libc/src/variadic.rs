//! Assembly thunks for variadic formatting, scanning and process calls.
//!
//! Rust cannot declare an `extern "sysv64"` function with `...`, so these
//! entry points are written in assembly. Each one materializes the System V
//! AMD64 `va_list` its caller's arguments imply, then tail-calls the matching
//! `kinakaze_*_impl` in [`crate::stdio`]. No libc semantics live here.
//!
//! # The frame each thunk builds
//!
//! The System V `va_list` is a cursor over a register save area:
//!
//! ```text
//!   rbp-176 .. rbp-128   the six integer argument registers
//!   rbp-128 .. rbp+0     the eight SSE argument registers
//!   rbp-208 .. rbp-184   the va_list itself
//!   rbp+16               the first stack argument (overflow area)
//! ```
//!
//! `gp_offset` starts past the fixed named arguments, so `printf`, which names
//! one, begins at 8 and `snprintf`, which names three, begins at 24. All eight
//! SSE registers are saved unconditionally: `al` carries the vector count, but
//! branching on it saves less than the branch costs.

// The thunks are only meaningful for the System V x86_64 ABI.
#[cfg(all(windows, target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
.text

// Builds the va_list frame. `\gp` is the initial gp_offset, which skips the
// named integer arguments the caller already consumed.
.macro KINAKAZE_VA_FRAME gp
    push    rbp
    mov     rbp, rsp
    sub     rsp, 208

    // Save the integer argument registers in ABI order.
    mov     [rbp-176], rdi
    mov     [rbp-168], rsi
    mov     [rbp-160], rdx
    mov     [rbp-152], rcx
    mov     [rbp-144], r8
    mov     [rbp-136], r9

    // Save the SSE argument registers. movups tolerates an unaligned frame,
    // which costs nothing measurable next to the formatting work.
    movups  [rbp-128], xmm0
    movups  [rbp-112], xmm1
    movups  [rbp-96], xmm2
    movups  [rbp-80], xmm3
    movups  [rbp-64], xmm4
    movups  [rbp-48], xmm5
    movups  [rbp-32], xmm6
    movups  [rbp-16], xmm7

    // va_list.gp_offset and va_list.fp_offset.
    mov     dword ptr [rbp-208], \gp
    mov     dword ptr [rbp-204], 48
    // va_list.overflow_arg_area: the first argument passed on the stack.
    lea     rax, [rbp+16]
    mov     [rbp-200], rax
    // va_list.reg_save_area.
    lea     rax, [rbp-176]
    mov     [rbp-192], rax
    // The va_list pointer every implementation expects.
    lea     rax, [rbp-208]
.endm

// printf(format, ...) -> kinakaze_printf_impl(format, va_list)
.globl kinakaze_abi_printf
kinakaze_abi_printf:
    KINAKAZE_VA_FRAME 8
    // rdi already holds `format`.
    mov     rsi, rax
    call    kinakaze_printf_impl
    leave
    ret

// fprintf(file, format, ...) -> kinakaze_fprintf_impl(file, format, va_list)
.globl kinakaze_abi_fprintf
kinakaze_abi_fprintf:
    KINAKAZE_VA_FRAME 16
    // rdi and rsi already hold `file` and `format`.
    mov     rdx, rax
    call    kinakaze_fprintf_impl
    leave
    ret

// error(status, errnum, format, ...)
//   -> kinakaze_error_impl(status, errnum, format, va_list)
.globl kinakaze_abi_error
kinakaze_abi_error:
    KINAKAZE_VA_FRAME 24
    mov     rcx, rax
    call    kinakaze_error_impl
    leave
    ret

// error_at_line(status, errnum, filename, line, format, ...)
.globl kinakaze_abi_error_at_line
kinakaze_abi_error_at_line:
    KINAKAZE_VA_FRAME 40
    mov     r9, rax
    call    kinakaze_error_at_line_impl
    leave
    ret

// snprintf(buffer, size, format, ...)
//   -> kinakaze_snprintf_impl(buffer, size, format, va_list)
.globl kinakaze_abi_snprintf
kinakaze_abi_snprintf:
    KINAKAZE_VA_FRAME 24
    // rdi, rsi and rdx already hold the three named arguments.
    mov     rcx, rax
    call    kinakaze_snprintf_impl
    leave
    ret

// sprintf(buffer, format, ...)
//   -> kinakaze_sprintf_impl(buffer, format, va_list)
.globl kinakaze_abi_swprintf
kinakaze_abi_swprintf:
    KINAKAZE_VA_FRAME 24
    mov     rcx, rax
    call    kinakaze_abi_vswprintf
    leave
    ret

.globl kinakaze_abi_sprintf
kinakaze_abi_sprintf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_sprintf_impl
    leave
    ret

// execl(path, arg0, ...) -> kinakaze_execl_impl(path, va_list)
.globl kinakaze_abi_execl
kinakaze_abi_execl:
    KINAKAZE_VA_FRAME 8
    mov     rsi, rax
    call    kinakaze_execl_impl
    leave
    ret

// execlp(file, arg0, ...) -> kinakaze_execlp_impl(file, va_list)
.globl kinakaze_abi_execlp
kinakaze_abi_execlp:
    KINAKAZE_VA_FRAME 8
    mov     rsi, rax
    call    kinakaze_execlp_impl
    leave
    ret

// execle(path, arg0, ..., NULL, envp) -> kinakaze_execle_impl(path, va_list)
.globl kinakaze_abi_execle
kinakaze_abi_execle:
    KINAKAZE_VA_FRAME 8
    mov     rsi, rax
    call    kinakaze_execle_impl
    leave
    ret

// scanf(format, ...) -> vscanf(format, va_list)
.globl kinakaze_abi_scanf
kinakaze_abi_scanf:
    KINAKAZE_VA_FRAME 8
    mov     rsi, rax
    call    kinakaze_abi_vscanf
    leave
    ret

// __isoc23_fscanf(file, format, ...) -> kinakaze_isoc23_fscanf_impl(file, format, va_list)
.globl kinakaze_abi___isoc23_fscanf
kinakaze_abi___isoc23_fscanf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_isoc23_fscanf_impl
    leave
    ret

// fscanf(file, format, ...) -> kinakaze_isoc23_fscanf_impl(file, format, va_list)
.globl kinakaze_abi_fscanf
kinakaze_abi_fscanf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_isoc23_fscanf_impl
    leave
    ret

// __isoc99_fscanf(file, format, ...) -> kinakaze_isoc23_fscanf_impl(file, format, va_list)
.globl kinakaze_abi___isoc99_fscanf
kinakaze_abi___isoc99_fscanf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_isoc23_fscanf_impl
    leave
    ret


// __isoc23_sscanf(string, format, ...) -> kinakaze_isoc23_sscanf_impl(string, format, va_list)
.globl kinakaze_abi___isoc23_sscanf
kinakaze_abi___isoc23_sscanf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_isoc23_sscanf_impl
    leave
    ret

// __isoc99_sscanf(string, format, ...) -> kinakaze_isoc23_sscanf_impl(string, format, va_list)
.globl kinakaze_abi___isoc99_sscanf
kinakaze_abi___isoc99_sscanf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_isoc23_sscanf_impl
    leave
    ret

// sscanf(string, format, ...) -> kinakaze_isoc23_sscanf_impl(string, format, va_list)
.globl kinakaze_abi_sscanf
kinakaze_abi_sscanf:
    KINAKAZE_VA_FRAME 16
    mov     rdx, rax
    call    kinakaze_isoc23_sscanf_impl
    leave
    ret

.globl kinakaze_abi_asprintf
kinakaze_abi_asprintf:
    KINAKAZE_VA_FRAME 16
    mov rdx, rax
    call kinakaze_abi_vasprintf
    leave
    ret

.globl kinakaze_abi_obstack_printf
kinakaze_abi_obstack_printf:
    KINAKAZE_VA_FRAME 16
    mov rdx, rax
    call kinakaze_abi_obstack_vprintf
    leave
    ret

.globl kinakaze_abi_warn
.globl kinakaze_abi_err
kinakaze_abi_err:
    KINAKAZE_VA_FRAME 16
    mov rdx, rax
    call kinakaze_abi_verr
    ud2

.globl kinakaze_abi_errx
kinakaze_abi_errx:
    KINAKAZE_VA_FRAME 16
    mov rdx, rax
    call kinakaze_abi_verrx
    ud2

kinakaze_abi_warn:
    KINAKAZE_VA_FRAME 8
    mov rsi, rax
    call kinakaze_abi_vwarn
    leave
    ret

.globl kinakaze_abi_warnx
kinakaze_abi_warnx:
    KINAKAZE_VA_FRAME 8
    mov rsi, rax
    call kinakaze_abi_vwarnx
    leave
    ret

// __asprintf_chk(result_ptr, flag, format, ...) -> kinakaze_asprintf_chk_impl(result_ptr, flag, format, va_list)
.globl kinakaze_abi___asprintf_chk
kinakaze_abi___asprintf_chk:
    KINAKAZE_VA_FRAME 24
    mov     rcx, rax
    call    kinakaze_asprintf_chk_impl
    leave
    ret
"#
);
