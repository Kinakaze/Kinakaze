//! Rebuild the native event source after descriptor restoration in a fork child.
use super::{CONNECTION, Connection, Endpoint};
use libc::fdio;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::MutexGuard;

const KEY: u64 = u64::from_le_bytes(*b"CYXCON04");
thread_local! {
    static FROZEN: RefCell<Option<MutexGuard<'static, Connection>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(connection) = CONNECTION.try_lock() else {
            return 11;
        };
        *slot.borrow_mut() = Some(connection);
        0
    })
}

unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(connection) = slot.as_ref() else {
            return -22;
        };
        let mut words = vec![
            KEY,
            connection.users as u64,
            connection.fd as i64 as u64,
            0,
            connection.displays.len() as u64,
        ];
        for (&display, endpoint) in &connection.displays {
            words.extend_from_slice(&[
                if display == crate::shared_display() as usize {
                    0
                } else {
                    display as u64
                },
                endpoint.fd as u64,
                endpoint.masks.len() as u64,
                endpoint.references as u64,
                endpoint.xcb_events as u64,
            ]);
            for (&window, &mask) in &endpoint.masks {
                let saved = if window == 1 {
                    0
                } else {
                    kinakaze_libdisplay::window::logical_for_native(window)
                        .unwrap_or(window as u64 | (1 << 63))
                };
                words.extend_from_slice(&[saved, mask as u64]);
            }
        }
        let size = words.len() * 8;
        if !output.is_null() {
            if capacity < size {
                return -22;
            }
            for (index, word) in words.into_iter().enumerate() {
                unsafe { output.add(index * 8).cast::<u64>().write_unaligned(word) };
            }
        }
        size as isize
    })
}

unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| slot.borrow_mut().take());
}

unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length < 40 || length % 8 != 0 {
        return 22;
    }
    let word = |index: usize| unsafe { input.add(index * 8).cast::<u64>().read_unaligned() };
    let users = word(1) as usize;
    let saved_fd = word(2) as i64;
    if word(0) != KEY
        || word(3) > 1
        || (users == 0 && saved_fd != -1)
        || (users != 0 && !(0..=i32::MAX as i64).contains(&saved_fd))
    {
        return 22;
    }
    if users == 0 {
        return 0;
    }
    crate::init_display_globals();
    let mut displays = BTreeMap::new();
    let mut cursor = 5;
    let mut references = 0usize;
    for _ in 0..word(4) as usize {
        if cursor + 5 > length / 8 {
            return 22;
        }
        let display = if word(cursor) == 0 {
            crate::shared_display()
        } else {
            word(cursor) as *mut crate::Display
        };
        let fd = match i32::try_from(word(cursor + 1)) {
            Ok(fd) => fd,
            Err(_) => return 22,
        };
        let count = word(cursor + 2) as usize;
        let refs = word(cursor + 3) as usize;
        let owner = word(cursor + 4);
        if owner > 1 {
            return 22;
        }
        if refs == 0 || refs > users {
            return 22;
        }
        references = match references.checked_add(refs) {
            Some(value) => value,
            None => return 22,
        };
        cursor += 5;
        if count > (length / 8 - cursor) / 2 {
            return 22;
        }
        let mut masks = BTreeMap::new();
        for _ in 0..count {
            let saved = word(cursor);
            let window = if saved == 0 {
                1
            } else if saved & (1 << 63) != 0 {
                (saved & !(1 << 63)) as usize
            } else {
                match kinakaze_libdisplay::window::native_handle(saved) {
                    Some(window) => window,
                    None => return 22,
                }
            };
            let mask = word(cursor + 1);
            if mask > 0x01ff_ffff {
                return 22;
            }
            masks.insert(window, mask as u32);
            cursor += 2;
        }
        let error = restore_fd(fd);
        if error != 0 {
            return error;
        }
        unsafe {
            (*display).fd = fd;
            (*display).qlen = 0;
        }
        displays.insert(
            display as usize,
            Endpoint {
                fd,
                references: refs,
                masks,
                xcb_events: owner != 0,
            },
        );
    }
    if cursor != length / 8 || references != users {
        return 22;
    }
    *CONNECTION.lock().unwrap_or_else(|e| e.into_inner()) = Connection {
        users,
        fd: saved_fd as i32,
        displays,
    };
    kinakaze_libdisplay::event::register_notifier(crate::notify_x11_event);
    crate::shared::open();
    kinakaze_libdisplay::ui::presentation::set_paint_handler(Some(crate::graphics::paint));
    kinakaze_libdisplay::ui::interaction::set_resize_handler(crate::wm::native_resize);
    super::notify();
    0
}
fn restore_fd(fd: i32) -> i32 {
    // HWNDs are recreated per process. Their notification counter must also be
    // private, or a child could consume a parent's wakeup. Keep the fd number
    // already stored in a guest's poll array, including its close-on-exec flag.
    let fresh = unsafe { fdio::kinakaze_abi_eventfd(0, fdio::EFD_NONBLOCK | fdio::EFD_CLOEXEC) };
    if fresh < 0 {
        return libc::kinakaze_errno();
    }
    if fresh != fd {
        let result = unsafe { fdio::kinakaze_abi_dup3(fresh, fd, fdio::EFD_CLOEXEC) };
        let error = libc::kinakaze_errno();
        libc::kinakaze_abi_close(fresh);
        if result < 0 {
            return error;
        }
    }
    0
}

extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 450,
        key: KEY,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
