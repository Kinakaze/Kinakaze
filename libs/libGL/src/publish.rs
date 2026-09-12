//! Publish redirected WGL clients to the XComposite backing store. The desktop
//! stage itself stays on the direct GPU presentation path.
use super::*;
use std::cell::RefCell;

thread_local! { static PIXELS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) }; }

pub(super) unsafe fn frame(drawable: usize) {
    if CURRENT_DRAWABLE.with(Cell::get) != drawable || !kinakaze_libX11::composite::manual(drawable)
    {
        return;
    }
    let root =
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(drawable as _, 2) };
    if kinakaze_libdisplay::ui::desktop::is_desktop(root as usize) {
        return;
    }
    let mut rect: RECT = unsafe { core::mem::zeroed() };
    if unsafe { GetClientRect(drawable as _, &mut rect) } == 0
        || rect.right <= 0
        || rect.bottom <= 0
    {
        return;
    }
    let (width, height) = (rect.right, rect.bottom);
    let Some(length) = (width as usize).checked_mul(height as usize) else {
        return;
    };
    PIXELS.with(|storage| unsafe {
        let mut pixels = storage.borrow_mut();
        pixels.resize(length, 0);
        let bind_buffer = native_gl_address(c"glBindBuffer".as_ptr().cast());
        let bind_framebuffer = native_gl_address(c"glBindFramebuffer".as_ptr().cast());
        let mut pbo = 0;
        let mut framebuffer = 0;
        if !bind_buffer.is_null() {
            glGetIntegerv(0x88ed, &mut pbo); // PIXEL_PACK_BUFFER_BINDING
            let bind: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_buffer);
            bind(0x88eb, 0);
        }
        if !bind_framebuffer.is_null() {
            glGetIntegerv(0x8caa, &mut framebuffer); // READ_FRAMEBUFFER_BINDING
            let bind: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_framebuffer);
            bind(0x8ca8, 0);
        }
        let mut read_buffer = 0;
        glGetIntegerv(0x0c02, &mut read_buffer);
        let stores = [0x0d05, 0x0d02, 0x0d03, 0x0d04, 0x0d00];
        let mut saved = [0; 5];
        for (index, parameter) in stores.iter().enumerate() {
            glGetIntegerv(*parameter, &mut saved[index]);
            glPixelStorei(*parameter, if index == 0 { 4 } else { 0 });
        }
        glReadBuffer(0x0405); // BACK, before SwapBuffers
        glReadPixels(
            0,
            0,
            width,
            height,
            0x80e1,
            0x1401,
            pixels.as_mut_ptr().cast(),
        );
        glReadBuffer(read_buffer as u32);
        for (parameter, value) in stores.into_iter().zip(saved) {
            glPixelStorei(parameter, value);
        }
        if !bind_framebuffer.is_null() {
            let bind: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_framebuffer);
            bind(0x8ca8, framebuffer as u32);
        }
        if !bind_buffer.is_null() {
            let bind: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_buffer);
            bind(0x88eb, pbo as u32);
        }
        kinakaze_libX11::graphics::publish_gl_frame(drawable, width, height, &pixels);
    });
}
