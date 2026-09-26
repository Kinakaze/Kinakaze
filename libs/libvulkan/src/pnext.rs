//! Owned copies of Vulkan `pNext` nodes.
//!
//! An application's chain is const input. Re-linking it in place to substitute a
//! reverse-ABI callback could write into read-only memory or race another create
//! call. The generated Khronos-registry table supplies the x86-64 size of every
//! extensible structure, allowing the chain to be copied and re-linked privately.

use core::ffi::c_void;

/// One registry-known `pNext` structure in eight-byte-aligned storage.
pub struct RawNode {
    storage: Box<[u64]>,
}

impl RawNode {
    /// Copies the complete node at `source`.
    ///
    /// # Safety
    ///
    /// `source` must point to a valid Vulkan structure and remain readable for
    /// the size associated with its leading `sType`.
    pub unsafe fn copy(source: *const c_void) -> Option<Self> {
        if source.is_null() {
            return None;
        }
        let s_type = unsafe { source.cast::<u32>().read() };
        let size = crate::generated_extensions::pnext_node_size(s_type)?;
        if size < 16 {
            return None;
        }
        let words = size.checked_add(7)?.checked_div(8)?;
        let mut storage = vec![0u64; words].into_boxed_slice();
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.cast::<u8>(),
                storage.as_mut_ptr().cast::<u8>(),
                size,
            )
        };
        Some(Self { storage })
    }

    pub fn as_ptr(&self) -> *const c_void {
        self.storage.as_ptr().cast()
    }

    pub fn p_next(&self) -> *const c_void {
        unsafe {
            self.storage
                .as_ptr()
                .cast::<u8>()
                .add(8)
                .cast::<*const c_void>()
                .read()
        }
    }

    #[expect(
        clippy::not_unsafe_ptr_arg_deref,
        reason = "stores an opaque pointer value in owned storage without dereferencing it"
    )]
    pub fn set_p_next(&mut self, p_next: *const c_void) {
        unsafe {
            self.storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(8)
                .cast::<*const c_void>()
                .write(p_next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct KnownNode {
        s_type: u32,
        _padding: u32,
        p_next: *const c_void,
        tail: [u64; 4],
    }

    #[test]
    fn a_registered_node_is_copied_and_relinked_without_touching_the_source() {
        // VkDebugUtilsMessengerCreateInfoEXT is a registered 48-byte node.
        let source = KnownNode {
            s_type: 1_000_128_004,
            _padding: 0,
            p_next: 0x1111usize as *const c_void,
            tail: [1, 2, 3, 4],
        };
        let mut copied = unsafe { RawNode::copy((&raw const source).cast()) }.expect("known sType");
        assert_ne!(copied.as_ptr(), (&raw const source).cast());
        assert_eq!(copied.p_next(), source.p_next);
        copied.set_p_next(0x2222usize as *const c_void);
        assert_eq!(copied.p_next(), 0x2222usize as *const c_void);
        assert_eq!(source.p_next, 0x1111usize as *const c_void);
        let copied = unsafe { &*copied.as_ptr().cast::<KnownNode>() };
        assert_eq!(copied.tail, source.tail);
    }
}
