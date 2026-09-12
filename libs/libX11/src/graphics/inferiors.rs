//! A redirected ancestor contains its children, including foreign WM clients.
use super::*;

pub(super) fn publish(mut window: usize, mut dirty: RECT) {
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(window as _, -16) }
        as u32
        & 0x4000_0000
        == 0
    {
        return;
    }
    for _ in 0..64 {
        let parent = crate::shared::parent(window);
        if parent <= 1 || parent == window || !crate::shared::mapped(window) {
            break;
        }
        let Some((x, y)) = crate::window_tree::origin(window) else {
            break;
        };
        if !prepare_back_buffer(parent, false) {
            break;
        }
        let (area, width, height) = {
            let Ok(buffers) = back_buffers().lock() else {
                break;
            };
            let (Some(child), Some(outer)) = (buffers.get(&window), buffers.get(&parent)) else {
                break;
            };
            synchronize_buffer(child);
            synchronize_buffer(outer);
            let left = dirty.left.max(0).max(-x);
            let top = dirty.top.max(0).max(-y);
            let right = dirty.right.min(child.width).min(outer.width - x);
            let bottom = dirty.bottom.min(child.height).min(outer.height - y);
            if right <= left || bottom <= top {
                break;
            }
            // Only the damaged rows are copied. The frame decoration remains
            // intact; native memcpy selects the platform's optimized kernel.
            for row in top..bottom {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (child.bits as *const u32)
                            .add(row as usize * child.width as usize + left as usize),
                        (outer.bits as *mut u32)
                            .add((row + y) as usize * outer.width as usize + (left + x) as usize),
                        (right - left) as usize,
                    );
                }
            }
            (
                RECT {
                    left: left + x,
                    top: top + y,
                    right: right + x,
                    bottom: bottom + y,
                },
                outer.width,
                outer.height,
            )
        };
        let rect = crate::XRectangle {
            x: area.left as i16,
            y: area.top as i16,
            width: (area.right - area.left).min(65535) as u16,
            height: (area.bottom - area.top).min(65535) as u16,
        };
        crate::damage::changed(
            parent,
            rect,
            width.min(65535) as u16,
            height.min(65535) as u16,
        );
        if crate::shared::window_owner(parent) != Some(crate::shared::pid()) {
            crate::shared::damage_from_child(
                parent,
                rect,
                width.min(65535) as u16,
                height.min(65535) as u16,
            );
        }
        window = parent;
        dirty = area;
    }
}
