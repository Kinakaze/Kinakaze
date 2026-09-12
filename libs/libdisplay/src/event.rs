//! The event record the guest reads, and the queue it is read from.
//!
//! Events are reported with Linux input codes throughout — `KEY_*` for keys,
//! `BTN_*` for pointer buttons, wheel notches rather than Windows' 120ths. See
//! [`crate::keycode`] for why translating rather than passing Windows values
//! through is the right call.
//!
//! ## Why a queue at all
//!
//! A Win32 window only receives messages on the thread that created it, and only
//! while that thread is pumping. The guest is not that thread and does not pump:
//! it calls a poll function whenever it likes, from wherever it likes. So the UI
//! thread translates messages as they arrive and appends them here, and the guest
//! drains at its own pace. That decoupling is the reason this library exists as
//! something more than a thin `CreateWindowExW` wrapper.
//!
//! The queue is bounded. A guest that stops polling — because it is busy, or
//! blocked, or has simply stopped caring — would otherwise grow this without limit
//! while the user moves the mouse. When it overflows the *oldest* events are
//! dropped and counted, and the count is reported to the guest rather than hidden:
//! a guest that sees a gap can resynchronise, while one that is told nothing
//! cannot.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// A provider-owned wake callback. Xlib uses this to make its Linux connection
/// fd readable whenever the Win32 UI thread appends an event.
pub type EventNotifier = unsafe extern "system" fn();

fn notifiers() -> &'static Mutex<Vec<EventNotifier>> {
    static NOTIFIERS: OnceLock<Mutex<Vec<EventNotifier>>> = OnceLock::new();
    NOTIFIERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Registers an idempotent process-lifetime notification callback.
pub fn register_notifier(notifier: EventNotifier) {
    let Ok(mut callbacks) = notifiers().lock() else {
        return;
    };
    if !callbacks
        .iter()
        .any(|existing| *existing as usize == notifier as usize)
    {
        callbacks.push(notifier);
    }
}

/// Removes a provider callback before the provider DLL can be unloaded.
pub fn unregister_notifier(notifier: EventNotifier) {
    if let Ok(mut callbacks) = notifiers().lock() {
        callbacks.retain(|existing| *existing as usize != notifier as usize);
    }
}

/// No event was available.
pub const EVENT_NONE: u32 = 0;
/// The user asked to close the window. The window is *not* destroyed: the guest
/// decides whether to honour it, which is what a Linux `WM_DELETE_WINDOW` client
/// message leaves open too.
pub const EVENT_CLOSE: u32 = 1;
/// The drawable size changed. `x` and `y` carry the new width and height.
pub const EVENT_RESIZE: u32 = 2;
/// A key changed state. `keycode` is a Linux `KEY_*` value.
pub const EVENT_KEY: u32 = 3;
/// The pointer moved. `x` and `y` are client-area coordinates.
pub const EVENT_POINTER_MOTION: u32 = 4;
/// A pointer button changed state. `button` is a Linux `BTN_*` value.
pub const EVENT_POINTER_BUTTON: u32 = 5;
/// The wheel turned. `scroll_x`/`scroll_y` are in notches, not Windows' 120ths.
pub const EVENT_SCROLL: u32 = 6;
/// Keyboard focus was gained or lost. `state` is 1 for gained.
pub const EVENT_FOCUS: u32 = 7;
/// The child rebuilt the process-local HWND after fork. Existing native graphics
/// surfaces are invalid and must be recreated from the stable window id.
pub const EVENT_REBOUND: u32 = 8;
/// Raw keyboard input from Win32 Raw Input, for XInput2 consumers.
pub const EVENT_RAW_KEY: u32 = 9;
/// Raw pointer-button input from Win32 Raw Input, for XInput2 consumers.
pub const EVENT_RAW_BUTTON: u32 = 10;
/// Unaccelerated relative pointer motion. `x` and `y` carry deltas.
pub const EVENT_RAW_MOTION: u32 = 11;
/// Native touch contacts; touch_id identifies one begin/update/end sequence.
pub const EVENT_TOUCH_BEGIN: u32 = 12;
pub const EVENT_TOUCH_UPDATE: u32 = 13;
pub const EVENT_TOUCH_END: u32 = 14;
/// The window moved; x/y are its client origin in parent coordinates.
pub const EVENT_MOVE: u32 = 15;
/// The window was minimized (`state` 1) or restored from minimized (`state` 0).
/// A minimized X11 window is unmapped from the client's point of view.
pub const EVENT_ICONIC: u32 = 16;
/// The pointer entered (`EVENT_POINTER_ENTER`) or left (`EVENT_POINTER_LEAVE`)
/// the window's client area; `x`/`y` are client coordinates.
/// `state` is the X crossing mode (0 normal, 1 grab, 2 ungrab). For grab
/// transitions, `button` carries the current XI button bitmask.
pub const EVENT_POINTER_ENTER: u32 = 17;
pub const EVENT_POINTER_LEAVE: u32 = 18;
/// The window was maximized (`state` 1) or left the maximized state (`state` 0).
pub const EVENT_MAXIMIZED: u32 = 19;
/// Modifier snapshot immediately preceding a native input record. `state`
/// contains X11 Shift/Lock/Control/Mod1..Mod5 bits. A separate record preserves
/// the public Event ABI while retaining state across delayed queue draining.
pub const EVENT_MODIFIERS: u32 = 20;
/// Native damage to the displayed window, independent of a size change.
/// x/y are the invalid rectangle origin; scroll_x/scroll_y carry its extent.
/// `state` is 1 when WM_PAINT already restored the retained frame.
pub const EVENT_EXPOSE: u32 = 21;
/// Native sibling order changed, independently of geometry.
pub const EVENT_STACK: u32 = 22;

/// `state` for a key or button that was released.
pub const STATE_RELEASED: u32 = 0;
/// `state` for a key or button that was pressed.
pub const STATE_PRESSED: u32 = 1;
/// `state` for an auto-repeated key.
///
/// Linux reports a repeat as value 2 on the same event code, which lets a guest
/// distinguish a held key from a rapid retap. Windows signals it in a `lParam` bit,
/// so the distinction survives the translation rather than being flattened.
pub const STATE_REPEAT: u32 = 2;

/// Linux `BTN_LEFT`.
pub const BTN_LEFT: u16 = 0x110;
/// Linux `BTN_RIGHT`.
pub const BTN_RIGHT: u16 = 0x111;
/// Linux `BTN_MIDDLE`.
pub const BTN_MIDDLE: u16 = 0x112;
/// Linux `BTN_SIDE`, the first extra button.
pub const BTN_SIDE: u16 = 0x113;
/// Linux `BTN_EXTRA`, the second extra button.
pub const BTN_EXTRA: u16 = 0x114;

/// One input event, as the guest sees it.
///
/// The layout is this library's own ABI, declared in
/// `include/kinakaze/display.h`. It is a flat record rather than a union because a
/// union would save 16 bytes and cost every reader a discriminant check that C
/// cannot verify — and events are polled, not streamed in bulk, so the size does
/// not matter.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// One of the `EVENT_*` constants.
    pub kind: u32,
    /// One of the `STATE_*` constants, for key, button and focus events.
    pub state: u32,
    /// A Linux `KEY_*` code, for [`EVENT_KEY`].
    pub keycode: u16,
    /// A Linux `BTN_*` code, for [`EVENT_POINTER_BUTTON`].
    pub button: u16,
    /// Pointer x, or width for [`EVENT_RESIZE`].
    pub x: i32,
    /// Pointer y, or height for [`EVENT_RESIZE`].
    pub y: i32,
    /// Horizontal wheel notches, positive rightward.
    pub scroll_x: i32,
    /// Vertical wheel notches, positive away from the user.
    pub scroll_y: i32,
    /// Contact identifier for touch events. Uses the existing ABI padding.
    pub touch_id: u32,
    /// X server time: milliseconds since boot, wrapping at 32 bits. Focus,
    /// grabs and input events must use the same clock across guest processes.
    pub time_ms: u64,
    /// Stable guest window id.  The HWND behind it may be rebuilt after fork.
    pub window: u64,
}

/// How many events may be held before the oldest are dropped.
///
/// A thousand is far more than a responsive guest accumulates between polls, and
/// far less than enough to matter as memory. It is a backstop against a guest that
/// has stopped polling, not a buffer sized for throughput.
pub const CAPACITY: usize = 1024;

/// The pending events, and a count of what overflow discarded.
#[derive(Default)]
pub struct Queue {
    events: VecDeque<Event>,
    /// Events dropped to overflow since the last time the guest was told.
    ///
    /// Reset when read, so the number always answers "since you last asked".
    dropped: u64,
}

impl Queue {
    /// Appends an event, discarding the oldest if the queue is full.
    ///
    /// The oldest goes rather than the newest because current state is what a guest
    /// acts on: dropping the newest would leave it holding stale positions and a
    /// key that never came up.
    pub fn push(&mut self, event: Event) {
        // Resize/move bursts describe replaceable state. Keep ordering across
        // input and focus transitions, but discard older geometry in the same
        // uninterrupted geometry run instead of making GTK redraw every size.
        if matches!(event.kind, EVENT_RESIZE | EVENT_MOVE) {
            let first = self
                .events
                .iter()
                .rposition(|pending| {
                    !matches!(
                        pending.kind,
                        EVENT_RESIZE
                            | EVENT_MOVE
                            | EVENT_MODIFIERS
                            | EVENT_POINTER_MOTION
                            | EVENT_RAW_MOTION
                    )
                })
                .map_or(0, |n| n + 1);
            if let Some(index) = (first..self.events.len()).find(|&index| {
                let pending = self.events[index];
                pending.window == event.window && pending.kind == event.kind
            }) {
                self.events.remove(index);
            }
        }
        if self.events.len() >= CAPACITY {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(event);
    }

    /// Removes and returns the oldest event.
    pub fn pop(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// The number of events waiting.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// How many events overflow has discarded since this was last called.
    pub fn take_dropped(&mut self) -> u64 {
        core::mem::take(&mut self.dropped)
    }
}

/// Copies pending input for the child snapshot.
pub fn fork_snapshot() -> (Vec<Event>, u64) {
    let queue = match queue().lock() {
        Ok(queue) => queue,
        Err(poisoned) => poisoned.into_inner(),
    };
    (queue.events.iter().copied().collect(), queue.dropped)
}

/// Replaces bootstrap-time events with the parent's pending queue plus rebound
/// notifications generated while child HWNDs were created.
pub fn fork_restore(events: Vec<Event>, dropped: u64) {
    let mut queue = match queue().lock() {
        Ok(queue) => queue,
        Err(poisoned) => poisoned.into_inner(),
    };
    queue.events = events.into_iter().collect();
    queue.dropped = dropped;
    while queue.events.len() > CAPACITY {
        queue.events.pop_front();
        queue.dropped = queue.dropped.saturating_add(1);
    }
}

/// The process-wide event queue.
///
/// One queue rather than one per window: a guest polls for input, not for input
/// *from a particular window*, and every event carries enough context to be acted
/// on. A per-window queue would also make the common case — one window — pay for a
/// lookup on every message.
pub fn queue() -> &'static Mutex<Queue> {
    static QUEUE: std::sync::OnceLock<Mutex<Queue>> = std::sync::OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(Queue::default()))
}

/// Serialises tests that consume the process-wide queue.
///
/// Rust runs unit tests concurrently.  A test that drains this global queue can
/// otherwise steal a native message from another test even though production
/// access to the queue itself is correctly synchronised.
#[cfg(test)]
pub fn test_queue_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Appends to the process-wide queue.
///
/// Called from the UI thread for every translated message. A poisoned lock is
/// recovered from rather than propagated: the UI thread must keep pumping, because
/// a Win32 thread that stops pumping freezes every window it owns, including ones
/// this library did not create.
pub fn publish(event: Event) {
    {
        let mut queue = match queue().lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        };
        queue.push(event);
    }
    wake();
}

/// Enqueue a snapshot and its event atomically so a consumer cannot drain the
/// snapshot alone before the UI thread publishes the corresponding key.
pub fn publish_with_modifiers(event: Event, modifiers: u32) {
    {
        let mut queue = queue().lock().unwrap_or_else(|e| e.into_inner());
        queue.push(Event {
            kind: EVENT_MODIFIERS,
            state: modifiers,
            window: event.window,
            time_ms: event.time_ms,
            ..Event::default()
        });
        queue.push(event);
    }
    wake();
}

/// Wake consumers after a provider queues an extension-specific event.
pub fn wake() {
    // Never invoke foreign code while holding the queue lock: a notifier is
    // allowed to wake a consumer which immediately drains this same queue.
    let callbacks = notifiers()
        .lock()
        .map(|callbacks| callbacks.clone())
        .unwrap_or_default();
    for callback in callbacks {
        unsafe { callback() };
    }
}

/// Use the same server clock as XSetInputFocus and XGrabKeyboard. Process-local
/// elapsed time makes real input appear older than a CurrentTime grab and
/// prevents window managers from accepting the following focus changes.
pub fn timestamp() -> u64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTickCount64() -> u64;
    }
    unsafe { GetTickCount64() as u32 as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resize_bursts_keep_latest_geometry_and_preserve_input_barriers() {
        let mut queue = Queue::default();
        for width in 100..300 {
            queue.push(Event {
                kind: EVENT_RESIZE,
                window: 7,
                x: width,
                y: 200,
                ..Event::default()
            });
            queue.push(Event {
                kind: EVENT_POINTER_MOTION,
                window: 7,
                ..Event::default()
            });
        }
        let sizes: Vec<_> = queue
            .events
            .iter()
            .filter(|e| e.kind == EVENT_RESIZE)
            .map(|e| e.x)
            .collect();
        assert_eq!(sizes, [299]);
        queue.push(Event {
            kind: EVENT_KEY,
            window: 7,
            ..Event::default()
        });
        queue.push(Event {
            kind: EVENT_RESIZE,
            window: 7,
            x: 400,
            ..Event::default()
        });
        assert_eq!(
            queue
                .events
                .iter()
                .filter(|e| e.kind == EVENT_RESIZE)
                .count(),
            2
        );
        assert_eq!(queue.take_dropped(), 0);
    }

    /// The layout is ABI: `include/kinakaze/display.h` declares the same struct and
    /// the guest reads fields at these offsets.
    #[test]
    fn the_event_layout_is_stable() {
        use core::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<Event>(), 48);
        assert_eq!(align_of::<Event>(), 8);
        assert_eq!(offset_of!(Event, kind), 0);
        assert_eq!(offset_of!(Event, state), 4);
        assert_eq!(offset_of!(Event, keycode), 8);
        assert_eq!(offset_of!(Event, button), 10);
        assert_eq!(offset_of!(Event, x), 12);
        assert_eq!(offset_of!(Event, y), 16);
        assert_eq!(offset_of!(Event, scroll_x), 20);
        assert_eq!(offset_of!(Event, scroll_y), 24);
        assert_eq!(offset_of!(Event, touch_id), 28);
        assert_eq!(offset_of!(Event, time_ms), 32);
        assert_eq!(offset_of!(Event, window), 40);
    }

    fn key(keycode: u16) -> Event {
        Event {
            kind: EVENT_KEY,
            state: STATE_PRESSED,
            keycode,
            ..Event::default()
        }
    }

    /// Events come back in the order they went in.
    #[test]
    fn the_queue_is_first_in_first_out() {
        let mut queue = Queue::default();
        for code in 1..=5 {
            queue.push(key(code));
        }
        for code in 1..=5 {
            assert_eq!(queue.pop().expect("five were pushed").keycode, code);
        }
        assert!(queue.pop().is_none());
    }

    /// Overflow drops the oldest, keeps the newest, and counts what went.
    ///
    /// Keeping the newest is the load-bearing half: a guest that resumes polling
    /// needs current state, not a thousand stale mouse positions.
    #[test]
    fn overflow_discards_the_oldest_and_counts_it() {
        let mut queue = Queue::default();
        // Push one and a half capacities so the first half is certainly gone.
        let total = CAPACITY + CAPACITY / 2;
        for index in 0..total {
            queue.push(key((index % 60_000) as u16));
        }
        assert_eq!(queue.len(), CAPACITY);
        assert_eq!(queue.take_dropped(), (total - CAPACITY) as u64);

        // The oldest survivor must be the one pushed `CAPACITY` from the end.
        let expected = ((total - CAPACITY) % 60_000) as u16;
        assert_eq!(queue.pop().expect("queue is full").keycode, expected);
    }

    /// The drop count answers "since you last asked", so reading clears it.
    #[test]
    fn the_dropped_count_resets_when_read() {
        let mut queue = Queue::default();
        for index in 0..CAPACITY + 10 {
            queue.push(key(index as u16));
        }
        assert_eq!(queue.take_dropped(), 10);
        assert_eq!(
            queue.take_dropped(),
            0,
            "a second read reports no new drops"
        );
    }

    /// An unfilled queue drops nothing.
    #[test]
    fn a_queue_within_capacity_drops_nothing() {
        let mut queue = Queue::default();
        for index in 0..CAPACITY {
            queue.push(key(index as u16));
        }
        assert_eq!(queue.len(), CAPACITY);
        assert_eq!(queue.take_dropped(), 0);
    }

    /// The button codes are Linux's, not Windows'. Worth pinning: they are the one
    /// set of constants here that a reader is likely to assume rather than check.
    #[test]
    fn the_button_codes_are_the_linux_ones() {
        assert_eq!(BTN_LEFT, 272);
        assert_eq!(BTN_RIGHT, 273);
        assert_eq!(BTN_MIDDLE, 274);
        assert_eq!(BTN_SIDE, 275);
        assert_eq!(BTN_EXTRA, 276);
    }

    /// Timestamps must not go backwards, or a guest computing deltas gets negative
    /// time.
    #[test]
    fn timestamps_are_monotonic() {
        let first = timestamp();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(timestamp() >= first);
    }
}
