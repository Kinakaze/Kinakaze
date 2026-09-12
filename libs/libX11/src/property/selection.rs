//! ICCCM ownership and conversion requests on the shared virtual display.
use super::*;
#[derive(Clone, Copy)]
pub(super) struct Owner {
    pub window: Window,
    pub time: u32,
}
fn owner(selection: Atom) -> Option<Owner> {
    let bytes = crate::shared::get(&format!("s/{selection}"))?;
    let window = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?) as usize;
    Some(Owner {
        window: if window == 0 || crate::shared::valid(window) {
            window
        } else {
            0
        },
        time: u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?),
    })
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClearEvent {
    pub kind: i32,
    pub serial: u64,
    pub sent: i32,
    pub display: *mut Display,
    pub window: Window,
    pub selection: Atom,
    pub time: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RequestEvent {
    pub kind: i32,
    pub serial: u64,
    pub sent: i32,
    pub display: *mut Display,
    pub owner: Window,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NotifyEvent {
    pub kind: i32,
    pub serial: u64,
    pub sent: i32,
    pub display: *mut Display,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: u64,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetSelectionOwner")]
pub unsafe extern "sysv64" fn XSetSelectionOwner(
    d: *mut Display,
    selection: Atom,
    owner: Window,
    time: u64,
) -> i32 {
    if owner != 0 && !valid_window(owner) {
        return unsafe { crate::errors::report(d, 3, 22, 0, owner) };
    }
    let current = crate::focus::now();
    let time = if time == 0 { current } else { time as u32 };
    if atoms::atom_name(selection).is_none() {
        return unsafe { crate::errors::report(d, 5, 22, 0, selection) };
    }
    if (time.wrapping_sub(current) as i32) > 0 {
        return 1;
    }
    let result = crate::shared::transaction(|tx| {
        let key = format!("s/{selection}");
        let previous = tx.get(&key).map(|bytes| Owner {
            window: u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize,
            time: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        });
        if previous.is_some_and(|old| (time.wrapping_sub(old.time) as i32) < 0) {
            return Ok(None);
        }
        let mut bytes = (owner as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&time.to_le_bytes());
        tx.set(key, bytes);
        Ok(Some(previous))
    });
    let previous = match result {
        Ok(Some(previous)) => previous,
        Ok(None) => return 1,
        Err(code) => return unsafe { crate::errors::report(d, code, 22, 0, selection) },
    };
    crate::xfixes::tracking::selection(selection, owner, time, 0);
    if let Some(old) = previous.filter(|old| old.window != 0 && old.window != owner) {
        let mut event: crate::XEvent = unsafe { core::mem::zeroed() };
        unsafe {
            (&mut event as *mut crate::XEvent)
                .cast::<ClearEvent>()
                .write(ClearEvent {
                    kind: 29,
                    serial: 0,
                    sent: 0,
                    display: d,
                    window: old.window,
                    selection,
                    time: time as u64,
                })
        };
        {
            crate::shared::send_xevent(d, old.window, &mut event);
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetSelectionOwner")]
pub unsafe extern "sysv64" fn XGetSelectionOwner(d: *mut Display, selection: Atom) -> Window {
    if atoms::atom_name(selection).is_none() {
        unsafe { crate::errors::report(d, 5, 23, 0, selection) };
        return 0;
    }
    owner(selection).map_or(0, |s| s.window)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XConvertSelection")]
pub unsafe extern "sysv64" fn XConvertSelection(
    d: *mut Display,
    selection: Atom,
    target: Atom,
    property: Atom,
    requestor: Window,
    time: u64,
) -> i32 {
    if !valid_window(requestor) {
        return unsafe { crate::errors::report(d, 3, 24, 0, requestor) };
    }
    for atom in [selection, target] {
        if atoms::atom_name(atom).is_none() {
            return unsafe { crate::errors::report(d, 5, 24, 0, atom) };
        }
    }
    if property != 0 && atoms::atom_name(property).is_none() {
        return unsafe { crate::errors::report(d, 5, 24, 0, property) };
    }
    let owner = self::owner(selection).map_or(0, |s| s.window);
    let time = if time == 0 {
        crate::focus::now() as u64
    } else {
        time
    };
    let mut event: crate::XEvent = unsafe { core::mem::zeroed() };
    if owner == 0 {
        unsafe {
            (&mut event as *mut crate::XEvent)
                .cast::<NotifyEvent>()
                .write(NotifyEvent {
                    kind: 31,
                    serial: 0,
                    sent: 0,
                    display: d,
                    requestor,
                    selection,
                    target,
                    property: 0,
                    time,
                })
        };
    } else {
        unsafe {
            (&mut event as *mut crate::XEvent)
                .cast::<RequestEvent>()
                .write(RequestEvent {
                    kind: 30,
                    serial: 0,
                    sent: 0,
                    display: d,
                    owner,
                    requestor,
                    selection,
                    target,
                    property,
                    time,
                })
        };
    }
    {
        crate::shared::send_xevent(d, if owner == 0 { requestor } else { owner }, &mut event);
    }
    1
}
