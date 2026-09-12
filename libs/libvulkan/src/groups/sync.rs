//! Synchronisation, queries, command pools, private data and queue submission.
//! 30 entry points.
//!
//! What ties this group together is that its objects are all *cheap* handles the
//! guest creates and destroys freely, which is why twelve of the thirty take an
//! allocator: a program that pools its Vulkan memory pools these objects most of
//! all. Every one of those twelve is declared with [`Allocator`], and the test at
//! the bottom of this file is what keeps that true as the group changes.

use crate::forward::Allocator;
use crate::passthrough;
use crate::types::*;

passthrough! {
    // ---------------------------------------------------------------------
    // Fences: device-to-host signalling.
    //
    // `timeout` is the one place in this group where getting a width wrong
    // would not look like an ABI bug. `UINT64_MAX` is Vulkan's documented
    // "wait forever"; truncated to 32 bits it becomes roughly four seconds, so
    // the symptom is an intermittent timeout rather than a crash.
    // ---------------------------------------------------------------------
    fn vkCreateFence(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pFence: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyFence(device: VkDevice, fence: VkHandle, pAllocator: Allocator);
    fn vkResetFences(device: VkDevice, fenceCount: u32, pFences: VkStruct) -> VkResult;
    fn vkGetFenceStatus(device: VkDevice, fence: VkHandle) -> VkResult;
    fn vkWaitForFences(
        device: VkDevice,
        fenceCount: u32,
        pFences: VkStruct,
        waitAll: VkBool32,
        timeout: u64,
    ) -> VkResult;

    // ---------------------------------------------------------------------
    // Semaphores, including the timeline operations from Vulkan 1.2.
    // ---------------------------------------------------------------------
    fn vkCreateSemaphore(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSemaphore: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroySemaphore(device: VkDevice, semaphore: VkHandle, pAllocator: Allocator);
    fn vkGetSemaphoreCounterValue(
        device: VkDevice,
        semaphore: VkHandle,
        pValue: *mut u64,
    ) -> VkResult;
    fn vkWaitSemaphores(device: VkDevice, pWaitInfo: VkStruct, timeout: u64) -> VkResult;
    fn vkSignalSemaphore(device: VkDevice, pSignalInfo: VkStruct) -> VkResult;

    // ---------------------------------------------------------------------
    // Events: host-to-device signalling.
    //
    // `vkGetEventStatus` reports through `VkResult` rather than `VkBool32`,
    // because `VK_EVENT_SET` and `VK_EVENT_RESET` are result codes and the call
    // can also fail outright. Declaring it as a boolean would be the same width
    // and would quietly read only the low half of the answer.
    // ---------------------------------------------------------------------
    fn vkCreateEvent(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pEvent: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyEvent(device: VkDevice, event: VkHandle, pAllocator: Allocator);
    fn vkGetEventStatus(device: VkDevice, event: VkHandle) -> VkResult;
    fn vkSetEvent(device: VkDevice, event: VkHandle) -> VkResult;
    fn vkResetEvent(device: VkDevice, event: VkHandle) -> VkResult;

    // ---------------------------------------------------------------------
    // Query pools.
    //
    // `vkGetQueryPoolResults` mixes two pointer-width-adjacent types that are
    // not interchangeable: `dataSize` is `size_t` and `stride` is
    // `VkDeviceSize`. They are both 64 bits here, but naming them accurately is
    // what makes the signature checkable against the specification by eye.
    // ---------------------------------------------------------------------
    fn vkCreateQueryPool(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pQueryPool: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyQueryPool(device: VkDevice, queryPool: VkHandle, pAllocator: Allocator);
    fn vkGetQueryPoolResults(
        device: VkDevice,
        queryPool: VkHandle,
        firstQuery: u32,
        queryCount: u32,
        dataSize: usize,
        pData: VkStructMut,
        stride: VkDeviceSize,
        flags: VkFlags,
    ) -> VkResult;
    fn vkResetQueryPool(
        device: VkDevice,
        queryPool: VkHandle,
        firstQuery: u32,
        queryCount: u32,
    );

    // ---------------------------------------------------------------------
    // Command pools. Recording itself is the `command` group; this is only the
    // pool lifecycle.
    // ---------------------------------------------------------------------
    fn vkCreateCommandPool(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pCommandPool: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyCommandPool(device: VkDevice, commandPool: VkHandle, pAllocator: Allocator);
    fn vkResetCommandPool(
        device: VkDevice,
        commandPool: VkHandle,
        flags: VkFlags,
    ) -> VkResult;
    fn vkTrimCommandPool(device: VkDevice, commandPool: VkHandle, flags: VkFlags);

    // ---------------------------------------------------------------------
    // Private data.
    //
    // Two distinct 64-bit values travel here and swapping them would be
    // invisible to the compiler: `objectHandle` is the handle of whatever
    // object the data is attached to, while `privateDataSlot` is the slot
    // itself. The `VkObjectType` before them is what tells the driver how to
    // interpret `objectHandle`.
    // ---------------------------------------------------------------------
    fn vkCreatePrivateDataSlot(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pPrivateDataSlot: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyPrivateDataSlot(
        device: VkDevice,
        privateDataSlot: VkHandle,
        pAllocator: Allocator,
    );
    fn vkSetPrivateData(
        device: VkDevice,
        objectType: VkFlags,
        objectHandle: u64,
        privateDataSlot: VkHandle,
        data: u64,
    ) -> VkResult;
    fn vkGetPrivateData(
        device: VkDevice,
        objectType: VkFlags,
        objectHandle: u64,
        privateDataSlot: VkHandle,
        pData: *mut u64,
    );

    // ---------------------------------------------------------------------
    // Queue submission.
    //
    // `vkQueueSubmit2` is not a wider `vkQueueSubmit`: the two take the same
    // number of arguments but different structure types, so only the array
    // contents differ and the ABI is identical. Both are declared because the
    // loader exports both and a guest built against 1.3 may use either.
    // ---------------------------------------------------------------------
    fn vkQueueSubmit(
        queue: VkQueue,
        submitCount: u32,
        pSubmits: VkStruct,
        fence: VkHandle,
    ) -> VkResult;
    fn vkQueueSubmit2(
        queue: VkQueue,
        submitCount: u32,
        pSubmits: VkStruct,
        fence: VkHandle,
    ) -> VkResult;
    fn vkQueueWaitIdle(queue: VkQueue) -> VkResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The group must hand back *our* thunk, never the host's address.
    ///
    /// Returning the host address would pass every other test in the crate and
    /// then fail in the guest: the address is real, the entry point works when
    /// called from Windows code, and the only thing wrong is that the guest will
    /// call it with the System V convention. So the assertion is specifically
    /// that the two addresses differ.
    #[test]
    fn lookup_returns_our_thunk_and_not_the_host() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let ours = lookup("vkQueueWaitIdle").expect("the loader has exported this since 1.0");
        let host = crate::host::symbol("vkQueueWaitIdle").expect("present by the same token");
        assert_ne!(
            ours, host,
            "lookup handed back the host address; the guest would call it with \
             the wrong calling convention"
        );
        assert_eq!(ours, vkQueueWaitIdle as *const () as usize);
    }

    /// Cross-checks every allocator declaration against the specification.
    ///
    /// Shaped as a source parse because there is nothing to inspect at runtime.
    /// [`Allocator`] is `#[repr(transparent)]` over a pointer, which is the
    /// point of it — the guest's call must not change — so a parameter written
    /// as `VkStruct` instead compiles, links, forwards, and returns
    /// `VK_SUCCESS`. The damage appears only later and elsewhere: the driver
    /// calls the guest's allocator with the Windows convention, and a pointer
    /// derived from the wrong registers comes back from inside an allocation
    /// path. No call this crate can make will reveal that, so the declaration
    /// itself is what gets checked.
    ///
    /// The source is cut at the test module so this function's own mention of
    /// `Allocator` cannot satisfy the check it is performing.
    #[test]
    fn every_allocator_parameter_is_declared_as_allocator() {
        // The twelve entry points the specification gives a
        // `const VkAllocationCallbacks* pAllocator`: each of the six object
        // kinds this group owns, at create and at destroy.
        const TAKES_AN_ALLOCATOR: &[&str] = &[
            "vkCreateFence",
            "vkDestroyFence",
            "vkCreateSemaphore",
            "vkDestroySemaphore",
            "vkCreateEvent",
            "vkDestroyEvent",
            "vkCreateQueryPool",
            "vkDestroyQueryPool",
            "vkCreateCommandPool",
            "vkDestroyCommandPool",
            "vkCreatePrivateDataSlot",
            "vkDestroyPrivateDataSlot",
        ];

        let source = include_str!("sync.rs");
        let (declarations, _) = source
            .split_once("#[cfg(test)]")
            .expect("this test module is in this file");

        let mut with_allocator: Vec<&str> = Vec::new();
        let mut declared = 0_usize;
        let mut rest = declarations;
        while let Some(position) = rest.find("fn ") {
            rest = &rest[position + "fn ".len()..];
            let Some(open) = rest.find('(') else { break };
            let name = rest[..open].trim();
            // Anything that is not a Vulkan entry point is prose or helper
            // code, and advancing past the "fn " is enough to make progress.
            if !name.starts_with("vk") || name.contains(char::is_whitespace) {
                continue;
            }
            let Some(close) = rest.find(')') else { break };
            declared += 1;
            if rest[open + 1..close].contains("Allocator") {
                with_allocator.push(name);
            }
            rest = &rest[close..];
        }

        assert_eq!(declared, NAMES.len(), "the parse missed a declaration");
        with_allocator.sort_unstable();
        let mut expected = TAKES_AN_ALLOCATOR.to_vec();
        expected.sort_unstable();
        assert_eq!(
            with_allocator, expected,
            "an allocator parameter is declared as a plain pointer, or one is \
             declared as an allocator that the specification does not give one"
        );
    }

    /// The two dispatch roots must not be answerable here.
    ///
    /// `dispatch.rs` owns them because they return function pointers, and it
    /// answers from the whole crate's table. A second answer from this group
    /// would be reached first or last depending on the order in `groups.rs`,
    /// which is not a thing that should decide anything.
    #[test]
    fn the_dispatch_roots_belong_to_dispatch() {
        assert!(lookup("vkGetInstanceProcAddr").is_none());
        assert!(lookup("vkGetDeviceProcAddr").is_none());
    }

    /// The group is exactly the set `scripts/check-vulkan-groups.py` assigns it.
    #[test]
    fn the_group_is_thirty_entry_points() {
        assert_eq!(NAMES.len(), 30);
    }
}
