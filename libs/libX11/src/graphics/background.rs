//! Background tiles retain their pixels after the client frees its pixmap id.
use super::*;
use std::sync::Arc;
#[derive(Clone)]
pub(super) enum Background {
    None,
    Parent,
    Pixel(u32),
    Tile(Arc<DrawableSnapshot>),
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWindowBackgroundPixmap")]
pub unsafe extern "sysv64" fn XSetWindowBackgroundPixmap(
    display: *mut Display,
    window: Window,
    pixmap: usize,
) -> c_int {
    if kinakaze_libdisplay::window::logical_for_native(window).is_none() {
        return unsafe { crate::errors::report(display, 3, 2, 0, window) };
    }
    let background = match pixmap {
        0 => Background::None,
        1 => Background::Parent,
        _ => {
            if !pixmap_buffers()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&pixmap)
            {
                return unsafe { crate::errors::report(display, 4, 2, 0, pixmap) };
            }
            match drawable_snapshot(pixmap) {
                Some(tile) if tile.depth == 24 || tile.depth == 32 => {
                    Background::Tile(Arc::new(tile))
                }
                Some(_) => return unsafe { crate::errors::report(display, 8, 2, 0, pixmap) },
                None => return unsafe { crate::errors::report(display, 4, 2, 0, pixmap) },
            }
        }
    };
    window_backgrounds()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(window, background);
    1
}
pub(super) fn paint(mut window: Window, pixels: &mut [u32], width: i32, height: i32, rect: RECT) {
    let (mut x_origin, mut y_origin) = (0, 0);
    let background = loop {
        let background = window_backgrounds()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&window)
            .cloned()
            .unwrap_or(Background::Pixel(0));
        if !matches!(background, Background::Parent) {
            break background;
        }
        let parent =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(window as HWND) };
        if parent.is_null() {
            return;
        }
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        unsafe {
            windows_sys::Win32::Graphics::Gdi::MapWindowPoints(
                window as HWND,
                parent,
                &mut point,
                1,
            );
        }
        x_origin += point.x;
        y_origin += point.y;
        window = parent as usize;
    };
    match background {
        Background::None | Background::Parent => (),
        Background::Pixel(pixel) => fill_pixels(pixels, width, height, rect, pixel & 0xffffff),
        Background::Tile(tile) => {
            if tile.width <= 0 || tile.height <= 0 {
                return;
            }
            for y in rect.top.clamp(0, height)..rect.bottom.clamp(0, height) {
                for x in rect.left.clamp(0, width)..rect.right.clamp(0, width) {
                    let tx = (x + x_origin).rem_euclid(tile.width);
                    let ty = (y + y_origin).rem_euclid(tile.height);
                    pixels[(y * width + x) as usize] = tile.pixels[(ty * tile.width + tx) as usize];
                }
            }
        }
    }
}
