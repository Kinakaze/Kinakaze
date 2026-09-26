//! Compatibility with the private ABI of the locked Debian glibc 2.36 package.
//! These layouts/IDs are version-specific, not a portable glibc interface.
use core::ffi::{CStr, c_int, c_void};
use std::sync::Once;

#[repr(C, align(8))]
pub struct LoaderReadOnly([u8; 896]);
#[unsafe(no_mangle)]
pub static mut kinakaze_abi__rtld_global_ro: LoaderReadOnly = LoaderReadOnly([0; 896]);

pub(crate) fn initialize() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let output = (&raw mut kinakaze_abi__rtld_global_ro).cast::<u8>();
        let word =
            |offset: usize, value: u64| output.add(offset).cast::<u64>().write_unaligned(value);
        let int =
            |offset: usize, value: u32| output.add(offset).cast::<u32>().write_unaligned(value);
        word(8, c"x86_64".as_ptr() as u64);
        word(16, 6);
        word(24, 4096);
        word(32, 2048);
        int(64, 100); // guest _SC_CLK_TCK
        int(72, 2); // diagnostics stderr
        output.add(88).cast::<u16>().write(0x37f);
        // cpu_features.basic starts at 0x70. CPUID records contain the raw
        // hardware values followed by features usable in this host process.
        use core::arch::x86_64::__cpuid_count;
        let root = __cpuid_count(0, 0);
        let extended = __cpuid_count(0x8000_0000, 0).eax;
        let leaf1 = __cpuid_count(1, 0);
        let kind = if root.ebx == 0x756e6547 {
            1
        } else if root.ebx == 0x68747541 {
            2
        } else {
            4
        };
        let family = (leaf1.eax >> 8) & 15;
        int(112, kind);
        int(116, root.eax);
        int(
            120,
            family
                + if family == 15 {
                    (leaf1.eax >> 20) & 255
                } else {
                    0
                },
        );
        int(
            124,
            ((leaf1.eax >> 4) & 15)
                | if family == 6 || family == 15 {
                    (leaf1.eax >> 12) & 240
                } else {
                    0
                },
        );
        int(128, leaf1.eax & 15);
        let leaves = [
            (1, 0),
            (7, 0),
            (0x80000001, 0),
            (13, 1),
            (0x80000007, 0),
            (0x80000008, 0),
            (7, 1),
            (25, 0),
            (20, 0),
        ];
        for (index, (leaf, subleaf)) in leaves.into_iter().enumerate() {
            if leaf
                > if leaf >= 0x80000000 {
                    extended
                } else {
                    root.eax
                }
            {
                continue;
            }
            let cpuid = __cpuid_count(leaf, subleaf);
            let raw = [cpuid.eax, cpuid.ebx, cpuid.ecx, cpuid.edx];
            let mut active = raw;
            if index == 0 {
                if !std::is_x86_feature_detected!("avx") {
                    active[2] &= !((1 << 28) | (1 << 29) | (1 << 12));
                }
                if !std::is_x86_feature_detected!("fma") {
                    active[2] &= !(1 << 12);
                }
            }
            if index == 1 {
                if !std::is_x86_feature_detected!("avx2") {
                    active[1] &= !(1 << 5);
                }
                if !std::is_x86_feature_detected!("avx512f") {
                    active[1] &= !0xdc230000;
                    active[2] &= !0x5f42;
                    active[3] &= !0x10c;
                }
            }
            for register in 0..4 {
                int(132 + index * 32 + register * 4, raw[register]);
                int(148 + index * 32 + register * 4, active[register]);
            }
        }
    });
}

// Obsolete malloc hooks are compatibility storage. Like glibc >= 2.34, the
// ordinary allocator does not invoke them; libc_malloc_debug owns that policy.
#[unsafe(no_mangle)]
pub static kinakaze_abi___malloc_initialize_hook: crate::copied::CopiedValue<usize> =
    crate::copied::CopiedValue::new(0);

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___libc_freeres() {
    crate::netdb::nss_dispatch::cleanup();
    if let Ok(mut strings) = TUNABLE_STRINGS.lock() {
        for (_, address) in strings.drain(..) {
            unsafe {
                kinakaze_alloc::guest::free(address as *mut u8);
            }
        }
    }
}

// The native loader never activates LD_PROFILE. This is glibc's conditional
// profiling hook: a call records nothing while the profiling map is absent.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi__dl_mcount_wrapper_check(_address: *const c_void) {}

struct Tunable {
    name: &'static CStr,
    kind: u32,
    min: u64,
    max: u64,
    value: u64,
}
include!("glibc_private_tunables.rs");

static TUNABLE_STRINGS: std::sync::Mutex<Vec<(Vec<u8>, usize)>> = std::sync::Mutex::new(Vec::new());
fn parse_value(spec: &Tunable, bytes: &[u8]) -> Option<u64> {
    if spec.kind == 3 {
        let mut strings = TUNABLE_STRINGS.lock().ok()?;
        if let Some((_, address)) = strings.iter().find(|(key, _)| key.as_slice() == bytes) {
            return Some(*address as u64);
        }
        let address = unsafe { kinakaze_alloc::guest::malloc(bytes.len().checked_add(1)?) };
        if address.is_null() {
            crate::set_errno(12);
            return None;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), address, bytes.len());
            address.add(bytes.len()).write(0);
        }
        strings.push((bytes.to_vec(), address as usize));
        return Some(address as u64);
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let number = if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()?
    } else if text.starts_with('0') && text.len() > 1 {
        u64::from_str_radix(&text[1..], 8).ok()?
    } else {
        text.parse::<u64>().ok()?
    };
    (number >= spec.min && number <= spec.max).then_some(number)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___tunable_get_val(
    id: c_int,
    output: *mut c_void,
    callback: Option<unsafe extern "sysv64" fn(*const u64)>,
) {
    let Some(spec) = TUNABLES.get(id as usize) else {
        crate::set_errno(22);
        return;
    };
    if output.is_null() {
        crate::set_errno(22);
        return;
    }
    let mut value = spec.value;
    let mut initialized = false;
    let alias = match id {
        2 => Some(c"MALLOC_TRIM_THRESHOLD_"),
        3 => Some(c"MALLOC_PERTURB_"),
        14 => Some(c"MALLOC_TOP_PAD_"),
        21 => Some(c"MALLOC_MMAP_MAX_"),
        27 => Some(c"MALLOC_ARENA_MAX"),
        28 => Some(c"MALLOC_MMAP_THRESHOLD_"),
        31 => Some(c"MALLOC_ARENA_TEST"),
        36 => Some(c"MALLOC_CHECK_"),
        _ => None,
    };
    if let Some(alias) = alias {
        let setting = unsafe { crate::process::kinakaze_abi_secure_getenv(alias.as_ptr()) };
        if !setting.is_null() {
            if let Some(parsed) = parse_value(spec, unsafe { CStr::from_ptr(setting) }.to_bytes()) {
                value = parsed;
                initialized = true;
            }
        }
    }
    let environment =
        unsafe { crate::process::kinakaze_abi_secure_getenv(c"GLIBC_TUNABLES".as_ptr()) };
    if !environment.is_null() {
        for assignment in unsafe { CStr::from_ptr(environment) }
            .to_bytes()
            .split(|&b| b == b':')
        {
            let Some(equal) = assignment.iter().position(|&b| b == b'=') else {
                continue;
            };
            if &assignment[..equal] != spec.name.to_bytes() {
                continue;
            }
            if let Some(parsed) = parse_value(spec, &assignment[equal + 1..]) {
                value = parsed;
                initialized = true;
            }
        }
    }
    unsafe {
        if spec.kind == 0 {
            output.cast::<u32>().write(value as u32);
        } else {
            output.cast::<u64>().write(value);
        }
        if initialized {
            if let Some(callback) = callback {
                callback(&value);
            }
        }
    }
}
