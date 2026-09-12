//! The Vulkan types this passthrough needs to name.
//!
//! Deliberately shallow. A passthrough only has to get the *ABI* of each
//! argument right — its width, and whether it travels in an integer or an SSE
//! register — never its contents. Vulkan passes every structure by pointer, and a
//! pointer's ABI does not depend on what it points at, so almost every parameter
//! here is an opaque pointer.
//!
//! This is not a shortcut. The guest and the host driver were both built against
//! the same Vulkan headers, so they already agree on every structure layout;
//! restating those layouts here would add hundreds of definitions whose only
//! possible effect is to be wrong. Two kinds of structure genuinely do have to be
//! understood, and both are handled elsewhere:
//!
//! * `VkAllocationCallbacks`, because it carries guest function pointers that the
//!   host would otherwise call with the wrong calling convention. See
//!   [`crate::callbacks`].
//! * Structures reached through a `pNext` chain that carry reverse callbacks,
//!   such as the debug-messenger creation structures. Those are translated by
//!   the extension-specific bridge before the host sees them.
//!
//! Handle types come in two kinds and the difference is ABI-visible on x86_64
//! only in that both happen to be 64 bits: a *dispatchable* handle
//! (`VkInstance`, `VkDevice`, `VkQueue`, `VkCommandBuffer`, `VkPhysicalDevice`)
//! is a pointer to a driver-owned dispatch table, while a *non-dispatchable*
//! handle (`VkBuffer`, `VkImage`, everything else) is a 64-bit integer the driver
//! interprets however it likes. They are kept as distinct aliases because reading
//! a signature and seeing which kind a parameter is tells you whether it can be
//! null.

use core::ffi::c_void;

// ---------------------------------------------------------------------------
// Dispatchable handles: pointers to a driver dispatch table.
// ---------------------------------------------------------------------------

/// `VkInstance`.
pub type VkInstance = *mut c_void;
/// `VkPhysicalDevice`.
pub type VkPhysicalDevice = *mut c_void;
/// `VkDevice`.
pub type VkDevice = *mut c_void;
/// `VkQueue`.
pub type VkQueue = *mut c_void;
/// `VkCommandBuffer`.
pub type VkCommandBuffer = *mut c_void;

// ---------------------------------------------------------------------------
// Non-dispatchable handles: 64-bit driver-defined values.
// ---------------------------------------------------------------------------

/// Any non-dispatchable handle: `VkBuffer`, `VkImage`, `VkSemaphore` and the
/// rest. All are `uint64_t` on every platform, which is why one alias serves.
pub type VkHandle = u64;

// ---------------------------------------------------------------------------
// Scalars.
// ---------------------------------------------------------------------------

/// `VkResult`. Negative values are errors, `VK_SUCCESS` is 0, and positive
/// values are non-error statuses such as `VK_SUBOPTIMAL_KHR`.
pub type VkResult = i32;
/// `VkFlags`, and every 32-bit enum and bitmask.
pub type VkFlags = u32;
/// `VkFlags64`, used by the newer bitmasks such as `VkPipelineStageFlags2`.
pub type VkFlags64 = u64;
/// `VkDeviceSize` and `VkDeviceAddress`.
pub type VkDeviceSize = u64;
/// `VkBool32`.
pub type VkBool32 = u32;

// ---------------------------------------------------------------------------
// Opaque pointers.
// ---------------------------------------------------------------------------

/// A pointer to any Vulkan structure, in or out.
///
/// See the module documentation for why the layout is deliberately not named.
pub type VkStruct = *const c_void;
/// A pointer to any Vulkan structure the callee writes.
pub type VkStructMut = *mut c_void;
/// A `const VkAllocationCallbacks *` as it arrives from the guest.
///
/// Typed distinctly from [`VkStruct`] so that every site which must translate it
/// is findable by name. Passing one of these to the host unchanged is a bug: see
/// [`crate::callbacks`].
pub type VkGuestAllocator = *const c_void;
