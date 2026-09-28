//! Fork coordination must not consume or remap a guest's small signal stack.
use kinakaze_tls::thread_pointer::*;

pub(super) unsafe fn fork(parent: u32, initializer: usize, flags: u64) -> i32 {
    let transition = kinakaze_tls::host_transition_pointer();
    if transition == 0 {
        return -crate::ENOMEM;
    }
    let stack: usize;
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) stack, options(nomem, nostack)) };
    let top = unsafe { ((transition + HOST_CALL_STACK_POINTER_OFFSET) as *const usize).read() };
    // Raw syscalls and synchronous handlers already own a host lane. Reusing
    // that lane avoids exhausting the nesting allowance on every fork.
    if top
        .checked_sub(HOST_CALL_STACK_SIZE)
        .is_some_and(|base| (base..top).contains(&stack))
    {
        return unsafe {
            kinakaze_runtime::kinakaze_process_clone_fork(parent, initializer, flags)
        };
    }
    unsafe { enter(transition, parent, initializer, flags) }
}

#[unsafe(naked)]
unsafe extern "sysv64" fn enter(
    _transition: usize,
    _parent: u32,
    _initializer: usize,
    _flags: u64,
) -> i32 {
    core::arch::naked_asm!(
        "push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
        "mov r12, rsp",
        "mov r13, rdi",
        "mov r14, [r13 + {used}]",
        "cmp r14, {last_lane}",
        "ja 2f",
        "mov rsp, [r13 + {top}]",
        "sub rsp, r14",
        "and rsp, -16",
        "add qword ptr [r13 + {used}], {lane}",
        "mov rbx, qword ptr gs:[0x08]",
        "mov rbp, qword ptr gs:[0x10]",
        "mov qword ptr gs:[0x08], rsp",
        "xor eax, eax",
        "mov qword ptr gs:[0x10], rax",
        "mov edi, esi",
        "mov rsi, rdx",
        "mov rdx, rcx",
        "call {fork}",
        "mov qword ptr gs:[0x08], rbx",
        "mov qword ptr gs:[0x10], rbp",
        "mov [r13 + {used}], r14",
        "jmp 3f",
        "2:", "mov eax, -12",
        "3:", "mov rsp, r12",
        "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx",
        "ret",
        top = const HOST_CALL_STACK_POINTER_OFFSET,
        used = const HOST_CALL_STACK_USED_OFFSET,
        lane = const HOST_CALL_STACK_LANE_SIZE,
        last_lane = const HOST_CALL_STACK_SIZE - HOST_CALL_STACK_LANE_SIZE,
        fork = sym kinakaze_runtime::kinakaze_process_clone_fork,
    )
}
