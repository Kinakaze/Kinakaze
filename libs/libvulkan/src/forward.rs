//! How each argument crosses from the guest call to the host call.
//!
//! Almost every Vulkan argument crosses unchanged: a handle, a scalar or a
//! pointer means the same thing on both sides, and the calling convention is the
//! compiler's problem. One does not — a `VkAllocationCallbacks` pointer has to be
//! translated, because the driver will call back through it.
//!
//! Rather than marking that argument at each of the roughly ninety sites that
//! take one, the *type* carries the obligation. [`Forward`] maps a guest-side
//! argument type to its host-side type and performs whatever conversion that
//! implies; for everything except [`Allocator`] the conversion is the identity.
//! The `passthrough!` macro applies `Forward` to every argument uniformly and
//! names `<T as Forward>::Host` in the host signature.
//!
//! This is deliberately arranged so the mistake cannot be made. An earlier
//! version of the macro took an `alloc` keyword marking the allocator argument,
//! which had two problems: `macro_rules!` cannot branch on a per-argument
//! optional token in the expansion at all, and even if it could, omitting the
//! keyword would still compile — the type is just a pointer — and would leave the
//! driver calling guest code with the wrong convention. Writing the argument as
//! [`Allocator`] instead makes the translation automatic and its absence a type
//! error.

use core::ffi::c_void;

/// A `const VkAllocationCallbacks *` as it arrives from the guest.
///
/// Declare an allocator parameter with this type and the translation happens.
/// `#[repr(transparent)]` keeps it a bare pointer for ABI purposes, so nothing
/// about the guest's call changes.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Allocator(pub *const c_void);

/// Maps a guest-side argument to its host-side form.
pub trait Forward {
    /// The type the host function expects.
    type Host;
    /// Produces the host-side value.
    fn forward(self) -> Self::Host;
}

impl Forward for Allocator {
    type Host = *const c_void;

    /// Interns the guest's callbacks behind host-convention shims.
    fn forward(self) -> Self::Host {
        // SAFETY: the guest passed this as a `VkAllocationCallbacks *`; Vulkan
        // requires it to be null or point to a readable one.
        unsafe { crate::callbacks::translate(self.0) }
    }
}

/// Pointers cross unchanged: an address is an address under either convention,
/// and the structure it points at was laid out by the same Vulkan headers on both
/// sides.
impl<T> Forward for *const T {
    type Host = *const T;
    fn forward(self) -> Self::Host {
        self
    }
}

impl<T> Forward for *mut T {
    type Host = *mut T;
    fn forward(self) -> Self::Host {
        self
    }
}

/// Scalars cross unchanged. Which register they arrive in differs between the two
/// conventions, but that is exactly what the compiler derives from the two
/// signatures.
macro_rules! identity {
    ( $( $type:ty ),* $(,)? ) => {
        $(
            impl Forward for $type {
                type Host = $type;
                fn forward(self) -> Self::Host { self }
            }
        )*
    };
}

identity!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrapper must not change the ABI, or every allocator-taking entry point
    /// would pass its argument differently from every other pointer.
    #[test]
    fn the_allocator_wrapper_is_pointer_shaped() {
        assert_eq!(
            core::mem::size_of::<Allocator>(),
            core::mem::size_of::<*const c_void>()
        );
        assert_eq!(
            core::mem::align_of::<Allocator>(),
            core::mem::align_of::<*const c_void>()
        );
    }

    /// A null allocator needs no translation and must stay null.
    #[test]
    fn a_null_allocator_forwards_as_null() {
        assert!(Allocator(core::ptr::null()).forward().is_null());
    }

    /// Everything else is the identity, which is what keeps the macro uniform.
    #[test]
    fn scalars_and_pointers_are_unchanged() {
        assert_eq!(7_u32.forward(), 7_u32);
        assert_eq!((-3_i32).forward(), -3_i32);
        assert_eq!(1.5_f32.forward(), 1.5_f32);
        let value = 42_u64;
        assert_eq!((&raw const value).forward(), &raw const value);
    }
}
