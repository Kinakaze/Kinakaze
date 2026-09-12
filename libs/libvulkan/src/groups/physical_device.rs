//! Physical-device capability, format and surface queries. 30 entry points.

use crate::passthrough;
use crate::types::*;

passthrough! {
    // -----------------------------------------------------------------------
    // Core properties and features.
    //
    // All write through a caller-provided structure and cannot fail, so all
    // return void. The `2` variants differ only in that their structure carries
    // a `pNext` chain, which is the driver's business and not this layer's.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceProperties(
        physicalDevice: VkPhysicalDevice,
        pProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceProperties2(
        physicalDevice: VkPhysicalDevice,
        pProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceFeatures(
        physicalDevice: VkPhysicalDevice,
        pFeatures: VkStructMut,
    );
    fn vkGetPhysicalDeviceFeatures2(
        physicalDevice: VkPhysicalDevice,
        pFeatures: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // Format capabilities.
    //
    // The two image-format queries are the only ones here that can fail: a
    // format the driver does not support at all is reported as
    // `VK_ERROR_FORMAT_NOT_SUPPORTED` rather than as an empty structure, so
    // they return VkResult while the plain format queries do not.
    //
    // `r#type` is the spec's `type`, which Rust reserves; the raw identifier
    // keeps the name checkable against the spec by eye.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceFormatProperties(
        physicalDevice: VkPhysicalDevice,
        format: VkFlags,
        pFormatProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceFormatProperties2(
        physicalDevice: VkPhysicalDevice,
        format: VkFlags,
        pFormatProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceImageFormatProperties(
        physicalDevice: VkPhysicalDevice,
        format: VkFlags,
        r#type: VkFlags,
        tiling: VkFlags,
        usage: VkFlags,
        flags: VkFlags,
        pImageFormatProperties: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceImageFormatProperties2(
        physicalDevice: VkPhysicalDevice,
        pImageFormatInfo: VkStruct,
        pImageFormatProperties: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceSparseImageFormatProperties(
        physicalDevice: VkPhysicalDevice,
        format: VkFlags,
        r#type: VkFlags,
        samples: VkFlags,
        usage: VkFlags,
        tiling: VkFlags,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceSparseImageFormatProperties2(
        physicalDevice: VkPhysicalDevice,
        pFormatInfo: VkStruct,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // Memory.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceMemoryProperties(
        physicalDevice: VkPhysicalDevice,
        pMemoryProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceMemoryProperties2(
        physicalDevice: VkPhysicalDevice,
        pMemoryProperties: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // Queue families.
    //
    // Two-call enumeration that nonetheless returns void: the count query has
    // no failure mode because the answer is a property of the device, not of
    // anything the caller supplied.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceQueueFamilyProperties(
        physicalDevice: VkPhysicalDevice,
        pQueueFamilyPropertyCount: *mut u32,
        pQueueFamilyProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceQueueFamilyProperties2(
        physicalDevice: VkPhysicalDevice,
        pQueueFamilyPropertyCount: *mut u32,
        pQueueFamilyProperties: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // External object capabilities.
    //
    // The handle type being asked about travels inside the info structure
    // rather than as its own argument, so nothing here needs a flag parameter.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceExternalBufferProperties(
        physicalDevice: VkPhysicalDevice,
        pExternalBufferInfo: VkStruct,
        pExternalBufferProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceExternalFenceProperties(
        physicalDevice: VkPhysicalDevice,
        pExternalFenceInfo: VkStruct,
        pExternalFenceProperties: VkStructMut,
    );
    fn vkGetPhysicalDeviceExternalSemaphoreProperties(
        physicalDevice: VkPhysicalDevice,
        pExternalSemaphoreInfo: VkStruct,
        pExternalSemaphoreProperties: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // Surface queries.
    //
    // These reach the WSI implementation and can fail for reasons outside the
    // caller's control — a surface lost when its window closes — so they
    // return VkResult. The Win32 presentation check is the exception: it
    // answers a static capability question and returns VkBool32, so a guest
    // that treated it as a VkResult would read success as "not supported".
    //
    // `VkPresentModeKHR` is a 32-bit enum rather than a structure, so the array
    // it fills is `*mut VkFlags`; the format and rectangle arrays are
    // structures.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceSurfaceSupportKHR(
        physicalDevice: VkPhysicalDevice,
        queueFamilyIndex: u32,
        surface: VkHandle,
        pSupported: *mut VkBool32,
    ) -> VkResult;
    fn vkGetPhysicalDeviceSurfaceCapabilitiesKHR(
        physicalDevice: VkPhysicalDevice,
        surface: VkHandle,
        pSurfaceCapabilities: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceSurfaceCapabilities2KHR(
        physicalDevice: VkPhysicalDevice,
        pSurfaceInfo: VkStruct,
        pSurfaceCapabilities: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceSurfaceFormatsKHR(
        physicalDevice: VkPhysicalDevice,
        surface: VkHandle,
        pSurfaceFormatCount: *mut u32,
        pSurfaceFormats: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceSurfaceFormats2KHR(
        physicalDevice: VkPhysicalDevice,
        pSurfaceInfo: VkStruct,
        pSurfaceFormatCount: *mut u32,
        pSurfaceFormats: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceSurfacePresentModesKHR(
        physicalDevice: VkPhysicalDevice,
        surface: VkHandle,
        pPresentModeCount: *mut u32,
        pPresentModes: *mut VkFlags,
    ) -> VkResult;
    fn vkGetPhysicalDevicePresentRectanglesKHR(
        physicalDevice: VkPhysicalDevice,
        surface: VkHandle,
        pRectCount: *mut u32,
        pRects: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceWin32PresentationSupportKHR(
        physicalDevice: VkPhysicalDevice,
        queueFamilyIndex: u32,
    ) -> VkBool32;

    // -----------------------------------------------------------------------
    // Display queries.
    //
    // `VK_KHR_display` enumerates the displays a device drives directly. The
    // host loader exports these whether or not any driver implements them, so
    // they are wrapped for completeness; a guest gets whatever the host reports.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceDisplayPropertiesKHR(
        physicalDevice: VkPhysicalDevice,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceDisplayProperties2KHR(
        physicalDevice: VkPhysicalDevice,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceDisplayPlanePropertiesKHR(
        physicalDevice: VkPhysicalDevice,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkGetPhysicalDeviceDisplayPlaneProperties2KHR(
        physicalDevice: VkPhysicalDevice,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Tools.
    //
    // Reports attached debuggers and profilers. Returns VkResult because the
    // two-call idiom here can answer `VK_INCOMPLETE`.
    // -----------------------------------------------------------------------
    fn vkGetPhysicalDeviceToolProperties(
        physicalDevice: VkPhysicalDevice,
        pToolCount: *mut u32,
        pToolProperties: VkStructMut,
    ) -> VkResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lookup must hand back *our* thunk, never the host's address.
    ///
    /// Returning the host address would look like success at every level a test
    /// can easily observe — the name resolves, the pointer is non-null, and
    /// calling it even works from Windows code — while handing the guest a
    /// function that expects the Windows convention. That is the one bug this
    /// crate exists to prevent, so it is checked directly.
    #[test]
    fn the_lookup_returns_our_thunk_and_not_the_host() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let name = "vkGetPhysicalDeviceProperties";
        let ours = lookup(name).expect("a core 1.0 entry point every loader exports");
        assert_eq!(ours, vkGetPhysicalDeviceProperties as *const () as usize);

        let host = crate::host::symbol(name).expect("the host exports it too");
        assert_ne!(ours, host, "{name} resolved to the host address");
    }

    /// Pins which entry points return nothing and which return a `VkResult`.
    ///
    /// This is a hand-maintained list checked against the source text, which is
    /// the right shape here for a reason that is worth stating: there is nothing
    /// else to check against. The declarations *are* the crate's only claim
    /// about these signatures, so a test derived from them would be a tautology,
    /// and the compiler cannot help either — a `void` host function declared as
    /// returning `VkResult` links and runs, and simply hands the guest whatever
    /// was left in `eax`. The guest then branches on a fabricated error.
    ///
    /// So the assertion has to come from outside the code, and the list below is
    /// that outside: it was read off the Vulkan specification. Editing a
    /// signature without editing the list fails, which forces a second look at
    /// exactly the mistake most easily missed in this group.
    #[test]
    fn the_void_and_result_split_matches_the_specification() {
        /// Queries that write through a pointer and cannot fail.
        const VOID: &[&str] = &[
            "vkGetPhysicalDeviceProperties",
            "vkGetPhysicalDeviceProperties2",
            "vkGetPhysicalDeviceFeatures",
            "vkGetPhysicalDeviceFeatures2",
            "vkGetPhysicalDeviceFormatProperties",
            "vkGetPhysicalDeviceFormatProperties2",
            "vkGetPhysicalDeviceSparseImageFormatProperties",
            "vkGetPhysicalDeviceSparseImageFormatProperties2",
            "vkGetPhysicalDeviceMemoryProperties",
            "vkGetPhysicalDeviceMemoryProperties2",
            "vkGetPhysicalDeviceQueueFamilyProperties",
            "vkGetPhysicalDeviceQueueFamilyProperties2",
            "vkGetPhysicalDeviceExternalBufferProperties",
            "vkGetPhysicalDeviceExternalFenceProperties",
            "vkGetPhysicalDeviceExternalSemaphoreProperties",
        ];
        /// Queries that can fail, or that can report `VK_INCOMPLETE`.
        const RESULT: &[&str] = &[
            "vkGetPhysicalDeviceImageFormatProperties",
            "vkGetPhysicalDeviceImageFormatProperties2",
            "vkGetPhysicalDeviceSurfaceSupportKHR",
            "vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
            "vkGetPhysicalDeviceSurfaceCapabilities2KHR",
            "vkGetPhysicalDeviceSurfaceFormatsKHR",
            "vkGetPhysicalDeviceSurfaceFormats2KHR",
            "vkGetPhysicalDeviceSurfacePresentModesKHR",
            "vkGetPhysicalDevicePresentRectanglesKHR",
            "vkGetPhysicalDeviceDisplayPropertiesKHR",
            "vkGetPhysicalDeviceDisplayProperties2KHR",
            "vkGetPhysicalDeviceDisplayPlanePropertiesKHR",
            "vkGetPhysicalDeviceDisplayPlaneProperties2KHR",
            "vkGetPhysicalDeviceToolProperties",
        ];
        /// The lone predicate in the group. Not a `VkResult`: success is `true`.
        const BOOL: &[&str] = &["vkGetPhysicalDeviceWin32PresentationSupportKHR"];

        // Only the declarations are examined. The test's own text repeats every
        // name in this file, so including it would match everything.
        let source = include_str!("physical_device.rs");
        let declarations = source
            .split_once("#[cfg(test)]")
            .expect("this test module marks the end of the declarations")
            .0;

        // Each declaration runs from its `fn` to the `;` that ends it, so the
        // return type is whatever sits between the closing paren and that `;`.
        let returns = |name: &str| -> String {
            let start = declarations
                .find(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("{name} is not declared"));
            let body = &declarations[start..];
            let end = body.find(';').expect("a declaration ends in a semicolon");
            let close = body[..end]
                .rfind(')')
                .expect("a declaration closes its argument list");
            body[close + 1..end].trim().to_owned()
        };

        for name in VOID {
            assert_eq!(
                returns(name),
                "",
                "{name} returns void in the specification but is declared with a return type"
            );
        }
        for name in RESULT {
            assert_eq!(
                returns(name),
                "-> VkResult",
                "{name} returns VkResult in the specification"
            );
        }
        for name in BOOL {
            assert_eq!(
                returns(name),
                "-> VkBool32",
                "{name} returns VkBool32, which is not a VkResult"
            );
        }

        // The three lists together must account for the whole group, or a new
        // declaration could be added and never cross-checked at all.
        assert_eq!(VOID.len() + RESULT.len() + BOOL.len(), NAMES.len());
        for name in NAMES {
            assert!(
                VOID.contains(name) || RESULT.contains(name) || BOOL.contains(name),
                "{name} is declared but this cross-check does not classify it"
            );
        }
    }
}
