//! Native DLL globals are fresh after fork. Preserve the guest's actual name
//! pointers (including direct guest writes) and COPY bindings before it resumes.
use super::*;

fn variables() -> [&'static crate::copied::CopiedPointer; 8] {
    [
        &kinakaze_abi_program_invocation_name,
        &program_invocation_name,
        &kinakaze_abi_program_invocation_short_name,
        &program_invocation_short_name,
        &kinakaze_abi___progname,
        &__progname,
        &kinakaze_abi___progname_full,
        &__progname_full,
    ]
}

#[repr(C)]
#[derive(Clone, Copy)]
struct State {
    magic: [u8; 8],
    entries: [[usize; 2]; 8],
}

unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    let size = size_of::<State>();
    if output.is_null() {
        return size as isize;
    }
    if capacity < size {
        return -22;
    }
    let state = State {
        magic: *b"KPROGN01",
        entries: variables().map(|v| [v.get() as usize, v.target() as usize]),
    };
    unsafe { output.cast::<State>().write_unaligned(state) };
    size as isize
}

unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != size_of::<State>() {
        return 22;
    }
    let state = unsafe { input.cast::<State>().read_unaligned() };
    if state.magic != *b"KPROGN01" {
        return 22;
    }
    for (variable, [value, target]) in variables().into_iter().zip(state.entries) {
        // Guest memory and native module bases are restored before participants.
        unsafe { variable.redirect(target as *mut *mut c_char) };
        variable.set(value as *mut c_char);
    }
    0
}

extern "C" fn register() {
    let _ = unsafe {
        kinakaze_runtime::register_fork_participant_without_inherited_handles(
            kinakaze_runtime::ForkParticipant {
                abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
                priority: 30,
                key: u64::from_le_bytes(*b"KPROGN01"),
                prepare: None,
                snapshot: Some(snapshot),
                parent: None,
                child: Some(child),
            },
        )
    };
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static REGISTER: extern "C" fn() = register;
