//! GLX_EXT_texture_from_pixmap backed by the shared X11 surface. Each bind
//! uploads the current pixels to the caller's texture, including redirected
//! windows retained by XCompositeNameWindowPixmap.
use super::*;
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Clone, Copy)]
struct Pixmap {
    source: usize,
    format: i32,
}
static PIXMAPS: Mutex<BTreeMap<usize, Pixmap>> = Mutex::new(BTreeMap::new());
static EVENT_MASKS: Mutex<BTreeMap<usize, u64>> = Mutex::new(BTreeMap::new());
#[unsafe(export_name = "kinakaze_engine_libGL_glXSelectEvent")]
pub unsafe extern "sysv64" fn glXSelectEvent(dpy: *mut c_void, drawable: usize, mask: u64) {
    if mask & !0x08000000 != 0 {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 2, 0, 0, mask as usize);
        }
        return;
    }
    let valid = PIXMAPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&drawable)
        || super::surface_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&drawable)
        || kinakaze_libX11::graphics::drawable_dimensions(drawable).is_some();
    if !valid {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 9, 0, 0, drawable);
        }
        return;
    }
    let mut masks = EVENT_MASKS.lock().unwrap_or_else(|e| e.into_inner());
    if mask == 0 {
        masks.remove(&drawable);
    } else {
        masks.insert(drawable, mask);
    }
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXGetSelectedEvent")]
pub unsafe extern "sysv64" fn glXGetSelectedEvent(
    _dpy: *mut c_void,
    drawable: usize,
    out: *mut u64,
) {
    if !out.is_null() {
        unsafe {
            *out = EVENT_MASKS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&drawable)
                .copied()
                .unwrap_or(0);
        }
    }
}
pub fn query(id: usize, attribute: i32) -> Option<u32> {
    let p = *PIXMAPS.lock().unwrap_or_else(|e| e.into_inner()).get(&id)?;
    let (w, h, _) = kinakaze_libX11::graphics::drawable_dimensions(p.source)?;
    Some(match attribute {
        0x801d => w as u32,
        0x801e => h as u32,
        0x8013 => 1,
        0x20d5 => p.format as u32,
        0x20d6 => 0x20dc,
        0x20d4 => 1,
        _ => 0,
    })
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXCreatePixmap")]
pub unsafe extern "sysv64" fn glXCreatePixmap(
    dpy: *mut c_void,
    _config: *mut c_void,
    source: usize,
    attributes: *const i32,
) -> usize {
    // XCompositeNameWindowPixmap is buffered by ELF Xlib. Complete preceding
    // requests before looking up the new drawable in the native server.
    unsafe {
        kinakaze_libX11::XFlush(dpy.cast());
    }
    if kinakaze_libX11::graphics::drawable_dimensions(source).is_none() {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 4, 0, 0, source);
        }
        return 0;
    }
    let mut format = 0x20d9;
    if !attributes.is_null() {
        let mut ended = false;
        for i in (0..128).step_by(2) {
            let key = unsafe { *attributes.add(i) };
            if key == 0 {
                ended = true;
                break;
            }
            let value = unsafe { *attributes.add(i + 1) };
            match (key, value) {
                (0x20d5, 0x20d9 | 0x20da) => format = value,
                (0x20d6, 0x20dc) | (0x20d7, 0) => {}
                _ => {
                    unsafe {
                        kinakaze_libX11::errors::report(dpy.cast(), 2, 0, 0, key as usize);
                    }
                    return 0;
                }
            }
        }
        if !ended {
            return 0;
        }
    }
    let Some(source) = kinakaze_libX11::graphics::retain_pixmap(source) else {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 4, 0, 0, source);
        }
        return 0;
    };
    let id = kinakaze_libX11::graphics::allocate_drawable_id();
    PIXMAPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, Pixmap { source, format });
    id
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXDestroyPixmap")]
pub unsafe extern "sysv64" fn glXDestroyPixmap(dpy: *mut c_void, id: usize) {
    let removed = PIXMAPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    if let Some(p) = removed {
        unsafe {
            kinakaze_libX11::graphics::XFreePixmap(dpy.cast(), p.source);
        }
    }
    EVENT_MASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXCreateGLXPixmap")]
pub unsafe extern "sysv64" fn glXCreateGLXPixmap(
    dpy: *mut c_void,
    _visual: *mut XVisualInfo,
    source: usize,
) -> usize {
    unsafe { glXCreatePixmap(dpy, core::ptr::null_mut(), source, core::ptr::null()) }
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXDestroyGLXPixmap")]
pub unsafe extern "sysv64" fn glXDestroyGLXPixmap(dpy: *mut c_void, id: usize) {
    unsafe { glXDestroyPixmap(dpy, id) }
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXBindTexImageEXT")]
pub unsafe extern "sysv64" fn glXBindTexImageEXT(
    dpy: *mut c_void,
    id: usize,
    buffer: i32,
    attributes: *const i32,
) {
    let p = PIXMAPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .copied();
    let Some(p) = p else {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 9, 0, 0, id);
        }
        return;
    };
    if buffer != 0x20de || (!attributes.is_null() && unsafe { *attributes } != 0) {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 2, 0, 0, buffer as usize);
        }
        return;
    }
    if unsafe { wglGetCurrentContext() }.is_null() {
        return;
    }
    kinakaze_libX11::graphics::read_drawable(p.source, |pixels, w, h, depth| unsafe {
        // XImage data is independent of the caller's pixel-store state.
        let mut alignment = 0;
        let mut row = 0;
        let mut skip_rows = 0;
        let mut skip_pixels = 0;
        let mut pbo = 0;
        glGetIntegerv(0x0cf5, &mut alignment);
        glGetIntegerv(0x0cf2, &mut row);
        glGetIntegerv(0x0cf3, &mut skip_rows);
        glGetIntegerv(0x0cf4, &mut skip_pixels);
        let bind = native_gl_address(c"glBindBuffer".as_ptr().cast());
        if !bind.is_null() {
            glGetIntegerv(0x88ef, &mut pbo);
            let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind);
            f(0x88ec, 0);
        }
        glPixelStorei(0x0cf5, 4);
        glPixelStorei(0x0cf2, 0);
        glPixelStorei(0x0cf3, 0);
        glPixelStorei(0x0cf4, 0);
        // Keep allocated GPU storage across frames. Reallocating every bind
        // stalls the compositor even when only a small application repaints.
        let format = if p.format == 0x20da && depth == 32 {
            0x1908
        } else {
            0x1907
        };
        let (mut old_width, mut old_height, mut old_format) = (0, 0, 0);
        glGetTexLevelParameteriv(0x0de1, 0, 0x1000, &mut old_width);
        glGetTexLevelParameteriv(0x0de1, 0, 0x1001, &mut old_height);
        glGetTexLevelParameteriv(0x0de1, 0, 0x1003, &mut old_format);
        if (old_width, old_height, old_format) == (w, h, format) {
            glTexSubImage2D(
                0x0de1,
                0,
                0,
                0,
                w,
                h,
                0x80e1,
                0x1401,
                pixels.as_ptr().cast(),
            );
        } else {
            // Depth-24 padding is not alpha: its compositor texture is opaque.
            glTexImage2D(
                0x0de1,
                0,
                format,
                w,
                h,
                0,
                0x80e1,
                0x1401,
                pixels.as_ptr().cast(),
            );
        }
        glPixelStorei(0x0cf5, alignment);
        glPixelStorei(0x0cf2, row);
        glPixelStorei(0x0cf3, skip_rows);
        glPixelStorei(0x0cf4, skip_pixels);
        if !bind.is_null() {
            let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind);
            f(0x88ec, pbo as u32);
        }
    });
}
#[unsafe(export_name = "kinakaze_engine_libGL_glXReleaseTexImageEXT")]
pub unsafe extern "sysv64" fn glXReleaseTexImageEXT(dpy: *mut c_void, id: usize, buffer: i32) {
    // Pixels were copied at bind time; releasing never alters texture storage.
    if !PIXMAPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&id)
        || buffer != 0x20de
    {
        unsafe {
            kinakaze_libX11::errors::report(dpy.cast(), 2, 0, 0, id);
        }
    }
}
