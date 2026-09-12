//! Cursor images use guest memory; native cursor ownership stays in this module.
use crate::{Cursor, Display};
use core::ffi::{c_char, c_int, c_uint};
use kinakaze_alloc::guest;
use kinakaze_libdisplay::ui::CursorShape;
use std::{
    collections::HashMap,
    ffi::CStr,
    ptr,
    sync::{Arc, Mutex, OnceLock},
};
mod native;
mod theme;

#[repr(C)]
pub struct XcursorImage {
    pub version: c_uint,
    pub size: c_uint,
    pub width: c_uint,
    pub height: c_uint,
    pub xhot: c_uint,
    pub yhot: c_uint,
    pub delay: c_uint,
    pub pixels: *mut c_uint,
}
#[repr(C)]
pub struct XcursorImages {
    pub nimage: c_int,
    pub images: *mut *mut XcursorImage,
    pub name: *mut c_char,
}
#[derive(Clone)]
struct CursorInfo {
    visible: bool,
    shape: CursorShape,
    native: Option<Arc<native::NativeCursor>>,
    image: Option<Arc<CursorImage>>,
}
/// Immutable source pixels shared with other frontends and fork snapshots.
pub struct CursorImage {
    pub width: u16,
    pub height: u16,
    pub x: u16,
    pub y: u16,
    pub pixels: Arc<[u32]>,
}
pub fn cursor_image(cursor: Cursor) -> Option<Arc<CursorImage>> {
    cursors().lock().ok()?.cursors.get(&cursor)?.image.clone()
}
pub(crate) fn exists(cursor: Cursor) -> bool {
    cursors()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cursors
        .contains_key(&cursor)
}
pub(crate) fn replace(source: Cursor, destination: Cursor) -> bool {
    let mut registry = cursors().lock().unwrap_or_else(|e| e.into_inner());
    let Some(image) = registry.cursors.get(&source).cloned() else {
        return false;
    };
    if !registry.cursors.contains_key(&destination) {
        return false;
    }
    registry.cursors.insert(destination, image);
    true
}
pub(crate) fn native_snapshot(cursor: usize) -> Option<Arc<CursorImage>> {
    unsafe {
        let p = native::snapshot(cursor, native::default_size());
        if p.is_null() {
            return None;
        }
        let i = &*p;
        let image = Arc::new(CursorImage {
            width: i.width as u16,
            height: i.height as u16,
            x: i.xhot as u16,
            y: i.yhot as u16,
            pixels: std::slice::from_raw_parts(i.pixels, i.width as usize * i.height as usize)
                .into(),
        });
        XcursorImageDestroy(p);
        Some(image)
    }
}
struct CursorRegistry {
    next: Cursor,
    cursors: HashMap<Cursor, CursorInfo>,
    theme: usize,
    size: c_int,
}
fn cursors() -> &'static Mutex<CursorRegistry> {
    static CURSORS: OnceLock<Mutex<CursorRegistry>> = OnceLock::new();
    CURSORS.get_or_init(|| {
        crate::register_display_close(close_display);
        Mutex::new(CursorRegistry {
            next: 0x1_0000,
            cursors: HashMap::new(),
            theme: 0,
            size: native::default_size(),
        })
    })
}
unsafe fn close_display(_: *mut Display) {
    let mut state = cursors().lock().unwrap_or_else(|e| e.into_inner());
    state.cursors.clear();
    unsafe {
        guest::free(state.theme as *mut u8);
    }
    state.theme = 0;
}
fn register(info: CursorInfo) -> Cursor {
    let mut state = cursors().lock().unwrap_or_else(|e| e.into_inner());
    let id = state.next;
    let Some(next) = id.checked_add(1) else {
        return 0;
    };
    state.next = next;
    state.cursors.insert(id, info);
    id
}
pub(crate) fn register_cursor(visible: bool) -> Cursor {
    register_cursor_shape(visible, CursorShape::Arrow)
}
pub(crate) fn register_cursor_shape(visible: bool, shape: CursorShape) -> Cursor {
    register(CursorInfo {
        visible,
        shape,
        native: None,
        image: None,
    })
}
pub(crate) fn cursor_visible(cursor: Cursor) -> bool {
    cursors()
        .lock()
        .ok()
        .and_then(|s| s.cursors.get(&cursor).map(|c| c.visible))
        .or_else(|| crate::render::cursor_has_visible_pixels(cursor))
        .unwrap_or(true)
}
pub(crate) fn cursor_shape(cursor: Cursor) -> CursorShape {
    cursors()
        .lock()
        .ok()
        .and_then(|s| s.cursors.get(&cursor).map(|c| c.shape))
        .unwrap_or(CursorShape::Arrow)
}
pub(crate) fn define(window: usize, cursor: Cursor) -> bool {
    let native = cursors()
        .lock()
        .ok()
        .and_then(|s| s.cursors.get(&cursor).and_then(|c| c.native.clone()));
    match native {
        Some(image) => kinakaze_libdisplay::ui::set_cursor_image(window, image.0),
        None => kinakaze_libdisplay::ui::set_cursor_shape(window, cursor_shape(cursor)),
    }
}
pub(crate) fn free_cursor(cursor: Cursor) -> bool {
    cursors()
        .lock()
        .ok()
        .and_then(|mut s| s.cursors.remove(&cursor))
        .is_some()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XcursorImageCreate")]
pub unsafe extern "sysv64" fn XcursorImageCreate(width: c_int, height: c_int) -> *mut XcursorImage {
    if width <= 0 || height <= 0 {
        return ptr::null_mut();
    }
    let Some(bytes) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(size_of::<XcursorImage>()))
    else {
        return ptr::null_mut();
    };
    let p = unsafe { guest::malloc(bytes).cast::<XcursorImage>() };
    if p.is_null() {
        return p;
    }
    unsafe {
        let pixels = p.add(1).cast();
        p.write(XcursorImage {
            version: 1,
            size: width.max(height) as u32,
            width: width as u32,
            height: height as u32,
            xhot: 0,
            yhot: 0,
            delay: 0,
            pixels,
        });
        ptr::write_bytes(pixels, 0, width as usize * height as usize);
    }
    p
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorImageDestroy")]
pub unsafe extern "sysv64" fn XcursorImageDestroy(image: *mut XcursorImage) {
    unsafe {
        guest::free(image.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorImagesCreate")]
pub unsafe extern "sysv64" fn XcursorImagesCreate(size: c_int) -> *mut XcursorImages {
    if size < 0 {
        return ptr::null_mut();
    }
    let Some(bytes) = (size as usize)
        .checked_mul(size_of::<*mut XcursorImage>())
        .and_then(|n| n.checked_add(size_of::<XcursorImages>()))
    else {
        return ptr::null_mut();
    };
    let p = unsafe { guest::malloc(bytes).cast::<XcursorImages>() };
    if !p.is_null() {
        unsafe {
            ptr::write_bytes(p.cast::<u8>(), 0, bytes);
            (*p).images = p.add(1).cast();
        }
    }
    p
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorImagesDestroy")]
pub unsafe extern "sysv64" fn XcursorImagesDestroy(images: *mut XcursorImages) {
    if images.is_null() {
        return;
    }
    unsafe {
        for i in 0..(*images).nimage.max(0) as usize {
            XcursorImageDestroy(*(*images).images.add(i));
        }
        guest::free((*images).name.cast());
        guest::free(images.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorImageLoadCursor")]
pub unsafe extern "sysv64" fn XcursorImageLoadCursor(
    _: *mut Display,
    image: *const XcursorImage,
) -> Cursor {
    if image.is_null() {
        return 0;
    }
    let image = unsafe { &*image };
    if image.width > u16::MAX as u32 || image.height > u16::MAX as u32 {
        return 0;
    }
    let Some(cursor) = (unsafe { native::from_image(image) }) else {
        return 0;
    };
    let pixels = unsafe {
        core::slice::from_raw_parts(image.pixels, image.width as usize * image.height as usize)
    };
    register(CursorInfo {
        visible: pixels.iter().any(|pixel| pixel >> 24 != 0),
        shape: CursorShape::Arrow,
        native: Some(Arc::new(cursor)),
        image: Some(Arc::new(CursorImage {
            width: image.width as u16,
            height: image.height as u16,
            x: image.xhot as u16,
            y: image.yhot as u16,
            pixels: Arc::from(pixels),
        })),
    })
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorImagesLoadCursor")]
pub unsafe extern "sysv64" fn XcursorImagesLoadCursor(
    dpy: *mut Display,
    images: *const XcursorImages,
) -> Cursor {
    // Animation needs a window-owned frame scheduler; do not silently discard frames.
    if images.is_null() || unsafe { (*images).nimage != 1 || (*images).images.is_null() } {
        return 0;
    }
    unsafe { XcursorImageLoadCursor(dpy, *(*images).images) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorSupportsARGB")]
pub unsafe extern "sysv64" fn XcursorSupportsARGB(_: *mut Display) -> c_int {
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorGetDefaultSize")]
pub unsafe extern "sysv64" fn XcursorGetDefaultSize(_: *mut Display) -> c_int {
    cursors().lock().unwrap_or_else(|e| e.into_inner()).size
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorSetDefaultSize")]
pub unsafe extern "sysv64" fn XcursorSetDefaultSize(_: *mut Display, size: c_int) -> c_int {
    if size <= 0 {
        return 0;
    }
    cursors().lock().unwrap_or_else(|e| e.into_inner()).size = size;
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorGetTheme")]
pub unsafe extern "sysv64" fn XcursorGetTheme(_: *mut Display) -> *mut c_char {
    let state = cursors().lock().unwrap_or_else(|e| e.into_inner());
    if state.theme == 0 {
        ptr::null_mut()
    } else {
        state.theme as *mut c_char
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorSetTheme")]
pub unsafe extern "sysv64" fn XcursorSetTheme(_: *mut Display, name: *const c_char) -> c_int {
    let new = if name.is_null() {
        ptr::null_mut()
    } else {
        let bytes = unsafe { CStr::from_ptr(name) }.to_bytes_with_nul();
        let p = unsafe { guest::malloc(bytes.len()) };
        if p.is_null() {
            return 0;
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        }
        p
    };
    let mut state = cursors().lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        guest::free(state.theme as *mut u8);
    }
    state.theme = new as usize;
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorLibraryLoadImages")]
pub unsafe extern "sysv64" fn XcursorLibraryLoadImages(
    name: *const c_char,
    theme: *const c_char,
    size: c_int,
) -> *mut XcursorImages {
    if name.is_null() || size <= 0 {
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    if theme.is_null() {
        return unsafe { native::load_images(name, size) };
    }
    unsafe { theme::load(name, CStr::from_ptr(theme).to_bytes(), size) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorLibraryLoadImage")]
pub unsafe extern "sysv64" fn XcursorLibraryLoadImage(
    name: *const c_char,
    theme: *const c_char,
    size: c_int,
) -> *mut XcursorImage {
    let images = unsafe { XcursorLibraryLoadImages(name, theme, size) };
    if images.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let image = if (*images).nimage > 0 {
            *(*images).images
        } else {
            ptr::null_mut()
        };
        if (*images).nimage > 0 {
            *(*images).images = ptr::null_mut();
        }
        XcursorImagesDestroy(images);
        image
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorLibraryLoadCursor")]
pub unsafe extern "sysv64" fn XcursorLibraryLoadCursor(
    dpy: *mut Display,
    name: *const c_char,
) -> Cursor {
    // Copy the selected theme under the lock; a concurrent setter can free its borrowed pointer.
    let (theme, size) = {
        let state = cursors().lock().unwrap_or_else(|e| e.into_inner());
        (
            if state.theme == 0 {
                None
            } else {
                Some(unsafe { CStr::from_ptr(state.theme as *const c_char) }.to_owned())
            },
            state.size,
        )
    };
    unsafe {
        let images = XcursorLibraryLoadImages(
            name,
            theme.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
            size,
        );
        let cursor = XcursorImagesLoadCursor(dpy, images);
        XcursorImagesDestroy(images);
        cursor
    }
}
fn shape_name(shape: c_uint) -> Option<&'static [u8]> {
    // Even glyph numbers in the standard X cursor font.
    const NAMES: &str = "X_cursor arrow based_arrow_down based_arrow_up boat bogosity bottom_left_corner bottom_right_corner bottom_side bottom_tee box_spiral center_ptr circle clock coffee_mug cross cross_reverse crosshair diamond_cross dot dotbox double_arrow draft_large draft_small draped_box exchange fleur gobbler gumby hand1 hand2 heart icon iron_cross left_ptr left_side left_tee leftbutton ll_angle lr_angle man middlebutton mouse pencil pirate plus question_arrow right_ptr right_side right_tee rightbutton rtl_logo sailboat sb_down_arrow sb_h_double_arrow sb_left_arrow sb_right_arrow sb_up_arrow sb_v_double_arrow shuttle sizing spider spraycan star target tcross top_left_arrow top_left_corner top_right_corner top_side top_tee trek ul_angle umbrella ur_angle watch xterm";
    if shape & 1 != 0 {
        return None;
    }
    NAMES.split(' ').nth(shape as usize / 2).map(str::as_bytes)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorShapeLoadImages")]
pub unsafe extern "sysv64" fn XcursorShapeLoadImages(
    shape: c_uint,
    theme: *const c_char,
    size: c_int,
) -> *mut XcursorImages {
    let Some(name) = shape_name(shape) else {
        return ptr::null_mut();
    };
    let name = std::ffi::CString::new(name).unwrap();
    unsafe { XcursorLibraryLoadImages(name.as_ptr(), theme, size) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XcursorShapeLoadCursor")]
pub unsafe extern "sysv64" fn XcursorShapeLoadCursor(dpy: *mut Display, shape: c_uint) -> Cursor {
    let Some(name) = shape_name(shape) else {
        return 0;
    };
    let name = std::ffi::CString::new(name).unwrap();
    unsafe { XcursorLibraryLoadCursor(dpy, name.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transparent_images_create_hidden_cursors() {
        let image = unsafe { XcursorImageCreate(2, 2) };
        assert!(!image.is_null());
        let hidden = unsafe { XcursorImageLoadCursor(ptr::null_mut(), image) };
        assert_ne!(hidden, 0);
        assert!(!cursor_visible(hidden));
        unsafe {
            *(*image).pixels = 0xffabcdef;
        }
        let visible = unsafe { XcursorImageLoadCursor(ptr::null_mut(), image) };
        assert_ne!(visible, 0);
        assert!(cursor_visible(visible));
        unsafe {
            XcursorImageDestroy(image);
        }
        assert!(free_cursor(hidden));
        assert!(free_cursor(visible));
    }
    #[test]
    fn system_images_contain_real_pixels() {
        let image = unsafe { XcursorLibraryLoadImage(c"nesw-resize".as_ptr(), ptr::null(), 32) };
        assert!(!image.is_null());
        unsafe {
            assert!((*image).width > 1 && (*image).height > 1);
            let pixels = core::slice::from_raw_parts(
                (*image).pixels,
                ((*image).width * (*image).height) as usize,
            );
            assert!(pixels.iter().any(|p| p >> 24 != 0));
            XcursorImageDestroy(image);
        }
    }
    #[test]
    fn theme_is_owned_and_can_be_cleared() {
        let theme = std::ffi::CString::new("explicit-theme").unwrap();
        assert_eq!(
            unsafe { XcursorSetTheme(ptr::null_mut(), theme.as_ptr()) },
            1
        );
        drop(theme);
        assert_eq!(
            unsafe { CStr::from_ptr(XcursorGetTheme(ptr::null_mut())) }.to_bytes(),
            b"explicit-theme"
        );
        assert_eq!(unsafe { XcursorSetTheme(ptr::null_mut(), ptr::null()) }, 1);
        assert!(unsafe { XcursorGetTheme(ptr::null_mut()) }.is_null());
    }
}
