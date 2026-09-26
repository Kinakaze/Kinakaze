//! The host Vulkan loader, and lazy binding to its exports.
//!
//! `vulkan-1.dll` is opened at first use rather than imported statically, for two
//! reasons. A static import would make this DLL fail to load at all on a machine
//! with no Vulkan runtime, taking the whole guest process with it even if the
//! guest never touches Vulkan. And the import would collide with our own
//! exports: this library exports `vkCreateInstance` itself, so it cannot also
//! import a symbol of that name into the same image.
//!
//! Each wrapped entry point therefore owns a [`Lazy`] holding the host address,
//! resolved once by name and cached.

use core::ffi::{c_char, c_void};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::types::{VkDevice, VkInstance};

/// The environment variable that overrides which loader is opened.
///
/// Present because a machine can have several loaders (a system one plus an SDK
/// one) and because pointing this at a validation-layer-enabled loader is the
/// only way to debug a guest's Vulkan usage from outside the guest.
pub const LOADER_OVERRIDE: &str = "KINAKAZE_VULKAN_LOADER";

/// The loader opened when [`LOADER_OVERRIDE`] is unset.
const DEFAULT_LOADER: &str = "vulkan-1.dll";

#[link(name = "kernel32")]
unsafe extern "system" {
    /// `LoadLibraryW`.
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    /// `GetProcAddress`. The name is ANSI even in the wide API.
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// The loaded host loader, or `None` when no Vulkan runtime is installed.
fn module() -> Option<*mut c_void> {
    static MODULE: OnceLock<usize> = OnceLock::new();
    let handle = *MODULE.get_or_init(|| {
        let name = std::env::var(LOADER_OVERRIDE).unwrap_or_else(|_| DEFAULT_LOADER.to_owned());
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        // SAFETY: `wide` is NUL-terminated and live for the call.
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        module as usize
    });
    (handle != 0).then_some(handle as *mut c_void)
}

/// Reports whether a host Vulkan loader is present at all.
///
/// Distinguishes "no Vulkan on this machine" from "this loader is too old for
/// that entry point", which are different answers to give a caller.
pub fn loader_present() -> bool {
    module().is_some()
}

/// The host address of `name`, or `None` if this loader does not export it.
pub fn symbol(name: &str) -> Option<usize> {
    let module = module()?;
    // SAFETY: `module` is a live library handle and the helper supplies a
    // NUL-terminated name for the duration of the call.
    let address = with_nul_terminated(name, |name| unsafe { GetProcAddress(module, name) });
    (!address.is_null()).then_some(address as usize)
}

type GetInstanceProcAddrFn = unsafe extern "system" fn(VkInstance, *const c_char) -> *const c_void;
type GetDeviceProcAddrFn = unsafe extern "system" fn(VkDevice, *const c_char) -> *const c_void;

fn host_get_instance_proc_addr() -> Option<GetInstanceProcAddrFn> {
    let address = symbol("vkGetInstanceProcAddr")?;
    Some(unsafe { core::mem::transmute(address) })
}

fn host_get_device_proc_addr() -> Option<GetDeviceProcAddrFn> {
    let address = symbol("vkGetDeviceProcAddr")?;
    Some(unsafe { core::mem::transmute(address) })
}

fn with_nul_terminated<T>(name: &str, use_name: impl FnOnce(*const u8) -> T) -> T {
    // All current Vulkan command names fit comfortably. The heap fallback keeps
    // the helper general for future registry additions without imposing a fixed
    // ABI limit.
    let mut stack = [0u8; 128];
    if name.len() < stack.len() {
        stack[..name.len()].copy_from_slice(name.as_bytes());
        return use_name(stack.as_ptr());
    }
    let mut bytes = Vec::with_capacity(name.len() + 1);
    bytes.extend_from_slice(name.as_bytes());
    bytes.push(0);
    use_name(bytes.as_ptr())
}

/// Resolves an instance-level or device-level command through the host loader.
/// Unlike [`symbol`], this reaches commands supplied only by an ICD extension.
pub(crate) fn instance_symbol(instance: VkInstance, name: &str) -> Option<usize> {
    if instance.is_null() {
        return None;
    }
    let get = host_get_instance_proc_addr()?;
    let address = with_nul_terminated(name, |name| unsafe { get(instance, name.cast()) });
    (!address.is_null()).then_some(address as usize)
}

/// Resolves a command enabled on one host logical device.
pub(crate) fn device_symbol(device: VkDevice, name: &str) -> Option<usize> {
    if device.is_null() {
        return None;
    }
    let get = host_get_device_proc_addr()?;
    let address = with_nul_terminated(name, |name| unsafe { get(device, name.cast()) });
    (!address.is_null()).then_some(address as usize)
}

static CURRENT_INSTANCE: AtomicUsize = AtomicUsize::new(0);

/// Records the most recently created live instance for directly linked
/// extension exports. Proc-address paths bind against their explicit handle.
pub fn register_instance(instance: VkInstance) {
    CURRENT_INSTANCE.store(instance as usize, Ordering::Release);
}

fn current_instance_symbol(name: &str) -> Option<usize> {
    let instance = CURRENT_INSTANCE.load(Ordering::Acquire) as VkInstance;
    instance_symbol(instance, name)
}

/// A host entry point resolved on first use.
///
/// `F` is the host-side signature, which is always `extern "system"` — that is
/// the whole point of the indirection. The guest calls our `extern "sysv64"`
/// export, and the compiler emits the argument shuffle between the two ABIs
/// because the two signatures differ only in their convention.
pub struct Lazy<F> {
    /// The export name, used for both resolution and diagnostics.
    name: &'static str,
    /// The cached address. `0` means "not yet looked up" and
    /// [`MISSING`] means "looked up and absent" — a real address can be neither,
    /// so one word carries all three states without a lock.
    cell: AtomicUsize,
    signature: PhantomData<F>,
}

/// A command exported by an ICD but not by `vulkan-1.dll` itself.
///
/// The guest still receives a statically compiled ABI thunk. Only the thunk's
/// host target is dynamic, populated from `vkGetInstanceProcAddr` or
/// `vkGetDeviceProcAddr` and then read lock-free on every call.
pub struct DynamicLazy<F> {
    name: &'static str,
    cell: AtomicUsize,
    signature: PhantomData<F>,
}

unsafe impl<F> Sync for DynamicLazy<F> {}

impl<F: Copy> DynamicLazy<F> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            cell: AtomicUsize::new(0),
            signature: PhantomData,
        }
    }

    pub fn bind(&self, address: usize) {
        if address > MISSING {
            self.cell.store(address, Ordering::Release);
        }
    }

    fn address(&self) -> Option<usize> {
        let cached = self.cell.load(Ordering::Acquire);
        if cached > MISSING {
            return Some(cached);
        }
        let found = current_instance_symbol(self.name)?;
        self.bind(found);
        Some(found)
    }

    pub fn resolve(&self) -> F {
        match self.address() {
            Some(address) => unsafe { core::mem::transmute_copy(&address) },
            None => missing(self.name),
        }
    }
}

/// The cache value meaning "this loader does not export the name".
///
/// Address 1 is never a real function: the lowest 64 KiB of the address space is
/// permanently reserved on Windows.
const MISSING: usize = 1;

// SAFETY: the only mutable state is an `AtomicUsize`, and `PhantomData<F>` holds
// nothing. Two threads racing to resolve the same name both call `GetProcAddress`
// and store the same answer.
unsafe impl<F> Sync for Lazy<F> {}

impl<F: Copy> Lazy<F> {
    /// Names a host entry point without resolving it.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            cell: AtomicUsize::new(0),
            signature: PhantomData,
        }
    }

    /// The host address, resolving on first call.
    fn address(&self) -> Option<usize> {
        match self.cell.load(Ordering::Acquire) {
            0 => {
                let found = symbol(self.name);
                self.cell.store(found.unwrap_or(MISSING), Ordering::Release);
                found
            }
            MISSING => None,
            address => Some(address),
        }
    }

    /// Reports whether the host loader exports this entry point.
    ///
    /// This is what makes `vkGetInstanceProcAddr` truthful. A caller that asks
    /// for an entry point the host cannot provide has to receive null, because
    /// the documented way to test for an extension function is exactly that null
    /// check. Handing back a thunk that would abort when called would convert a
    /// graceful feature check into a crash.
    pub fn present(&self) -> bool {
        self.address().is_some()
    }

    /// The host function, for calling.
    ///
    /// # Panics
    ///
    /// If the loader does not export the name. Reaching this means the guest
    /// called an entry point without checking for it first — every path this
    /// library hands out goes through [`Self::present`] — so there is no correct
    /// value to return: the signature has no error case, and returning a
    /// fabricated `VkResult` would report a driver failure that did not happen.
    pub fn resolve(&self) -> F {
        match self.address() {
            // SAFETY: `F` is a function-pointer type, so it is one word wide and
            // `transmute_copy` from the address is the correct reconstruction.
            // The signature was written to match the host's at the macro site.
            Some(address) => unsafe { core::mem::transmute_copy(&address) },
            None => missing(self.name),
        }
    }
}

/// Reports an entry point the host loader does not have, and exits.
#[cold]
#[inline(never)]
fn missing(name: &str) -> ! {
    if loader_present() {
        eprintln!(
            "kinakaze: the host Vulkan loader does not export {name}; \
             it is older than this passthrough expects"
        );
    } else {
        eprintln!(
            "kinakaze: no host Vulkan loader (tried {DEFAULT_LOADER}); \
             set {LOADER_OVERRIDE} to point at one"
        );
    }
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole passthrough rests on this: if the host loader cannot be opened
    /// and queried, nothing else in the crate can work.
    #[test]
    fn the_host_loader_exports_the_dispatch_root() {
        if !loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        assert!(symbol("vkGetInstanceProcAddr").is_some());
        assert!(symbol("vkCreateInstance").is_some());
    }

    /// A name no loader exports must resolve to `None`, not to a stale address.
    #[test]
    fn an_absent_name_is_absent() {
        assert!(symbol("vkThisIsNotAVulkanFunction").is_none());
    }

    /// The three cache states must not be confusable. A `Lazy` for a missing
    /// name has to keep reporting absent rather than retrying and, worse,
    /// eventually reporting a different answer.
    #[test]
    fn absence_is_cached_as_absence() {
        static ABSENT: Lazy<unsafe extern "system" fn()> = Lazy::new("vkNotReal");
        assert!(!ABSENT.present());
        assert!(!ABSENT.present());
    }

    /// A real name caches as present, and repeatedly.
    #[test]
    fn presence_is_cached_as_presence() {
        if !loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        static PRESENT: Lazy<unsafe extern "system" fn()> = Lazy::new("vkCreateInstance");
        assert!(PRESENT.present());
        assert!(PRESENT.present());
    }
}
