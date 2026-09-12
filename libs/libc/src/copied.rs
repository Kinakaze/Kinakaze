//! Variables the guest owns a copy of, and how this libc reaches them.
//!
//! ## The problem
//!
//! A non-PIE-ish executable that references a libc *variable* gets an
//! `R_X86_64_COPY` relocation: the linker reserves space for the variable in the
//! executable's own `.bss` and the dynamic linker copies libc's initial value into
//! it at load time. Debian's BusyBox does this for `optind`, `optarg`, `opterr`,
//! `__environ`, `re_syntax_options`, and the three standard streams.
//!
//! On Linux that works because libc's *own* references to those variables go
//! through its GOT. The dynamic linker points libc's GOT entry at the executable's
//! copy, so both sides read and write one location.
//!
//! Here they do not. `libc.dll` is a Windows PE whose internal accesses are direct,
//! RIP-relative loads and stores against its own `.data`, and no relocation the ELF
//! loader applies can redirect them. So after a COPY there are **two** variables:
//! the guest reads and writes its own, this libc reads and writes its own, and
//! neither sees the other.
//!
//! That failure is quiet and confusing. `busybox ls` parsed `-l` correctly — the
//! option handling is all internal — and then listed a file called `ls`, because
//! `getopt` had advanced *our* `optind` while BusyBox collected operands starting
//! from *its* `optind`, which was still 0.
//!
//! ## The fix
//!
//! Every COPY-able variable is accessed through a redirect: a pointer that starts
//! out addressing this DLL's own storage, and which the loader overwrites with the
//! guest's address once it has applied the COPY. After that, both sides are looking
//! at the same memory and the divergence cannot arise.
//!
//! The loader calls [`kinakaze_copied_redirect`] for each COPY relocation it
//! resolves against this DLL. A name it does not recognise is ignored, which is the
//! right behaviour: this only matters for variables both sides *write*, and the
//! streams work correctly under a plain COPY because both copies then hold the same
//! `FILE *`.

use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicPtr, Ordering};

/// COPY-relocatable payload with caller-serialized access. Only the leading
/// `T` is exported to Linux; the catalog records its size, not this wrapper's.
#[repr(C)]
pub struct CopiedValue<T: Copy> {
    storage: UnsafeCell<T>,
    location: AtomicPtr<T>,
}

// SAFETY: callers serialize publication and must not race guest accesses.
// These are the same rules as the POSIX timezone globals this type implements.
unsafe impl<T: Copy> Sync for CopiedValue<T> {}

impl<T: Copy> CopiedValue<T> {
    pub const fn new(value: T) -> Self {
        Self {
            storage: UnsafeCell::new(value),
            location: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    pub(crate) fn target(&self) -> *mut T {
        self.location.load(Ordering::Acquire)
    }

    /// Caller must serialize reads with writes to both the provider and guest.
    pub(crate) unsafe fn get(&self) -> T {
        let target = self.target();
        unsafe {
            if target.is_null() {
                self.storage.get()
            } else {
                target
            }
            .read()
        }
    }

    /// Caller must serialize publication with all accesses to this payload.
    pub(crate) unsafe fn set(&self, value: T) {
        unsafe {
            self.storage.get().write(value);
            let target = self.target();
            if !target.is_null() {
                target.write(value);
            }
        }
    }

    /// A non-null target must be writable, correctly aligned storage for `T`
    /// lasting until process exit. Null restores the provider's own storage.
    pub(crate) unsafe fn redirect(&self, target: *mut T) {
        self.location.store(target, Ordering::Release);
    }
}

/// A redirectable `int`, as exported to the guest.
///
/// The value lives in `storage` until the loader supplies the guest's address.
/// `#[repr(C)]` because the guest's COPY relocation reads `size_of` bytes from the
/// exported symbol's address, so the first field has to be the value itself.
#[repr(C)]
pub struct CopiedInt {
    /// This DLL's own storage, and the COPY source.
    storage: UnsafeCell<c_int>,
    /// Where reads and writes actually go. Never null after construction.
    location: AtomicPtr<c_int>,
}

// SAFETY: all access goes through the atomic pointer and raw reads/writes of a
// naturally-aligned `int`, which is atomic on x86_64 for the single-threaded-guest
// use these variables have. The guest's copy is plain memory it also touches, so no
// stronger guarantee is available or expected — this matches what a Linux libc
// provides for `optind`, which is documented as not thread-safe.
unsafe impl Sync for CopiedInt {}

impl CopiedInt {
    /// Creates the variable with its initial value.
    pub const fn new(value: c_int) -> Self {
        Self {
            storage: UnsafeCell::new(value),
            location: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// The address reads and writes go to.
    fn address(&self) -> *mut c_int {
        let redirect = self.location.load(Ordering::Acquire);
        if redirect.is_null() {
            // Not redirected: our own storage. Cast away the shared reference
            // because the guest treats this as a mutable global.
            self.storage.get()
        } else {
            redirect
        }
    }

    /// Reads the current value.
    pub fn get(&self) -> c_int {
        // SAFETY: `address` is either our own field or the guest's storage, which
        // the loader verified is a writable `int` before installing it.
        unsafe { self.address().read() }
    }

    /// Publishes a new value.
    pub fn set(&self, value: c_int) {
        unsafe {
            self.storage.get().write(value);
            let redirect = self.location.load(Ordering::Acquire);
            if !redirect.is_null() {
                redirect.write(value);
            }
        }
    }

    /// Points this variable at the guest's copy.
    ///
    /// # Safety
    ///
    /// `target` must address a writable `int` that outlives the process.
    unsafe fn redirect(&self, target: *mut c_int) {
        self.location.store(target, Ordering::Release);
    }

    /// Sends reads and writes back to this DLL's own storage.
    ///
    /// Exists for tests. In a running guest a redirect is installed once, at load
    /// time, against storage that lives as long as the process — so nothing ever
    /// needs to undo one. A test that installs one against a shorter-lived target
    /// does, and without this the exported statics stay pointed at a dead stack
    /// frame for the rest of the suite.
    ///
    /// That is not hypothetical: the first version of the test below redirected the
    /// real `optind` at a local and returned, and the next `getopt` test to run read
    /// through the dangling pointer. The suite died with a `STATUS_ACCESS_VIOLATION`
    /// several tests later, in a test that passes in isolation.
    #[cfg(test)]
    fn clear_redirect(&self) {
        self.location
            .store(core::ptr::null_mut(), Ordering::Release);
    }
}

/// A redirectable `char *`, for `optarg`.
#[repr(C)]
pub struct CopiedPointer {
    storage: UnsafeCell<*mut c_char>,
    location: AtomicPtr<*mut c_char>,
}

// SAFETY: as `CopiedInt`.
unsafe impl Sync for CopiedPointer {}

impl CopiedPointer {
    pub const fn new() -> Self {
        Self::with_value(core::ptr::null_mut())
    }

    /// Creates a pointer variable with a non-null fallback value.
    pub const fn with_value(value: *mut c_char) -> Self {
        Self {
            storage: UnsafeCell::new(value),
            location: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    fn address(&self) -> *mut *mut c_char {
        let redirect = self.location.load(Ordering::Acquire);
        if redirect.is_null() {
            self.storage.get()
        } else {
            redirect
        }
    }

    /// Reads the current pointer.
    pub fn get(&self) -> *mut c_char {
        // SAFETY: as `CopiedInt::get`.
        unsafe { self.address().read() }
    }

    /// Publishes a new pointer.
    ///
    /// `value` is stored, never dereferenced — it is `optarg`, which points into the
    /// guest's own `argv` and is only ever read by the guest. Clippy flags a public
    /// function taking a raw pointer, which is the right default; the allow is narrow
    /// and the reason is that nothing here follows the pointer.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn set(&self, value: *mut c_char) {
        unsafe {
            self.storage.get().write(value);
            let redirect = self.location.load(Ordering::Acquire);
            if !redirect.is_null() {
                redirect.write(value);
            }
        }
    }

    /// # Safety
    ///
    /// `target` must address a writable pointer-sized slot that outlives the process.
    unsafe fn redirect(&self, target: *mut *mut c_char) {
        self.location.store(target, Ordering::Release);
    }

    /// Sends reads and writes back to this DLL's own storage. See
    /// [`CopiedInt::clear_redirect`].
    #[cfg(test)]
    fn clear_redirect(&self) {
        self.location
            .store(core::ptr::null_mut(), Ordering::Release);
    }
}

impl Default for CopiedPointer {
    fn default() -> Self {
        Self::new()
    }
}

/// Points one exported variable at the guest's own copy of it.
///
/// Called by the loader for each `R_X86_64_COPY` relocation it resolves against this
/// DLL, after the copy has been performed. `name` is the bare Linux symbol name.
///
/// Returns 1 when the name was recognised and redirected, 0 otherwise. A 0 is not an
/// error: most COPY-able variables need no redirect, because only the ones *both*
/// sides write can diverge.
///
/// # Safety
///
/// `name` must be a NUL-terminated string, and `target` must address writable
/// storage of at least the variable's size that outlives the process — which the
/// guest's `.bss` does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_copied_redirect(
    name: *const c_char,
    target: *mut c_void,
) -> c_int {
    if name.is_null() || target.is_null() {
        return 0;
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let name = unsafe { core::ffi::CStr::from_ptr(name) };
    let Ok(name) = name.to_str() else {
        return 0;
    };

    match name {
        "__libc_stack_end" => {
            unsafe { crate::startup::kinakaze_abi___libc_stack_end.redirect(target.cast()) };
            1
        }
        "optind" => {
            // SAFETY: the caller promises writable `int`-sized storage.
            unsafe { crate::getopt::kinakaze_abi_optind.redirect(target.cast()) };
            1
        }
        "opterr" => {
            // SAFETY: as above.
            unsafe { crate::getopt::kinakaze_abi_opterr.redirect(target.cast()) };
            1
        }
        "optopt" => {
            // SAFETY: as above.
            unsafe { crate::getopt::kinakaze_abi_optopt.redirect(target.cast()) };
            1
        }
        "optarg" => {
            // SAFETY: the caller promises writable pointer-sized storage.
            unsafe { crate::getopt::kinakaze_abi_optarg.redirect(target.cast()) };
            1
        }
        // Both spellings name one variable, and a guest may COPY either.
        "environ" | "__environ" | "_environ" => {
            // SAFETY: as above. `EnvironPointer::redirect` also carries the current
            // value across, because the guest's copy was taken during relocation —
            // before `__libc_start_main` published the real block — and so holds null.
            unsafe { crate::process::kinakaze_abi_environ.redirect(target.cast()) };
            1
        }
        "program_invocation_name" => {
            // SAFETY: the caller promises writable pointer-sized storage.
            unsafe { crate::misc::program_invocation_name.redirect(target.cast()) };
            1
        }
        "program_invocation_short_name" => {
            // SAFETY: as above.
            unsafe { crate::misc::program_invocation_short_name.redirect(target.cast()) };
            1
        }
        "__progname" => {
            // SAFETY: as above.
            unsafe { crate::misc::__progname.redirect(target.cast()) };
            1
        }
        "__progname_full" => {
            // SAFETY: as above.
            unsafe { crate::misc::__progname_full.redirect(target.cast()) };
            1
        }
        "obstack_alloc_failed_handler" => {
            unsafe {
                crate::obstack::kinakaze_abi_obstack_alloc_failed_handler.redirect(target.cast())
            };
            1
        }
        "obstack_exit_failure" => {
            unsafe { crate::obstack::kinakaze_abi_obstack_exit_failure.redirect(target.cast()) };
            1
        }
        "timezone" | "__timezone" => {
            unsafe { crate::time::zone_globals::kinakaze_abi_timezone.redirect(target.cast()) };
            1
        }
        "tzname" | "__tzname" => {
            unsafe { crate::time::zone_globals::kinakaze_abi_tzname.redirect(target.cast()) };
            1
        }
        "daylight" | "__daylight" => {
            unsafe { crate::time::zone_globals::kinakaze_abi_daylight.redirect(target.cast()) };
            1
        }
        _ => i32::from(unsafe { crate::stdio::gnu_error::redirect(name, target) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Before any redirect, the variable is its own storage.
    #[test]
    fn an_unredirected_variable_uses_its_own_storage() {
        let value = CopiedInt::new(7);
        assert_eq!(value.get(), 7);
        value.set(9);
        assert_eq!(value.get(), 9);
    }

    /// After a redirect, reads and writes reach the supplied location — which is the
    /// whole point: the guest's copy becomes authoritative.
    #[test]
    fn a_redirected_variable_reads_and_writes_the_target() {
        let value = CopiedInt::new(1);
        let mut guest: c_int = 42;

        // SAFETY: `guest` is a live, writable `int` for the rest of this test.
        unsafe { value.redirect(&raw mut guest) };

        // The redirect takes effect immediately, so the guest's existing value is
        // what is seen — not the value the variable held before.
        assert_eq!(value.get(), 42);

        value.set(3);
        assert_eq!(guest, 3, "the write must land in the guest's copy");
        assert_eq!(value.get(), 3);
    }

    /// A pointer variable behaves the same way.
    #[test]
    fn a_redirected_pointer_reads_and_writes_the_target() {
        let value = CopiedPointer::new();
        let mut guest: *mut c_char = core::ptr::null_mut();
        // SAFETY: `guest` is a live, writable pointer slot.
        unsafe { value.redirect(&raw mut guest) };

        let mut text = *b"x\0";
        let pointer = text.as_mut_ptr().cast::<c_char>();
        value.set(pointer);
        assert_eq!(guest, pointer, "the write must land in the guest's copy");
        assert_eq!(value.get(), pointer);
    }

    /// The exported entry point recognises the variables it can honour and refuses
    /// anything else, rather than silently accepting a name it cannot serve.
    ///
    /// This test touches the **real exported statics**, which the rest of the suite
    /// also uses, so it has two obligations beyond checking the return value:
    ///
    /// * the storage it redirects to must outlive the test, hence `Box::leak` — a
    ///   stack local left `optind` dangling and killed the suite with an access
    ///   violation several tests later, in a `getopt` test that passes alone
    /// * the redirects must be undone before returning, or every later `getopt` test
    ///   would operate on this test's slot instead of the real variable
    #[test]
    fn the_redirect_entry_point_recognises_the_getopt_variables() {
        // The `getopt` tests share these exact statics, and `cargo test` runs in
        // parallel — so installing a redirect here while a scan is running there
        // would move `optind` out from under it. Taking the same lock is what makes
        // this test safe to run alongside them.
        let _guard = crate::getopt::tests::serialise();

        // Leaked deliberately: a redirect is permanent by design, so the target has
        // to be too. This mirrors the guest's `.bss`, which lives for the process.
        let slot: &'static mut c_int = Box::leak(Box::new(0));
        let slot = &raw mut *slot;

        for name in [c"optind", c"opterr", c"optopt"] {
            // SAFETY: a live NUL-terminated name and leaked, writable `int` storage.
            let result = unsafe { kinakaze_copied_redirect(name.as_ptr(), slot.cast()) };
            assert_eq!(result, 1, "{name:?} should be recognised");
        }

        let pointer: &'static mut *mut c_char = Box::leak(Box::new(core::ptr::null_mut()));
        let pointer = &raw mut *pointer;
        // SAFETY: as above, with leaked pointer-sized storage.
        let result = unsafe { kinakaze_copied_redirect(c"optarg".as_ptr(), pointer.cast()) };
        assert_eq!(result, 1);

        // SAFETY: as above.
        let unknown = unsafe { kinakaze_copied_redirect(c"not_a_variable".as_ptr(), slot.cast()) };
        assert_eq!(unknown, 0, "an unknown name must not claim to be handled");

        // SAFETY: null arguments are explicitly handled.
        assert_eq!(
            unsafe { kinakaze_copied_redirect(core::ptr::null(), slot.cast()) },
            0
        );
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_copied_redirect(c"optind".as_ptr(), core::ptr::null_mut()) },
            0
        );

        // Restore the shared state this test borrowed. Without this the suite still
        // would not crash — the targets are leaked — but every later `getopt` test
        // would read and write this test's slot rather than the real `optind`.
        crate::getopt::kinakaze_abi_optind.clear_redirect();
        crate::getopt::kinakaze_abi_opterr.clear_redirect();
        crate::getopt::kinakaze_abi_optopt.clear_redirect();
        crate::getopt::kinakaze_abi_optarg.clear_redirect();
    }

    /// The exported layout is ABI: the guest's COPY relocation reads `size_of` bytes
    /// from the symbol's address, so the value must be first and the redirect must
    /// not be part of what the guest copies.
    ///
    /// This is why `storage` precedes `location` and why the struct is `repr(C)`. If
    /// the order were reversed the guest would copy a pointer into its `int`.
    #[test]
    fn the_value_is_the_first_field() {
        assert_eq!(core::mem::offset_of!(CopiedInt, storage), 0);
        assert_eq!(core::mem::offset_of!(CopiedPointer, storage), 0);
    }
}
