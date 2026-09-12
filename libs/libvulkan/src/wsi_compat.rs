//! Surface extension bridging for Xlib / XCB / Wayland to native Win32 surfaces.

use crate::forward::Allocator;
use crate::types::{VkBool32, VkHandle, VkInstance, VkPhysicalDevice, VkResult};
use core::ffi::{c_char, c_void};
use std::sync::OnceLock;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkExtensionProperties {
    pub extension_name: [c_char; 256],
    pub spec_version: u32,
}

fn make_ext_prop(name: &[u8], version: u32) -> VkExtensionProperties {
    let mut ext = VkExtensionProperties {
        extension_name: [0; 256],
        spec_version: version,
    };
    for (i, &b) in name.iter().enumerate() {
        if i < 255 {
            ext.extension_name[i] = b as c_char;
        }
    }
    ext
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkApplicationInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub application_name: *const c_char,
    pub application_version: u32,
    pub engine_name: *const c_char,
    pub engine_version: u32,
    pub api_version: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkInstanceCreateInfo {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub p_application_info: *const VkApplicationInfo,
    pub enabled_layer_count: u32,
    pub pp_enabled_layer_names: *const *const c_char,
    pub enabled_extension_count: u32,
    pub pp_enabled_extension_names: *const *const c_char,
}

#[repr(C)]
pub struct VkXlibSurfaceCreateInfoKHR {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub dpy: *mut c_void,
    pub window: usize,
}

#[repr(C)]
pub struct VkXcbSurfaceCreateInfoKHR {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub connection: *mut c_void,
    pub window: u32,
}

#[repr(C)]
pub struct VkWaylandSurfaceCreateInfoKHR {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub display: *mut c_void,
    pub surface: *mut c_void,
}

#[repr(C)]
pub struct VkWin32SurfaceCreateInfoKHR {
    pub s_type: u32,
    pub p_next: *const c_void,
    pub flags: u32,
    pub hinstance: *mut c_void,
    pub hwnd: *mut c_void,
}

type HostEnumExtPropsFn =
    unsafe extern "system" fn(*const c_char, *mut u32, *mut c_void) -> VkResult;

fn host_enum_ext_props() -> Option<HostEnumExtPropsFn> {
    static FP: OnceLock<Option<usize>> = OnceLock::new();
    let &addr = FP.get_or_init(|| crate::host::symbol("vkEnumerateInstanceExtensionProperties"));
    let addr = addr?;
    Some(unsafe { core::mem::transmute(addr) })
}

#[cfg(test)]
fn extension_name(property: &VkExtensionProperties) -> &[u8] {
    let end = property
        .extension_name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(property.extension_name.len());
    // Vulkan extension names are ASCII, so preserving each low byte is exact.
    unsafe { core::slice::from_raw_parts(property.extension_name.as_ptr().cast(), end) }
}

type HostCreateInstanceFn =
    unsafe extern "system" fn(*const c_void, *const c_void, *mut VkInstance) -> VkResult;

fn host_create_instance() -> Option<HostCreateInstanceFn> {
    static FP: OnceLock<Option<usize>> = OnceLock::new();
    let &addr = FP.get_or_init(|| crate::host::symbol("vkCreateInstance"));
    let addr = addr?;
    Some(unsafe { core::mem::transmute(addr) })
}

type HostGetQueueFamilyPropertiesFn =
    unsafe extern "system" fn(VkPhysicalDevice, *mut u32, *mut c_void);

fn host_get_queue_family_properties(version_two: bool) -> Option<HostGetQueueFamilyPropertiesFn> {
    static LEGACY: OnceLock<Option<usize>> = OnceLock::new();
    static VERSION_TWO: OnceLock<Option<usize>> = OnceLock::new();
    let address = if version_two {
        *VERSION_TWO
            .get_or_init(|| crate::host::symbol("vkGetPhysicalDeviceQueueFamilyProperties2"))
    } else {
        *LEGACY.get_or_init(|| crate::host::symbol("vkGetPhysicalDeviceQueueFamilyProperties"))
    }?;
    Some(unsafe { core::mem::transmute(address) })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkQueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: [u32; 3],
}

#[repr(C)]
struct VkQueueFamilyProperties2 {
    s_type: u32,
    _padding: u32,
    p_next: *mut c_void,
    properties: VkQueueFamilyProperties,
}

fn trace_queue_families() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_VULKAN_TRACE").is_some())
}

unsafe fn trace_queue_family_result(
    version_two: bool,
    capacity: u32,
    count: *mut u32,
    properties: *mut c_void,
) {
    if !trace_queue_families() || count.is_null() {
        return;
    }
    let returned = unsafe { *count };
    if properties.is_null() {
        eprintln!(
            "[libvulkan] vkGetPhysicalDeviceQueueFamilyProperties{} count={returned}",
            if version_two { "2" } else { "" }
        );
        return;
    }
    let readable = capacity.min(returned) as usize;
    let flags: Vec<String> = if version_two {
        let entries = unsafe {
            core::slice::from_raw_parts(properties.cast::<VkQueueFamilyProperties2>(), readable)
        };
        entries
            .iter()
            .map(|entry| {
                format!(
                    "{:#x}/{}(sType={})",
                    entry.properties.queue_flags, entry.properties.queue_count, entry.s_type
                )
            })
            .collect()
    } else {
        let entries = unsafe {
            core::slice::from_raw_parts(properties.cast::<VkQueueFamilyProperties>(), readable)
        };
        entries
            .iter()
            .map(|entry| format!("{:#x}/{}", entry.queue_flags, entry.queue_count))
            .collect()
    };
    eprintln!(
        "[libvulkan] vkGetPhysicalDeviceQueueFamilyProperties{} capacity={capacity} returned={returned} flags=[{}]",
        if version_two { "2" } else { "" },
        flags.join(", ")
    );
}

/// Dispatch-target wrapper used for queue-family diagnostics. The call remains
/// a direct host passthrough; tracing is opt-in and reads only the structures the
/// driver has just initialized.
pub unsafe extern "sysv64" fn get_physical_device_queue_family_properties(
    physical_device: VkPhysicalDevice,
    count: *mut u32,
    properties: *mut c_void,
) {
    let capacity = if count.is_null() {
        0
    } else {
        unsafe { *count }
    };
    if let Some(host) = host_get_queue_family_properties(false) {
        unsafe { host(physical_device, count, properties) };
        unsafe { trace_queue_family_result(false, capacity, count, properties) };
    }
}

/// The Vulkan 1.1 structure-chain variant of
/// [`get_physical_device_queue_family_properties`].
pub unsafe extern "sysv64" fn get_physical_device_queue_family_properties2(
    physical_device: VkPhysicalDevice,
    count: *mut u32,
    properties: *mut c_void,
) {
    let capacity = if count.is_null() {
        0
    } else {
        unsafe { *count }
    };
    if let Some(host) = host_get_queue_family_properties(true) {
        unsafe { host(physical_device, count, properties) };
        unsafe { trace_queue_family_result(true, capacity, count, properties) };
    }
}

/// Linux window-system presentation predicates have the same meaning as the
/// Win32 predicate after their surface creation is translated to an `HWND`.
/// The display, connection and visual arguments identify the Linux window
/// system only; the native Win32 driver does not need them.
#[unsafe(export_name = "kinakaze_engine_libvulkan_vkGetPhysicalDeviceXcbPresentationSupportKHR")]
pub unsafe extern "sysv64" fn vkGetPhysicalDeviceXcbPresentationSupportKHR(
    physical_device: VkPhysicalDevice,
    queue_family_index: u32,
    _connection: *mut c_void,
    _visual_id: u32,
) -> VkBool32 {
    unsafe {
        crate::groups::physical_device::vkGetPhysicalDeviceWin32PresentationSupportKHR(
            physical_device,
            queue_family_index,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkGetPhysicalDeviceXlibPresentationSupportKHR")]
pub unsafe extern "sysv64" fn vkGetPhysicalDeviceXlibPresentationSupportKHR(
    physical_device: VkPhysicalDevice,
    queue_family_index: u32,
    _display: *mut c_void,
    _visual_id: usize,
) -> VkBool32 {
    unsafe {
        crate::groups::physical_device::vkGetPhysicalDeviceWin32PresentationSupportKHR(
            physical_device,
            queue_family_index,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkGetPhysicalDeviceWaylandPresentationSupportKHR")]
pub unsafe extern "sysv64" fn vkGetPhysicalDeviceWaylandPresentationSupportKHR(
    physical_device: VkPhysicalDevice,
    queue_family_index: u32,
    _display: *mut c_void,
) -> VkBool32 {
    unsafe {
        crate::groups::physical_device::vkGetPhysicalDeviceWin32PresentationSupportKHR(
            physical_device,
            queue_family_index,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkEnumerateInstanceExtensionProperties")]
pub unsafe extern "sysv64" fn vkEnumerateInstanceExtensionProperties(
    p_layer_name: *const c_char,
    p_property_count: *mut u32,
    p_properties: *mut VkExtensionProperties,
) -> VkResult {
    if p_property_count.is_null() {
        return -1;
    }
    let Some(host_fn) = host_enum_ext_props() else {
        return -1;
    };

    // Query into private storage before projecting host capabilities to the
    // guest. Passing the caller's buffer straight through would make it
    // impossible to remove extensions whose callbacks cannot cross the ABI.
    let mut host_count = 0u32;
    let count_result = unsafe { host_fn(p_layer_name, &mut host_count, core::ptr::null_mut()) };
    if count_result < 0 {
        return count_result;
    }

    let mut host = vec![make_ext_prop(b"", 0); host_count as usize];
    if host_count != 0 {
        let fill_result = unsafe {
            host_fn(
                p_layer_name,
                &mut host_count,
                host.as_mut_ptr().cast::<c_void>(),
            )
        };
        if fill_result < 0 {
            return fill_result;
        }
        host.truncate((host_count as usize).min(host.len()));
    }

    let mut projected = Vec::with_capacity(host.len() + 3);
    for property in host {
        projected.push(property);
    }

    // These are instance extensions, not layer extensions. Surface creation is
    // translated to a native Win32 surface by this module.
    if p_layer_name.is_null() {
        for property in [
            make_ext_prop(b"VK_KHR_xcb_surface", 6),
            make_ext_prop(b"VK_KHR_xlib_surface", 6),
            make_ext_prop(b"VK_KHR_wayland_surface", 6),
        ] {
            let end = property
                .extension_name
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(property.extension_name.len());
            if !projected.iter().any(|existing| {
                existing.extension_name[..end] == property.extension_name[..end]
                    && existing.extension_name.get(end).copied().unwrap_or(0) == 0
            }) {
                projected.push(property);
            }
        }
    }

    if p_properties.is_null() {
        unsafe { *p_property_count = projected.len() as u32 };
        return 0;
    }

    let capacity = unsafe { *p_property_count } as usize;
    let written = capacity.min(projected.len());
    unsafe {
        core::ptr::copy_nonoverlapping(projected.as_ptr(), p_properties, written);
        *p_property_count = written as u32;
    }
    if written < projected.len() { 5 } else { 0 }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateInstance")]
pub unsafe extern "sysv64" fn vkCreateInstance(
    p_create_info: *const VkInstanceCreateInfo,
    p_allocator: Allocator,
    p_instance: *mut VkInstance,
) -> VkResult {
    if p_create_info.is_null() {
        return -1;
    }
    let orig = unsafe { &*p_create_info };
    let mut extensions: Vec<*const c_char> = Vec::new();
    if !orig.pp_enabled_extension_names.is_null() && orig.enabled_extension_count > 0 {
        let slice = unsafe {
            core::slice::from_raw_parts(
                orig.pp_enabled_extension_names,
                orig.enabled_extension_count as usize,
            )
        };
        for &ext_ptr in slice {
            if !ext_ptr.is_null() {
                let name = unsafe { core::ffi::CStr::from_ptr(ext_ptr) }.to_bytes();
                if name == b"VK_KHR_xlib_surface"
                    || name == b"VK_KHR_xcb_surface"
                    || name == b"VK_KHR_wayland_surface"
                {
                    continue;
                }
                extensions.push(ext_ptr);
            }
        }
    }
    static WIN32_SURFACE: &[u8] = b"VK_KHR_win32_surface\0";
    static KHR_SURFACE: &[u8] = b"VK_KHR_surface\0";
    extensions.push(WIN32_SURFACE.as_ptr() as *const c_char);
    extensions.push(KHR_SURFACE.as_ptr() as *const c_char);

    let mut modified_info = *orig;
    modified_info.enabled_extension_count = extensions.len() as u32;
    modified_info.pp_enabled_extension_names = extensions.as_ptr();
    let translated_debug_head =
        unsafe { crate::debug_callbacks::translate_instance_chain_head(modified_info.p_next) };
    if let Some(head) = translated_debug_head.as_ref() {
        modified_info.p_next = head.as_ptr();
    }

    let Some(host_fn) = host_create_instance() else {
        return -1;
    };
    use crate::forward::Forward;
    let alloc = p_allocator.forward();
    let result = unsafe {
        host_fn(
            &modified_info as *const _ as *const c_void,
            alloc,
            p_instance,
        )
    };
    if result == 0 && !p_instance.is_null() {
        let instance = unsafe { *p_instance };
        if !instance.is_null() {
            crate::host::register_instance(instance);
        }
    }
    result
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateXlibSurfaceKHR")]
pub unsafe extern "sysv64" fn vkCreateXlibSurfaceKHR(
    instance: VkInstance,
    p_create_info: *const VkXlibSurfaceCreateInfoKHR,
    p_allocator: Allocator,
    p_surface: *mut VkHandle,
) -> VkResult {
    if p_create_info.is_null() {
        return -1;
    }
    let hwnd = unsafe { (*p_create_info).window } as *mut c_void;
    let win32_info = VkWin32SurfaceCreateInfoKHR {
        s_type: 1000009000,
        p_next: core::ptr::null(),
        flags: 0,
        hinstance: kinakaze_libdisplay::ui::module_instance() as *mut c_void,
        hwnd,
    };
    if trace_queue_families() {
        eprintln!(
            "[libvulkan] vkCreateXlibSurfaceKHR window={:#x} -> hinstance={:?} hwnd={:?}",
            unsafe { (*p_create_info).window },
            win32_info.hinstance,
            win32_info.hwnd
        );
    }
    unsafe {
        crate::groups::wsi::vkCreateWin32SurfaceKHR(
            instance,
            &win32_info as *const _ as *const c_void,
            p_allocator,
            p_surface,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateXcbSurfaceKHR")]
pub unsafe extern "sysv64" fn vkCreateXcbSurfaceKHR(
    instance: VkInstance,
    p_create_info: *const VkXcbSurfaceCreateInfoKHR,
    p_allocator: Allocator,
    p_surface: *mut VkHandle,
) -> VkResult {
    if p_create_info.is_null() {
        return -1;
    }
    let wid = unsafe { (*p_create_info).window };
    let hwnd: *mut c_void = if let Some(h) = kinakaze_libxcb::get_native_hwnd(wid) {
        h as *mut c_void
    } else if let Some(h) = kinakaze_libdisplay::window::native_handle(wid as u64) {
        h as *mut c_void
    } else {
        wid as usize as *mut c_void
    };
    let win32_info = VkWin32SurfaceCreateInfoKHR {
        s_type: 1000009000,
        p_next: core::ptr::null(),
        flags: 0,
        hinstance: kinakaze_libdisplay::ui::module_instance() as *mut c_void,
        hwnd,
    };
    if trace_queue_families() {
        eprintln!(
            "[libvulkan] vkCreateXcbSurfaceKHR window={wid:#x} -> hinstance={:?} hwnd={:?}",
            win32_info.hinstance, win32_info.hwnd
        );
    }
    unsafe {
        crate::groups::wsi::vkCreateWin32SurfaceKHR(
            instance,
            &win32_info as *const _ as *const c_void,
            p_allocator,
            p_surface,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libvulkan_vkCreateWaylandSurfaceKHR")]
pub unsafe extern "sysv64" fn vkCreateWaylandSurfaceKHR(
    instance: VkInstance,
    p_create_info: *const VkWaylandSurfaceCreateInfoKHR,
    p_allocator: Allocator,
    p_surface: *mut VkHandle,
) -> VkResult {
    if p_create_info.is_null() {
        return -1;
    }
    // libdisplay's Wayland compatibility object stores the native HWND in the
    // surface slot, just as the Xlib/XCB compatibility objects store it in
    // their window id.  The host Vulkan driver therefore receives the same
    // native surface regardless of which Linux WSI frontend selected it.
    let hwnd = unsafe { (*p_create_info).surface };
    let win32_info = VkWin32SurfaceCreateInfoKHR {
        s_type: 1000009000,
        p_next: core::ptr::null(),
        flags: 0,
        hinstance: kinakaze_libdisplay::ui::module_instance() as *mut c_void,
        hwnd,
    };
    unsafe {
        crate::groups::wsi::vkCreateWin32SurfaceKHR(
            instance,
            &win32_info as *const _ as *const c_void,
            p_allocator,
            p_surface,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_extension_list_includes_translated_debug_callbacks() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }

        let mut count = 0;
        assert_eq!(
            unsafe {
                vkEnumerateInstanceExtensionProperties(
                    core::ptr::null(),
                    &raw mut count,
                    core::ptr::null_mut(),
                )
            },
            0
        );
        let mut properties = vec![make_ext_prop(b"", 0); count as usize];
        assert_eq!(
            unsafe {
                vkEnumerateInstanceExtensionProperties(
                    core::ptr::null(),
                    &raw mut count,
                    properties.as_mut_ptr(),
                )
            },
            0
        );
        properties.truncate(count as usize);
        let names: Vec<&[u8]> = properties.iter().map(extension_name).collect();
        assert!(names.contains(&b"VK_KHR_xlib_surface".as_slice()));
        assert!(names.contains(&b"VK_KHR_xcb_surface".as_slice()));
        assert!(names.contains(&b"VK_EXT_debug_utils".as_slice()));
        assert!(names.contains(&b"VK_EXT_debug_report".as_slice()));
    }

    #[test]
    fn extension_enumeration_reports_incomplete_for_a_short_buffer() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let mut count = 1;
        let mut property = make_ext_prop(b"", 0);
        assert_eq!(
            unsafe {
                vkEnumerateInstanceExtensionProperties(
                    core::ptr::null(),
                    &raw mut count,
                    &raw mut property,
                )
            },
            5
        );
        assert_eq!(count, 1);
    }
}
