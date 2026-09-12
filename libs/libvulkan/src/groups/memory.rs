//! Memory, buffers, images, and the queries that size them. 46 entry points.
//!
//! This is the group a render loop reaches immediately after it has a device:
//! allocate memory, create a buffer or an image, bind one to the other, and take
//! command buffers out of a pool. Nothing here is emulated — every call lands in
//! the driver and the allocations are real GPU allocations.
//!
//! ## Two things in this group are easy to get wrong and hard to notice
//!
//! **Allocators.** Ten of these entry points take a
//! `const VkAllocationCallbacks *`, which is the one argument in the whole crate
//! that cannot cross unchanged: the driver calls *back* through it, so it must be
//! declared [`Allocator`] and not `VkStruct`. Both are pointers, so the wrong
//! choice compiles and links and only shows up as a corrupt pointer inside a
//! driver allocation path. `every_allocator_is_declared_as_an_allocator` below
//! checks the declarations against the specification instead of trusting the eye.
//!
//! **Two kinds of handle array.** Almost every handle here is non-dispatchable
//! and so is written through `*mut VkHandle`. Command buffers are the exception:
//! they are *dispatchable*, so `vkAllocateCommandBuffers` writes
//! `*mut VkCommandBuffer` and `vkFreeCommandBuffers` reads
//! `*const VkCommandBuffer`. Both spellings are eight bytes wide, which is why
//! that distinction is pinned by a test rather than left to the compiler.

use core::ffi::c_void;

use crate::forward::Allocator;
use crate::passthrough;
use crate::types::*;

passthrough! {
    // -----------------------------------------------------------------------
    // Device memory.
    // -----------------------------------------------------------------------

    fn vkAllocateMemory(
        device: VkDevice,
        pAllocateInfo: VkStruct,
        pAllocator: Allocator,
        pMemory: *mut VkHandle,
    ) -> VkResult;
    fn vkFreeMemory(device: VkDevice, memory: VkHandle, pAllocator: Allocator);
    /// `pCommittedMemoryInBytes` is a `VkDeviceSize *`, so eight bytes and not
    /// four: this reports how much of a lazily-allocated heap is really resident.
    fn vkGetDeviceMemoryCommitment(
        device: VkDevice,
        memory: VkHandle,
        pCommittedMemoryInBytes: *mut VkDeviceSize,
    );
    fn vkGetDeviceMemoryOpaqueCaptureAddress(device: VkDevice, pInfo: VkStruct) -> u64;

    // -----------------------------------------------------------------------
    // Mapping.
    //
    // The `2` forms are not spelling variants: they fold the handle, offset,
    // size and flags into one info structure, so they take three arguments and
    // two where the originals take six and two. `vkUnmapMemory2` also returns a
    // `VkResult` where `vkUnmapMemory` returns nothing, because unmapping can
    // fail once a memory-placement extension is in play.
    // -----------------------------------------------------------------------

    /// `ppData` is a `void **`: the driver writes the mapped address into guest
    /// memory rather than returning it, so this is a pointer to a pointer.
    fn vkMapMemory(
        device: VkDevice,
        memory: VkHandle,
        offset: VkDeviceSize,
        size: VkDeviceSize,
        flags: VkFlags,
        ppData: *mut *mut c_void,
    ) -> VkResult;
    fn vkUnmapMemory(device: VkDevice, memory: VkHandle);
    fn vkMapMemory2(
        device: VkDevice,
        pMemoryMapInfo: VkStruct,
        ppData: *mut *mut c_void,
    ) -> VkResult;
    fn vkUnmapMemory2(device: VkDevice, pMemoryUnmapInfo: VkStruct) -> VkResult;
    fn vkFlushMappedMemoryRanges(
        device: VkDevice,
        memoryRangeCount: u32,
        pMemoryRanges: VkStruct,
    ) -> VkResult;
    fn vkInvalidateMappedMemoryRanges(
        device: VkDevice,
        memoryRangeCount: u32,
        pMemoryRanges: VkStruct,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Buffers.
    //
    // `vkBindBufferMemory2` binds a batch from an array of
    // `VkBindBufferMemoryInfo`, so the buffer, memory and offset that the
    // original takes as three separate arguments are inside the structures.
    // -----------------------------------------------------------------------

    fn vkCreateBuffer(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pBuffer: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyBuffer(device: VkDevice, buffer: VkHandle, pAllocator: Allocator);
    fn vkBindBufferMemory(
        device: VkDevice,
        buffer: VkHandle,
        memory: VkHandle,
        memoryOffset: VkDeviceSize,
    ) -> VkResult;
    fn vkBindBufferMemory2(
        device: VkDevice,
        bindInfoCount: u32,
        pBindInfos: VkStruct,
    ) -> VkResult;
    /// Returns a `VkDeviceAddress`, which is a GPU-side address rather than a
    /// status: there is no `VkResult` to check and a zero return is the failure.
    fn vkGetBufferDeviceAddress(device: VkDevice, pInfo: VkStruct) -> VkDeviceSize;
    fn vkGetBufferOpaqueCaptureAddress(device: VkDevice, pInfo: VkStruct) -> VkDeviceSize;

    // -----------------------------------------------------------------------
    // Buffer views.
    // -----------------------------------------------------------------------

    fn vkCreateBufferView(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pView: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyBufferView(device: VkDevice, bufferView: VkHandle, pAllocator: Allocator);

    // -----------------------------------------------------------------------
    // Images.
    //
    // `vkGetImageSubresourceLayout2` keeps all four arguments of the original
    // and only widens the two structures, which makes it the exception among
    // the `2` forms here.
    // -----------------------------------------------------------------------

    fn vkCreateImage(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pImage: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyImage(device: VkDevice, image: VkHandle, pAllocator: Allocator);
    fn vkBindImageMemory(
        device: VkDevice,
        image: VkHandle,
        memory: VkHandle,
        memoryOffset: VkDeviceSize,
    ) -> VkResult;
    fn vkBindImageMemory2(
        device: VkDevice,
        bindInfoCount: u32,
        pBindInfos: VkStruct,
    ) -> VkResult;
    fn vkGetImageSubresourceLayout(
        device: VkDevice,
        image: VkHandle,
        pSubresource: VkStruct,
        pLayout: VkStructMut,
    );
    fn vkGetImageSubresourceLayout2(
        device: VkDevice,
        image: VkHandle,
        pSubresource: VkStruct,
        pLayout: VkStructMut,
    );
    /// Answers the layout question without an image existing yet: the would-be
    /// `VkImageCreateInfo` travels inside `pInfo`, so there is no image handle.
    fn vkGetDeviceImageSubresourceLayout(
        device: VkDevice,
        pInfo: VkStruct,
        pLayout: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // Image views.
    // -----------------------------------------------------------------------

    fn vkCreateImageView(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pView: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyImageView(device: VkDevice, imageView: VkHandle, pAllocator: Allocator);

    // -----------------------------------------------------------------------
    // Memory requirements.
    //
    // None of these can fail, so all six return void. Declaring one as
    // returning `VkResult` would have the guest branch on whatever the driver
    // happened to leave in `eax`.
    //
    // The `vkGetDevice*` pair answers the same question from a creation
    // structure instead of a live handle, which is how a guest sizes an
    // allocation before it has anything to bind.
    // -----------------------------------------------------------------------

    fn vkGetBufferMemoryRequirements(
        device: VkDevice,
        buffer: VkHandle,
        pMemoryRequirements: VkStructMut,
    );
    fn vkGetBufferMemoryRequirements2(
        device: VkDevice,
        pInfo: VkStruct,
        pMemoryRequirements: VkStructMut,
    );
    fn vkGetImageMemoryRequirements(
        device: VkDevice,
        image: VkHandle,
        pMemoryRequirements: VkStructMut,
    );
    fn vkGetImageMemoryRequirements2(
        device: VkDevice,
        pInfo: VkStruct,
        pMemoryRequirements: VkStructMut,
    );
    fn vkGetDeviceBufferMemoryRequirements(
        device: VkDevice,
        pInfo: VkStruct,
        pMemoryRequirements: VkStructMut,
    );
    fn vkGetDeviceImageMemoryRequirements(
        device: VkDevice,
        pInfo: VkStruct,
        pMemoryRequirements: VkStructMut,
    );

    // -----------------------------------------------------------------------
    // Sparse resources.
    //
    // The three query forms use the two-call count idiom, and the count is
    // `uint32_t *` in all of them.
    // -----------------------------------------------------------------------

    fn vkGetImageSparseMemoryRequirements(
        device: VkDevice,
        image: VkHandle,
        pSparseMemoryRequirementCount: *mut u32,
        pSparseMemoryRequirements: VkStructMut,
    );
    fn vkGetImageSparseMemoryRequirements2(
        device: VkDevice,
        pInfo: VkStruct,
        pSparseMemoryRequirementCount: *mut u32,
        pSparseMemoryRequirements: VkStructMut,
    );
    fn vkGetDeviceImageSparseMemoryRequirements(
        device: VkDevice,
        pInfo: VkStruct,
        pSparseMemoryRequirementCount: *mut u32,
        pSparseMemoryRequirements: VkStructMut,
    );
    /// The only queue submission in this group: sparse binding is a queue
    /// operation, not a device one, so this takes a `VkQueue`.
    fn vkQueueBindSparse(
        queue: VkQueue,
        bindInfoCount: u32,
        pBindInfo: VkStruct,
        fence: VkHandle,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Command buffers.
    //
    // No allocator anywhere here: command buffers come out of a
    // `VkCommandPool`, which owns their memory, so the pool is what took an
    // allocator when it was created. The handles are dispatchable, hence
    // `VkCommandBuffer` rather than `VkHandle` in the arrays.
    // -----------------------------------------------------------------------

    fn vkAllocateCommandBuffers(
        device: VkDevice,
        pAllocateInfo: VkStruct,
        pCommandBuffers: *mut VkCommandBuffer,
    ) -> VkResult;
    fn vkFreeCommandBuffers(
        device: VkDevice,
        commandPool: VkHandle,
        commandBufferCount: u32,
        pCommandBuffers: *const VkCommandBuffer,
    );
    fn vkBeginCommandBuffer(commandBuffer: VkCommandBuffer, pBeginInfo: VkStruct) -> VkResult;
    fn vkEndCommandBuffer(commandBuffer: VkCommandBuffer) -> VkResult;
    fn vkResetCommandBuffer(commandBuffer: VkCommandBuffer, flags: VkFlags) -> VkResult;

    // -----------------------------------------------------------------------
    // Host image copies.
    //
    // Copies between host memory and an image with no command buffer and no
    // queue involved, so each takes only the device and one info structure.
    // -----------------------------------------------------------------------

    fn vkCopyMemoryToImage(device: VkDevice, pCopyMemoryToImageInfo: VkStruct) -> VkResult;
    fn vkCopyImageToMemory(device: VkDevice, pCopyImageToMemoryInfo: VkStruct) -> VkResult;
    fn vkCopyImageToImage(device: VkDevice, pCopyImageToImageInfo: VkStruct) -> VkResult;
    fn vkTransitionImageLayout(
        device: VkDevice,
        transitionCount: u32,
        pTransitions: VkStruct,
    ) -> VkResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// `vkAllocateCommandBuffers` must resolve to *our* thunk.
    ///
    /// This is the entry point the guest demo stopped on, so it is the one worth
    /// naming here. The host address is compared against explicitly because
    /// returning it is the failure that looks like success: the pointer is
    /// non-null and names the right function, but the guest would call a Windows
    /// x64 function with the System V convention.
    #[test]
    fn command_buffer_allocation_resolves_to_our_thunk() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let ours = lookup("vkAllocateCommandBuffers").expect("declared and host-supported");
        let host = crate::host::symbol("vkAllocateCommandBuffers").expect("host exports it");
        assert_ne!(
            ours, host,
            "returned the host address, which the guest would call with the wrong ABI"
        );
        assert_eq!(
            ours, vkAllocateCommandBuffers as *const () as usize,
            "must be this crate's own export"
        );
    }

    /// Which of this group's entry points the Vulkan specification gives a
    /// `const VkAllocationCallbacks *`.
    ///
    /// Written out rather than derived, because the mistake being guarded is a
    /// *missing* `Allocator`: a list read out of the source could not detect its
    /// own omission. Every `vkCreate*` and `vkDestroy*` here takes one, plus the
    /// memory pair. Notably absent are the command-buffer entry points — those
    /// handles come from a `VkCommandPool`, which owns their memory — and every
    /// `vkGet*`, `vkBind*` and host-copy entry point, none of which create or
    /// destroy an object.
    const TAKE_AN_ALLOCATOR: &[&str] = &[
        "vkAllocateMemory",
        "vkCreateBuffer",
        "vkCreateBufferView",
        "vkCreateImage",
        "vkCreateImageView",
        "vkDestroyBuffer",
        "vkDestroyBufferView",
        "vkDestroyImage",
        "vkDestroyImageView",
        "vkFreeMemory",
    ];

    /// Each declared entry point, paired with the text of its parameter list.
    ///
    /// Read out of this file's own text because the properties checked below are
    /// not observable at runtime. `Allocator`, `VkStruct` and
    /// `*mut VkCommandBuffer` are all eight bytes wide and pointer-shaped, so
    /// declaring one where another belongs compiles, links, and passes every
    /// behavioural test — right up until a driver calls the guest's allocator
    /// with the wrong convention. The source is the only place the distinction
    /// still exists.
    ///
    /// The text is split at the test module's own attribute so that the lists and
    /// assertions below, which name these functions too, cannot be mistaken for
    /// declarations.
    fn declarations() -> Vec<(String, String)> {
        let source = include_str!("memory.rs");
        let (declared, _) = source
            .split_once("#[cfg(test)]")
            .expect("this file has a test module");
        // Comment lines are dropped first: the module docs and section headers
        // name these entry points and their types in prose, and a bare `fn`
        // inside one would otherwise be read as a declaration.
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
                (name.trim().to_owned(), parameters.to_owned())
            })
            .collect()
    }

    /// The declared type of one parameter of one entry point.
    ///
    /// The *type* is what decides how an argument crosses, so it has to be read
    /// as a type rather than searched for as a substring: the conventional name
    /// of an allocator parameter is `pAllocator`, and a substring search over
    /// the whole declaration would match that name and call a wrongly-typed
    /// parameter correct.
    fn parameter_type(declaration: &str, wanted: &str) -> Option<String> {
        declaration.split(',').find_map(|parameter| {
            let (name, kind) = parameter.split_once(':')?;
            (name.trim() == wanted).then(|| kind.trim().to_owned())
        })
    }

    /// The declarations must agree with the specification about which entry
    /// points take an allocator — in both directions.
    #[test]
    fn every_allocator_is_declared_as_an_allocator() {
        let declared: BTreeSet<String> = declarations()
            .into_iter()
            .filter(|(_, parameters)| {
                parameters.split(',').any(|parameter| {
                    parameter
                        .split_once(':')
                        .is_some_and(|(_, kind)| kind.trim() == "Allocator")
                })
            })
            .map(|(name, _)| name)
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

    /// Command buffers are dispatchable, so their arrays are `VkCommandBuffer`.
    ///
    /// Nothing else can catch this: `VkHandle` is `u64` and `VkCommandBuffer` is
    /// a pointer, both eight bytes, so the two are interchangeable to the
    /// compiler and to every call that happens to work. Keeping them distinct is
    /// what makes a reader able to tell from the signature that these handles are
    /// pointers into a driver dispatch table and cannot be treated as opaque
    /// integers.
    #[test]
    fn the_command_buffer_arrays_are_dispatchable() {
        let declared = declarations();
        let find = |name: &str| -> String {
            declared
                .iter()
                .find(|(declared, _)| declared == name)
                .map(|(_, parameters)| parameters.clone())
                .unwrap_or_else(|| panic!("{name} is not declared"))
        };

        assert_eq!(
            parameter_type(&find("vkAllocateCommandBuffers"), "pCommandBuffers").as_deref(),
            Some("*mut VkCommandBuffer"),
            "the allocated handles are dispatchable, not `VkHandle`"
        );
        assert_eq!(
            parameter_type(&find("vkFreeCommandBuffers"), "pCommandBuffers").as_deref(),
            Some("*const VkCommandBuffer"),
            "the freed handles are dispatchable and read-only, not `VkHandle`"
        );
    }

    /// The parse underlying the tests above has to see every entry point, or an
    /// undeclared allocator could hide in a declaration it silently skipped.
    #[test]
    fn the_source_parse_sees_the_whole_group() {
        let parsed: BTreeSet<String> = declarations().into_iter().map(|(name, _)| name).collect();
        let declared: BTreeSet<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(
            parsed, declared,
            "the allocator cross-check reads a different set of names than the group declares"
        );
        assert_eq!(NAMES.len(), 46, "this group owns 46 entry points");
    }
}
