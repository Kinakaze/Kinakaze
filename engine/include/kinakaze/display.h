#ifndef KINAKAZE_DISPLAY_H
#define KINAKAZE_DISPLAY_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * libdisplay: a native Win32 window, for a Linux guest.
 *
 * A guest that wants to draw needs a window. On Linux it would ask X11 or Wayland;
 * neither exists here. This gives it a real HWND instead, and hands the handle
 * over so a Vulkan surface can be built directly on it. The swapchain the driver
 * creates is the same one a native Windows program would get: no display server,
 * no compositor, no intermediate blit.
 *
 * The cost is stated plainly: this is not an Xlib-shaped API, so guest code has to
 * be written against this header rather than ported unchanged. Implementing enough
 * X protocol to satisfy vkCreateXlibSurfaceKHR would mean writing a display
 * server, and the driver would still need an HWND underneath — the Display* would
 * be a fiction wrapped around the handle below.
 *
 * Input is reported with Linux codes throughout: KEY_* from
 * linux/input-event-codes.h for keys, BTN_* for pointer buttons, wheel movement in
 * notches rather than Windows' 120ths. Guest input handling therefore does not have
 * to know which platform it is on. The translation goes through the hardware
 * scancode rather than the Windows virtual-key code, because virtual keys are
 * layout-dependent and would mistranslate every non-US keyboard.
 *
 * Threading: every function here may be called from any thread. Windows are owned
 * by a thread this library runs, because a Win32 window only receives messages on
 * its creating thread and freezes if that thread stops pumping — which a guest,
 * blocked on a fence in a render loop, would certainly do.
 */

/* ------------------------------------------------------------------------- */
/* Events                                                                     */
/* ------------------------------------------------------------------------- */

#define KINAKAZE_EVENT_NONE 0
/*
 * The user asked to close the window. The window is NOT destroyed: the guest
 * decides whether to honour it, as an X11 client decides what to do with
 * WM_DELETE_WINDOW. Ignoring this leaves the window open.
 */
#define KINAKAZE_EVENT_CLOSE 1
/* The drawable size changed; x and y carry the new width and height. */
#define KINAKAZE_EVENT_RESIZE 2
/* A key changed state; keycode is a Linux KEY_* value. */
#define KINAKAZE_EVENT_KEY 3
/* The pointer moved; x and y are client-area coordinates and may be negative. */
#define KINAKAZE_EVENT_POINTER_MOTION 4
/* A pointer button changed state; button is a Linux BTN_* value. */
#define KINAKAZE_EVENT_POINTER_BUTTON 5
/* The wheel turned; scroll_x and scroll_y are in notches. */
#define KINAKAZE_EVENT_SCROLL 6
/* Keyboard focus changed; state is 1 for gained, 0 for lost. */
#define KINAKAZE_EVENT_FOCUS 7
/* The child rebuilt its HWND after fork; recreate native graphics surfaces. */
#define KINAKAZE_EVENT_REBOUND 8
/* Raw keyboard input for XInput2 bridges; keycode and state are populated. */
#define KINAKAZE_EVENT_RAW_KEY 9
/* Raw pointer-button input for XInput2 bridges; button and state are populated. */
#define KINAKAZE_EVENT_RAW_BUTTON 10
/* Unaccelerated relative motion for XInput2 bridges; x and y are deltas. */
#define KINAKAZE_EVENT_RAW_MOTION 11
/* Window moved; x/y are the client origin in parent coordinates. */
#define KINAKAZE_EVENT_MOVE 15
/* Window minimized (state 1) or restored from minimized (state 0). */
#define KINAKAZE_EVENT_ICONIC 16
/* Pointer entered or left the client area; x and y are client coordinates. */
#define KINAKAZE_EVENT_POINTER_ENTER 17
#define KINAKAZE_EVENT_POINTER_LEAVE 18
/* Window maximized (state 1) or no longer maximized (state 0). */
#define KINAKAZE_EVENT_MAXIMIZED 19
/* Modifier snapshot; state contains X11 modifier bits. */
#define KINAKAZE_EVENT_MODIFIERS 20
/* Paint damage; x/y and scroll_x/scroll_y give its rectangle. */
#define KINAKAZE_EVENT_EXPOSE 21
/* Native sibling order changed without a geometry change. */
#define KINAKAZE_EVENT_STACK 22

#define KINAKAZE_STATE_RELEASED 0
#define KINAKAZE_STATE_PRESSED 1
/*
 * An auto-repeated key. Linux reports a repeat as value 2 on the same event code,
 * which lets a guest tell a held key from a rapid retap; the distinction survives
 * translation rather than being flattened into a second press.
 */
#define KINAKAZE_STATE_REPEAT 2

/* Linux BTN_* values, not Windows button numbers. */
#define KINAKAZE_BTN_LEFT 0x110
#define KINAKAZE_BTN_RIGHT 0x111
#define KINAKAZE_BTN_MIDDLE 0x112
#define KINAKAZE_BTN_SIDE 0x113
#define KINAKAZE_BTN_EXTRA 0x114

/*
 * One input event. A flat record rather than a union: a union would save 16 bytes
 * and cost every reader a discriminant check C cannot verify, and events are
 * polled rather than streamed in bulk.
 *
 * The layout is ABI. Do not reorder.
 */
struct kinakaze_event {
    uint32_t kind;     /* KINAKAZE_EVENT_*                                  */
    uint32_t state;    /* KINAKAZE_STATE_*, for key, button and focus       */
    uint16_t keycode;  /* Linux KEY_*, for KINAKAZE_EVENT_KEY               */
    uint16_t button;   /* Linux BTN_*, for KINAKAZE_EVENT_POINTER_BUTTON    */
    int32_t x;         /* pointer x, or width for a resize                  */
    int32_t y;         /* pointer y, or height for a resize                 */
    int32_t scroll_x;  /* horizontal notches, positive rightward            */
    int32_t scroll_y;  /* vertical notches, positive away from the user      */
    uint64_t time_ms;  /* milliseconds from a monotonic source              */
    uint64_t window;   /* stable window id; the HWND may change after fork  */
};

/* ------------------------------------------------------------------------- */
/* Lifecycle                                                                  */
/* ------------------------------------------------------------------------- */

/*
 * Starts the windowing subsystem. Returns 0 on success, -1 if it could not start.
 *
 * Idempotent. A -1 means no window can be created: reporting success would hand
 * back a window that appears and then hangs, because nothing would pump it.
 */
int kinakaze_display_open(void);

/*
 * Stops pumping messages. Windows are not destroyed — destroy them first if that
 * is what you want, since tearing them down here could pull a surface out from
 * under a driver that is still presenting.
 */
void kinakaze_display_close(void);

/* ------------------------------------------------------------------------- */
/* Windows                                                                    */
/* ------------------------------------------------------------------------- */

/*
 * Creates a window whose DRAWABLE area is width by height, and returns an opaque
 * id, or 0 on failure.
 *
 * The size is the client area rather than the outer frame, because that is what a
 * swapchain is built from. A request for 800x600 yields an 800x600 drawable.
 *
 * `title` may be NULL. The window is created visible.
 */
uint64_t kinakaze_window_create(int width, int height, const char *title);

/* Destroys a window. Returns 0 on success. */
int kinakaze_window_destroy(uint64_t window);

/* Shows (visible non-zero) or hides a window. Returns 0 on success. */
int kinakaze_window_show(uint64_t window, int visible);

/* Sets a window's title. `title` may be NULL. Returns 0 on success. */
int kinakaze_window_set_title(uint64_t window, const char *title);

/*
 * Writes the current drawable size. Either pointer may be NULL. Returns 0 on
 * success, -1 if the window is gone.
 *
 * Worth calling on every KINAKAZE_EVENT_RESIZE rather than trusting the event's
 * own x and y: it is the size the swapchain must be rebuilt at, and reading it
 * from the window means a burst of resizes cannot leave the two disagreeing.
 */
int kinakaze_window_size(uint64_t window, int *width, int *height);

/* ------------------------------------------------------------------------- */
/* Native handles, for Vulkan                                                 */
/* ------------------------------------------------------------------------- */

/*
 * The HWND, for VkWin32SurfaceCreateInfoKHR::hwnd.
 *
 * The opaque id is stable across fork. The child rebuilds its process-local HWND,
 * so always resolve it here rather than caching the native value.
 *
 * Together they are the whole of the native binding:
 *
 *   VkWin32SurfaceCreateInfoKHR info = {
 *       .sType     = VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR,
 *       .hinstance = (void *)kinakaze_window_native_instance(),
 *       .hwnd      = (void *)kinakaze_window_native_handle(window),
 *   };
 *   vkCreateWin32SurfaceKHR(instance, &info, NULL, &surface);
 *
 * The instance must ask for VK_KHR_win32_surface — not VK_KHR_xlib_surface, which
 * the host loader does not have and which nothing here emulates.
 */
uint64_t kinakaze_window_native_handle(uint64_t window);

/* The HINSTANCE. Vulkan requires it; a null one fails surface creation. */
uint64_t kinakaze_window_native_instance(void);

/* ------------------------------------------------------------------------- */
/* Input                                                                      */
/* ------------------------------------------------------------------------- */

/*
 * Takes the next event. Returns 1 when one was written, 0 when none was waiting,
 * -1 on a NULL argument. Never blocks.
 */
int kinakaze_display_poll_event(struct kinakaze_event *event);

/* How many events are waiting. */
uint64_t kinakaze_display_pending_events(void);

/*
 * How many events overflow discarded since this was last called, then resets.
 *
 * The queue is bounded, so a guest that stops polling long enough will lose
 * events. That is reported rather than hidden: a guest which knows the size of the
 * gap can resynchronise — re-read the window size, treat held keys as released —
 * while one told nothing cannot. A non-zero result means the input history has a
 * hole in it, not that anything is broken.
 */
uint64_t kinakaze_display_dropped_events(void);

/* ------------------------------------------------------------------------- */
/* Clipboard                                                                  */
/* ------------------------------------------------------------------------- */

/* Reads UTF-8 text from clipboard. Returns length, or -1 on error/empty. */
int64_t kinakaze_clipboard_get_text(char *buffer, size_t max_len);

/* Writes UTF-8 text to clipboard. Returns 0 on success. */
int kinakaze_clipboard_set_text(const char *text);

/* Clears clipboard. Returns 0 on success. */
int kinakaze_clipboard_clear(void);

/* ------------------------------------------------------------------------- */
/* System Tray                                                                */
/* ------------------------------------------------------------------------- */

/* Creates a system tray icon for a window. Returns tray_id or 0 on failure. */
uint64_t kinakaze_tray_create(uint64_t window, const char *tooltip);

/* Updates tooltip text. Returns 0 on success. */
int kinakaze_tray_set_tooltip(uint64_t tray_id, const char *tooltip);

/* Shows notification balloon. Returns 0 on success. */
int kinakaze_tray_show_balloon(uint64_t tray_id, const char *title, const char *message, uint32_t timeout_ms);

/* Destroys a tray icon. Returns 0 on success. */
int kinakaze_tray_destroy(uint64_t tray_id);

/* ------------------------------------------------------------------------- */
/* Cursor                                                                     */
/* ------------------------------------------------------------------------- */

/* Sets cursor visibility (1 to show, 0 to hide). */
int kinakaze_cursor_set_visible(int visible);

/* Sets cursor screen coordinates. Returns 0 on success. */
int kinakaze_cursor_set_pos(int x, int y);

/* ------------------------------------------------------------------------- */
/* Native Dialogs                                                             */
/* ------------------------------------------------------------------------- */

/* Opens native file picker dialog. Returns path length or -1. */
int64_t kinakaze_dialog_open_file(const char *title, const char *filter, char *buffer, size_t max_len);

/* Opens native save file dialog. Returns path length or -1. */
int64_t kinakaze_dialog_save_file(const char *title, const char *filter, const char *default_name, char *buffer, size_t max_len);

/* Shows native message box. Returns button ID (1=OK, 2=Cancel, 6=Yes, 7=No). */
int kinakaze_dialog_message(const char *title, const char *message, uint32_t flags);

/* ------------------------------------------------------------------------- */
/* System Integration                                                         */
/* ------------------------------------------------------------------------- */

/* Returns 1 if Windows is in Dark Mode, 0 if Light Mode. */
int kinakaze_system_is_dark_mode(void);

/* Returns DPI (e.g. 96, 120, 144, 192). */
uint32_t kinakaze_system_get_dpi(uint64_t window);

/* Returns DPI scaling factor (e.g. 1.0, 1.25, 1.5, 2.0). */
float kinakaze_system_get_scale_factor(uint64_t window);

/* Returns system accent color (0x00RRGGBB). */
uint32_t kinakaze_system_get_accent_color(void);

/* Prevents (enable=1) or restores (enable=0) system/display sleep. */
void kinakaze_power_prevent_sleep(int enable);

/* Opens a URL or file with default Windows host application. Returns 0 on success. */
int kinakaze_shell_open(const char *target);

#ifdef __cplusplus
}
#endif

#endif /* KINAKAZE_DISPLAY_H */
