//! Window-system integration: surfaces, swapchains, presentation. 19 entry points.
//!
//! This is the presentation path, and it is the group that makes the graphics
//! story native rather than emulated. The guest asks `libdisplay.dll` for a real
//! `HWND`, passes it to `vkCreateWin32SurfaceKHR`, and the driver builds a
//! swapchain on the same window a native Windows program would get. There is no
//! X server, no compositor and no intermediate blit anywhere in that path.
//!
//! ## Why there is no `vkCreateXlibSurfaceKHR`
//!
//! A reader arriving from Linux will look for the Xlib entry point first, so:
//! it is not here, and neither is `vkCreateWaylandSurfaceKHR`. The host loader
//! does not export either one — that is measured, not assumed, by
//! `the_host_offers_win32_surfaces_and_not_xlib` below — and nothing in this
//! crate emulates them. It could not usefully: `VkXlibSurfaceCreateInfoKHR`
//! carries a `Display *` and an X `Window`, and satisfying it would mean writing
//! a display server whose windows would still need an `HWND` underneath. The
//! `Display *` would be a fiction wrapped around the handle we already have.
//!
//! So a guest must ask its instance for `VK_KHR_win32_surface` and call
//! `vkCreateWin32SurfaceKHR`. Extension enumeration reports what the host really
//! supports, so a guest that checks before creating a surface sees the truth
//! rather than discovering it at surface-creation time.

use crate::forward::Allocator;
use crate::passthrough;
use crate::types::*;

passthrough! {
    // -----------------------------------------------------------------------
    // Surfaces.
    //
    // A `VkSurfaceKHR` is instance-level, not device-level: it describes a
    // window, and which device will present to it is decided afterwards. That
    // is why these take a `VkInstance` while the swapchain calls take a
    // `VkDevice`.
    // -----------------------------------------------------------------------

    /// The seam between `libdisplay.dll` and the driver. `pCreateInfo` is a
    /// `VkWin32SurfaceCreateInfoKHR` holding the `HINSTANCE` and `HWND` from
    /// `kinakaze_window_native_instance` and `kinakaze_window_native_handle`.
    fn vkCreateWin32SurfaceKHR(
        instance: VkInstance,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSurface: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroySurfaceKHR(instance: VkInstance, surface: VkHandle, pAllocator: Allocator);
    fn vkCreateDisplayPlaneSurfaceKHR(
        instance: VkInstance,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSurface: *mut VkHandle,
    ) -> VkResult;
    fn vkCreateHeadlessSurfaceEXT(
        instance: VkInstance,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSurface: *mut VkHandle,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Swapchains.
    // -----------------------------------------------------------------------

    fn vkCreateSwapchainKHR(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSwapchain: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroySwapchainKHR(device: VkDevice, swapchain: VkHandle, pAllocator: Allocator);
    /// The two-call enumeration idiom: `pSwapchainImages` null to learn the
    /// count, then non-null to fill it. The images are non-dispatchable handles
    /// owned by the swapchain, so the array is `*mut VkHandle`.
    fn vkGetSwapchainImagesKHR(
        device: VkDevice,
        swapchain: VkHandle,
        pSwapchainImageCount: *mut u32,
        pSwapchainImages: *mut VkHandle,
    ) -> VkResult;
    /// Creates `swapchainCount` swapchains at once, from parallel arrays: one
    /// `VkSwapchainCreateInfoKHR` and one output handle per element.
    fn vkCreateSharedSwapchainsKHR(
        device: VkDevice,
        swapchainCount: u32,
        pCreateInfos: VkStruct,
        pAllocator: Allocator,
        pSwapchains: *mut VkHandle,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Presentation.
    //
    // These are the two entry points whose *positive* return values matter as
    // much as their errors: `VK_SUBOPTIMAL_KHR`, `VK_TIMEOUT` and
    // `VK_NOT_READY` are all successes that a render loop must act on.
    // `VkResult` is `i32` and carries both signs, so nothing is lost here.
    // -----------------------------------------------------------------------

    /// `timeout` is `uint64_t`, and it must stay 64 bits: `UINT64_MAX` means
    /// "wait forever", and truncating it to 32 would turn an infinite wait into
    /// roughly four seconds — an intermittent timeout that looks like a driver
    /// bug rather than like an ABI mistake.
    fn vkAcquireNextImageKHR(
        device: VkDevice,
        swapchain: VkHandle,
        timeout: u64,
        semaphore: VkHandle,
        fence: VkHandle,
        pImageIndex: *mut u32,
    ) -> VkResult;
    /// The device-group form, which folds the five arguments above into a
    /// `VkAcquireNextImageInfoKHR`. Same operation, quite different signature.
    fn vkAcquireNextImage2KHR(
        device: VkDevice,
        pAcquireInfo: VkStruct,
        pImageIndex: *mut u32,
    ) -> VkResult;
    fn vkQueuePresentKHR(queue: VkQueue, pPresentInfo: VkStruct) -> VkResult;
    fn vkGetDeviceGroupPresentCapabilitiesKHR(
        device: VkDevice,
        pDeviceGroupPresentCapabilities: VkStructMut,
    ) -> VkResult;
    fn vkGetDeviceGroupSurfacePresentModesKHR(
        device: VkDevice,
        surface: VkHandle,
        pModes: *mut VkFlags,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Display control.
    //
    // Direct-to-display: presenting to a monitor with no window system in the
    // way. Every one of these takes a `VkPhysicalDevice` rather than a
    // `VkDevice`, because a display is enumerated from the adapter before any
    // logical device exists.
    // -----------------------------------------------------------------------

    fn vkGetDisplayModePropertiesKHR(
        physicalDevice: VkPhysicalDevice,
        display: VkHandle,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkGetDisplayModeProperties2KHR(
        physicalDevice: VkPhysicalDevice,
        display: VkHandle,
        pPropertyCount: *mut u32,
        pProperties: VkStructMut,
    ) -> VkResult;
    fn vkCreateDisplayModeKHR(
        physicalDevice: VkPhysicalDevice,
        display: VkHandle,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pMode: *mut VkHandle,
    ) -> VkResult;
    fn vkGetDisplayPlaneCapabilitiesKHR(
        physicalDevice: VkPhysicalDevice,
        mode: VkHandle,
        planeIndex: u32,
        pCapabilities: VkStructMut,
    ) -> VkResult;
    /// The `2` form takes the mode and plane index inside a
    /// `VkDisplayPlaneInfo2KHR`, so it has three arguments where the original
    /// has four.
    fn vkGetDisplayPlaneCapabilities2KHR(
        physicalDevice: VkPhysicalDevice,
        pDisplayPlaneInfo: VkStruct,
        pCapabilities: VkStructMut,
    ) -> VkResult;
    fn vkGetDisplayPlaneSupportedDisplaysKHR(
        physicalDevice: VkPhysicalDevice,
        planeIndex: u32,
        pDisplayCount: *mut u32,
        pDisplays: *mut VkHandle,
    ) -> VkResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// `vkGetInstanceProcAddr` must hand back *our* thunk for the entry point
    /// the whole presentation path goes through.
    ///
    /// The host address is compared against explicitly, because returning it is
    /// the one bug that would look like success here: the pointer is non-null
    /// and it names the right function, but the guest would call a Windows x64
    /// function with the System V convention.
    #[test]
    fn win32_surface_creation_resolves_to_our_thunk() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let ours = lookup("vkCreateWin32SurfaceKHR").expect("declared and host-supported");
        let host = crate::host::symbol("vkCreateWin32SurfaceKHR").expect("host exports it");
        assert_ne!(
            ours, host,
            "returned the host address, which the guest would call with the wrong ABI"
        );
        assert_eq!(
            ours, vkCreateWin32SurfaceKHR as *const () as usize,
            "must be this crate's own export"
        );
    }

    /// The concrete reason a guest must ask for `VK_KHR_win32_surface`.
    ///
    /// The module documentation claims the host loader offers Win32 surfaces and
    /// not Xlib ones. That claim is a measurable fact about the installed
    /// loader, so it is measured here rather than asserted in prose: if a future
    /// loader ever did export `vkCreateXlibSurfaceKHR`, this test would fail and
    /// the documentation would need revisiting instead of quietly becoming
    /// wrong.
    #[test]
    fn the_host_offers_win32_surfaces_and_not_xlib() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        assert!(
            crate::host::symbol("vkCreateWin32SurfaceKHR").is_some(),
            "the presentation path depends on this entry point existing"
        );
        for absent in [
            "vkCreateXlibSurfaceKHR",
            "vkCreateXcbSurfaceKHR",
            "vkCreateWaylandSurfaceKHR",
        ] {
            assert!(
                crate::host::symbol(absent).is_none(),
                "{absent} is exported after all; the group's documentation is out of date"
            );
        }
    }

    /// Which of this group's entry points the Vulkan specification gives a
    /// `const VkAllocationCallbacks *`.
    ///
    /// Kept as a written list because the mistake being guarded is a *missing*
    /// `Allocator`, and a list derived from the source could not detect its own
    /// omission.
    const TAKE_AN_ALLOCATOR: &[&str] = &[
        "vkCreateDisplayModeKHR",
        "vkCreateDisplayPlaneSurfaceKHR",
        "vkCreateHeadlessSurfaceEXT",
        "vkCreateSharedSwapchainsKHR",
        "vkCreateSwapchainKHR",
        "vkCreateWin32SurfaceKHR",
        "vkDestroySurfaceKHR",
        "vkDestroySwapchainKHR",
    ];

    /// Each declared entry point, and whether its parameters mention
    /// `Allocator`.
    ///
    /// Read out of this file's own text because the property is not observable
    /// at runtime: `Allocator` and `VkStruct` are both pointer-shaped, so an
    /// allocator declared as `VkStruct` compiles, links, and passes every
    /// behavioural test — right up until a driver calls the guest's allocator
    /// with the wrong convention. The source is the only place the distinction
    /// still exists.
    ///
    /// The text is split at the test module's own attribute so that the lists
    /// and assertions below, which name these functions too, cannot be mistaken
    /// for declarations.
    fn declarations() -> Vec<(String, bool)> {
        let source = include_str!("wsi.rs");
        let (declared, _) = source
            .split_once("#[cfg(test)]")
            .expect("this file has a test module");
        // Comment lines are dropped first: the section headers and doc comments
        // above name these entry points in prose, and a bare `fn` inside one
        // would otherwise be read as a declaration.
        let code = declared
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .collect::<Vec<_>>()
            .join(" ");

        code.split("fn ")
            .skip(1)
            .map(|chunk| {
                let (name, rest) = chunk.split_once('(').expect("a declaration has parameters");
                let (parameters, _) = rest.split_once(')').expect("a parameter list closes");
                // The *type* is what decides translation, and it has to be read
                // as a type: the conventional parameter name is `pAllocator`, so
                // a substring search over the whole declaration matches the name
                // and would call a wrongly-typed parameter correct.
                let translated = parameters.split(',').any(|parameter| {
                    parameter
                        .split_once(':')
                        .is_some_and(|(_, kind)| kind.trim() == "Allocator")
                });
                (name.trim().to_owned(), translated)
            })
            .collect()
    }

    /// The declarations must agree with the specification about which entry
    /// points take an allocator — in both directions.
    #[test]
    fn every_allocator_is_declared_as_an_allocator() {
        let declared: BTreeSet<String> = declarations()
            .into_iter()
            .filter_map(|(name, translated)| translated.then_some(name))
            .collect();
        let expected: BTreeSet<String> = TAKE_AN_ALLOCATOR
            .iter()
            .map(|name| (*name).to_owned())
            .collect();

        let missing: Vec<&String> = expected.difference(&declared).collect();
        assert!(
            missing.is_empty(),
            "these take a pAllocator but do not declare it as `Allocator`, so the \
             driver would call the guest's allocator with the wrong convention: {missing:?}"
        );
        let extra: Vec<&String> = declared.difference(&expected).collect();
        assert!(
            extra.is_empty(),
            "these declare an `Allocator` the specification does not give them, so \
             some other argument is being translated: {extra:?}"
        );
    }

    /// The parse underlying the test above has to see every entry point, or an
    /// undeclared allocator could hide in a declaration it silently skipped.
    #[test]
    fn the_source_parse_sees_the_whole_group() {
        let parsed: BTreeSet<String> = declarations().into_iter().map(|(name, _)| name).collect();
        let declared: BTreeSet<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(
            parsed, declared,
            "the allocator cross-check reads a different set of names than the group declares"
        );
        assert_eq!(NAMES.len(), 19, "this group owns 19 entry points");
    }
}
