//! DWM owns preview surfaces and scales them on the compositor. The destination
//! process must register its own thumbnails, so the bridge sends only geometry.
use super::*;
use std::collections::HashMap;
const MAGIC: usize = 0x43595448;
#[repr(C)]
struct CopyData {
    tag: usize,
    size: u32,
    data: *const u8,
}
#[repr(C)]
struct Properties {
    flags: u32,
    destination: Rect,
    source: Rect,
    opacity: u8,
    visible: i32,
    client_only: i32,
}
#[repr(C)]
struct Size {
    width: i32,
    height: i32,
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmRegisterThumbnail(destination: Handle, source: Handle, thumbnail: *mut Handle) -> i32;
    fn DwmUnregisterThumbnail(thumbnail: Handle) -> i32;
    fn DwmUpdateThumbnailProperties(thumbnail: Handle, properties: *const Properties) -> i32;
    fn DwmQueryThumbnailSourceSize(thumbnail: Handle, size: *mut Size) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
}
fn previews() -> &'static Mutex<HashMap<(usize, usize), usize>> {
    static PREVIEWS: OnceLock<Mutex<HashMap<(usize, usize), usize>>> = OnceLock::new();
    PREVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) fn clear(window: usize) {
    previews()
        .lock()
        .unwrap()
        .retain(|(destination, _), thumbnail| {
            if *destination == window {
                unsafe {
                    DwmUnregisterThumbnail(*thumbnail as _);
                }
                false
            } else {
                true
            }
        });
}
pub(super) unsafe fn receive(window: Handle, data: isize) -> isize {
    if data == 0 || !desktop::is_desktop(window as usize) {
        return 0;
    }
    let request = unsafe { &*(data as *const CopyData) };
    if request.tag != MAGIC
        || request.data.is_null()
        || request.size < 8
        || request.size > 8 + 64 * 24
    {
        return 0;
    }
    let bytes = unsafe { core::slice::from_raw_parts(request.data, request.size as usize) };
    let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let count = word(4) as usize;
    if word(0) != 1 || count > 64 || bytes.len() != 8 + count * 24 {
        return 0;
    }
    let mut table = previews().lock().unwrap();
    let mut wanted = Vec::new();
    let mut success = true;
    for index in 0..count {
        let offset = 8 + index * 24;
        let source = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
        let mut process = 0;
        unsafe {
            GetWindowThreadProcessId(source as _, &raw mut process);
        }
        if source == window as usize || kinakaze_runtime::job::namespace_pid(process).is_none() {
            success = false;
            continue;
        }
        let key = (window as usize, source);
        let mut destination = Rect {
            left: word(offset + 8) as i32,
            top: word(offset + 12) as i32,
            right: word(offset + 16) as i32,
            bottom: word(offset + 20) as i32,
        };
        if destination.right <= destination.left || destination.bottom <= destination.top {
            continue;
        }
        wanted.push(key);
        if !table.contains_key(&key) {
            let mut thumbnail = core::ptr::null_mut();
            if unsafe { DwmRegisterThumbnail(window, source as _, &raw mut thumbnail) } < 0 {
                success = false;
                continue;
            }
            table.insert(key, thumbnail as usize);
        }
        let thumbnail = table[&key] as Handle;
        let mut size = Size {
            width: 0,
            height: 0,
        };
        if unsafe { DwmQueryThumbnailSourceSize(thumbnail, &raw mut size) } >= 0
            && size.width > 0
            && size.height > 0
        {
            let scale = ((destination.right - destination.left) as f64 / size.width as f64)
                .min((destination.bottom - destination.top) as f64 / size.height as f64);
            let width = (size.width as f64 * scale).round() as i32;
            let height = (size.height as f64 * scale).round() as i32;
            destination.left += (destination.right - destination.left - width) / 2;
            destination.top += (destination.bottom - destination.top - height) / 2;
            destination.right = destination.left + width;
            destination.bottom = destination.top + height;
        }
        let properties = Properties {
            flags: 1 | 4 | 8 | 16,
            destination,
            source: Rect::default(),
            opacity: 255,
            visible: 1,
            client_only: 0,
        };
        if unsafe { DwmUpdateThumbnailProperties(thumbnail, &properties) } < 0 {
            success = false;
        }
    }
    table.retain(|key, thumbnail| {
        if key.0 == window as usize && !wanted.contains(key) {
            unsafe {
                DwmUnregisterThumbnail(*thumbnail as _);
            }
            false
        } else {
            true
        }
    });
    isize::from(success)
}
