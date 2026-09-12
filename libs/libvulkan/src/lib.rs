//! `libvulkan.dll`: Vulkan passthrough from a Linux guest to the host driver.
//!
//! A guest ELF that names `libvulkan.so.1` resolves to this PE-backed
//! `/usr/lib/libvulkan.so.1` provider, and every entry point forwards to the real
//! `vulkan-1.dll`. There is no emulation and no software rasteriser: calls
//! reach the installed driver and run on the actual GPU.
//!
//! ## What this layer is actually for
//!
//! Exactly one thing: the calling convention. A Linux guest calls with the System
//! V AMD64 convention and the host driver expects Windows x64. The first four
//! integer arguments live in different registers, the fifth and sixth are in
//! registers on System V but on the stack on Windows, and Windows wants 32 bytes
//! of shadow space the guest never reserved. Every export here is
//! `extern "sysv64"`, every host binding is `extern "system"`, and the compiler
//! emits the shuffle between them from the two signatures. Nothing else about a
//! Vulkan call needs changing: handles are 64-bit on both sides, and every
//! structure is passed by pointer, so the guest and the driver already agree on
//! all of it.
//!
//! Two things do not translate themselves, and both are handled explicitly:
//!
//! * **Allocation callbacks** travel the other way — the driver calls into the
//!   guest — so a struct passed through unchanged would be invoked with the wrong
//!   convention. See [`callbacks`].
//! * **`vkGetInstanceProcAddr`** returns function pointers, and a host address
//!   returned to the guest would be called with the wrong convention. It is
//!   answered from this crate's own table instead. See [`dispatch`].
//!
//! ## Surfaces and extensions
//!
//! Linux Xlib/XCB/Wayland surface requests are projected onto the native `HWND`
//! owned by `libdisplay`, so the host still creates a real Win32 swapchain.  Core
//! commands bind from the loader export table; every remaining command in the
//! Khronos registry has a generated thunk and is exposed only when the host's
//! `vkGetInstanceProcAddr`/`vkGetDeviceProcAddr` returns it.  Host capability and
//! guest-callable capability therefore stay identical instead of maintaining an
//! application-specific allowlist.
//!
//! Function pointers travelling back into the guest need the opposite ABI
//! conversion. Allocation callbacks plus `VK_EXT_debug_utils` and
//! `VK_EXT_debug_report` are translated explicitly; raw host function pointers
//! are never returned to guest code.

#![allow(non_snake_case)]

pub mod callbacks;
pub mod debug_callbacks;
pub mod device_callbacks;
pub mod dispatch;
pub mod forward;
pub mod generated_extensions;
pub mod groups;
pub mod host;
pub mod pnext;
pub mod reverse_abi;
pub mod reverse_driver;
pub mod types;
pub mod wsi_compat;

pub use debug_callbacks::{vkCreateDebugReportCallbackEXT, vkCreateDebugUtilsMessengerEXT};
pub use wsi_compat::{
    vkCreateInstance, vkCreateWaylandSurfaceKHR, vkCreateXcbSurfaceKHR, vkCreateXlibSurfaceKHR,
    vkEnumerateInstanceExtensionProperties, vkGetPhysicalDeviceWaylandPresentationSupportKHR,
    vkGetPhysicalDeviceXcbPresentationSupportKHR, vkGetPhysicalDeviceXlibPresentationSupportKHR,
};

#[cfg(test)]
mod integration;

/// Declares a group of passthrough entry points.
///
/// Each declaration expands into three things:
///
/// 1. A module named after the entry point, holding the host signature and a
///    [`host::Lazy`] that resolves it once. The module and the function share a
///    name, which Rust permits because types and values occupy separate
///    namespaces.
/// 2. A `#[no_mangle] extern "sysv64"` export under the bare Vulkan name, which
///    is what the guest's relocations bind to.
/// 3. An arm in that module's `lookup` function, so `vkGetInstanceProcAddr` can
///    hand back *our* thunk rather than the host's address.
///
/// The signature is written once and used for both sides, so the two can never
/// disagree about arity or argument width.
///
/// Arguments are written with their guest-side types, and each is passed through
/// [`forward::Forward`], which is the identity for everything except an
/// [`forward::Allocator`]:
///
/// ```ignore
/// passthrough! {
///     fn vkDeviceWaitIdle(device: VkDevice) -> VkResult;
///     fn vkDestroyBuffer(device: VkDevice, buffer: VkHandle, allocator: Allocator);
/// }
/// ```
///
/// Writing an allocator parameter as `Allocator` rather than as a bare pointer is
/// what triggers its translation, and the host signature is derived from
/// `<T as Forward>::Host` so the two sides cannot disagree.
#[macro_export]
macro_rules! passthrough {
    ( $( $( #[$attribute:meta] )* fn $name:ident (
        $( $argument:ident : $type:ty ),* $(,)?
    ) $( -> $result:ty )? ; )* ) => {
        $(
            #[doc = concat!("Host binding for `", stringify!($name), "`.")]
            #[allow(non_snake_case)]
            mod $name {
                #[allow(unused_imports)]
                use super::*;
                use $crate::forward::Forward;
                /// The host's signature.
                ///
                /// Each argument is the *host* form of the guest type, which is
                /// the same type for everything but an allocator. Deriving it
                /// rather than restating it is what keeps a translated argument
                /// from being declared untranslated on one side.
                pub type Signature = unsafe extern "system" fn(
                    $( <$type as Forward>::Host ),*
                ) $( -> $result )?;
                /// The host address, resolved on first use.
                pub static HOST: $crate::host::Lazy<Signature> =
                    $crate::host::Lazy::new(stringify!($name));
            }

            $( #[$attribute] )*
            #[doc = concat!("`", stringify!($name), "`, forwarded to the host driver.")]
            ///
            /// # Safety
            ///
            /// The caller must satisfy this entry point's own Vulkan contract:
            /// live handles, structures whose `sType` and `pNext` chain are
            /// valid, and output pointers with room for what is written. Those
            /// requirements are Vulkan's and are not restated per entry point
            /// here — 265 copies of the same paragraph would obscure the two
            /// obligations that are specific to *this* layer:
            ///
            /// * A `VkAllocationCallbacks` argument must be declared as
            ///   [`forward::Allocator`], or the driver will call the guest's
            ///   allocator with the wrong calling convention.
            /// * The signature must match the host's in arity and argument
            ///   width, since that is what the ABI shuffle is derived from.
            ///
            /// Both are properties of the declaration rather than of the call, so
            /// they are checked by `scripts/check-vulkan-groups.py` and by this
            /// crate's tests rather than left to each caller.
            #[unsafe(export_name = concat!("kinakaze_engine_libvulkan_", stringify!($name)))]
            #[allow(non_snake_case)]
            pub unsafe extern "sysv64" fn $name(
                $( $argument : $type ),*
            ) $( -> $result )? {
                use $crate::forward::Forward;
                // SAFETY: the signatures differ only in calling convention, and
                // the guest's contract for each argument is Vulkan's own.
                unsafe { ($name::HOST.resolve())($( $argument.forward() ),*) }
            }
        )*

        /// This group's name-to-thunk table.
        ///
        /// Returns the address of *our* export, and only when the host loader
        /// really has the entry point — see [`$crate::host::Lazy::present`].
        pub fn lookup(name: &str) -> Option<usize> {
            match name {
                $(
                    stringify!($name) => if $name::HOST.present() {
                        Some($name as *const () as usize)
                    } else {
                        None
                    },
                )*
                _ => None,
            }
        }

        /// Every entry point this group declares, whether or not the host has it.
        ///
        /// Used by the crate's tests to check the groups partition the loader's
        /// export table with no name declared twice and none left out.
        pub const NAMES: &[&str] = &[ $( stringify!($name) ),* ];
    };
}

/// Declares proc-address-only extension commands.
///
/// This is deliberately separate from [`passthrough!`]: core/loader exports
/// bind through `GetProcAddress`, while these targets are supplied by an ICD
/// through Vulkan's own instance/device proc-address functions.
#[macro_export]
macro_rules! dynamic_passthrough {
    ( $( fn $name:ident (
        $( $argument:ident : $type:ty ),* $(,)?
    ) $( -> $result:ty )? ; )* ) => {
        $(
            #[allow(non_snake_case)]
            pub mod $name {
                #[allow(unused_imports)]
                use super::*;
                use $crate::forward::Forward;
                pub type Signature = unsafe extern "system" fn(
                    $( <$type as Forward>::Host ),*
                ) $( -> $result )?;
                pub static HOST: $crate::host::DynamicLazy<Signature> =
                    $crate::host::DynamicLazy::new(stringify!($name));
            }

            #[unsafe(export_name = concat!("kinakaze_engine_libvulkan_", stringify!($name)))]
            #[allow(non_snake_case)]
            pub unsafe extern "sysv64" fn $name(
                $( $argument : $type ),*
            ) $( -> $result )? {
                use $crate::forward::Forward;
                unsafe { ($name::HOST.resolve())($( $argument.forward() ),*) }
            }
        )*
    };
}

mod object_layout;
