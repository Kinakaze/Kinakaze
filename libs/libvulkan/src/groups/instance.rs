//! Instance and device lifecycle, and enumeration. 15 entry points.
//!
//! This group is also the crate's worked example: it was written first, and the
//! other six groups follow its shape. If you are adding entry points, read the
//! `passthrough!` documentation in [`crate`] and then copy from here.

use crate::forward::Allocator;
use crate::passthrough;
use crate::types::*;
use core::ffi::c_char;

passthrough! {
    // -----------------------------------------------------------------------
    // Instances
    // -----------------------------------------------------------------------

    fn vkDestroyInstance(instance: VkInstance, pAllocator: Allocator);

    // -----------------------------------------------------------------------
    // Enumeration
    //
    // Each of these uses the two-call idiom: call with a null array to learn the
    // count, then again with storage. The count is `uint32_t*`, never `size_t*` —
    // declaring it eight bytes wide would have the driver overwrite whatever the
    // guest put after a four-byte count.
    // -----------------------------------------------------------------------

    fn vkEnumerateInstanceVersion(pApiVersion: *mut u32) -> VkResult;
    fn vkEnumerateInstanceLayerProperties(
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkEnumeratePhysicalDevices(
        instance: VkInstance,
        pPhysicalDeviceCount: *mut u32,
        pPhysicalDevices: *mut VkPhysicalDevice,
    ) -> VkResult;
    fn vkEnumeratePhysicalDeviceGroups(
        instance: VkInstance,
        pPhysicalDeviceGroupCount: *mut u32,
        pPhysicalDeviceGroupProperties: VkStructMut,
    ) -> VkResult;
    fn vkEnumerateDeviceLayerProperties(
        physicalDevice: VkPhysicalDevice,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkEnumerateDeviceExtensionProperties(
        physicalDevice: VkPhysicalDevice,
        pLayerName: *const c_char,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    // -----------------------------------------------------------------------
    // Devices and queues
    // -----------------------------------------------------------------------

    fn vkDestroyDevice(device: VkDevice, pAllocator: Allocator);
    fn vkDeviceWaitIdle(device: VkDevice) -> VkResult;
    fn vkGetDeviceQueue(
        device: VkDevice,
        queueFamilyIndex: u32,
        queueIndex: u32,
        pQueue: *mut VkQueue,
    );
    fn vkGetDeviceQueue2(device: VkDevice, pQueueInfo: VkStruct, pQueue: *mut VkQueue);
    fn vkGetDeviceGroupPeerMemoryFeatures(
        device: VkDevice,
        heapIndex: u32,
        localDeviceIndex: u32,
        remoteDeviceIndex: u32,
        pPeerMemoryFeatures: *mut VkFlags,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The end-to-end check that a call actually reaches the host driver.
    ///
    /// `vkEnumerateInstanceVersion` is the only entry point that needs no
    /// instance, no device and no structures, so it isolates the one thing being
    /// tested: that a `sysv64` call arrives correctly at an `extern "system"`
    /// function and brings a real answer back.
    #[test]
    fn a_call_reaches_the_real_loader() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let mut version = 0_u32;
        // SAFETY: `version` is a live, writable `u32`, which is what this entry
        // point requires.
        let result = unsafe { vkEnumerateInstanceVersion(&raw mut version) };
        assert_eq!(result, 0, "VK_SUCCESS expected");

        // Decode the packed version. A garbled ABI would show up here as a
        // nonsense major number rather than as a crash, which is exactly the
        // failure worth catching: 1.x is the only thing Vulkan has ever reported.
        let major = version >> 22;
        let minor = (version >> 12) & 0x3ff;
        assert_eq!(major, 1, "major version {major} from packed {version:#x}");
        assert!(minor >= 1, "minor version {minor} looks implausible");
    }

    /// The two-call enumeration idiom, which is the shape most of the API uses.
    ///
    /// Worth testing here rather than only in the integration test: it exercises a
    /// `*mut u32` count and a null array pointer, and a count that came back
    /// implausible would mean the width was wrong.
    #[test]
    fn the_count_query_idiom_works() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let mut count = 0_u32;
        // SAFETY: a null layer name asks for the implicit layer's extensions, and a
        // null property array asks for the count only.
        let result = unsafe {
            crate::vkEnumerateInstanceExtensionProperties(
                core::ptr::null(),
                &raw mut count,
                core::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0, "VK_SUCCESS expected");
        // Any real loader supports at least VK_KHR_surface plus a platform surface.
        assert!(
            count >= 2,
            "only {count} instance extensions, which is implausible"
        );
        assert!(
            count < 1000,
            "{count} instance extensions, which is implausible"
        );
    }

    /// A wrapped name must resolve to our thunk, never the host's address — the
    /// guest would call a host address with the wrong convention.
    #[test]
    fn lookup_returns_our_thunk() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let ours = lookup("vkEnumerateInstanceVersion").expect("declared and present");
        let host = crate::host::symbol("vkEnumerateInstanceVersion").expect("host exports it");
        assert_ne!(
            ours, host,
            "returned the host address rather than our thunk"
        );
    }
}
