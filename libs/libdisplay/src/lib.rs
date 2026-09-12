//! `libdisplay.dll`: native Win32 windows for a Linux guest.
//!
//! A guest that wants to put pixels on screen needs a window, and on Linux it
//! would get one from X11 or Wayland. Neither exists here. This library gives it a
//! real `HWND` instead — created, owned and pumped by a Windows thread — and hands
//! the handle over so the guest can build a Vulkan surface directly on it.
//!
//! That is what makes the graphics path native rather than emulated. The swapchain
//! the driver creates is the same one a Windows application would get; there is no
//! X server, no compositor, no blit through an intermediate buffer, and nothing
//! between the guest's rendering and the screen that would not also be there for a
//! native program.
//!
//! ## Why not emulate X11 instead
//!
//! It would let unmodified Linux programs run untouched, which is genuinely
//! valuable, and it is the approach a compatibility layer usually reaches for. It
//! is not what this is, for a reason worth being direct about: implementing enough
//! of the X protocol to satisfy `vkCreateXlibSurfaceKHR` means writing a display
//! server, and the Vulkan driver would still need a real `HWND` underneath to
//! present to. The `Display *` would be a fiction wrapped around the handle this
//! library already exposes.
//!
//! So the handle is exposed directly. Guest code has to be written against this
//! header rather than against Xlib — that is the cost, and it is stated here rather
//! than hidden behind an API that looks like X11 and is not.
//!
//! ## The shape of it
//!
//! * [`kinakaze_display_open`] starts the UI thread. Win32 windows only receive
//!   messages on their creating thread and freeze if it stops pumping, and a guest
//!   never pumps anything — so this library owns a thread that does nothing else.
//!   See [`ui`] for why that is not optional.
//! * [`kinakaze_window_create`] returns a stable opaque id. The current process's
//!   `HWND` is resolved separately and may be rebuilt after `fork`.
//! * [`kinakaze_window_native_handle`] and [`kinakaze_window_native_instance`] give
//!   the `HWND` and `HINSTANCE` that `VkWin32SurfaceCreateInfoKHR` needs.
//! * [`kinakaze_display_poll_event`] drains input, reported with Linux `KEY_*` and
//!   `BTN_*` codes so guest input handling does not have to know it is on Windows.
//!   See [`keycode`] for how that translation is done and why it uses scancodes.
//!
//! The C declarations are in `include/kinakaze/display.h`.

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod clipboard;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod dialog;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod event;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod keycode;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod system;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod tray;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod ui;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod window;

#[cfg(all(windows, target_arch = "x86_64"))]
mod exports {
    use core::ffi::{CStr, c_char, c_int};

    use crate::event::{Event, queue};
    use crate::ui;

    /// Success.
    const OK: c_int = 0;
    /// The display could not be opened, or the call needs one that is not open.
    const FAILED: c_int = -1;

    /// Starts the windowing subsystem.
    ///
    /// Idempotent: the UI thread starts once and later calls report the same state.
    /// Returns 0 on success and -1 if the thread could not be started, in which case
    /// no window can be created — reporting success would hand back a window that
    /// appears and then hangs, because nothing would be pumping it.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_display_open")]
    pub extern "sysv64" fn kinakaze_display_open() -> c_int {
        if ui::available() { OK } else { FAILED }
    }

    /// Stops pumping messages.
    ///
    /// Windows are not destroyed: a guest that wants them gone destroys them, and
    /// tearing them down here would pull a surface out from under a driver that may
    /// still be presenting.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_display_close")]
    pub extern "sysv64" fn kinakaze_display_close() {
        ui::stop();
    }

    /// Creates a window whose *drawable* area is `width` by `height`.
    ///
    /// The size is the client area rather than the frame, because that is what a
    /// swapchain is built from. Returns 0 on failure.
    ///
    /// # Safety
    ///
    /// `title` must be null or a NUL-terminated UTF-8 string.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_create")]
    pub unsafe extern "sysv64" fn kinakaze_window_create(
        width: c_int,
        height: c_int,
        title: *const c_char,
    ) -> u64 {
        if width <= 0 || height <= 0 {
            return 0;
        }
        // SAFETY: the caller promises null or a terminated string.
        let title = unsafe { borrow_title(title) };
        crate::window::create(width, height, &title).unwrap_or(0)
    }

    /// Reads a title argument, falling back to a name that identifies the layer.
    ///
    /// Invalid UTF-8 is replaced rather than rejected: a window title is cosmetic,
    /// and refusing to create a window over a mis-encoded byte would be a worse
    /// answer than showing a replacement character.
    ///
    /// # Safety
    ///
    /// `title` must be null or a NUL-terminated string.
    unsafe fn borrow_title(title: *const c_char) -> String {
        if title.is_null() {
            return "kinakaze".to_owned();
        }
        // SAFETY: the caller promises a terminated string.
        unsafe { CStr::from_ptr(title) }
            .to_string_lossy()
            .into_owned()
    }

    /// Destroys a window. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_destroy")]
    pub extern "sysv64" fn kinakaze_window_destroy(window: u64) -> c_int {
        if window == 0 || !crate::window::destroy(window) {
            return FAILED;
        }
        OK
    }

    /// Shows or hides a window. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_show")]
    pub extern "sysv64" fn kinakaze_window_show(window: u64, visible: c_int) -> c_int {
        if window == 0 || !crate::window::set_visible(window, visible != 0) {
            return FAILED;
        }
        OK
    }

    /// Sets a window's title. Returns 0 on success.
    ///
    /// # Safety
    ///
    /// `title` must be null or a NUL-terminated UTF-8 string.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_set_title")]
    pub unsafe extern "sysv64" fn kinakaze_window_set_title(
        window: u64,
        title: *const c_char,
    ) -> c_int {
        if window == 0 {
            return FAILED;
        }
        // SAFETY: the caller promises null or a terminated string.
        let title = unsafe { borrow_title(title) };
        if crate::window::set_title(window, &title) {
            OK
        } else {
            FAILED
        }
    }

    /// Writes a window's current drawable size.
    ///
    /// Worth calling on every [`crate::event::EVENT_RESIZE`]: it is the size the
    /// swapchain has to be rebuilt at, and reading it from the window rather than
    /// trusting the event means a burst of resizes cannot leave the two disagreeing.
    ///
    /// # Safety
    ///
    /// `width` and `height` must be null or point to writable `int`s.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_size")]
    pub unsafe extern "sysv64" fn kinakaze_window_size(
        window: u64,
        width: *mut c_int,
        height: *mut c_int,
    ) -> c_int {
        let Some((client_width, client_height)) = crate::window::client_size(window) else {
            return FAILED;
        };
        if !width.is_null() {
            // SAFETY: checked non-null; the caller promises it is writable.
            unsafe { width.write(client_width) };
        }
        if !height.is_null() {
            // SAFETY: checked non-null; the caller promises it is writable.
            unsafe { height.write(client_height) };
        }
        OK
    }

    /// The window's `HWND`, for `VkWin32SurfaceCreateInfoKHR::hwnd`.
    ///
    /// Resolves a stable guest id to the current process's `HWND`.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_native_handle")]
    pub extern "sysv64" fn kinakaze_window_native_handle(window: u64) -> u64 {
        crate::window::native_handle(window).unwrap_or(0) as u64
    }

    /// The `HINSTANCE`, for `VkWin32SurfaceCreateInfoKHR::hinstance`.
    ///
    /// Vulkan requires both, and surface creation fails with a null instance.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_window_native_instance")]
    pub extern "sysv64" fn kinakaze_window_native_instance() -> u64 {
        ui::module_instance() as u64
    }

    /// Takes the next event, if any.
    ///
    /// Returns 1 when an event was written, 0 when none was waiting, and -1 on a bad
    /// argument. Non-blocking: a guest driving a render loop must not be stalled by
    /// its input path.
    ///
    /// # Safety
    ///
    /// `event` must point to writable storage for one [`Event`].
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_display_poll_event")]
    pub unsafe extern "sysv64" fn kinakaze_display_poll_event(event: *mut Event) -> c_int {
        if event.is_null() {
            return FAILED;
        }
        let mut queue = match queue().lock() {
            Ok(queue) => queue,
            // Recovered rather than propagated: a panic must not cross back into
            // guest C, which has no way to catch it.
            Err(poisoned) => poisoned.into_inner(),
        };
        match queue.pop() {
            Some(next) => {
                // SAFETY: checked non-null; the caller promises room for one event.
                unsafe { event.write(next) };
                1
            }
            None => 0,
        }
    }

    /// How many events are waiting.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_display_pending_events")]
    pub extern "sysv64" fn kinakaze_display_pending_events() -> u64 {
        match queue().lock() {
            Ok(queue) => queue.len() as u64,
            Err(poisoned) => poisoned.into_inner().len() as u64,
        }
    }

    /// How many events overflow discarded since this was last called.
    ///
    /// Reported rather than hidden. A guest that stopped polling long enough to
    /// overflow the queue has a gap in its input history, and one that knows the
    /// size of the gap can resynchronise — re-read the window size, treat held keys
    /// as released — while one told nothing cannot.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_display_dropped_events")]
    pub extern "sysv64" fn kinakaze_display_dropped_events() -> u64 {
        match queue().lock() {
            Ok(mut queue) => queue.take_dropped(),
            Err(poisoned) => poisoned.into_inner().take_dropped(),
        }
    }

    /// Reads UTF-8 text from the Win32 clipboard.
    ///
    /// Returns the length of the string on success, or -1 on error/empty.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_clipboard_get_text")]
    pub unsafe extern "sysv64" fn kinakaze_clipboard_get_text(
        buffer: *mut c_char,
        max_len: usize,
    ) -> isize {
        crate::clipboard::get_text(buffer, max_len)
    }

    /// Sets UTF-8 text to the Win32 clipboard. Returns 0 on success, -1 on failure.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_clipboard_set_text")]
    pub unsafe extern "sysv64" fn kinakaze_clipboard_set_text(text: *const c_char) -> c_int {
        crate::clipboard::set_text(text)
    }

    /// Clears the Win32 clipboard. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_clipboard_clear")]
    pub extern "sysv64" fn kinakaze_clipboard_clear() -> c_int {
        crate::clipboard::clear()
    }

    /// Creates a system tray icon associated with a window. Returns tray_id or 0.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_tray_create")]
    pub unsafe extern "sysv64" fn kinakaze_tray_create(window: u64, tooltip: *const c_char) -> u64 {
        let tip = if tooltip.is_null() {
            "kinakaze"
        } else {
            match unsafe { CStr::from_ptr(tooltip) }.to_str() {
                Ok(s) => s,
                Err(_) => "kinakaze",
            }
        };
        crate::tray::create(window, tip).unwrap_or(0)
    }

    /// Updates the system tray icon tooltip. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_tray_set_tooltip")]
    pub unsafe extern "sysv64" fn kinakaze_tray_set_tooltip(
        tray_id: u64,
        tooltip: *const c_char,
    ) -> c_int {
        if tooltip.is_null() {
            return FAILED;
        }
        let tip = match unsafe { CStr::from_ptr(tooltip) }.to_str() {
            Ok(s) => s,
            Err(_) => return FAILED,
        };
        if crate::tray::set_tooltip(tray_id, tip) {
            OK
        } else {
            FAILED
        }
    }

    /// Shows a system tray notification balloon. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_tray_show_balloon")]
    pub unsafe extern "sysv64" fn kinakaze_tray_show_balloon(
        tray_id: u64,
        title: *const c_char,
        message: *const c_char,
        timeout_ms: u32,
    ) -> c_int {
        let t = if title.is_null() {
            ""
        } else {
            match unsafe { CStr::from_ptr(title) }.to_str() {
                Ok(s) => s,
                Err(_) => return FAILED,
            }
        };
        let m = if message.is_null() {
            ""
        } else {
            match unsafe { CStr::from_ptr(message) }.to_str() {
                Ok(s) => s,
                Err(_) => return FAILED,
            }
        };
        if crate::tray::show_balloon(tray_id, t, m, timeout_ms) {
            OK
        } else {
            FAILED
        }
    }

    /// Destroys a system tray icon. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_tray_destroy")]
    pub extern "sysv64" fn kinakaze_tray_destroy(tray_id: u64) -> c_int {
        if crate::tray::destroy(tray_id) {
            OK
        } else {
            FAILED
        }
    }

    /// Sets cursor visibility. Returns cursor display counter.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_cursor_set_visible")]
    pub extern "sysv64" fn kinakaze_cursor_set_visible(visible: c_int) -> c_int {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn ShowCursor(bShow: i32) -> i32;
        }
        unsafe { ShowCursor(if visible != 0 { 1 } else { 0 }) }
    }

    /// Sets cursor position in screen coordinates. Returns 0 on success.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_cursor_set_pos")]
    pub extern "sysv64" fn kinakaze_cursor_set_pos(x: c_int, y: c_int) -> c_int {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetCursorPos(X: i32, Y: i32) -> i32;
        }
        if unsafe { SetCursorPos(x, y) } != 0 {
            OK
        } else {
            FAILED
        }
    }

    /// Opens a native Win32 file picker dialog. Returns path length or -1.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_dialog_open_file")]
    pub unsafe extern "sysv64" fn kinakaze_dialog_open_file(
        title: *const c_char,
        filter: *const c_char,
        buffer: *mut c_char,
        max_len: usize,
    ) -> isize {
        let t = if title.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(title) }.to_str().ok()
        };
        let f = if filter.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(filter) }.to_str().ok()
        };
        crate::dialog::open_file(t, f, buffer, max_len)
    }

    /// Opens a native Win32 save file dialog. Returns path length or -1.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_dialog_save_file")]
    pub unsafe extern "sysv64" fn kinakaze_dialog_save_file(
        title: *const c_char,
        filter: *const c_char,
        default_name: *const c_char,
        buffer: *mut c_char,
        max_len: usize,
    ) -> isize {
        let t = if title.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(title) }.to_str().ok()
        };
        let f = if filter.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(filter) }.to_str().ok()
        };
        let n = if default_name.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(default_name) }.to_str().ok()
        };
        crate::dialog::save_file(t, f, n, buffer, max_len)
    }

    /// Shows a Win32 message box. Returns clicked button ID (1=OK, 2=Cancel, 6=Yes, 7=No).
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_dialog_message")]
    pub unsafe extern "sysv64" fn kinakaze_dialog_message(
        title: *const c_char,
        message: *const c_char,
        flags: u32,
    ) -> c_int {
        let t = if title.is_null() {
            "Message"
        } else {
            match unsafe { CStr::from_ptr(title) }.to_str() {
                Ok(s) => s,
                Err(_) => "Message",
            }
        };
        let m = if message.is_null() {
            ""
        } else {
            match unsafe { CStr::from_ptr(message) }.to_str() {
                Ok(s) => s,
                Err(_) => "",
            }
        };
        crate::dialog::message_box(t, m, flags)
    }

    /// Returns 1 if Windows is currently in Dark Mode, 0 if Light Mode.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_system_is_dark_mode")]
    pub extern "sysv64" fn kinakaze_system_is_dark_mode() -> c_int {
        if crate::system::is_dark_mode() { 1 } else { 0 }
    }

    /// Gets DPI for a window or primary display (e.g. 96, 120, 144, 192).
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_system_get_dpi")]
    pub extern "sysv64" fn kinakaze_system_get_dpi(window: u64) -> u32 {
        crate::system::get_dpi(window)
    }

    /// Gets DPI scaling factor (e.g. 1.0, 1.25, 1.5, 2.0).
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_system_get_scale_factor")]
    pub extern "sysv64" fn kinakaze_system_get_scale_factor(window: u64) -> core::ffi::c_float {
        crate::system::get_scale_factor(window)
    }

    /// Gets system accent color (0x00RRGGBB).
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_system_get_accent_color")]
    pub extern "sysv64" fn kinakaze_system_get_accent_color() -> u32 {
        crate::system::get_accent_color()
    }

    /// Prevents or restores system and display sleep/screensaver.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_power_prevent_sleep")]
    pub extern "sysv64" fn kinakaze_power_prevent_sleep(enable: c_int) {
        crate::system::prevent_sleep(enable != 0);
    }

    /// Opens a URL or file path using the default Windows application.
    #[unsafe(export_name = "kinakaze_engine_libdisplay_kinakaze_shell_open")]
    pub unsafe extern "sysv64" fn kinakaze_shell_open(target: *const c_char) -> c_int {
        if target.is_null() {
            return FAILED;
        }
        let s = match unsafe { CStr::from_ptr(target) }.to_str() {
            Ok(s) => s,
            Err(_) => return FAILED,
        };
        if crate::system::shell_open(s) {
            OK
        } else {
            FAILED
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub use exports::*;

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;

    fn clipboard_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    use crate::event::Event;
    use std::ffi::{CStr, CString, c_char};

    /// The C entry points, exercised as a guest would call them: open, create, ask
    /// for the native handles, resize-query, destroy.
    #[test]
    fn the_c_surface_creates_and_describes_a_window() {
        assert_eq!(kinakaze_display_open(), 0);

        let title = CString::new("kinakaze abi test").expect("no interior NUL");
        // SAFETY: `title` is a live NUL-terminated string.
        let window = unsafe { kinakaze_window_create(800, 600, title.as_ptr()) };
        assert_ne!(window, 0, "window creation should succeed");

        // The two values a Vulkan Win32 surface is built from. Both must be real, or
        // `vkCreateWin32SurfaceKHR` fails.
        assert_ne!(kinakaze_window_native_handle(window), 0);
        assert_ne!(kinakaze_window_native_instance(), 0);

        let mut width = 0;
        let mut height = 0;
        // SAFETY: both are live, writable ints.
        let result = unsafe { kinakaze_window_size(window, &raw mut width, &raw mut height) };
        assert_eq!(result, 0);
        assert_eq!(
            (width, height),
            (800, 600),
            "the drawable area is what was asked for"
        );

        assert_eq!(kinakaze_window_destroy(window), 0);
    }

    /// A null title is legal and must not crash; nonsensical sizes must be refused
    /// rather than passed to Win32.
    #[test]
    fn bad_arguments_are_refused() {
        assert_eq!(kinakaze_display_open(), 0);
        // SAFETY: a null title is explicitly permitted.
        let window = unsafe { kinakaze_window_create(100, 100, core::ptr::null()) };
        assert_ne!(window, 0, "a null title should be defaulted, not rejected");
        assert_eq!(kinakaze_window_destroy(window), 0);

        // SAFETY: null title again; the sizes are what is under test.
        assert_eq!(
            unsafe { kinakaze_window_create(0, 100, core::ptr::null()) },
            0
        );
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_window_create(100, -1, core::ptr::null()) },
            0
        );
        assert_eq!(kinakaze_window_destroy(0), -1);
    }

    /// Polling must be non-blocking and must tolerate a null argument, since it sits
    /// in a render loop where a stall or a crash is immediately visible.
    #[test]
    fn polling_an_empty_queue_returns_immediately() {
        let _queue_test = crate::event::test_queue_lock();
        // Drain whatever earlier tests left behind.
        let mut event = Event::default();
        // SAFETY: `event` is live and writable.
        while unsafe { kinakaze_display_poll_event(&raw mut event) } == 1 {}

        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_display_poll_event(&raw mut event) }, 0);
        assert_eq!(kinakaze_display_pending_events(), 0);
        // SAFETY: null is explicitly handled.
        assert_eq!(
            unsafe { kinakaze_display_poll_event(core::ptr::null_mut()) },
            -1
        );
    }

    /// A published event reaches the guest with its fields intact. Synthesised
    /// rather than driven by real input, because a test cannot press a key.
    #[test]
    fn a_published_event_is_polled_back() {
        let _queue_test = crate::event::test_queue_lock();
        let mut drain = Event::default();
        // SAFETY: `drain` is live and writable.
        while unsafe { kinakaze_display_poll_event(&raw mut drain) } == 1 {}

        crate::event::publish(Event {
            kind: crate::event::EVENT_KEY,
            state: crate::event::STATE_PRESSED,
            // KEY_A, which is scancode 0x1E — see `keycode`.
            keycode: 30,
            ..Event::default()
        });

        let mut event = Event::default();
        // SAFETY: `event` is live and writable.
        assert_eq!(unsafe { kinakaze_display_poll_event(&raw mut event) }, 1);
        assert_eq!(event.kind, crate::event::EVENT_KEY);
        assert_eq!(event.keycode, 30);
        assert_eq!(event.state, crate::event::STATE_PRESSED);
    }

    #[test]
    fn clipboard_set_and_get_text_roundtrip() {
        let _clipboard_test = clipboard_test_lock();
        let test_text = CString::new("Hello from Kinakaze!").unwrap();
        assert_eq!(
            unsafe { kinakaze_clipboard_set_text(test_text.as_ptr()) },
            0
        );

        let mut buf = [0u8; 128];
        let len =
            unsafe { kinakaze_clipboard_get_text(buf.as_mut_ptr() as *mut c_char, buf.len()) };
        assert!(len > 0);
        let s = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) }
            .to_str()
            .unwrap();
        assert_eq!(s, "Hello from Kinakaze!");
    }

    #[test]
    fn cursor_and_tray_helpers() {
        let _clipboard_test = clipboard_test_lock();
        let _ = kinakaze_cursor_set_visible(1);
        let _ = kinakaze_cursor_set_pos(100, 100);
        assert_eq!(kinakaze_clipboard_clear(), 0);
    }

    #[test]
    fn system_integration_helpers() {
        let _dark = kinakaze_system_is_dark_mode();
        let dpi = kinakaze_system_get_dpi(0);
        assert!(dpi >= 96);
        let scale = kinakaze_system_get_scale_factor(0);
        assert!(scale >= 1.0);
        let _color = kinakaze_system_get_accent_color();
        kinakaze_power_prevent_sleep(0);
    }
}

mod object_layout;
