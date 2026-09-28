//! Nonlocal signal returns still leave through the native exception dispatcher.
//! Jumping directly out of a VEH would strand its host stack lane and TEB bounds.
use super::*;

#[repr(C)]
struct Frame {
    escape: [u64; 8],
    previous: *mut Frame,
    action: Action,
    handler: Handler,
    signal: i32,
    info: *mut c_void,
    context: *mut PendingUContext,
    jumped: Option<u64>,
}

thread_local! {
    static CURRENT: Cell<*mut Frame> = const { Cell::new(core::ptr::null_mut()) };
}

pub(super) fn snapshot() -> usize {
    CURRENT.get() as usize
}

#[inline(never)]
pub(super) fn restore(address: usize) {
    // The engine restores registered guest/host stacks at their verified bases
    // before providers resume the sole surviving fork thread.
    CURRENT.set(address as *mut Frame);
}

fn same_allocation(left: usize, right: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut first: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    let mut second: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    let size = core::mem::size_of::<MEMORY_BASIC_INFORMATION>();
    unsafe {
        VirtualQuery(left as _, &raw mut first, size) != 0
            && VirtualQuery(right as _, &raw mut second, size) != 0
            && !first.AllocationBase.is_null()
            && first.AllocationBase == second.AllocationBase
    }
}

fn destination(stack: usize) -> *mut Frame {
    let mut frame = CURRENT.get();
    while !frame.is_null() {
        // Frames are live until the synchronous dispatcher returns; a nonlocal
        // exit restores the selected frame's predecessor before that return.
        let interrupted = unsafe { (*(*frame).context).gregs[15] as usize };
        if stack >= interrupted && same_allocation(stack, interrupted) {
            return frame;
        }
        frame = unsafe { (*frame).previous };
    }
    core::ptr::null_mut()
}

pub(super) fn valid_target(stack: usize) -> bool {
    !destination(stack).is_null()
}

/// Called after longjmp applies its optional saved mask. Returning means the
/// destination remains inside the handler or no synchronous handler is active.
pub(super) fn resume(registers: &[u64; 8], value: i32) {
    let frame = destination(registers[6] as usize);
    if frame.is_null() {
        return;
    }
    unsafe {
        set_registers(&mut *(*frame).context, registers, value);
        let mask = Some(BLOCKED.get());
        (*frame).jumped = mask;
        // Leave each nested native exception through its own dispatcher. The
        // inner continuation returns to the outer callback boundary; only the
        // selected outer frame finally resumes guest code.
        let mut inner = CURRENT.get();
        while inner != frame {
            let outer = (*inner).previous;
            set_registers(&mut *(*inner).context, &(*outer).escape, 0);
            (*inner).jumped = mask;
            inner = outer;
        }
        escape(CURRENT.get());
    }
}

fn set_registers(context: &mut PendingUContext, registers: &[u64; 8], value: i32) {
    for (source, target) in [
        (0, 11),
        (1, 10),
        (2, 4),
        (3, 5),
        (4, 6),
        (5, 7),
        (6, 15),
        (7, 16),
    ] {
        context.gregs[target] = registers[source];
    }
    context.gregs[13] = if value == 0 { 1 } else { value as i64 as u64 };
}

/// Returns the mask selected by a nonlocal return, or None after normal return.
pub(super) unsafe fn invoke(
    action: Action,
    handler: Handler,
    signal: i32,
    info: *mut c_void,
    context: *mut c_void,
) -> Option<u64> {
    let mut frame = Frame {
        escape: [0; 8],
        previous: CURRENT.get(),
        action,
        handler,
        signal,
        info,
        context: context.cast(),
        jumped: None,
    };
    let alternate = ON_ALT_STACK.get();
    CURRENT.set(&raw mut frame);
    unsafe { enter(&raw mut frame) };
    // A guest callback may fork. Resolve Rust TLS again in a separate call;
    // an inlined Cell::set can reuse the parent's native TLS address.
    restore(frame.previous as usize);
    set_on_alternate_stack(alternate);
    frame.jumped
}

unsafe extern "sysv64" fn call(frame: *mut Frame) {
    unsafe {
        invoke_handler(
            (*frame).action,
            (*frame).handler,
            (*frame).signal,
            (*frame).info,
            (*frame).context.cast(),
        );
    }
}

// Unlike setjmp, this call returns exactly once: the handler either returns
// normally or resumes this same boundary after editing the interrupted context.
#[unsafe(naked)]
unsafe extern "sysv64" fn enter(_frame: *mut Frame) {
    core::arch::naked_asm!(
        "mov [rdi], rbx", "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12", "mov [rdi + 24], r13",
        "mov [rdi + 32], r14", "mov [rdi + 40], r15",
        "lea rax, [rsp + 8]", "mov [rdi + 48], rax",
        "mov rax, [rsp]", "mov [rdi + 56], rax",
        "jmp {call}", call = sym call,
    )
}

#[unsafe(naked)]
unsafe extern "sysv64" fn escape(_frame: *mut Frame) -> ! {
    core::arch::naked_asm!(
        "mov rbx, [rdi]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",
        "mov rdx, [rdi + 56]",
        "mov rsp, [rdi + 48]",
        "jmp rdx",
    )
}
