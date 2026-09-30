//! Marshal the three System V complex formats to pointer-based C routines.
//! Windows C complex/long-double return conventions must never reach ELF code.

unsafe extern "win64" {
    fn kinakaze_complex80(operation: i32, precision: i32, input: *const u8, output: *mut u8);
    fn kinakaze_real80(operation: i32, input: *const u8, output: *mut u8);
}

macro_rules! complex32 {
    ($name:ident, $operation:expr) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 72",
                "movq qword ptr [rsp + 32], xmm0",
                "movq qword ptr [rsp + 40], xmm1",
                "mov ecx, {operation}",
                "mov edx, 32",
                "lea r8, [rsp + 32]",
                "lea r9, [rsp + 48]",
                "call {calculate}",
                "movq xmm0, qword ptr [rsp + 48]",
                "add rsp, 72",
                "ret",
                operation = const $operation,
                calculate = sym kinakaze_complex80,
            );
        }
    };
}

macro_rules! complex64 {
    ($name:ident, $operation:expr) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 88",
                "movsd qword ptr [rsp + 32], xmm0",
                "movsd qword ptr [rsp + 40], xmm1",
                "movsd qword ptr [rsp + 48], xmm2",
                "movsd qword ptr [rsp + 56], xmm3",
                "mov ecx, {operation}",
                "mov edx, 64",
                "lea r8, [rsp + 32]",
                "lea r9, [rsp + 64]",
                "call {calculate}",
                "movsd xmm0, qword ptr [rsp + 64]",
                "movsd xmm1, qword ptr [rsp + 72]",
                "add rsp, 88",
                "ret",
                operation = const $operation,
                calculate = sym kinakaze_complex80,
            );
        }
    };
}

macro_rules! complex80 {
    ($name:ident, $operation:expr, $imaginary:literal) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 72",
                "mov ecx, {operation}",
                "mov edx, 80",
                "lea r8, [rsp + 80]",
                "lea r9, [rsp + 32]",
                "call {calculate}",
                $imaginary,
                "fld tbyte ptr [rsp + 32]",
                "add rsp, 72",
                "ret",
                operation = const $operation,
                calculate = sym kinakaze_complex80,
            );
        }
    };
}

macro_rules! complex_family {
    ($double:ident, $float:ident, $extended:ident, $operation:expr) => {
        complex64!($double, $operation);
        complex32!($float, $operation);
        complex80!($extended, $operation, "fld tbyte ptr [rsp + 48]");
    };
}

complex_family!(cacos, cacosf, cacosl, 0);
complex_family!(casin, casinf, casinl, 1);
complex_family!(catan, catanf, catanl, 2);
complex_family!(cacosh, cacoshf, cacoshl, 3);
complex_family!(casinh, casinhf, casinhl, 4);
complex_family!(catanh, catanhf, catanhl, 5);
complex_family!(ccosh, ccoshf, ccoshl, 6);
complex_family!(csinh, csinhf, csinhl, 7);
complex_family!(ctanh, ctanhf, ctanhl, 8);
complex_family!(ctan, ctanf, ctanl, 9);
complex_family!(cpow, cpowf, cpowl, 10);
complex_family!(conj, conjf, conjl, 11);
complex_family!(cproj, cprojf, cprojl, 12);
complex32!(cargf, 13);
complex80!(cargl, 13, "");
complex64!(creal, 14);
complex32!(crealf, 14);
complex80!(creall, 14, "");
complex64!(cimag, 15);
complex32!(cimagf, 15);
complex80!(cimagl, 15, "");

macro_rules! real80 {
    ($name:ident, $operation:expr) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 56",
                "mov ecx, {operation}",
                "lea rdx, [rsp + 64]",
                "lea r8, [rsp + 32]",
                "call {calculate}",
                "fld tbyte ptr [rsp + 32]",
                "add rsp, 56",
                "ret",
                operation = const $operation,
                calculate = sym kinakaze_real80,
            );
        }
    };
}
real80!(expm1l, 0);
real80!(cbrtl, 1);
