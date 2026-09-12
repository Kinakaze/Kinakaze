//! XIM varargs contain pointer/integer attributes only. Capture all five
//! remaining SysV GP registers and then walk the caller's stack arguments.
use super::*;
#[repr(C)]
pub(super) struct Saved {
    registers: [usize; 5],
    stack: *const usize,
}
pub(super) struct Cursor {
    saved: *const Saved,
    index: usize,
}
impl Cursor {
    pub(super) unsafe fn new(saved: *const Saved) -> Self {
        Self { saved, index: 0 }
    }
    pub(super) unsafe fn next(&mut self) -> usize {
        let index = self.index;
        self.index += 1;
        if index < 5 {
            unsafe { (*self.saved).registers[index] }
        } else {
            unsafe { (*self.saved).stack.add(index - 5).read() }
        }
    }
}
macro_rules! entry {
    ($name:ident, $input:ty, $output:ty, $body:ident) => {
        #[unsafe(naked)]
        #[unsafe(export_name = concat!("kinakaze_engine_libX11_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(_object: $input) -> $output {
            core::arch::naked_asm!(
                "sub rsp, 56",
                "mov [rsp], rsi", "mov [rsp+8], rdx", "mov [rsp+16], rcx",
                "mov [rsp+24], r8", "mov [rsp+32], r9",
                "lea rax, [rsp+64]", "mov [rsp+40], rax", "mov rsi, rsp",
                "call {body}", "add rsp, 56", "ret", body = sym $body,
            );
        }
    };
}
entry!(XCreateIC, *mut InputMethod, *mut InputContext, create);
entry!(XGetICValues, *mut InputContext, *mut c_char, get_ic);
entry!(XSetICValues, *mut InputContext, *mut c_char, set_ic);
entry!(XGetIMValues, *mut InputMethod, *mut c_char, get_im);
entry!(XSetIMValues, *mut InputMethod, *mut c_char, set_im);
