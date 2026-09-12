//! ABI translation for Vulkan debug-report and debug-utils callbacks.

use core::ffi::{c_char, c_void};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::forward::{Allocator, Forward};
use crate::types::{VkBool32, VkHandle, VkInstance, VkResult};

const DEBUG_REPORT_CREATE_INFO: u32 = 1_000_011_000;
const DEBUG_UTILS_CREATE_INFO: u32 = 1_000_128_004;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DebugUtilsCreateInfo {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    flags: u32,
    message_severity: u32,
    message_type: u32,
    _padding2: u32,
    callback: usize,
    user_data: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DebugReportCreateInfo {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    flags: u32,
    _padding2: u32,
    callback: usize,
    user_data: *mut c_void,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CallbackKey {
    callback: usize,
    user_data: usize,
}

struct Bridge {
    key: CallbackKey,
}

fn bridges() -> &'static Mutex<HashMap<CallbackKey, usize>> {
    static STORE: OnceLock<Mutex<HashMap<CallbackKey, usize>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn bridge(callback: usize, user_data: *mut c_void) -> *mut c_void {
    let key = CallbackKey {
        callback,
        user_data: user_data as usize,
    };
    let mut store = bridges()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(&existing) = store.get(&key) {
        return existing as *mut c_void;
    }
    let bridge = Box::leak(Box::new(Bridge { key })) as *mut Bridge as usize;
    store.insert(key, bridge);
    bridge as *mut c_void
}

type GuestUtils = unsafe extern "sysv64" fn(u32, u32, *const c_void, *mut c_void) -> VkBool32;

unsafe extern "system" fn utils_callback(
    severity: u32,
    kinds: u32,
    data: *const c_void,
    opaque: *mut c_void,
) -> VkBool32 {
    crate::reverse_abi::restore_guest_tls();
    let bridge = unsafe { &*(opaque as *const Bridge) };
    let guest: GuestUtils = unsafe { core::mem::transmute(bridge.key.callback) };
    unsafe { guest(severity, kinds, data, bridge.key.user_data as *mut c_void) }
}

type GuestReport = unsafe extern "sysv64" fn(
    u32,
    u32,
    u64,
    usize,
    i32,
    *const c_char,
    *const c_char,
    *mut c_void,
) -> VkBool32;

unsafe extern "system" fn report_callback(
    flags: u32,
    object_type: u32,
    object: u64,
    location: usize,
    message_code: i32,
    layer_prefix: *const c_char,
    message: *const c_char,
    opaque: *mut c_void,
) -> VkBool32 {
    crate::reverse_abi::restore_guest_tls();
    let bridge = unsafe { &*(opaque as *const Bridge) };
    let guest: GuestReport = unsafe { core::mem::transmute(bridge.key.callback) };
    unsafe {
        guest(
            flags,
            object_type,
            object,
            location,
            message_code,
            layer_prefix,
            message,
            bridge.key.user_data as *mut c_void,
        )
    }
}

unsafe fn translate_utils(source: *const DebugUtilsCreateInfo) -> DebugUtilsCreateInfo {
    let mut translated = unsafe { *source };
    if translated.callback != 0 {
        translated.user_data = bridge(translated.callback, translated.user_data);
        translated.callback = utils_callback as *const () as usize;
    }
    translated
}

unsafe fn translate_report(source: *const DebugReportCreateInfo) -> DebugReportCreateInfo {
    let mut translated = unsafe { *source };
    if translated.callback != 0 {
        translated.user_data = bridge(translated.callback, translated.user_data);
        translated.callback = report_callback as *const () as usize;
    }
    translated
}

enum InstanceChainNode {
    Utils(Box<DebugUtilsCreateInfo>),
    Report(Box<DebugReportCreateInfo>),
    Direct(crate::reverse_driver::TranslatedDirectDrivers),
    Raw(crate::pnext::RawNode),
}

/// Storage for the translated callback-bearing prefix of an instance `pNext`
/// chain. Each cloned node points at the next owned clone; the last one retains
/// the first unknown tail exactly as the application supplied it.
pub struct InstanceChainHead {
    node: InstanceChainNode,
    _next: Option<Box<InstanceChainHead>>,
}

impl InstanceChainNode {
    pub fn as_ptr(&self) -> *const c_void {
        match self {
            Self::Utils(info) => (&raw const **info).cast(),
            Self::Report(info) => (&raw const **info).cast(),
            Self::Direct(info) => info.as_ptr(),
            Self::Raw(info) => info.as_ptr(),
        }
    }

    fn p_next(&self) -> *const c_void {
        match self {
            Self::Utils(info) => info.p_next,
            Self::Report(info) => info.p_next,
            Self::Direct(info) => info.p_next(),
            Self::Raw(info) => info.p_next(),
        }
    }

    fn set_p_next(&mut self, p_next: *const c_void) {
        match self {
            Self::Utils(info) => info.p_next = p_next,
            Self::Report(info) => info.p_next = p_next,
            Self::Direct(info) => info.set_p_next(p_next),
            Self::Raw(info) => info.set_p_next(p_next),
        }
    }
}

impl InstanceChainHead {
    pub fn as_ptr(&self) -> *const c_void {
        self.node.as_ptr()
    }
}

pub unsafe fn translate_instance_chain_head(p_next: *const c_void) -> Option<InstanceChainHead> {
    unsafe { translate_instance_chain(p_next, 0) }
}

unsafe fn translate_instance_chain(
    p_next: *const c_void,
    depth: usize,
) -> Option<InstanceChainHead> {
    if p_next.is_null() {
        return None;
    }
    if depth >= 256 {
        return None;
    }
    let s_type = unsafe { *(p_next as *const u32) };
    let mut node = match s_type {
        DEBUG_UTILS_CREATE_INFO => {
            InstanceChainNode::Utils(Box::new(unsafe { translate_utils(p_next.cast()) }))
        }
        DEBUG_REPORT_CREATE_INFO => {
            InstanceChainNode::Report(Box::new(unsafe { translate_report(p_next.cast()) }))
        }
        _ => match unsafe { crate::reverse_driver::translate_node(p_next) } {
            Some(direct) => InstanceChainNode::Direct(direct),
            None => InstanceChainNode::Raw(unsafe { crate::pnext::RawNode::copy(p_next)? }),
        },
    };
    let next = unsafe { translate_instance_chain(node.p_next(), depth + 1) }.map(Box::new);
    if let Some(next) = next.as_ref() {
        node.set_p_next(next.as_ptr());
    }
    Some(InstanceChainHead { node, _next: next })
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateDebugUtilsMessengerEXT")]
pub unsafe extern "sysv64" fn vkCreateDebugUtilsMessengerEXT(
    instance: VkInstance,
    create_info: *const DebugUtilsCreateInfo,
    allocator: Allocator,
    messenger: *mut VkHandle,
) -> VkResult {
    if create_info.is_null() {
        return -1;
    }
    type Host = unsafe extern "system" fn(
        VkInstance,
        *const DebugUtilsCreateInfo,
        *const c_void,
        *mut VkHandle,
    ) -> VkResult;
    let Some(address) = crate::host::instance_symbol(instance, "vkCreateDebugUtilsMessengerEXT")
    else {
        return -7;
    };
    let host: Host = unsafe { core::mem::transmute(address) };
    let translated = unsafe { translate_utils(create_info) };
    unsafe {
        host(
            instance,
            &raw const translated,
            allocator.forward(),
            messenger,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateDebugReportCallbackEXT")]
pub unsafe extern "sysv64" fn vkCreateDebugReportCallbackEXT(
    instance: VkInstance,
    create_info: *const DebugReportCreateInfo,
    allocator: Allocator,
    callback: *mut VkHandle,
) -> VkResult {
    if create_info.is_null() {
        return -1;
    }
    type Host = unsafe extern "system" fn(
        VkInstance,
        *const DebugReportCreateInfo,
        *const c_void,
        *mut VkHandle,
    ) -> VkResult;
    let Some(address) = crate::host::instance_symbol(instance, "vkCreateDebugReportCallbackEXT")
    else {
        return -7;
    };
    let host: Host = unsafe { core::mem::transmute(address) };
    let translated = unsafe { translate_report(create_info) };
    unsafe {
        host(
            instance,
            &raw const translated,
            allocator.forward(),
            callback,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static SEEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "sysv64" fn guest_utils(
        severity: u32,
        kinds: u32,
        _data: *const c_void,
        user_data: *mut c_void,
    ) -> VkBool32 {
        SEEN.store(
            severity as usize ^ kinds as usize ^ user_data as usize,
            Ordering::SeqCst,
        );
        1
    }

    #[test]
    fn layouts_match_the_vulkan_abi() {
        assert_eq!(core::mem::size_of::<DebugUtilsCreateInfo>(), 48);
        assert_eq!(core::mem::size_of::<DebugReportCreateInfo>(), 40);
    }

    #[test]
    fn debug_utils_callback_crosses_both_abis() {
        let source = DebugUtilsCreateInfo {
            s_type: DEBUG_UTILS_CREATE_INFO,
            _padding: 0,
            p_next: core::ptr::null(),
            flags: 0,
            message_severity: 0,
            message_type: 0,
            _padding2: 0,
            callback: guest_utils as *const () as usize,
            user_data: 0x1234usize as *mut c_void,
        };
        let translated = unsafe { translate_utils(&raw const source) };
        let host: unsafe extern "system" fn(u32, u32, *const c_void, *mut c_void) -> VkBool32 =
            unsafe { core::mem::transmute(translated.callback) };
        SEEN.store(0, Ordering::SeqCst);
        assert_eq!(
            unsafe { host(4, 2, core::ptr::null(), translated.user_data) },
            1
        );
        assert_eq!(SEEN.load(Ordering::SeqCst), 4 ^ 2 ^ 0x1234);
    }
}
