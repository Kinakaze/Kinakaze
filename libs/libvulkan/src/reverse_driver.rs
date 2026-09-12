//! Translation for `VK_LUNARG_direct_driver_loading` proc-address callbacks.
//!
//! The Windows loader invokes the supplied `pfnGetInstanceProcAddr` and every
//! pointer it returns with the Windows x64 convention. An ELF application supplies
//! a System V function instead. A proc-address bridge calls that guest function,
//! then replaces every returned guest address with a signature-correct reverse ABI
//! bridge generated from Khronos' command registry.

use core::ffi::{CStr, c_char, c_void};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::reverse_abi::{ArgumentClass, for_target};
use crate::types::VkInstance;

const DIRECT_DRIVER_LOADING_LIST_LUNARG: u32 = 1_000_459_001;

type GuestProcAddress = unsafe extern "sysv64" fn(VkInstance, *const c_char) -> *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirectDriverLoadingInfo {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    flags: u32,
    _padding2: u32,
    get_instance_proc_addr: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirectDriverLoadingList {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    mode: u32,
    driver_count: u32,
    drivers: *const DirectDriverLoadingInfo,
}

/// Owns both the translated list and the array it points to through instance
/// creation. Proc-address bridge code itself is process-lifetime storage.
pub struct TranslatedDirectDrivers {
    list: Box<DirectDriverLoadingList>,
    _drivers: Box<[DirectDriverLoadingInfo]>,
}

impl TranslatedDirectDrivers {
    pub fn as_ptr(&self) -> *const c_void {
        (&raw const *self.list).cast()
    }

    pub(crate) fn p_next(&self) -> *const c_void {
        self.list.p_next
    }

    pub(crate) fn set_p_next(&mut self, p_next: *const c_void) {
        self.list.p_next = p_next;
    }
}

fn callbacks() -> &'static Mutex<HashMap<usize, usize>> {
    static CALLBACKS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    CALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Creates a two-argument proc-address callback which carries `target` in R8 to
/// the common three-argument dispatcher. R8 is the third Windows integer argument
/// and neither overlaps nor disturbs the original RCX/RDX pair.
pub fn proc_address_callback(target: usize) -> Option<usize> {
    if target == 0 {
        return None;
    }
    let mut table = callbacks().lock().ok()?;
    if let Some(address) = table.get(&target).copied() {
        return Some(address);
    }
    let mut code = Vec::with_capacity(23);
    code.extend_from_slice(&[0x49, 0xb8]); // mov r8, target
    code.extend_from_slice(&(target as u64).to_le_bytes());
    code.extend_from_slice(&[0x48, 0xb8]); // mov rax, proc_address_dispatch
    code.extend_from_slice(&(proc_address_dispatch as *const () as usize as u64).to_le_bytes());
    code.extend_from_slice(&[0xff, 0xe0]); // jmp rax
    let address = crate::reverse_abi::publish(&code)?;
    table.insert(target, address);
    Some(address)
}

extern "system" fn proc_address_dispatch(
    dispatchable: VkInstance,
    name: *const c_char,
    target: usize,
) -> *const c_void {
    if name.is_null() || target == 0 {
        return core::ptr::null();
    }
    crate::reverse_abi::restore_guest_tls();
    let guest: GuestProcAddress = unsafe { core::mem::transmute(target) };
    let returned = unsafe { guest(dispatchable, name) } as usize;
    if returned == 0 {
        return core::ptr::null();
    }
    let Ok(name) = (unsafe { CStr::from_ptr(name) }).to_str() else {
        return core::ptr::null();
    };

    let converted = match name {
        // All three query functions return another function pointer, so their
        // result needs this same recursive conversion instead of an ordinary ABI
        // shuffle that would leak the guest address back to the Windows loader.
        "vkGetInstanceProcAddr"
        | "vkGetDeviceProcAddr"
        | "vk_icdGetInstanceProcAddr"
        | "vk_icdGetPhysicalDeviceProcAddr" => proc_address_callback(returned),
        // Private Loader/ICD interface version 7 functions are outside vk.xml but
        // are queryable through the supplied GIPA by specification.
        "vk_icdNegotiateLoaderICDInterfaceVersion" => {
            for_target(returned, &[ArgumentClass::Integer])
        }
        "vk_icdEnumerateAdapterPhysicalDevices" => for_target(
            returned,
            &[
                ArgumentClass::Integer,
                ArgumentClass::Integer,
                ArgumentClass::Integer,
                ArgumentClass::Integer,
            ],
        ),
        _ => crate::generated_extensions::reverse_lookup(name, returned),
    };
    converted.map_or(core::ptr::null(), |address| address as *const c_void)
}

/// Translates one direct-driver list node. Instance-chain ownership and recursion
/// are handled by `debug_callbacks`, alongside the other reverse-callback nodes.
pub(crate) unsafe fn translate_node(p_next: *const c_void) -> Option<TranslatedDirectDrivers> {
    if p_next.is_null()
        || unsafe { p_next.cast::<u32>().read() } != DIRECT_DRIVER_LOADING_LIST_LUNARG
    {
        return None;
    }
    let mut list = unsafe { p_next.cast::<DirectDriverLoadingList>().read() };
    if list.driver_count == 0 || list.drivers.is_null() {
        return None;
    }
    let source = unsafe {
        core::slice::from_raw_parts(list.drivers, usize::try_from(list.driver_count).ok()?)
    };
    let mut drivers = Vec::with_capacity(source.len());
    for source in source {
        let mut translated = *source;
        translated.get_instance_proc_addr = proc_address_callback(source.get_instance_proc_addr)?;
        drivers.push(translated);
    }
    let drivers = drivers.into_boxed_slice();
    list.drivers = drivers.as_ptr();
    Some(TranslatedDirectDrivers {
        list: Box::new(list),
        _drivers: drivers,
    })
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static SEEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "sysv64" fn guest_command(a: usize, b: usize) -> usize {
        SEEN.store(a ^ b, Ordering::SeqCst);
        a.wrapping_add(b)
    }

    unsafe extern "sysv64" fn guest_gipa(
        _instance: VkInstance,
        name: *const c_char,
    ) -> *const c_void {
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        if name == b"vkGetBufferDeviceAddress" {
            guest_command as *const () as *const c_void
        } else {
            core::ptr::null()
        }
    }

    #[test]
    fn layouts_match_the_extension_abi() {
        assert_eq!(core::mem::size_of::<DirectDriverLoadingInfo>(), 32);
        assert_eq!(core::mem::size_of::<DirectDriverLoadingList>(), 32);
    }

    #[test]
    fn proc_address_and_returned_command_cross_both_abis() {
        let callback = proc_address_callback(guest_gipa as *const () as usize).expect("callback");
        let host_gipa: unsafe extern "system" fn(VkInstance, *const c_char) -> *const c_void =
            unsafe { core::mem::transmute(callback) };
        let command =
            unsafe { host_gipa(core::ptr::null_mut(), c"vkGetBufferDeviceAddress".as_ptr()) };
        assert!(!command.is_null());
        assert_ne!(command as usize, guest_command as *const () as usize);
        let host_command: unsafe extern "system" fn(usize, usize) -> usize =
            unsafe { core::mem::transmute(command) };
        SEEN.store(0, Ordering::SeqCst);
        assert_eq!(unsafe { host_command(0x1234, 0x5678) }, 0x68ac);
        assert_eq!(SEEN.load(Ordering::SeqCst), 0x1234 ^ 0x5678);
    }

    #[test]
    fn direct_driver_list_replaces_every_guest_gipa() {
        let drivers = [
            DirectDriverLoadingInfo {
                s_type: 1_000_459_000,
                _padding: 0,
                p_next: core::ptr::null(),
                flags: 0,
                _padding2: 0,
                get_instance_proc_addr: guest_gipa as *const () as usize,
            },
            DirectDriverLoadingInfo {
                s_type: 1_000_459_000,
                _padding: 0,
                p_next: core::ptr::null(),
                flags: 0,
                _padding2: 0,
                get_instance_proc_addr: guest_gipa as *const () as usize,
            },
        ];
        let list = DirectDriverLoadingList {
            s_type: DIRECT_DRIVER_LOADING_LIST_LUNARG,
            _padding: 0,
            p_next: core::ptr::null(),
            mode: 1,
            driver_count: drivers.len() as u32,
            drivers: drivers.as_ptr(),
        };
        let translated = unsafe {
            crate::debug_callbacks::translate_instance_chain_head((&raw const list).cast())
        }
        .expect("translated instance chain");
        let translated_list =
            unsafe { translated.as_ptr().cast::<DirectDriverLoadingList>().read() };
        assert_eq!(translated_list.driver_count, 2);
        let translated_drivers =
            unsafe { core::slice::from_raw_parts(translated_list.drivers, drivers.len()) };
        for driver in translated_drivers {
            assert_ne!(
                driver.get_instance_proc_addr,
                guest_gipa as *const () as usize
            );
            assert_eq!(
                driver.get_instance_proc_addr,
                proc_address_callback(guest_gipa as *const () as usize).unwrap()
            );
        }
    }

    #[test]
    fn a_direct_driver_after_an_unrelated_instance_node_is_translated() {
        #[repr(C)]
        struct ValidationFlags {
            s_type: u32,
            _padding: u32,
            p_next: *const c_void,
            disabled_check_count: u32,
            _padding2: u32,
            disabled_checks: *const u32,
        }

        let driver = DirectDriverLoadingInfo {
            s_type: 1_000_459_000,
            _padding: 0,
            p_next: core::ptr::null(),
            flags: 0,
            _padding2: 0,
            get_instance_proc_addr: guest_gipa as *const () as usize,
        };
        let list = DirectDriverLoadingList {
            s_type: DIRECT_DRIVER_LOADING_LIST_LUNARG,
            _padding: 0,
            p_next: core::ptr::null(),
            mode: 1,
            driver_count: 1,
            drivers: &raw const driver,
        };
        let validation = ValidationFlags {
            s_type: 1_000_061_000,
            _padding: 0,
            p_next: (&raw const list).cast(),
            disabled_check_count: 0,
            _padding2: 0,
            disabled_checks: core::ptr::null(),
        };
        let translated = unsafe {
            crate::debug_callbacks::translate_instance_chain_head((&raw const validation).cast())
        }
        .expect("complete translated chain");
        let translated_validation = unsafe { &*translated.as_ptr().cast::<ValidationFlags>() };
        assert_ne!(translated_validation.p_next, validation.p_next);
        let translated_list = unsafe {
            &*translated_validation
                .p_next
                .cast::<DirectDriverLoadingList>()
        };
        let translated_driver = unsafe { &*translated_list.drivers };
        assert_ne!(
            translated_driver.get_instance_proc_addr,
            guest_gipa as *const () as usize
        );
    }
}
