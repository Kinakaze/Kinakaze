//! Translating `VkAllocationCallbacks` across the ABI boundary.
//!
//! Every other argument in this library travels one way: the guest calls us, we
//! call the host. `VkAllocationCallbacks` is the exception. It carries five
//! function pointers that the *driver* calls, so a struct passed through
//! unchanged would have the host invoking guest code with the Windows x64
//! convention against a System V function. The first four arguments would arrive
//! in the wrong registers and the allocator would return a pointer derived from
//! garbage — and it would do so inside a driver, on an allocation path, which is
//! about the worst place to debug.
//!
//! Passing null instead would be safe, and Vulkan permits it. It is rejected
//! here because it silently substitutes a different allocator for the one the
//! guest asked for: an application that supplies callbacks to account for or
//! pool its Vulkan memory would keep working while its accounting quietly went
//! blank. This layer's rule is that a substitution has to be visible.
//!
//! What happens instead: the host receives a `VkAllocationCallbacks` whose five
//! entries are *our* `extern "system"` shims and whose `pUserData` points at a
//! [`Bridge`] holding the guest's real `pUserData` and its five sysv64 pointers.
//! When the driver calls a shim, the shim recovers the bridge from `pUserData`
//! and forwards to the guest with the right convention. The guest's own
//! `pUserData` reaches its callbacks unchanged, so guest code cannot tell.
//!
//! ## Why the translation is interned rather than built per call
//!
//! A driver may keep the `VkAllocationCallbacks` pointer it was given at create
//! time and use it at destroy time. Vulkan also requires that the allocator
//! passed to `vkDestroy*` be *compatible* with the one passed to the matching
//! `vkCreate*`. Building a fresh translation per call would hand out a pointer
//! into a temporary — a dangling pointer the moment the call returned — and would
//! hand out a different address each time.
//!
//! So each distinct callback set is translated once and kept for the life of the
//! process, keyed by the guest structure's contents. Interning by *contents*
//! rather than by guest address is deliberate: it means two identical callback
//! structs at different guest addresses map to one host struct, which is exactly
//! the compatibility Vulkan asks for. The store is never emptied, which is bounded
//! in practice — an application has one allocator, or a handful.

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;

/// `VkSystemAllocationScope` and `VkInternalAllocationType`, both 32-bit enums.
type Scope = u32;

/// The guest's `VkAllocationCallbacks`, exactly as Vulkan lays it out.
///
/// 48 bytes: one data pointer then five function pointers. The order is ABI and
/// is read directly out of guest memory.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GuestCallbacks {
    user_data: usize,
    allocation: usize,
    reallocation: usize,
    free: usize,
    internal_allocation: usize,
    internal_free: usize,
}

/// The guest's five entry points, with the convention they actually have.
type GuestAllocation = unsafe extern "sysv64" fn(*mut c_void, usize, usize, Scope) -> *mut c_void;
type GuestReallocation =
    unsafe extern "sysv64" fn(*mut c_void, *mut c_void, usize, usize, Scope) -> *mut c_void;
type GuestFree = unsafe extern "sysv64" fn(*mut c_void, *mut c_void);
type GuestInternalNotification = unsafe extern "sysv64" fn(*mut c_void, usize, Scope, Scope);

/// What a shim needs to reach the guest: its callbacks and its own `pUserData`.
///
/// This is what the host sees as `pUserData`, which is the whole trick — the one
/// pointer Vulkan already threads through every callback is the one place a
/// translation can store its own state without changing any signature.
struct Bridge {
    guest: GuestCallbacks,
}

/// The structure handed to the host: our shims, and the bridge as `pUserData`.
#[repr(C)]
struct HostCallbacks {
    user_data: *mut c_void,
    allocation: unsafe extern "system" fn(*mut c_void, usize, usize, Scope) -> *mut c_void,
    reallocation:
        unsafe extern "system" fn(*mut c_void, *mut c_void, usize, usize, Scope) -> *mut c_void,
    free: unsafe extern "system" fn(*mut c_void, *mut c_void),
    internal_allocation: unsafe extern "system" fn(*mut c_void, usize, Scope, Scope),
    internal_free: unsafe extern "system" fn(*mut c_void, usize, Scope, Scope),
}

/// Recovers the bridge a shim was called with.
///
/// # Safety
///
/// `user_data` must be the `pUserData` from a [`HostCallbacks`] this module
/// built, which is the only value the host can pass here.
unsafe fn bridge(user_data: *mut c_void) -> &'static Bridge {
    // SAFETY: the bridge is leaked at intern time and so outlives every call.
    unsafe { &*(user_data as *const Bridge) }
}

/// The `pfnAllocation` shim.
unsafe extern "system" fn allocation(
    user_data: *mut c_void,
    size: usize,
    alignment: usize,
    scope: Scope,
) -> *mut c_void {
    crate::reverse_abi::restore_guest_tls();
    // SAFETY: the host passes back the `pUserData` we installed.
    let bridge = unsafe { bridge(user_data) };
    // SAFETY: the guest supplied this pointer in its callback struct, and
    // Vulkan requires it to be a valid `PFN_vkAllocationFunction`.
    let guest: GuestAllocation = unsafe { core::mem::transmute(bridge.guest.allocation) };
    // SAFETY: forwarding the driver's arguments unchanged, with the guest's own
    // `pUserData` restored.
    unsafe {
        guest(
            bridge.guest.user_data as *mut c_void,
            size,
            alignment,
            scope,
        )
    }
}

/// The `pfnReallocation` shim.
unsafe extern "system" fn reallocation(
    user_data: *mut c_void,
    original: *mut c_void,
    size: usize,
    alignment: usize,
    scope: Scope,
) -> *mut c_void {
    crate::reverse_abi::restore_guest_tls();
    // SAFETY: as `allocation`.
    let bridge = unsafe { bridge(user_data) };
    // SAFETY: as `allocation`.
    let guest: GuestReallocation = unsafe { core::mem::transmute(bridge.guest.reallocation) };
    // SAFETY: as `allocation`.
    unsafe {
        guest(
            bridge.guest.user_data as *mut c_void,
            original,
            size,
            alignment,
            scope,
        )
    }
}

/// The `pfnFree` shim.
unsafe extern "system" fn free(user_data: *mut c_void, memory: *mut c_void) {
    crate::reverse_abi::restore_guest_tls();
    // SAFETY: as `allocation`.
    let bridge = unsafe { bridge(user_data) };
    // SAFETY: as `allocation`.
    let guest: GuestFree = unsafe { core::mem::transmute(bridge.guest.free) };
    // SAFETY: as `allocation`.
    unsafe { guest(bridge.guest.user_data as *mut c_void, memory) }
}

/// The `pfnInternalAllocation` shim.
///
/// Optional in Vulkan: a guest may supply the three required entries and leave
/// these two null. The shim is still installed, and checks — the driver decides
/// whether to call it by looking at our struct, where the pointer is non-null.
unsafe extern "system" fn internal_allocation(
    user_data: *mut c_void,
    size: usize,
    kind: Scope,
    scope: Scope,
) {
    crate::reverse_abi::restore_guest_tls();
    // SAFETY: as `allocation`.
    let bridge = unsafe { bridge(user_data) };
    if bridge.guest.internal_allocation == 0 {
        return;
    }
    // SAFETY: as `allocation`.
    let guest: GuestInternalNotification =
        unsafe { core::mem::transmute(bridge.guest.internal_allocation) };
    // SAFETY: as `allocation`.
    unsafe { guest(bridge.guest.user_data as *mut c_void, size, kind, scope) }
}

/// The `pfnInternalFree` shim.
unsafe extern "system" fn internal_free(
    user_data: *mut c_void,
    size: usize,
    kind: Scope,
    scope: Scope,
) {
    crate::reverse_abi::restore_guest_tls();
    // SAFETY: as `allocation`.
    let bridge = unsafe { bridge(user_data) };
    if bridge.guest.internal_free == 0 {
        return;
    }
    // SAFETY: as `allocation`.
    let guest: GuestInternalNotification =
        unsafe { core::mem::transmute(bridge.guest.internal_free) };
    // SAFETY: as `allocation`.
    unsafe { guest(bridge.guest.user_data as *mut c_void, size, kind, scope) }
}

/// The interned translations, keyed by the guest structure's contents.
fn store() -> &'static Mutex<HashMap<GuestCallbacks, usize>> {
    static STORE: std::sync::OnceLock<Mutex<HashMap<GuestCallbacks, usize>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Translates a guest `VkAllocationCallbacks` into one the host may call.
///
/// Returns null for a null input, which is Vulkan's "use the driver's own
/// allocator" and needs no translation. Otherwise returns a stable pointer, equal
/// for equal guest structures, valid for the life of the process.
///
/// # Safety
///
/// `guest` must be null or point to a readable `VkAllocationCallbacks`.
pub unsafe fn translate(guest: *const c_void) -> *const c_void {
    if guest.is_null() {
        return core::ptr::null();
    }
    // SAFETY: the caller guarantees a readable callback structure, whose layout
    // is fixed by Vulkan and mirrored by `GuestCallbacks`.
    let guest = unsafe { *(guest as *const GuestCallbacks) };

    // A structure whose three required entries are not all present is not a
    // usable allocator. Vulkan requires all three; treating it as absent is the
    // honest reading, and it keeps a partly-filled struct from producing a call
    // through a null pointer inside the driver.
    if guest.allocation == 0 || guest.reallocation == 0 || guest.free == 0 {
        return core::ptr::null();
    }

    let mut store = match store().lock() {
        Ok(store) => store,
        // A poisoned lock means another thread panicked while interning. The
        // translations already published are still valid, so recover rather than
        // propagating a panic into a driver callback.
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(&existing) = store.get(&guest) {
        return existing as *const c_void;
    }

    // Leaked deliberately: the driver keeps this pointer for the lifetime of
    // every object created with it, and there is no later moment at which it is
    // known to be unreachable.
    let bridge: &'static Bridge = Box::leak(Box::new(Bridge { guest }));
    let host: &'static HostCallbacks = Box::leak(Box::new(HostCallbacks {
        user_data: (bridge as *const Bridge) as *mut c_void,
        allocation,
        reallocation,
        free,
        internal_allocation,
        internal_free,
    }));
    let address = (host as *const HostCallbacks) as usize;
    store.insert(guest, address);
    address as *const c_void
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// Both structures are ABI. A wrong size means the fields are being read from
    /// the wrong offsets in guest memory.
    #[test]
    fn both_layouts_are_forty_eight_bytes() {
        assert_eq!(core::mem::size_of::<GuestCallbacks>(), 48);
        assert_eq!(core::mem::size_of::<HostCallbacks>(), 48);
    }

    /// Null means "driver's own allocator" and must survive as null.
    #[test]
    fn a_null_allocator_stays_null() {
        // SAFETY: null is explicitly permitted.
        assert!(unsafe { translate(core::ptr::null()) }.is_null());
    }

    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static SEEN_SIZE: AtomicUsize = AtomicUsize::new(0);
    static SEEN_USER_DATA: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "sysv64" fn guest_alloc(
        user_data: *mut c_void,
        size: usize,
        _alignment: usize,
        _scope: Scope,
    ) -> *mut c_void {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        SEEN_SIZE.store(size, Ordering::SeqCst);
        SEEN_USER_DATA.store(user_data as usize, Ordering::SeqCst);
        0x1234_usize as *mut c_void
    }

    unsafe extern "sysv64" fn guest_realloc(
        _user_data: *mut c_void,
        _original: *mut c_void,
        _size: usize,
        _alignment: usize,
        _scope: Scope,
    ) -> *mut c_void {
        core::ptr::null_mut()
    }

    unsafe extern "sysv64" fn guest_free(_user_data: *mut c_void, _memory: *mut c_void) {}

    fn sample() -> GuestCallbacks {
        GuestCallbacks {
            user_data: 0xDEAD_BEEF,
            allocation: guest_alloc as *const () as usize,
            reallocation: guest_realloc as *const () as usize,
            free: guest_free as *const () as usize,
            internal_allocation: 0,
            internal_free: 0,
        }
    }

    /// The point of the whole module: the driver calls our shim with the Windows
    /// convention, and the guest's System V callback receives the right
    /// arguments and its own `pUserData`.
    #[test]
    fn the_shim_forwards_across_the_abi_boundary() {
        let guest = sample();
        // SAFETY: `guest` is a live, fully-populated callback structure.
        let host = unsafe { translate((&raw const guest).cast()) };
        assert!(!host.is_null());

        ALLOCATIONS.store(0, Ordering::SeqCst);
        // SAFETY: `host` points at a `HostCallbacks` this module built.
        let host = unsafe { &*(host as *const HostCallbacks) };
        // SAFETY: calling the shim exactly as the driver would.
        let result = unsafe { (host.allocation)(host.user_data, 4096, 16, 1) };

        assert_eq!(result as usize, 0x1234);
        assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 1);
        assert_eq!(SEEN_SIZE.load(Ordering::SeqCst), 4096);
        // The guest must see its own `pUserData`, not our bridge.
        assert_eq!(SEEN_USER_DATA.load(Ordering::SeqCst), 0xDEAD_BEEF);
    }

    /// Vulkan requires a destroy-time allocator compatible with the create-time
    /// one, so equal guest structures must map to one host structure.
    #[test]
    fn equal_callbacks_intern_to_one_translation() {
        let first = sample();
        let second = sample();
        // SAFETY: both are live callback structures.
        let a = unsafe { translate((&raw const first).cast()) };
        // SAFETY: as above.
        let b = unsafe { translate((&raw const second).cast()) };
        assert_eq!(a, b, "identical callbacks must share one host translation");
    }

    /// Different callbacks must not collapse together, or a driver would call
    /// the wrong allocator.
    #[test]
    fn different_callbacks_get_different_translations() {
        let first = sample();
        let mut second = sample();
        second.user_data = 0x5678;
        // SAFETY: both are live callback structures.
        let a = unsafe { translate((&raw const first).cast()) };
        // SAFETY: as above.
        let b = unsafe { translate((&raw const second).cast()) };
        assert_ne!(a, b);
    }

    /// A struct missing a required entry is not a usable allocator, and must not
    /// become a null call inside the driver.
    #[test]
    fn an_incomplete_allocator_is_treated_as_absent() {
        let mut partial = sample();
        partial.free = 0;
        // SAFETY: `partial` is readable; its contents are the point of the test.
        assert!(unsafe { translate((&raw const partial).cast()) }.is_null());
    }

    /// The optional notifications may be null in the guest struct; the shim is
    /// installed regardless and must not call through a null pointer.
    #[test]
    fn absent_internal_notifications_are_not_called() {
        let guest = sample();
        // SAFETY: `guest` is live and complete.
        let host = unsafe { translate((&raw const guest).cast()) };
        // SAFETY: built by this module.
        let host = unsafe { &*(host as *const HostCallbacks) };
        // SAFETY: the driver is entitled to call this; it must return quietly
        // because the guest supplied no notification callback.
        unsafe { (host.internal_allocation)(host.user_data, 64, 0, 1) };
    }
}
