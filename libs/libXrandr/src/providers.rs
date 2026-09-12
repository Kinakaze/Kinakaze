//! Read-only provider discovery for the existing primary-output backend.
use super::*;

pub type RRProvider = usize;

#[repr(C)]
pub struct XRRProviderResources {
    pub timestamp: Time,
    pub nproviders: c_int,
    pub providers: *mut RRProvider,
}

#[repr(C)]
pub struct XRRProviderInfo {
    pub capabilities: c_uint,
    pub ncrtcs: c_int,
    pub crtcs: *mut RRCrtc,
    pub noutputs: c_int,
    pub outputs: *mut RROutput,
    pub name: *mut c_char,
    pub nassociatedproviders: c_int,
    pub associated_providers: *mut RRProvider,
    pub associated_capability: *mut c_uint,
    pub nameLen: c_int,
}

#[repr(C)]
struct ProviderResources {
    value: XRRProviderResources,
    provider: RRProvider,
}

const NAME: &[u8] = b"Windows primary output\0";

#[repr(C)]
struct ProviderInfo {
    value: XRRProviderInfo,
    crtc: RRCrtc,
    output: RROutput,
    name: [u8; NAME.len()],
}

#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetProviderResources")]
pub unsafe extern "sysv64" fn XRRGetProviderResources(
    _dpy: *mut Display,
    _window: Window,
) -> *mut XRRProviderResources {
    let available = current().is_some();
    let block = unsafe { allocate::<ProviderResources>() };
    if !block.is_null() && available {
        unsafe {
            (*block).provider = 1;
            (*block).value.nproviders = 1;
            (*block).value.providers = &raw mut (*block).provider;
        }
    }
    block.cast()
}

#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeProviderResources")]
pub unsafe extern "sysv64" fn XRRFreeProviderResources(resources: *mut XRRProviderResources) {
    unsafe { guest::free(resources.cast()) };
}

#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetProviderInfo")]
pub unsafe extern "sysv64" fn XRRGetProviderInfo(
    dpy: *mut Display,
    resources: *mut XRRScreenResources,
    provider: RRProvider,
) -> *mut XRRProviderInfo {
    if provider != 1 || current().is_none() {
        // RandR BadProvider is the extension's error base plus three.
        unsafe { kinakaze_libX11::errors::report(dpy, 179, 140, 33, provider) };
        return ptr::null_mut();
    }
    if resources.is_null() {
        return ptr::null_mut();
    }
    let block = unsafe { allocate::<ProviderInfo>() };
    if !block.is_null() {
        unsafe {
            let b = &mut *block;
            b.crtc = 1;
            b.output = 1;
            b.name.copy_from_slice(NAME);
            b.value.ncrtcs = 1;
            b.value.crtcs = &raw mut b.crtc;
            b.value.noutputs = 1;
            b.value.outputs = &raw mut b.output;
            b.value.name = b.name.as_mut_ptr().cast();
            b.value.nameLen = (NAME.len() - 1) as c_int;
            // No PRIME output sourcing, render offload, or associated GPUs.
            // Keep the protocol version at 1.3 until all 1.4 requests exist.
        }
    }
    block.cast()
}

#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeProviderInfo")]
pub unsafe extern "sysv64" fn XRRFreeProviderInfo(info: *mut XRRProviderInfo) {
    unsafe { guest::free(info.cast()) };
}
