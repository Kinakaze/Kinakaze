//! Out-of-line versions of Xlib macros read the same ABI fields as guest code.
use crate::{Colormap, Display, Screen, Visual, Window, c_long};
use core::ffi::{c_char, c_int, c_void};

macro_rules! screen_field {
    ($name:ident,$field:ident,$result:ty) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libX11_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(screen: *mut Screen) -> $result {
            unsafe { (*screen).$field }
        }
    };
}
screen_field!(XWidthOfScreen, width, c_int);
screen_field!(XHeightOfScreen, height, c_int);
screen_field!(XWidthMMOfScreen, mwidth, c_int);
screen_field!(XHeightMMOfScreen, mheight, c_int);
screen_field!(XRootWindowOfScreen, root, Window);
screen_field!(XDefaultVisualOfScreen, root_visual, *mut Visual);
screen_field!(XDefaultDepthOfScreen, root_depth, c_int);
screen_field!(XDefaultGCOfScreen, default_gc, *mut c_void);
screen_field!(XDefaultColormapOfScreen, cmap, Colormap);
screen_field!(XBlackPixelOfScreen, black_pixel, usize);
screen_field!(XWhitePixelOfScreen, white_pixel, usize);
screen_field!(XMinCmapsOfScreen, min_maps, c_int);
screen_field!(XMaxCmapsOfScreen, max_maps, c_int);
screen_field!(XDoesSaveUnders, save_unders, c_int);
screen_field!(XDoesBackingStore, backing_store, c_int);
screen_field!(XEventMaskOfScreen, root_input_mask, c_long);
screen_field!(XPlanesOfScreen, root_depth, c_int);

macro_rules! display_field {
    ($name:ident,$field:ident,$result:ty) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libX11_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(display: *mut Display) -> $result {
            unsafe { (*display).$field }
        }
    };
}
display_field!(XConnectionNumber, fd, c_int);
display_field!(XImageByteOrder, byte_order, c_int);
display_field!(XBitmapUnit, bitmap_unit, c_int);
display_field!(XBitmapBitOrder, bitmap_bit_order, c_int);
display_field!(XBitmapPad, bitmap_pad, c_int);
display_field!(XProtocolVersion, proto_major_version, c_int);
display_field!(XProtocolRevision, proto_minor_version, c_int);
display_field!(XServerVendor, vendor, *const c_char);
display_field!(XVendorRelease, release, c_int);
display_field!(XLastKnownRequestProcessed, last_request_read, usize);

#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultScreenOfDisplay")]
pub unsafe extern "sysv64" fn XDefaultScreenOfDisplay(d: *mut Display) -> *mut Screen {
    unsafe { (*d).screens.add((*d).default_screen as usize) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayHeightMM")]
pub unsafe extern "sysv64" fn XDisplayHeightMM(d: *mut Display, screen: c_int) -> c_int {
    unsafe { (*(*d).screens.add(screen as usize)).mheight }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayWidthMM")]
pub unsafe extern "sysv64" fn XDisplayWidthMM(d: *mut Display, screen: c_int) -> c_int {
    unsafe { (*(*d).screens.add(screen as usize)).mwidth }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultGC")]
pub unsafe extern "sysv64" fn XDefaultGC(d: *mut Display, screen: c_int) -> *mut c_void {
    unsafe { (*(*d).screens.add(screen as usize)).default_gc }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCellsOfScreen")]
pub unsafe extern "sysv64" fn XCellsOfScreen(screen: *mut Screen) -> c_int {
    unsafe { (*(*screen).root_visual).map_entries }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XScreenNumberOfScreen")]
pub unsafe extern "sysv64" fn XScreenNumberOfScreen(screen: *mut Screen) -> c_int {
    unsafe { screen.offset_from((*(*screen).display).screens) as c_int }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XExtendedMaxRequestSize")]
pub unsafe extern "sysv64" fn XExtendedMaxRequestSize(_d: *mut Display) -> c_long {
    0 // BIG-REQUESTS is not advertised by this native display.
}
#[unsafe(export_name = "kinakaze_engine_libX11__XReadEvents")]
pub unsafe extern "sysv64" fn _XReadEvents(d: *mut Display) {
    let mut event = unsafe { core::mem::zeroed() };
    unsafe {
        crate::XPeekEvent(d, &raw mut event);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultRootWindow")]
pub unsafe extern "sysv64" fn XDefaultRootWindow(d: *mut Display) -> Window {
    unsafe { (*XDefaultScreenOfDisplay(d)).root }
}
