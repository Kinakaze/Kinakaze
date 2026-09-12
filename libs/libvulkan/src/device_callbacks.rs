//! Reverse-ABI callbacks carried by device-creation extension structures.

use core::ffi::c_void;

use crate::forward::{Allocator, Forward};
use crate::reverse_abi::{ArgumentClass, for_target};
use crate::types::{VkDevice, VkPhysicalDevice, VkResult};

const DEVICE_DEVICE_MEMORY_REPORT_CREATE_INFO_EXT: u32 = 1_000_284_001;
const FAULT_CALLBACK_INFO: u32 = 1_000_298_008;

/// Prefix and complete x86-64 layout of `VkDeviceCreateInfo`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceCreateInfo {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    queue_create_infos: *const c_void,
    enabled_layer_count: u32,
    _padding2: u32,
    enabled_layer_names: *const *const i8,
    enabled_extension_count: u32,
    _padding3: u32,
    enabled_extension_names: *const *const i8,
    enabled_features: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceMemoryReportCreateInfo {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    flags: u32,
    _padding2: u32,
    callback: usize,
    user_data: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FaultCallbackInfo {
    s_type: u32,
    _padding: u32,
    p_next: *const c_void,
    fault_count: u32,
    _padding2: u32,
    faults: *const c_void,
    callback: usize,
}

enum DeviceChainNode {
    MemoryReport(Box<DeviceMemoryReportCreateInfo>),
    Fault(Box<FaultCallbackInfo>),
    Raw(crate::pnext::RawNode),
}

impl DeviceChainNode {
    fn as_ptr(&self) -> *const c_void {
        match self {
            Self::MemoryReport(info) => (&raw const **info).cast(),
            Self::Fault(info) => (&raw const **info).cast(),
            Self::Raw(info) => info.as_ptr(),
        }
    }

    fn p_next(&self) -> *const c_void {
        match self {
            Self::MemoryReport(info) => info.p_next,
            Self::Fault(info) => info.p_next,
            Self::Raw(info) => info.p_next(),
        }
    }

    fn set_p_next(&mut self, p_next: *const c_void) {
        match self {
            Self::MemoryReport(info) => info.p_next = p_next,
            Self::Fault(info) => info.p_next = p_next,
            Self::Raw(info) => info.set_p_next(p_next),
        }
    }
}

/// Owns the translated callback-bearing prefix of a device-create chain.
pub struct DeviceChainHead {
    node: DeviceChainNode,
    _next: Option<Box<DeviceChainHead>>,
}

impl DeviceChainHead {
    fn as_ptr(&self) -> *const c_void {
        self.node.as_ptr()
    }
}

/// Translates a callback-bearing structure when it is the chain head.
///
/// The replacement retains the original `pNext`, so every later extension
/// structure reaches the driver unchanged. Vulkan only retains the callback
/// values, not the create-info pointer, allowing this owned copy to expire once
/// `vkCreateDevice` returns.
unsafe fn translate_chain_head(p_next: *const c_void) -> Option<DeviceChainHead> {
    unsafe { translate_chain(p_next, 0) }
}

unsafe fn translate_chain(p_next: *const c_void, depth: usize) -> Option<DeviceChainHead> {
    if p_next.is_null() {
        return None;
    }
    if depth >= 256 {
        return None;
    }
    let s_type = unsafe { p_next.cast::<u32>().read() };
    let mut node = match s_type {
        DEVICE_DEVICE_MEMORY_REPORT_CREATE_INFO_EXT => {
            let mut translated = unsafe { p_next.cast::<DeviceMemoryReportCreateInfo>().read() };
            if translated.callback != 0 {
                translated.callback = for_target(
                    translated.callback,
                    &[ArgumentClass::Integer, ArgumentClass::Integer],
                )?;
            }
            DeviceChainNode::MemoryReport(Box::new(translated))
        }
        FAULT_CALLBACK_INFO => {
            let mut translated = unsafe { p_next.cast::<FaultCallbackInfo>().read() };
            if translated.callback != 0 {
                translated.callback = for_target(
                    translated.callback,
                    &[
                        ArgumentClass::Integer,
                        ArgumentClass::Integer,
                        ArgumentClass::Integer,
                    ],
                )?;
            }
            DeviceChainNode::Fault(Box::new(translated))
        }
        _ => DeviceChainNode::Raw(unsafe { crate::pnext::RawNode::copy(p_next)? }),
    };
    let next = unsafe { translate_chain(node.p_next(), depth + 1) }.map(Box::new);
    if let Some(next) = next.as_ref() {
        node.set_p_next(next.as_ptr());
    }
    Some(DeviceChainHead { node, _next: next })
}

type HostCreateDevice = unsafe extern "system" fn(
    VkPhysicalDevice,
    *const DeviceCreateInfo,
    *const c_void,
    *mut VkDevice,
) -> VkResult;

static HOST: crate::host::Lazy<HostCreateDevice> = crate::host::Lazy::new("vkCreateDevice");

pub fn host_present() -> bool {
    HOST.present()
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateDevice")]
pub unsafe extern "sysv64" fn vkCreateDevice(
    physical_device: VkPhysicalDevice,
    create_info: *const DeviceCreateInfo,
    allocator: Allocator,
    device: *mut VkDevice,
) -> VkResult {
    if create_info.is_null() {
        return -1;
    }
    let mut translated = unsafe { create_info.read() };
    let chain_head = unsafe { translate_chain_head(translated.p_next) };
    if let Some(head) = chain_head.as_ref() {
        translated.p_next = head.as_ptr();
    }
    unsafe {
        (HOST.resolve())(
            physical_device,
            &raw const translated,
            allocator.forward(),
            device,
        )
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static MEMORY_SEEN: AtomicUsize = AtomicUsize::new(0);
    static FAULT_SEEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "sysv64" fn memory_report(data: *const c_void, user: *mut c_void) {
        MEMORY_SEEN.store(data as usize ^ user as usize, Ordering::SeqCst);
    }

    unsafe extern "sysv64" fn fault_report(unrecorded: u32, count: u32, faults: *const c_void) {
        FAULT_SEEN.store(
            unrecorded as usize ^ count as usize ^ faults as usize,
            Ordering::SeqCst,
        );
    }

    #[test]
    fn layouts_match_the_vulkan_x86_64_abi() {
        assert_eq!(core::mem::size_of::<DeviceCreateInfo>(), 72);
        assert_eq!(core::mem::size_of::<DeviceMemoryReportCreateInfo>(), 40);
        assert_eq!(core::mem::size_of::<FaultCallbackInfo>(), 40);
    }

    #[test]
    fn device_memory_report_callback_crosses_both_abis() {
        let source = DeviceMemoryReportCreateInfo {
            s_type: DEVICE_DEVICE_MEMORY_REPORT_CREATE_INFO_EXT,
            _padding: 0,
            p_next: core::ptr::null(),
            flags: 0,
            _padding2: 0,
            callback: memory_report as *const () as usize,
            user_data: 0x4567usize as *mut c_void,
        };
        let translated = unsafe { translate_chain_head((&raw const source).cast()) }
            .expect("translated callback");
        let DeviceChainNode::MemoryReport(translated) = translated.node else {
            panic!("wrong chain kind")
        };
        let host: unsafe extern "system" fn(*const c_void, *mut c_void) =
            unsafe { core::mem::transmute(translated.callback) };
        MEMORY_SEEN.store(0, Ordering::SeqCst);
        unsafe { host(0x1234usize as *const c_void, translated.user_data) };
        assert_eq!(MEMORY_SEEN.load(Ordering::SeqCst), 0x1234 ^ 0x4567);
    }

    #[test]
    fn fault_callback_without_user_data_still_binds_its_target() {
        let source = FaultCallbackInfo {
            s_type: FAULT_CALLBACK_INFO,
            _padding: 0,
            p_next: core::ptr::null(),
            fault_count: 0,
            _padding2: 0,
            faults: core::ptr::null(),
            callback: fault_report as *const () as usize,
        };
        let translated = unsafe { translate_chain_head((&raw const source).cast()) }
            .expect("translated callback");
        let DeviceChainNode::Fault(translated) = translated.node else {
            panic!("wrong chain kind")
        };
        let host: unsafe extern "system" fn(u32, u32, *const c_void) =
            unsafe { core::mem::transmute(translated.callback) };
        FAULT_SEEN.store(0, Ordering::SeqCst);
        unsafe { host(1, 3, 0x8000usize as *const c_void) };
        assert_eq!(FAULT_SEEN.load(Ordering::SeqCst), 1 ^ 3 ^ 0x8000);
    }

    #[test]
    fn a_callback_after_an_unrelated_feature_node_is_still_translated() {
        #[repr(C)]
        struct DynamicRenderingFeature {
            s_type: u32,
            _padding: u32,
            p_next: *const c_void,
            dynamic_rendering: u32,
            _padding2: u32,
        }

        let callback = DeviceMemoryReportCreateInfo {
            s_type: DEVICE_DEVICE_MEMORY_REPORT_CREATE_INFO_EXT,
            _padding: 0,
            p_next: core::ptr::null(),
            flags: 0,
            _padding2: 0,
            callback: memory_report as *const () as usize,
            user_data: 0x2222usize as *mut c_void,
        };
        let feature = DynamicRenderingFeature {
            s_type: 1_000_044_003,
            _padding: 0,
            p_next: (&raw const callback).cast(),
            dynamic_rendering: 1,
            _padding2: 0,
        };
        let translated = unsafe { translate_chain_head((&raw const feature).cast()) }
            .expect("complete translated chain");
        let translated_feature = unsafe { &*translated.as_ptr().cast::<DynamicRenderingFeature>() };
        assert_ne!(translated_feature.p_next, feature.p_next);
        assert_eq!(feature.p_next, (&raw const callback).cast());
        let translated_callback = unsafe {
            &*translated_feature
                .p_next
                .cast::<DeviceMemoryReportCreateInfo>()
        };
        assert_ne!(translated_callback.callback, callback.callback);
        let host: unsafe extern "system" fn(*const c_void, *mut c_void) =
            unsafe { core::mem::transmute(translated_callback.callback) };
        MEMORY_SEEN.store(0, Ordering::SeqCst);
        unsafe { host(0x1111usize as *const c_void, translated_callback.user_data) };
        assert_eq!(MEMORY_SEEN.load(Ordering::SeqCst), 0x1111 ^ 0x2222);
    }
}
