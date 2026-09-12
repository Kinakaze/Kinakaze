use super::*;
pub type RROutput = usize;
pub type RRCrtc = usize;
pub type RRMode = usize;
pub type Rotation = c_ushort;
pub type Connection = c_ushort;
pub type SubpixelOrder = c_ushort;

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn output_info_matches_xrandr_lp64_abi() {
        assert_eq!(size_of::<XRROutputInfo>(), 96);
        assert_eq!(core::mem::offset_of!(XRROutputInfo, subpixel_order), 50);
        assert_eq!(core::mem::offset_of!(XRROutputInfo, ncrtc), 52);
        assert_eq!(core::mem::offset_of!(XRROutputInfo, clones), 72);
        assert_eq!(core::mem::offset_of!(XRROutputInfo, modes), 88);
    }
}

pub const RR_Connected: Connection = 0;
pub const RR_Disconnected: Connection = 1;
pub const RR_UnknownConnection: Connection = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XRRModeInfo {
    pub id: RRMode,
    pub width: c_uint,
    pub height: c_uint,
    pub dotClock: c_ulong,
    pub hSyncStart: c_uint,
    pub hSyncEnd: c_uint,
    pub hTotal: c_uint,
    pub hSkew: c_uint,
    pub vSyncStart: c_uint,
    pub vSyncEnd: c_uint,
    pub vTotal: c_uint,
    pub name: *mut c_char,
    pub nameLength: c_uint,
    pub modeFlags: c_ulong,
}

#[repr(C)]
pub struct XRRScreenResources {
    pub timestamp: Time,
    pub configTimestamp: Time,
    pub ncrtc: c_int,
    pub crtcs: *mut RRCrtc,
    pub noutput: c_int,
    pub outputs: *mut RROutput,
    pub nmode: c_int,
    pub modes: *mut XRRModeInfo,
}

#[repr(C)]
pub struct XRRCrtcInfo {
    pub timestamp: Time,
    pub x: c_int,
    pub y: c_int,
    pub width: c_uint,
    pub height: c_uint,
    pub mode: RRMode,
    pub rotation: Rotation,
    pub noutput: c_int,
    pub outputs: *mut RROutput,
    pub rotations: Rotation,
    pub npossible: c_int,
    pub possible: *mut RROutput,
}

#[repr(C)]
pub struct XRROutputInfo {
    pub timestamp: Time,
    pub crtc: RRCrtc,
    pub name: *mut c_char,
    pub nameLen: c_int,
    pub mm_width: c_ulong,
    pub mm_height: c_ulong,
    pub connection: Connection,
    pub subpixel_order: SubpixelOrder,
    pub ncrtc: c_int,
    pub crtcs: *mut RRCrtc,
    pub nclone: c_int,
    pub clones: *mut RROutput,
    pub nmode: c_int,
    pub npreferred: c_int,
    pub modes: *mut RRMode,
}

#[repr(C)]
pub struct XRRCrtcGamma {
    pub size: c_int,
    pub red: *mut u16,
    pub green: *mut u16,
    pub blue: *mut u16,
}
