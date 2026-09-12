//! MIT-SHM images borrow SysV storage; the server owns a separate attachment.
//! Pixmap sharing is not advertised: GDI pixmaps do not use SysV-backed DIBs.
use super::*;
use kinakaze_libX11::{
    XEvent, errors, graphics,
    image::{self, XImage},
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
mod lifecycle;
const OPCODE: u8 = 130;
const EVENT_BASE: c_int = 72;
#[repr(C)]
pub struct XShmSegmentInfo {
    pub shmseg: u64,
    pub shmid: c_int,
    pub shmaddr: *mut c_char,
    pub read_only: Bool,
}
#[repr(C)]
struct Completion {
    kind: c_int,
    serial: u64,
    send_event: Bool,
    display: *mut Display,
    drawable: usize,
    major_code: c_int,
    minor_code: c_int,
    shmseg: u64,
    offset: u64,
}
struct Attachment {
    display: usize,
    address: usize,
    size: usize,
    read_only: bool,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        unsafe {
            libc::sysvipc::kinakaze_abi_shmdt(self.address as *const c_void);
        }
    }
}
struct State {
    next: u64,
    active: usize,
    segments: BTreeMap<u64, Arc<Attachment>>,
}
static STATE: Mutex<State> = Mutex::new(State {
    next: 1,
    active: 0,
    segments: BTreeMap::new(),
});
struct Transfer {
    attachment: Arc<Attachment>,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).active -= 1;
    }
}
unsafe fn failure(display: *mut Display, code: u8, minor: u8, id: usize) -> Bool {
    unsafe {
        errors::report(display, code, OPCODE, minor, id);
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmQueryExtension")]
pub unsafe extern "sysv64" fn XShmQueryExtension(_display: *mut Display) -> Bool {
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmQueryVersion")]
pub unsafe extern "sysv64" fn XShmQueryVersion(
    _display: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
    pixmaps: *mut Bool,
) -> Bool {
    unsafe {
        if !major.is_null() {
            *major = 1;
        }
        if !minor.is_null() {
            *minor = 1;
        }
        if !pixmaps.is_null() {
            *pixmaps = 0;
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmGetEventBase")]
pub unsafe extern "sysv64" fn XShmGetEventBase(_display: *mut Display) -> c_int {
    EVENT_BASE
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmPixmapFormat")]
pub unsafe extern "sysv64" fn XShmPixmapFormat(_display: *mut Display) -> c_int {
    2
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmAttach")]
pub unsafe extern "sysv64" fn XShmAttach(
    display: *mut Display,
    info: *mut XShmSegmentInfo,
) -> Bool {
    if info.is_null() {
        return unsafe { failure(display, 2, 1, 0) };
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let info = unsafe { &mut *info };
    let mut stat = libc::sysvipc::ShmidDs::default();
    if unsafe {
        libc::sysvipc::kinakaze_abi_shmctl(info.shmid, libc::sysvipc::IPC_STAT, &raw mut stat)
    } != 0
    {
        drop(state);
        return unsafe { failure(display, 10, 1, info.shmid as usize) };
    }
    if state.next == u64::MAX {
        drop(state);
        return unsafe { failure(display, 11, 1, 0) };
    }
    let address = unsafe {
        libc::sysvipc::kinakaze_abi_shmat(
            info.shmid,
            ptr::null(),
            if info.read_only != 0 {
                libc::sysvipc::SHM_RDONLY
            } else {
                0
            },
        )
    };
    if address as usize == usize::MAX {
        drop(state);
        return unsafe { failure(display, 10, 1, info.shmid as usize) };
    }
    let id = state.next;
    state.next += 1;
    state.segments.insert(
        id,
        Arc::new(Attachment {
            display: display as usize,
            address: address as usize,
            size: stat.shm_segsz,
            read_only: info.read_only != 0,
        }),
    );
    info.shmseg = id;
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmDetach")]
pub unsafe extern "sysv64" fn XShmDetach(
    display: *mut Display,
    info: *mut XShmSegmentInfo,
) -> Bool {
    if info.is_null() {
        return unsafe { failure(display, 2, 2, 0) };
    }
    let id = unsafe { (*info).shmseg };
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let removed = if state
        .segments
        .get(&id)
        .is_some_and(|s| s.display == display as usize)
    {
        state.segments.remove(&id)
    } else {
        None
    };
    drop(state);
    if removed.is_none() {
        return unsafe { failure(display, 136, 2, id as usize) };
    }
    drop(removed);
    1
}
unsafe extern "sysv64" fn destroy_shared_image(image: *mut XImage) -> c_int {
    unsafe {
        guest::free(image.cast());
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmCreateImage")]
pub unsafe extern "sysv64" fn XShmCreateImage(
    display: *mut Display,
    visual: *mut Visual,
    depth: c_uint,
    format: c_int,
    data: *mut c_char,
    info: *mut XShmSegmentInfo,
    width: c_uint,
    height: c_uint,
) -> *mut XImage {
    if info.is_null() {
        return ptr::null_mut();
    }
    let image = unsafe {
        image::XCreateImage(
            display, visual, depth, format, 0, data, width, height, 32, 0,
        )
    };
    if !image.is_null() {
        unsafe {
            (*image).obdata = info.cast();
            (*image).functions[1] = destroy_shared_image as *const () as usize;
        }
    }
    image
}
unsafe fn transfer(
    display: *mut Display,
    image: *mut XImage,
    write: bool,
) -> Result<(Transfer, usize, u64, XImage), u8> {
    if image.is_null() {
        return Err(2);
    }
    let mut view = unsafe { ptr::read(image) };
    if unsafe { image::XInitImage(&raw mut view) } == 0 {
        return Err(2);
    }
    let image = &view;
    if image.obdata.is_null()
        || image.data.is_null()
        || image.height < 0
        || image.bytes_per_line < 0
        || !(0..=2).contains(&image.format)
        || !(1..=32).contains(&image.depth)
    {
        return Err(2);
    }
    let info = unsafe { &*image.obdata.cast::<XShmSegmentInfo>() };
    let offset = (image.data as usize)
        .checked_sub(info.shmaddr as usize)
        .ok_or(2)?;
    let planes = if image.format == 1 {
        image.depth as usize
    } else {
        1
    };
    let bytes = (image.bytes_per_line as usize)
        .checked_mul(image.height as usize)
        .and_then(|n| n.checked_mul(planes))
        .ok_or(2)?;
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let attachment = state
        .segments
        .get(&info.shmseg)
        .filter(|s| s.display == display as usize)
        .cloned()
        .ok_or(136)?;
    if write && attachment.read_only {
        return Err(10);
    }
    if offset
        .checked_add(bytes)
        .is_none_or(|end| end > attachment.size)
    {
        return Err(2);
    }
    state.active += 1;
    Ok((Transfer { attachment }, offset, info.shmseg, view))
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmPutImage")]
pub unsafe extern "sysv64" fn XShmPutImage(
    display: *mut Display,
    drawable: usize,
    gc: graphics::GC,
    image: *mut XImage,
    sx: c_int,
    sy: c_int,
    dx: c_int,
    dy: c_int,
    width: c_uint,
    height: c_uint,
    send_event: Bool,
) -> Bool {
    let (transfer, offset, segment, mut view) = match unsafe { transfer(display, image, false) } {
        Ok(value) => value,
        Err(code) => return unsafe { failure(display, code, 3, drawable) },
    };
    // Borrow server storage through a stack XImage; never rewrite the client's header.
    view.data = (transfer.attachment.address + offset) as *mut c_char;
    if unsafe {
        image::XPutImage(
            display,
            drawable,
            gc,
            &raw mut view,
            sx,
            sy,
            dx,
            dy,
            width,
            height,
        )
    } != 0
    {
        return 0;
    }
    if send_event != 0 {
        let completion = Completion {
            kind: EVENT_BASE,
            serial: if display.is_null() {
                0
            } else {
                unsafe { (*display).request as u64 }
            },
            send_event: 0,
            display,
            drawable,
            major_code: OPCODE as i32,
            minor_code: 3,
            shmseg: segment,
            offset: offset as u64,
        };
        let mut event: XEvent = unsafe { mem::zeroed() };
        unsafe {
            ptr::copy_nonoverlapping(
                (&raw const completion).cast::<u8>(),
                (&raw mut event).cast::<u8>(),
                mem::size_of::<Completion>(),
            );
        }
        kinakaze_libX11::queue_extension_event(display, event);
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmGetImage")]
pub unsafe extern "sysv64" fn XShmGetImage(
    display: *mut Display,
    drawable: usize,
    image: *mut XImage,
    x: c_int,
    y: c_int,
    mask: u64,
) -> Bool {
    let (transfer, offset, _, mut view) = match unsafe { transfer(display, image, true) } {
        Ok(value) => value,
        Err(code) => return unsafe { failure(display, code, 4, drawable) },
    };
    view.data = (transfer.attachment.address + offset) as *mut c_char;
    match unsafe { image::read_into(drawable, &raw mut view, x, y, mask) } {
        Ok(()) => 1,
        Err(code) => unsafe { failure(display, code, 4, drawable) },
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShmCreatePixmap")]
pub unsafe extern "sysv64" fn XShmCreatePixmap(
    display: *mut Display,
    drawable: usize,
    _data: *mut c_char,
    _info: *mut XShmSegmentInfo,
    _width: c_uint,
    _height: c_uint,
    _depth: c_uint,
) -> usize {
    unsafe {
        failure(display, 17, 5, drawable);
    }
    0
}
unsafe fn close_display(display: *mut Display) {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .segments
        .retain(|_, s| s.display != display as usize);
}
