//! Whole-path tests: a real instance, a real device, a real surface.
//!
//! The per-group tests check that a name resolves to our thunk and that the
//! address is not the host's. That is necessary and not sufficient — it says
//! nothing about whether a *structure* survives the crossing, and structures are
//! where a passthrough would actually fail.
//!
//! So these tests do what a guest does. They fill in a `VkInstanceCreateInfo`,
//! create an instance, enumerate the physical devices, read their properties, find
//! a graphics queue family, create a logical device, and create a
//! `VkSurfaceKHR` on a real `HWND`. Every one of those calls goes out through the
//! `extern "sysv64"` exports the guest binds to, so what is being tested is the
//! whole path rather than the dispatch table.
//!
//! The structures below are declared here and nowhere else in the crate. That is
//! deliberate: the passthrough itself must not know any layout (see
//! [`crate::types`]), but a *test* has to build one to have anything to pass, and
//! these are the only layouts anywhere in the crate that could drift from the
//! headers. Their offsets are asserted before they are used.

#![cfg(test)]

use core::ffi::{CStr, c_char, c_void};
use std::ffi::CString;

use crate::device_callbacks::vkCreateDevice;
use crate::forward::Allocator;
use crate::groups::instance::{vkDestroyDevice, vkDestroyInstance, vkEnumeratePhysicalDevices};
use crate::types::{VkDevice, VkInstance, VkPhysicalDevice};
use crate::vkCreateInstance;

/// "Use the driver's own allocator", which is what Vulkan reads a null as.
///
/// Spelled out rather than passed as a bare `0` so that the argument's type is
/// visible at each call: an allocator is the one parameter in this API that is not
/// forwarded unchanged, and a reader should be able to see which one it is.
fn no_allocator() -> Allocator {
    Allocator(core::ptr::null())
}

/// `VK_STRUCTURE_TYPE_APPLICATION_INFO`.
const APPLICATION_INFO: u32 = 0;
/// `VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO`.
const INSTANCE_CREATE_INFO: u32 = 1;
/// `VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO`.
const DEVICE_QUEUE_CREATE_INFO: u32 = 2;
/// `VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO`.
const DEVICE_CREATE_INFO: u32 = 3;
/// `VK_SUCCESS`.
const SUCCESS: i32 = 0;
/// `VK_QUEUE_GRAPHICS_BIT`.
const QUEUE_GRAPHICS: u32 = 0x1;

/// `VkApplicationInfo`.
#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    next: *const c_void,
    application_name: *const c_char,
    application_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

/// `VkInstanceCreateInfo`.
#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    next: *const c_void,
    flags: u32,
    application_info: *const ApplicationInfo,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
}

/// `VkDeviceQueueCreateInfo`.
#[repr(C)]
struct DeviceQueueCreateInfo {
    s_type: u32,
    next: *const c_void,
    flags: u32,
    queue_family_index: u32,
    queue_count: u32,
    queue_priorities: *const f32,
}

/// `VkDeviceCreateInfo`.
#[repr(C)]
struct DeviceCreateInfo {
    s_type: u32,
    next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    queue_create_infos: *const DeviceQueueCreateInfo,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
    enabled_features: *const c_void,
}

/// The leading fields of `VkPhysicalDeviceProperties`.
///
/// Only the prefix is declared. The full structure ends with `VkPhysicalDeviceLimits`
/// and `VkPhysicalDeviceSparseProperties`, together several hundred bytes of fields
/// no test here reads — and restating them would create exactly the drift risk this
/// crate otherwise avoids. Reads go through a generously sized buffer, so the driver
/// writing the whole structure is safe.
#[repr(C)]
struct PhysicalDevicePropertiesPrefix {
    api_version: u32,
    driver_version: u32,
    vendor_id: u32,
    device_id: u32,
    device_type: u32,
    device_name: [c_char; 256],
}

/// `VkPhysicalDeviceProperties` is under 1 KiB in every Vulkan version; 4 KiB is
/// room to spare for a structure whose tail this test does not model.
const PROPERTIES_BUFFER: usize = 4096;

/// The leading fields of `VkQueueFamilyProperties`, which is small enough to state
/// in full: flags, count, timestamp bits, then a `VkExtent3D`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: [u32; 3],
}

/// `VkWin32SurfaceCreateInfoKHR`.
#[repr(C)]
struct Win32SurfaceCreateInfo {
    s_type: u32,
    next: *const c_void,
    flags: u32,
    hinstance: *mut c_void,
    hwnd: *mut c_void,
}

/// `VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR`.
const WIN32_SURFACE_CREATE_INFO: u32 = 1000009000;

/// Whether the host can service these tests at all.
fn host_ready() -> bool {
    if !crate::host::loader_present() {
        eprintln!("skipped: no host Vulkan loader on this machine");
        return false;
    }
    true
}

/// Creates an instance with no layers and the given extensions.
fn create_instance(extensions: &[*const c_char]) -> Option<VkInstance> {
    let name = c"kinakaze-integration";
    let application = ApplicationInfo {
        s_type: APPLICATION_INFO,
        next: core::ptr::null(),
        application_name: name.as_ptr(),
        application_version: 1,
        engine_name: name.as_ptr(),
        engine_version: 1,
        // 1.0 so that any conformant loader accepts it.
        api_version: 1 << 22,
    };
    let create = InstanceCreateInfo {
        s_type: INSTANCE_CREATE_INFO,
        next: core::ptr::null(),
        flags: 0,
        application_info: &raw const application,
        enabled_layer_count: 0,
        enabled_layer_names: core::ptr::null(),
        enabled_extension_count: extensions.len() as u32,
        enabled_extension_names: if extensions.is_empty() {
            core::ptr::null()
        } else {
            extensions.as_ptr()
        },
    };
    let mut instance: VkInstance = core::ptr::null_mut();
    // SAFETY: the structures are live for the call and `instance` is writable.
    let result = unsafe {
        vkCreateInstance(
            (&raw const create).cast(),
            no_allocator(),
            &raw mut instance,
        )
    };
    (result == SUCCESS && !instance.is_null()).then_some(instance)
}

/// The physical devices on an instance.
fn physical_devices(instance: VkInstance) -> Vec<VkPhysicalDevice> {
    let mut count = 0_u32;
    // SAFETY: a null array asks for the count only.
    unsafe { vkEnumeratePhysicalDevices(instance, &raw mut count, core::ptr::null_mut()) };
    if count == 0 {
        return Vec::new();
    }
    let mut devices = vec![core::ptr::null_mut(); count as usize];
    // SAFETY: `devices` has room for `count` handles.
    unsafe { vkEnumeratePhysicalDevices(instance, &raw mut count, devices.as_mut_ptr()) };
    devices.truncate(count as usize);
    devices
}

/// The structures this file declares are the only layouts in the crate, so their
/// offsets are checked before anything is passed to a driver. A wrong offset would
/// present as a driver rejecting a valid request, which is a confusing way to find
/// out.
///
/// The numbers come from `scripts/vk-offsets.c`, which declares the same structures
/// and prints what the C compiler computes — the arbiter, since a hand-written
/// layout can disagree both with the headers and with the guest's own copy. Two of
/// these assertions were wrong when first written, in both cases by forgetting that
/// a `u32` followed by a pointer takes four bytes of padding.
#[test]
fn the_declared_layouts_match_the_headers() {
    use core::mem::{offset_of, size_of};

    // `api_version` is at 44, not 40: `engine_version` at 40 is a `u32` and the two
    // pack together. Getting this wrong is exactly the class of arithmetic error
    // these assertions exist to catch, and it caught one being written.
    assert_eq!(size_of::<ApplicationInfo>(), 48);
    assert_eq!(offset_of!(ApplicationInfo, engine_version), 40);
    assert_eq!(offset_of!(ApplicationInfo, api_version), 44);

    // `application_info` is at 24, not 16: `flags` at 16 is a `u32` and the pointer
    // that follows must be 8-aligned, so four bytes of padding sit between them.
    assert_eq!(size_of::<InstanceCreateInfo>(), 64);
    assert_eq!(offset_of!(InstanceCreateInfo, flags), 16);
    assert_eq!(offset_of!(InstanceCreateInfo, application_info), 24);
    assert_eq!(offset_of!(InstanceCreateInfo, enabled_extension_count), 48);
    assert_eq!(offset_of!(InstanceCreateInfo, enabled_extension_names), 56);

    assert_eq!(size_of::<DeviceQueueCreateInfo>(), 40);
    assert_eq!(offset_of!(DeviceQueueCreateInfo, queue_count), 24);
    assert_eq!(offset_of!(DeviceQueueCreateInfo, queue_priorities), 32);

    assert_eq!(size_of::<DeviceCreateInfo>(), 72);
    assert_eq!(offset_of!(DeviceCreateInfo, queue_create_infos), 24);
    assert_eq!(offset_of!(DeviceCreateInfo, enabled_extension_count), 48);
    assert_eq!(offset_of!(DeviceCreateInfo, enabled_features), 64);

    assert_eq!(size_of::<PhysicalDevicePropertiesPrefix>(), 276);
    assert_eq!(offset_of!(PhysicalDevicePropertiesPrefix, device_name), 20);
    assert_eq!(size_of::<QueueFamilyProperties>(), 24);

    assert_eq!(size_of::<Win32SurfaceCreateInfo>(), 40);
    assert_eq!(offset_of!(Win32SurfaceCreateInfo, hinstance), 24);
    assert_eq!(offset_of!(Win32SurfaceCreateInfo, hwnd), 32);
}

/// An instance created through our exports, which is the first call a guest makes
/// that passes a structure rather than a scalar.
#[test]
fn an_instance_can_be_created_and_destroyed() {
    if !host_ready() {
        return;
    }
    let instance = create_instance(&[]).expect("vkCreateInstance should succeed");
    // SAFETY: `instance` came from `vkCreateInstance` and is destroyed once.
    unsafe { vkDestroyInstance(instance, no_allocator()) };
}

/// Proc-address-only commands must report exactly the host's capability while
/// returning our ABI thunk rather than the raw Windows function pointer.  The
/// sample spans promoted KHR aliases and vendor extensions; unsupported samples
/// are valid too, but both sides must agree that they are absent.
#[test]
fn extension_proc_addresses_match_the_host_without_leaking_host_pointers() {
    if !host_ready() {
        return;
    }
    let instance = create_instance(&[]).expect("vkCreateInstance");
    for name in [
        "vkCmdPipelineBarrier2KHR",
        "vkCmdPushDescriptorSetKHR",
        "vkCmdDrawMultiEXT",
        "vkCmdSetCheckpointNV",
        "vkCmdWriteBufferMarker2AMD",
    ] {
        let host = crate::host::instance_symbol(instance, name);
        let c_name = CString::new(name).expect("Vulkan command names contain no NUL");
        let guest = unsafe { crate::dispatch::vkGetInstanceProcAddr(instance, c_name.as_ptr()) };
        assert_eq!(
            guest.is_null(),
            host.is_none(),
            "capability mismatch for {name}"
        );
        if let Some(host) = host {
            assert_ne!(guest as usize, host, "{name} leaked a host ABI pointer");
        }
    }
    unsafe { vkDestroyInstance(instance, no_allocator()) };
}

/// The GPU is real, and its identity survives the crossing.
///
/// This is the test that proves the passthrough is a passthrough. A driver name
/// read back out of a structure the guest-side ABI filled in cannot be produced by
/// accident: if the argument shuffle were wrong, `vkCreateInstance` would have
/// failed long before, and if the structure offsets were wrong the name would be
/// mojibake rather than a device.
#[test]
fn the_real_gpu_is_reachable() {
    if !host_ready() {
        return;
    }
    let instance = create_instance(&[]).expect("vkCreateInstance");
    let devices = physical_devices(instance);
    assert!(
        !devices.is_empty(),
        "no physical devices, so nothing to pass through to"
    );

    for device in &devices {
        let mut buffer = vec![0_u8; PROPERTIES_BUFFER];
        // SAFETY: the buffer is far larger than `VkPhysicalDeviceProperties`, and
        // the entry point writes only that structure.
        unsafe {
            crate::groups::physical_device::vkGetPhysicalDeviceProperties(
                *device,
                buffer.as_mut_ptr().cast(),
            )
        };
        // SAFETY: the driver has written a `VkPhysicalDeviceProperties` here, whose
        // prefix is what is being read.
        let properties = unsafe { &*(buffer.as_ptr() as *const PhysicalDevicePropertiesPrefix) };

        // SAFETY: Vulkan guarantees `deviceName` is NUL-terminated.
        let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) };
        let name = name.to_string_lossy();
        assert!(!name.is_empty(), "a device reported an empty name");
        assert!(
            name.is_ascii(),
            "device name {name:?} is not ASCII, which suggests the structure was read at the wrong offset"
        );
        assert_eq!(properties.api_version >> 22, 1, "implausible API version");
        assert_ne!(properties.vendor_id, 0, "implausible vendor id");

        eprintln!(
            "reached: {name} (vendor {:#06x}, device {:#06x}, type {})",
            properties.vendor_id, properties.device_id, properties.device_type
        );
    }

    // SAFETY: destroyed once, after every use.
    unsafe { vkDestroyInstance(instance, no_allocator()) };
}

/// A logical device, created on the first physical device with a graphics queue.
///
/// Goes further than the properties query in one way that matters: `VkDeviceCreateInfo`
/// contains a *pointer to an array of another structure*, which is the shape most of
/// Vulkan uses and the shape a naive passthrough would break.
#[test]
fn a_logical_device_can_be_created() {
    if !host_ready() {
        return;
    }
    let instance = create_instance(&[]).expect("vkCreateInstance");
    let devices = physical_devices(instance);
    assert!(!devices.is_empty());
    let physical = devices[0];

    // Find a graphics queue family, using the two-call idiom.
    let mut family_count = 0_u32;
    // SAFETY: a null array asks for the count only.
    unsafe {
        crate::groups::physical_device::vkGetPhysicalDeviceQueueFamilyProperties(
            physical,
            &raw mut family_count,
            core::ptr::null_mut(),
        )
    };
    assert!(family_count > 0, "a GPU with no queue families");
    let mut families = vec![QueueFamilyProperties::default(); family_count as usize];
    // SAFETY: `families` has room for `family_count` entries.
    unsafe {
        crate::groups::physical_device::vkGetPhysicalDeviceQueueFamilyProperties(
            physical,
            &raw mut family_count,
            families.as_mut_ptr().cast(),
        )
    };

    let graphics = families
        .iter()
        .position(|family| family.queue_flags & QUEUE_GRAPHICS != 0)
        .expect("a GPU with no graphics queue family");
    assert!(
        families[graphics].queue_count > 0,
        "a graphics family with no queues, which means the structure was misread"
    );

    let priority = 1.0_f32;
    let queue = DeviceQueueCreateInfo {
        s_type: DEVICE_QUEUE_CREATE_INFO,
        next: core::ptr::null(),
        flags: 0,
        queue_family_index: graphics as u32,
        queue_count: 1,
        queue_priorities: &raw const priority,
    };
    let create = DeviceCreateInfo {
        s_type: DEVICE_CREATE_INFO,
        next: core::ptr::null(),
        flags: 0,
        queue_create_info_count: 1,
        queue_create_infos: &raw const queue,
        enabled_layer_count: 0,
        enabled_layer_names: core::ptr::null(),
        enabled_extension_count: 0,
        enabled_extension_names: core::ptr::null(),
        enabled_features: core::ptr::null(),
    };

    let mut device: VkDevice = core::ptr::null_mut();
    // SAFETY: every structure is live for the call and `device` is writable.
    let result = unsafe {
        vkCreateDevice(
            physical,
            (&raw const create).cast(),
            no_allocator(),
            &raw mut device,
        )
    };
    assert_eq!(result, SUCCESS, "vkCreateDevice failed with {result}");
    assert!(!device.is_null());

    // A queue handle, which proves the device is usable and not merely non-null.
    let mut queue_handle = core::ptr::null_mut();
    // SAFETY: the device is live and `queue_handle` is writable.
    unsafe {
        crate::groups::instance::vkGetDeviceQueue(device, graphics as u32, 0, &raw mut queue_handle)
    };
    assert!(!queue_handle.is_null(), "a null queue from a live device");

    // SAFETY: idle first, then destroy once, innermost object first.
    unsafe {
        crate::groups::instance::vkDeviceWaitIdle(device);
        vkDestroyDevice(device, no_allocator());
        vkDestroyInstance(instance, no_allocator());
    }
}

/// A `VkSurfaceKHR` on a real `HWND`, which is the whole point of the exercise.
///
/// This is the seam between the two libraries: `libdisplay` creates a Win32 window
/// on its own UI thread, and the handle goes straight into
/// `VkWin32SurfaceCreateInfoKHR`. A surface coming back non-null means the driver
/// accepted our window — there is no emulation anywhere in that path.
///
/// The window is created here with the same Win32 calls `libdisplay` uses rather
/// than by depending on that crate, because a `cdylib` cannot be linked as a
/// library dependency. What is being tested is the Vulkan side.
#[test]
fn a_surface_can_be_created_on_a_real_window() {
    if !host_ready() {
        return;
    }

    let surface_extensions = [c"VK_KHR_surface".as_ptr(), c"VK_KHR_win32_surface".as_ptr()];
    let Some(instance) = create_instance(&surface_extensions) else {
        // A loader without `VK_KHR_win32_surface` cannot present at all. Reported
        // rather than asserted: it is a property of the machine, not a bug here.
        eprintln!("skipped: the host loader does not support VK_KHR_win32_surface");
        return;
    };

    let Some(window) = window::create_hidden() else {
        eprintln!("skipped: could not create a window on this machine");
        // SAFETY: created above, destroyed once.
        unsafe { vkDestroyInstance(instance, no_allocator()) };
        return;
    };

    let create = Win32SurfaceCreateInfo {
        s_type: WIN32_SURFACE_CREATE_INFO,
        next: core::ptr::null(),
        flags: 0,
        hinstance: window.instance,
        hwnd: window.handle,
    };
    let mut surface = 0_u64;
    // SAFETY: the structure is live, the window is live, and `surface` is writable.
    let result = unsafe {
        crate::groups::wsi::vkCreateWin32SurfaceKHR(
            instance,
            (&raw const create).cast(),
            no_allocator(),
            &raw mut surface,
        )
    };
    assert_eq!(
        result, SUCCESS,
        "vkCreateWin32SurfaceKHR failed with {result}"
    );
    assert_ne!(surface, 0, "a null surface reported as success");
    eprintln!("created a Vulkan surface on HWND {:?}", window.handle);

    // The driver must agree that some queue family can present to it, or the
    // surface is real but useless.
    let devices = physical_devices(instance);
    let mut presentable = false;
    for device in &devices {
        let mut family_count = 0_u32;
        // SAFETY: a null array asks for the count only.
        unsafe {
            crate::groups::physical_device::vkGetPhysicalDeviceQueueFamilyProperties(
                *device,
                &raw mut family_count,
                core::ptr::null_mut(),
            )
        };
        for family in 0..family_count {
            let mut supported = 0_u32;
            // SAFETY: the device and surface are live; `supported` is writable.
            let result = unsafe {
                crate::groups::physical_device::vkGetPhysicalDeviceSurfaceSupportKHR(
                    *device,
                    family,
                    surface,
                    &raw mut supported,
                )
            };
            if result == SUCCESS && supported != 0 {
                presentable = true;
            }
        }
    }
    assert!(
        presentable,
        "no queue family can present to the surface, so the swapchain path is not reachable"
    );

    // SAFETY: surface first, then instance; each destroyed once.
    unsafe {
        crate::groups::wsi::vkDestroySurfaceKHR(instance, surface, no_allocator());
        vkDestroyInstance(instance, no_allocator());
    }
}

/// A minimal hidden window, for the surface test only.
mod window {
    use core::ffi::c_void;

    type Handle = *mut c_void;

    #[repr(C)]
    struct WindowClass {
        size: u32,
        style: u32,
        procedure: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
        class_extra: i32,
        window_extra: i32,
        instance: Handle,
        icon: Handle,
        cursor: Handle,
        background: Handle,
        menu_name: *const u16,
        class_name: *const u16,
        small_icon: Handle,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassExW(class: *const WindowClass) -> u16;
        fn CreateWindowExW(
            extended_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Handle,
            menu: Handle,
            instance: Handle,
            parameter: *mut c_void,
        ) -> Handle;
        fn DestroyWindow(window: Handle) -> i32;
        fn DefWindowProcW(window: Handle, message: u32, w: usize, l: isize) -> isize;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Handle;
    }

    /// A window and the instance it belongs to, destroyed together.
    pub struct Window {
        pub handle: Handle,
        pub instance: Handle,
    }

    impl Drop for Window {
        fn drop(&mut self) {
            // SAFETY: created by `create_hidden` and destroyed once.
            unsafe { DestroyWindow(self.handle) };
        }
    }

    /// Creates a hidden window suitable for a surface.
    ///
    /// Not shown, and never pumped: a surface only needs the `HWND` to exist. A
    /// swapchain would need more, which is why the test stops at the surface.
    pub fn create_hidden() -> Option<Window> {
        let class: Vec<u16> = "kinakaze.vulkan.surface.test"
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();
        // SAFETY: null asks for this module's base address.
        let instance = unsafe { GetModuleHandleW(core::ptr::null()) };

        let descriptor = WindowClass {
            size: core::mem::size_of::<WindowClass>() as u32,
            style: 0x0020, // CS_OWNDC, as a surface-bearing window wants
            procedure: Some(procedure),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: core::ptr::null_mut(),
            cursor: core::ptr::null_mut(),
            background: core::ptr::null_mut(),
            menu_name: core::ptr::null(),
            class_name: class.as_ptr(),
            small_icon: core::ptr::null_mut(),
        };
        // Re-registration fails harmlessly if a previous test already registered it.
        // SAFETY: the descriptor is live and fully populated.
        unsafe { RegisterClassExW(&raw const descriptor) };

        // SAFETY: the class name is live; a zero style leaves the window hidden.
        let handle = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                core::ptr::null(),
                0,
                0,
                0,
                64,
                64,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                instance,
                core::ptr::null_mut(),
            )
        };
        (!handle.is_null()).then_some(Window { handle, instance })
    }

    unsafe extern "system" fn procedure(window: Handle, message: u32, w: usize, l: isize) -> isize {
        // SAFETY: the default handler is valid for every message.
        unsafe { DefWindowProcW(window, message, w, l) }
    }
}
