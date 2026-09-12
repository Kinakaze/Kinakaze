//! `libGL.dll`: native OpenGL passthrough from a Linux guest to the host GPU driver.
//!
//! Maps Linux `libGL.so.1` / `libGLX.so.0` / `libEGL.so.1` entry points to the host's
//! `opengl32.dll`, performing calling convention translation between System V AMD64
//! and Windows x64.
//!
//! Backed by Win32 `HWND` / `HDC` and native WGL context management, allowing full
//! hardware-accelerated OpenGL rendering and presentation.

#![allow(non_snake_case)]

extern crate kinakaze_libdisplay;
mod pixmap;
mod publish;

use core::ffi::{c_char, c_double, c_float, c_int, c_uchar, c_uint, c_void};
use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};
use windows_sys::Win32::Foundation::{HMODULE, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetDC, HDC, ReleaseDC};
use windows_sys::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, GetPixelFormat, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE,
    PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR, SetPixelFormat, SwapBuffers,
    wglCreateContext, wglDeleteContext, wglGetCurrentContext, wglGetProcAddress, wglMakeCurrent,
    wglShareLists,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW, GetProcAddress, LoadLibraryA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

// A GUI process may have closed or redirected standard handles. Diagnostics
// must never unwind through the guest ABI, and remain absent from hot paths
// unless explicitly requested.
macro_rules! gl_trace {
    ($($argument:tt)*) => {
        if trace_enabled() {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr().lock(), $($argument)*);
        }
    };
}

#[derive(Clone, Copy)]
struct SyncModule(*mut c_void);
unsafe impl Sync for SyncModule {}
unsafe impl Send for SyncModule {}

static OPENGL32: OnceLock<SyncModule> = OnceLock::new();

fn host_module() -> *mut c_void {
    OPENGL32
        .get_or_init(|| unsafe {
            SyncModule(LoadLibraryA(b"opengl32.dll\0".as_ptr()) as *mut c_void)
        })
        .0
}

fn provider_module() -> *mut c_void {
    static MODULE: OnceLock<SyncModule> = OnceLock::new();
    MODULE
        .get_or_init(|| {
            let mut module: HMODULE = core::ptr::null_mut();
            let found = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    provider_module as *const () as *const u16,
                    &raw mut module,
                )
            };
            SyncModule(if found != 0 {
                module as *mut c_void
            } else {
                core::ptr::null_mut()
            })
        })
        .0
}

fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_GL_TRACE").is_some())
}

fn valid_driver_address(address: *mut c_void) -> bool {
    !address.is_null() && !matches!(address as usize, 1 | 2 | 3 | usize::MAX)
}

unsafe fn native_gl_address(symbol: *const u8) -> *mut c_void {
    let module = host_module();
    if !module.is_null()
        && let Some(address) = unsafe { GetProcAddress(module, symbol) }
    {
        return address as *const () as *mut c_void;
    }
    let address = unsafe { wglGetProcAddress(symbol) }
        .map(|function| function as *const () as *mut c_void)
        .unwrap_or(core::ptr::null_mut());
    if valid_driver_address(address) {
        address
    } else {
        core::ptr::null_mut()
    }
}

#[cold]
unsafe fn resolve_gl_function(cell: &AtomicPtr<c_void>, symbol: *const u8) -> *mut c_void {
    let found = unsafe { native_gl_address(symbol) };
    if !valid_driver_address(found) {
        return core::ptr::null_mut();
    }
    match cell.compare_exchange(
        core::ptr::null_mut(),
        found,
        Ordering::Relaxed,
        Ordering::Relaxed,
    ) {
        Ok(_) => found,
        Err(published) => published,
    }
}

macro_rules! gl_fn {
    ($name:ident ( $($arg:ident : $arg_ty:ty),* ) -> $ret:ty) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libGL_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name( $($arg : $arg_ty),* ) -> $ret {
            type HostFn = unsafe extern "system" fn( $($arg_ty),* ) -> $ret;
            static FN_PTR: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
            let mut address = FN_PTR.load(Ordering::Relaxed);
            if address.is_null() {
                let symbol = concat!(stringify!($name), "\0");
                address = unsafe { resolve_gl_function(&FN_PTR, symbol.as_ptr()) };
            }
            if !address.is_null() {
                let function: HostFn = unsafe { core::mem::transmute(address) };
                return unsafe { function( $($arg),* ) };
            }
            Default::default()
        }
    };
    ($name:ident ( $($arg:ident : $arg_ty:ty),* )) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libGL_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name( $($arg : $arg_ty),* ) {
            type HostFn = unsafe extern "system" fn( $($arg_ty),* );
            static FN_PTR: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
            let mut address = FN_PTR.load(Ordering::Relaxed);
            if address.is_null() {
                let symbol = concat!(stringify!($name), "\0");
                address = unsafe { resolve_gl_function(&FN_PTR, symbol.as_ptr()) };
            }
            if !address.is_null() {
                let function: HostFn = unsafe { core::mem::transmute(address) };
                unsafe { function( $($arg),* ) }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Standard Core OpenGL 1.1 / 1.2 / 2.0 API Forwarders
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libGL_glViewport")]
pub unsafe extern "sysv64" fn glViewport(x: c_int, y: c_int, width: c_int, height: c_int) {
    type HostFn = unsafe extern "system" fn(c_int, c_int, c_int, c_int);
    static FUNCTION: OnceLock<Option<HostFn>> = OnceLock::new();
    let function = FUNCTION.get_or_init(|| unsafe {
        let module = host_module();
        (!module.is_null())
            .then(|| GetProcAddress(module, b"glViewport\0".as_ptr()))
            .flatten()
            .map(|address| core::mem::transmute(address))
    });
    if trace_enabled() {
        gl_trace!("[libGL] glViewport({x}, {y}, {width}, {height})");
    }
    if let Some(function) = function {
        unsafe { function(x, y, width, height) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glFrustum")]
pub unsafe extern "sysv64" fn glFrustum(
    left: c_double,
    right: c_double,
    bottom: c_double,
    top: c_double,
    near_val: c_double,
    far_val: c_double,
) {
    type HostFn =
        unsafe extern "system" fn(c_double, c_double, c_double, c_double, c_double, c_double);
    static FUNCTION: OnceLock<Option<HostFn>> = OnceLock::new();
    let function = FUNCTION.get_or_init(|| unsafe {
        let module = host_module();
        (!module.is_null())
            .then(|| GetProcAddress(module, b"glFrustum\0".as_ptr()))
            .flatten()
            .map(|address| core::mem::transmute(address))
    });
    if trace_enabled() {
        gl_trace!("[libGL] glFrustum({left}, {right}, {bottom}, {top}, {near_val}, {far_val})");
    }
    if let Some(function) = function {
        unsafe { function(left, right, bottom, top, near_val, far_val) };
    }
}
gl_fn!(glGenLists(range: c_int) -> c_uint);
gl_fn!(glNewList(list: c_uint, mode: c_uint));
gl_fn!(glEndList());
gl_fn!(glCallList(list: c_uint));
gl_fn!(glClearColor(red: c_float, green: c_float, blue: c_float, alpha: c_float));
gl_fn!(glClear(mask: c_uint));
gl_fn!(glClearDepth(depth: c_double));
gl_fn!(glClearStencil(s: c_int));
gl_fn!(glDrawBuffer(mode: c_uint));
gl_fn!(glReadBuffer(mode: c_uint));
gl_fn!(glEnable(cap: c_uint));
gl_fn!(glDisable(cap: c_uint));
gl_fn!(glIsEnabled(cap: c_uint) -> c_uchar);
gl_fn!(glBegin(mode: c_uint));
gl_fn!(glEnd());
gl_fn!(glVertex2f(x: c_float, y: c_float));
gl_fn!(glVertex3f(x: c_float, y: c_float, z: c_float));
gl_fn!(glVertex4f(x: c_float, y: c_float, z: c_float, w: c_float));
gl_fn!(glColor3f(red: c_float, green: c_float, blue: c_float));
gl_fn!(glColor4f(red: c_float, green: c_float, blue: c_float, alpha: c_float));
gl_fn!(glColor4ub(red: c_uchar, green: c_uchar, blue: c_uchar, alpha: c_uchar));
gl_fn!(glTexCoord2f(s: c_float, t: c_float));
gl_fn!(glNormal3f(nx: c_float, ny: c_float, nz: c_float));
gl_fn!(glDrawArrays(mode: c_uint, first: c_int, count: c_int));
gl_fn!(glDrawElements(mode: c_uint, count: c_int, type_: c_uint, indices: *const c_void));
gl_fn!(glBindTexture(target: c_uint, texture: c_uint));
gl_fn!(glGenTextures(n: c_int, textures: *mut c_uint));
gl_fn!(glDeleteTextures(n: c_int, textures: *const c_uint));
gl_fn!(glTexParameteri(target: c_uint, pname: c_uint, param: c_int));
gl_fn!(glTexParameterf(target: c_uint, pname: c_uint, param: c_float));
gl_fn!(glTexImage2D(target: c_uint, level: c_int, internalformat: c_int, width: c_int, height: c_int, border: c_int, format: c_uint, type_: c_uint, pixels: *const c_void));
gl_fn!(glTexSubImage2D(target: c_uint, level: c_int, xoffset: c_int, yoffset: c_int, width: c_int, height: c_int, format: c_uint, type_: c_uint, pixels: *const c_void));
gl_fn!(glCopyTexImage2D(target: c_uint, level: c_int, internalformat: c_uint, x: c_int, y: c_int, width: c_int, height: c_int, border: c_int));
gl_fn!(glBlendFunc(sfactor: c_uint, dfactor: c_uint));
gl_fn!(glDepthFunc(func: c_uint));
gl_fn!(glDepthMask(flag: c_uchar));
gl_fn!(glCullFace(mode: c_uint));
gl_fn!(glFrontFace(mode: c_uint));
gl_fn!(glPointSize(size: c_float));
gl_fn!(glLineWidth(width: c_float));
gl_fn!(glPolygonMode(face: c_uint, mode: c_uint));
gl_fn!(glScissor(x: c_int, y: c_int, width: c_int, height: c_int));
gl_fn!(glFlush());
gl_fn!(glFinish());
gl_fn!(glGetError() -> c_uint);
gl_fn!(glGetString(name: c_uint) -> *const c_uchar);
gl_fn!(glGetIntegerv(pname: c_uint, data: *mut c_int));
gl_fn!(glGetFloatv(pname: c_uint, data: *mut c_float));
gl_fn!(glGetBooleanv(pname: c_uint, data: *mut c_uchar));
gl_fn!(glPixelStorei(pname: c_uint, param: c_int));
gl_fn!(glReadPixels(x: c_int, y: c_int, width: c_int, height: c_int, format: c_uint, type_: c_uint, pixels: *mut c_void));
gl_fn!(glMatrixMode(mode: c_uint));
gl_fn!(glLoadIdentity());
gl_fn!(glPushMatrix());
gl_fn!(glPopMatrix());
gl_fn!(glTranslatef(x: c_float, y: c_float, z: c_float));
gl_fn!(glRotatef(angle: c_float, x: c_float, y: c_float, z: c_float));
gl_fn!(glScalef(x: c_float, y: c_float, z: c_float));
gl_fn!(glOrtho(left: c_double, right: c_double, bottom: c_double, top: c_double, near_val: c_double, far_val: c_double));
gl_fn!(glLightfv(light: c_uint, pname: c_uint, params: *const c_float));
gl_fn!(glLightf(light: c_uint, pname: c_uint, param: c_float));
gl_fn!(glLightModelfv(pname: c_uint, params: *const c_float));
gl_fn!(glLightModeli(pname: c_uint, param: c_int));
gl_fn!(glMaterialfv(face: c_uint, pname: c_uint, params: *const c_float));
gl_fn!(glMaterialf(face: c_uint, pname: c_uint, param: c_float));
gl_fn!(glShadeModel(mode: c_uint));
gl_fn!(glDeleteLists(list: c_uint, range: c_int));
gl_fn!(glNormal3fv(v: *const c_float));
gl_fn!(glVertex3fv(v: *const c_float));
gl_fn!(glColor3fv(v: *const c_float));
gl_fn!(glColor4fv(v: *const c_float));
gl_fn!(glEnableClientState(array: c_uint));
gl_fn!(glDisableClientState(array: c_uint));
gl_fn!(glVertexPointer(size: c_int, type_: c_uint, stride: c_int, ptr: *const c_void));
gl_fn!(glNormalPointer(type_: c_uint, stride: c_int, ptr: *const c_void));
gl_fn!(glColorPointer(size: c_int, type_: c_uint, stride: c_int, ptr: *const c_void));
gl_fn!(glTexCoordPointer(size: c_int, type_: c_uint, stride: c_int, ptr: *const c_void));

// Compatibility contexts still use texture-coordinate generation (including
// Firefox's desktop GL function table). Preserve the scalar/vector and float/
// double/integer ABIs; the native driver owns the state and its validation.
gl_fn!(glTexGend(coord: c_uint, pname: c_uint, param: c_double));
gl_fn!(glTexGenf(coord: c_uint, pname: c_uint, param: c_float));
gl_fn!(glTexGeni(coord: c_uint, pname: c_uint, param: c_int));
gl_fn!(glTexGendv(coord: c_uint, pname: c_uint, params: *const c_double));
gl_fn!(glTexGenfv(coord: c_uint, pname: c_uint, params: *const c_float));
gl_fn!(glTexGeniv(coord: c_uint, pname: c_uint, params: *const c_int));
gl_fn!(glGetTexGendv(coord: c_uint, pname: c_uint, params: *mut c_double));
gl_fn!(glGetTexGenfv(coord: c_uint, pname: c_uint, params: *mut c_float));
gl_fn!(glGetTexGeniv(coord: c_uint, pname: c_uint, params: *mut c_int));

// OpenGL 2.x entry points used by GLES2 programs. WGL exposes these through
// wglGetProcAddress once a context is current; each exported wrapper is SysV so
// a Linux guest never calls a raw Windows-ABI function pointer.
gl_fn!(glCreateShader(shader_type: c_uint) -> c_uint);
gl_fn!(glCompileShader(shader: c_uint));
gl_fn!(glGetShaderiv(shader: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetShaderInfoLog(shader: c_uint, max_length: c_int, length: *mut c_int, info_log: *mut c_char));
gl_fn!(glDeleteShader(shader: c_uint));
gl_fn!(glCreateProgram() -> c_uint);
gl_fn!(glAttachShader(program: c_uint, shader: c_uint));
gl_fn!(glBindAttribLocation(program: c_uint, index: c_uint, name: *const c_char));
gl_fn!(glLinkProgram(program: c_uint));
gl_fn!(glGetProgramiv(program: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetProgramInfoLog(program: c_uint, max_length: c_int, length: *mut c_int, info_log: *mut c_char));
gl_fn!(glUseProgram(program: c_uint));
gl_fn!(glDeleteProgram(program: c_uint));
gl_fn!(glGetUniformLocation(program: c_uint, name: *const c_char) -> c_int);
gl_fn!(glGetAttribLocation(program: c_uint, name: *const c_char) -> c_int);
gl_fn!(glUniform1i(location: c_int, value: c_int));
gl_fn!(glUniform4fv(location: c_int, count: c_int, value: *const c_float));
gl_fn!(glUniformMatrix4fv(location: c_int, count: c_int, transpose: c_uchar, value: *const c_float));
gl_fn!(glEnableVertexAttribArray(index: c_uint));
gl_fn!(glDisableVertexAttribArray(index: c_uint));
gl_fn!(glVertexAttribPointer(index: c_uint, size: c_int, type_: c_uint, normalized: c_uchar, stride: c_int, pointer: *const c_void));
gl_fn!(glGenBuffers(n: c_int, buffers: *mut c_uint));
gl_fn!(glBindBuffer(target: c_uint, buffer: c_uint));
gl_fn!(glBufferData(target: c_uint, size: isize, data: *const c_void, usage: c_uint));
gl_fn!(glDeleteBuffers(n: c_int, buffers: *const c_uint));

// Modern desktop OpenGL entry points used by GL 2.x/3.x applications and LWJGL.
// These are deliberately thin ABI bridges: all resources and commands remain in
// the native ICD, so adding an entry point does not introduce a copy or software
// rendering path.
gl_fn!(glActiveTexture(texture: c_uint));
gl_fn!(glClientActiveTexture(texture: c_uint));
gl_fn!(glBlendColor(red: c_float, green: c_float, blue: c_float, alpha: c_float));
gl_fn!(glBlendEquation(mode: c_uint));
gl_fn!(glBlendEquationSeparate(mode_rgb: c_uint, mode_alpha: c_uint));
gl_fn!(glBlendFuncSeparate(src_rgb: c_uint, dst_rgb: c_uint, src_alpha: c_uint, dst_alpha: c_uint));
gl_fn!(glColorMask(red: c_uchar, green: c_uchar, blue: c_uchar, alpha: c_uchar));
gl_fn!(glStencilFunc(func: c_uint, reference: c_int, mask: c_uint));
gl_fn!(glStencilMask(mask: c_uint));
gl_fn!(glStencilOp(fail: c_uint, zfail: c_uint, zpass: c_uint));
gl_fn!(glStencilFuncSeparate(face: c_uint, func: c_uint, reference: c_int, mask: c_uint));
gl_fn!(glStencilMaskSeparate(face: c_uint, mask: c_uint));
gl_fn!(glStencilOpSeparate(face: c_uint, sfail: c_uint, dpfail: c_uint, dppass: c_uint));
gl_fn!(glSampleCoverage(value: c_float, invert: c_uchar));
gl_fn!(glDrawRangeElements(mode: c_uint, start: c_uint, end: c_uint, count: c_int, type_: c_uint, indices: *const c_void));
gl_fn!(glDrawBuffers(count: c_int, buffers: *const c_uint));
gl_fn!(glMultiDrawArrays(mode: c_uint, first: *const c_int, count: *const c_int, draw_count: c_int));
gl_fn!(glMultiDrawElements(mode: c_uint, count: *const c_int, type_: c_uint, indices: *const *const c_void, draw_count: c_int));
gl_fn!(glTexImage3D(target: c_uint, level: c_int, internal_format: c_int, width: c_int, height: c_int, depth: c_int, border: c_int, format: c_uint, type_: c_uint, pixels: *const c_void));
gl_fn!(glTexSubImage3D(target: c_uint, level: c_int, x_offset: c_int, y_offset: c_int, z_offset: c_int, width: c_int, height: c_int, depth: c_int, format: c_uint, type_: c_uint, pixels: *const c_void));
gl_fn!(glCopyTexSubImage3D(target: c_uint, level: c_int, x_offset: c_int, y_offset: c_int, z_offset: c_int, x: c_int, y: c_int, width: c_int, height: c_int));
gl_fn!(glCompressedTexImage2D(target: c_uint, level: c_int, internal_format: c_uint, width: c_int, height: c_int, border: c_int, image_size: c_int, data: *const c_void));
gl_fn!(glCompressedTexSubImage2D(target: c_uint, level: c_int, x_offset: c_int, y_offset: c_int, width: c_int, height: c_int, format: c_uint, image_size: c_int, data: *const c_void));
gl_fn!(glGetCompressedTexImage(target: c_uint, level: c_int, image: *mut c_void));
gl_fn!(glGetTexImage(target: c_uint, level: c_int, format: c_uint, type_: c_uint, pixels: *mut c_void));
gl_fn!(glGetTexLevelParameteriv(target: c_uint, level: c_int, pname: c_uint, params: *mut c_int));
gl_fn!(glGetTexParameteriv(target: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetTexParameterfv(target: c_uint, pname: c_uint, params: *mut c_float));
gl_fn!(glBufferSubData(target: c_uint, offset: isize, size: isize, data: *const c_void));
gl_fn!(glBufferStorage(target: c_uint, size: isize, data: *const c_void, flags: c_uint));
gl_fn!(glNamedBufferStorage(buffer: c_uint, size: isize, data: *const c_void, flags: c_uint));
gl_fn!(glGetBufferSubData(target: c_uint, offset: isize, size: isize, data: *mut c_void));
gl_fn!(glMapBuffer(target: c_uint, access: c_uint) -> *mut c_void);
gl_fn!(glUnmapBuffer(target: c_uint) -> c_uchar);
gl_fn!(glGetBufferParameteriv(target: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetBufferPointerv(target: c_uint, pname: c_uint, params: *mut *mut c_void));
gl_fn!(glIsBuffer(buffer: c_uint) -> c_uchar);
gl_fn!(glGenQueries(count: c_int, ids: *mut c_uint));
gl_fn!(glDeleteQueries(count: c_int, ids: *const c_uint));
gl_fn!(glIsQuery(id: c_uint) -> c_uchar);
gl_fn!(glBeginQuery(target: c_uint, id: c_uint));
gl_fn!(glEndQuery(target: c_uint));
gl_fn!(glGetQueryiv(target: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetQueryObjectiv(id: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetQueryObjectuiv(id: c_uint, pname: c_uint, params: *mut c_uint));
gl_fn!(glDetachShader(program: c_uint, shader: c_uint));
gl_fn!(glIsShader(shader: c_uint) -> c_uchar);
gl_fn!(glIsProgram(program: c_uint) -> c_uchar);
gl_fn!(glValidateProgram(program: c_uint));
gl_fn!(glGetAttachedShaders(program: c_uint, max_count: c_int, count: *mut c_int, shaders: *mut c_uint));
gl_fn!(glGetActiveAttrib(program: c_uint, index: c_uint, buffer_size: c_int, length: *mut c_int, size: *mut c_int, type_: *mut c_uint, name: *mut c_char));
gl_fn!(glGetActiveUniform(program: c_uint, index: c_uint, buffer_size: c_int, length: *mut c_int, size: *mut c_int, type_: *mut c_uint, name: *mut c_char));
gl_fn!(glGetUniformfv(program: c_uint, location: c_int, params: *mut c_float));
gl_fn!(glGetUniformiv(program: c_uint, location: c_int, params: *mut c_int));
gl_fn!(glUniform1f(location: c_int, x: c_float));
gl_fn!(glUniform2f(location: c_int, x: c_float, y: c_float));
gl_fn!(glUniform3f(location: c_int, x: c_float, y: c_float, z: c_float));
gl_fn!(glUniform4f(location: c_int, x: c_float, y: c_float, z: c_float, w: c_float));
gl_fn!(glUniform2i(location: c_int, x: c_int, y: c_int));
gl_fn!(glUniform3i(location: c_int, x: c_int, y: c_int, z: c_int));
gl_fn!(glUniform4i(location: c_int, x: c_int, y: c_int, z: c_int, w: c_int));
gl_fn!(glUniform1fv(location: c_int, count: c_int, values: *const c_float));
gl_fn!(glUniform2fv(location: c_int, count: c_int, values: *const c_float));
gl_fn!(glUniform3fv(location: c_int, count: c_int, values: *const c_float));
gl_fn!(glUniform1iv(location: c_int, count: c_int, values: *const c_int));
gl_fn!(glUniform2iv(location: c_int, count: c_int, values: *const c_int));
gl_fn!(glUniform3iv(location: c_int, count: c_int, values: *const c_int));
gl_fn!(glUniform4iv(location: c_int, count: c_int, values: *const c_int));
gl_fn!(glUniformMatrix2fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix3fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix2x3fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix3x2fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix2x4fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix4x2fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix3x4fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glUniformMatrix4x3fv(location: c_int, count: c_int, transpose: c_uchar, values: *const c_float));
gl_fn!(glVertexAttrib1f(index: c_uint, x: c_float));
gl_fn!(glVertexAttrib2f(index: c_uint, x: c_float, y: c_float));
gl_fn!(glVertexAttrib3f(index: c_uint, x: c_float, y: c_float, z: c_float));
gl_fn!(glVertexAttrib4f(index: c_uint, x: c_float, y: c_float, z: c_float, w: c_float));
gl_fn!(glGetVertexAttribiv(index: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetVertexAttribfv(index: c_uint, pname: c_uint, params: *mut c_float));
gl_fn!(glGetVertexAttribPointerv(index: c_uint, pname: c_uint, pointer: *mut *mut c_void));

// Remaining OpenGL 1.1-3.3 core entry points. Keeping the complete advertised
// core surface is important because loaders treat a missing address as an
// unavailable function even if the render path reaches it only much later.
// These remain ABI-only bridges to the host ICD, like the wrappers above.
gl_fn!(glHint(target: c_uint, mode: c_uint));
gl_fn!(glLogicOp(opcode: c_uint));
gl_fn!(glPixelStoref(pname: c_uint, param: c_float));
gl_fn!(glPolygonOffset(factor: c_float, units: c_float));
gl_fn!(glDepthRange(near_value: c_double, far_value: c_double));
gl_fn!(glIsTexture(texture: c_uint) -> c_uchar);
gl_fn!(glGetDoublev(pname: c_uint, data: *mut c_double));
gl_fn!(glGetPointerv(pname: c_uint, data: *mut *mut c_void));
gl_fn!(glGetTexLevelParameterfv(target: c_uint, level: c_int, pname: c_uint, params: *mut c_float));
gl_fn!(glTexParameteriv(target: c_uint, pname: c_uint, params: *const c_int));
gl_fn!(glTexParameterfv(target: c_uint, pname: c_uint, params: *const c_float));
gl_fn!(glTexImage1D(target: c_uint, level: c_int, internal_format: c_int, width: c_int, border: c_int, format: c_uint, type_: c_uint, pixels: *const c_void));
gl_fn!(glTexSubImage1D(target: c_uint, level: c_int, x_offset: c_int, width: c_int, format: c_uint, type_: c_uint, pixels: *const c_void));
gl_fn!(glCopyTexImage1D(target: c_uint, level: c_int, internal_format: c_uint, x: c_int, y: c_int, width: c_int, border: c_int));
gl_fn!(glCopyTexSubImage1D(target: c_uint, level: c_int, x_offset: c_int, x: c_int, y: c_int, width: c_int));
gl_fn!(glCopyTexSubImage2D(target: c_uint, level: c_int, x_offset: c_int, y_offset: c_int, x: c_int, y: c_int, width: c_int, height: c_int));
gl_fn!(glCompressedTexImage1D(target: c_uint, level: c_int, internal_format: c_uint, width: c_int, border: c_int, image_size: c_int, data: *const c_void));
gl_fn!(glCompressedTexImage3D(target: c_uint, level: c_int, internal_format: c_uint, width: c_int, height: c_int, depth: c_int, border: c_int, image_size: c_int, data: *const c_void));
gl_fn!(glCompressedTexSubImage1D(target: c_uint, level: c_int, x_offset: c_int, width: c_int, format: c_uint, image_size: c_int, data: *const c_void));
gl_fn!(glCompressedTexSubImage3D(target: c_uint, level: c_int, x_offset: c_int, y_offset: c_int, z_offset: c_int, width: c_int, height: c_int, depth: c_int, format: c_uint, image_size: c_int, data: *const c_void));
gl_fn!(glGetShaderSource(shader: c_uint, buffer_size: c_int, length: *mut c_int, source: *mut c_char));
gl_fn!(glPointParameterf(pname: c_uint, param: c_float));
gl_fn!(glPointParameterfv(pname: c_uint, params: *const c_float));
gl_fn!(glPointParameteri(pname: c_uint, param: c_int));
gl_fn!(glPointParameteriv(pname: c_uint, params: *const c_int));
gl_fn!(glGetVertexAttribdv(index: c_uint, pname: c_uint, params: *mut c_double));
gl_fn!(glVertexAttrib1d(index: c_uint, x: c_double));
gl_fn!(glVertexAttrib1dv(index: c_uint, values: *const c_double));
gl_fn!(glVertexAttrib1fv(index: c_uint, values: *const c_float));
gl_fn!(glVertexAttrib1s(index: c_uint, x: i16));
gl_fn!(glVertexAttrib1sv(index: c_uint, values: *const i16));
gl_fn!(glVertexAttrib2d(index: c_uint, x: c_double, y: c_double));
gl_fn!(glVertexAttrib2dv(index: c_uint, values: *const c_double));
gl_fn!(glVertexAttrib2fv(index: c_uint, values: *const c_float));
gl_fn!(glVertexAttrib2s(index: c_uint, x: i16, y: i16));
gl_fn!(glVertexAttrib2sv(index: c_uint, values: *const i16));
gl_fn!(glVertexAttrib3d(index: c_uint, x: c_double, y: c_double, z: c_double));
gl_fn!(glVertexAttrib3dv(index: c_uint, values: *const c_double));
gl_fn!(glVertexAttrib3fv(index: c_uint, values: *const c_float));
gl_fn!(glVertexAttrib3s(index: c_uint, x: i16, y: i16, z: i16));
gl_fn!(glVertexAttrib3sv(index: c_uint, values: *const i16));
gl_fn!(glVertexAttrib4d(index: c_uint, x: c_double, y: c_double, z: c_double, w: c_double));
gl_fn!(glVertexAttrib4dv(index: c_uint, values: *const c_double));
gl_fn!(glVertexAttrib4fv(index: c_uint, values: *const c_float));
gl_fn!(glVertexAttrib4s(index: c_uint, x: i16, y: i16, z: i16, w: i16));
gl_fn!(glVertexAttrib4sv(index: c_uint, values: *const i16));
gl_fn!(glVertexAttrib4bv(index: c_uint, values: *const i8));
gl_fn!(glVertexAttrib4iv(index: c_uint, values: *const c_int));
gl_fn!(glVertexAttrib4ubv(index: c_uint, values: *const u8));
gl_fn!(glVertexAttrib4uiv(index: c_uint, values: *const c_uint));
gl_fn!(glVertexAttrib4usv(index: c_uint, values: *const u16));
gl_fn!(glVertexAttrib4Nub(index: c_uint, x: u8, y: u8, z: u8, w: u8));
gl_fn!(glVertexAttrib4Nbv(index: c_uint, values: *const i8));
gl_fn!(glVertexAttrib4Nsv(index: c_uint, values: *const i16));
gl_fn!(glVertexAttrib4Niv(index: c_uint, values: *const c_int));
gl_fn!(glVertexAttrib4Nubv(index: c_uint, values: *const u8));
gl_fn!(glVertexAttrib4Nusv(index: c_uint, values: *const u16));
gl_fn!(glVertexAttrib4Nuiv(index: c_uint, values: *const c_uint));
gl_fn!(glVertexAttribP1ui(index: c_uint, type_: c_uint, normalized: c_uchar, value: c_uint));
gl_fn!(glVertexAttribP2ui(index: c_uint, type_: c_uint, normalized: c_uchar, value: c_uint));
gl_fn!(glVertexAttribP3ui(index: c_uint, type_: c_uint, normalized: c_uchar, value: c_uint));
gl_fn!(glVertexAttribP4ui(index: c_uint, type_: c_uint, normalized: c_uchar, value: c_uint));
gl_fn!(glVertexAttribP1uiv(index: c_uint, type_: c_uint, normalized: c_uchar, value: *const c_uint));
gl_fn!(glVertexAttribP2uiv(index: c_uint, type_: c_uint, normalized: c_uchar, value: *const c_uint));
gl_fn!(glVertexAttribP3uiv(index: c_uint, type_: c_uint, normalized: c_uchar, value: *const c_uint));
gl_fn!(glVertexAttribP4uiv(index: c_uint, type_: c_uint, normalized: c_uchar, value: *const c_uint));
gl_fn!(glSamplerParameterIiv(sampler: c_uint, pname: c_uint, params: *const c_int));
gl_fn!(glSamplerParameterIuiv(sampler: c_uint, pname: c_uint, params: *const c_uint));
gl_fn!(glGetSamplerParameterIiv(sampler: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetSamplerParameterIuiv(sampler: c_uint, pname: c_uint, params: *mut c_uint));

// OpenGL 3.0-3.3 core surface.
gl_fn!(glColorMaski(index: c_uint, red: c_uchar, green: c_uchar, blue: c_uchar, alpha: c_uchar));
gl_fn!(glGetBooleani_v(target: c_uint, index: c_uint, data: *mut c_uchar));
gl_fn!(glGetIntegeri_v(target: c_uint, index: c_uint, data: *mut c_int));
gl_fn!(glEnablei(target: c_uint, index: c_uint));
gl_fn!(glDisablei(target: c_uint, index: c_uint));
gl_fn!(glIsEnabledi(target: c_uint, index: c_uint) -> c_uchar);
gl_fn!(glBindBufferRange(target: c_uint, index: c_uint, buffer: c_uint, offset: isize, size: isize));
gl_fn!(glBindBufferBase(target: c_uint, index: c_uint, buffer: c_uint));
gl_fn!(glBeginTransformFeedback(primitive_mode: c_uint));
gl_fn!(glEndTransformFeedback());
gl_fn!(glGenTransformFeedbacks(count: c_int, ids: *mut c_uint));
gl_fn!(glBindTransformFeedback(target: c_uint, id: c_uint));
gl_fn!(glDeleteTransformFeedbacks(count: c_int, ids: *const c_uint));
gl_fn!(glIsTransformFeedback(id: c_uint) -> c_uchar);
gl_fn!(glPauseTransformFeedback());
gl_fn!(glResumeTransformFeedback());
gl_fn!(glTransformFeedbackVaryings(program: c_uint, count: c_int, varyings: *const *const c_char, buffer_mode: c_uint));
gl_fn!(glGetTransformFeedbackVarying(program: c_uint, index: c_uint, buffer_size: c_int, length: *mut c_int, size: *mut c_int, type_: *mut c_uint, name: *mut c_char));
gl_fn!(glClampColor(target: c_uint, clamp: c_uint));
gl_fn!(glBeginConditionalRender(id: c_uint, mode: c_uint));
gl_fn!(glEndConditionalRender());
gl_fn!(glBindFragDataLocation(program: c_uint, color: c_uint, name: *const c_char));
gl_fn!(glGetFragDataLocation(program: c_uint, name: *const c_char) -> c_int);
gl_fn!(glUniform1ui(location: c_int, value: c_uint));
gl_fn!(glUniform2ui(location: c_int, x: c_uint, y: c_uint));
gl_fn!(glUniform3ui(location: c_int, x: c_uint, y: c_uint, z: c_uint));
gl_fn!(glUniform4ui(location: c_int, x: c_uint, y: c_uint, z: c_uint, w: c_uint));
gl_fn!(glUniform1uiv(location: c_int, count: c_int, values: *const c_uint));
gl_fn!(glUniform2uiv(location: c_int, count: c_int, values: *const c_uint));
gl_fn!(glUniform3uiv(location: c_int, count: c_int, values: *const c_uint));
gl_fn!(glUniform4uiv(location: c_int, count: c_int, values: *const c_uint));
gl_fn!(glGetUniformuiv(program: c_uint, location: c_int, params: *mut c_uint));
gl_fn!(glVertexAttribIPointer(index: c_uint, size: c_int, type_: c_uint, stride: c_int, pointer: *const c_void));
gl_fn!(glGetVertexAttribIiv(index: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetVertexAttribIuiv(index: c_uint, pname: c_uint, params: *mut c_uint));
gl_fn!(glVertexAttribI1i(index: c_uint, x: c_int));
gl_fn!(glVertexAttribI2i(index: c_uint, x: c_int, y: c_int));
gl_fn!(glVertexAttribI3i(index: c_uint, x: c_int, y: c_int, z: c_int));
gl_fn!(glVertexAttribI4i(index: c_uint, x: c_int, y: c_int, z: c_int, w: c_int));
gl_fn!(glVertexAttribI1ui(index: c_uint, x: c_uint));
gl_fn!(glVertexAttribI2ui(index: c_uint, x: c_uint, y: c_uint));
gl_fn!(glVertexAttribI3ui(index: c_uint, x: c_uint, y: c_uint, z: c_uint));
gl_fn!(glVertexAttribI4ui(index: c_uint, x: c_uint, y: c_uint, z: c_uint, w: c_uint));
gl_fn!(glVertexAttribI1iv(index: c_uint, values: *const c_int));
gl_fn!(glVertexAttribI2iv(index: c_uint, values: *const c_int));
gl_fn!(glVertexAttribI3iv(index: c_uint, values: *const c_int));
gl_fn!(glVertexAttribI4iv(index: c_uint, values: *const c_int));
gl_fn!(glVertexAttribI1uiv(index: c_uint, values: *const c_uint));
gl_fn!(glVertexAttribI2uiv(index: c_uint, values: *const c_uint));
gl_fn!(glVertexAttribI3uiv(index: c_uint, values: *const c_uint));
gl_fn!(glVertexAttribI4uiv(index: c_uint, values: *const c_uint));
gl_fn!(glVertexAttribI4bv(index: c_uint, values: *const i8));
gl_fn!(glVertexAttribI4sv(index: c_uint, values: *const i16));
gl_fn!(glVertexAttribI4ubv(index: c_uint, values: *const u8));
gl_fn!(glVertexAttribI4usv(index: c_uint, values: *const u16));
gl_fn!(glTexParameterIiv(target: c_uint, pname: c_uint, params: *const c_int));
gl_fn!(glTexParameterIuiv(target: c_uint, pname: c_uint, params: *const c_uint));
gl_fn!(glGetTexParameterIiv(target: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetTexParameterIuiv(target: c_uint, pname: c_uint, params: *mut c_uint));
gl_fn!(glClearBufferiv(buffer: c_uint, draw_buffer: c_int, value: *const c_int));
gl_fn!(glClearBufferuiv(buffer: c_uint, draw_buffer: c_int, value: *const c_uint));
gl_fn!(glClearBufferfv(buffer: c_uint, draw_buffer: c_int, value: *const c_float));
gl_fn!(glClearBufferfi(buffer: c_uint, draw_buffer: c_int, depth: c_float, stencil: c_int));
gl_fn!(glGetStringi(name: c_uint, index: c_uint) -> *const c_uchar);
gl_fn!(glIsRenderbuffer(renderbuffer: c_uint) -> c_uchar);
gl_fn!(glBindRenderbuffer(target: c_uint, renderbuffer: c_uint));
gl_fn!(glDeleteRenderbuffers(count: c_int, renderbuffers: *const c_uint));
gl_fn!(glGenRenderbuffers(count: c_int, renderbuffers: *mut c_uint));
gl_fn!(glRenderbufferStorage(target: c_uint, internal_format: c_uint, width: c_int, height: c_int));
gl_fn!(glGetRenderbufferParameteriv(target: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glIsFramebuffer(framebuffer: c_uint) -> c_uchar);
gl_fn!(glBindFramebuffer(target: c_uint, framebuffer: c_uint));
gl_fn!(glDeleteFramebuffers(count: c_int, framebuffers: *const c_uint));
gl_fn!(glGenFramebuffers(count: c_int, framebuffers: *mut c_uint));
gl_fn!(glCheckFramebufferStatus(target: c_uint) -> c_uint);
gl_fn!(glFramebufferTexture1D(target: c_uint, attachment: c_uint, textarget: c_uint, texture: c_uint, level: c_int));
gl_fn!(glFramebufferTexture2D(target: c_uint, attachment: c_uint, textarget: c_uint, texture: c_uint, level: c_int));
gl_fn!(glFramebufferTexture3D(target: c_uint, attachment: c_uint, textarget: c_uint, texture: c_uint, level: c_int, layer: c_int));
gl_fn!(glFramebufferRenderbuffer(target: c_uint, attachment: c_uint, renderbuffer_target: c_uint, renderbuffer: c_uint));
gl_fn!(glGetFramebufferAttachmentParameteriv(target: c_uint, attachment: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGenerateMipmap(target: c_uint));
gl_fn!(glBlitFramebuffer(src_x0: c_int, src_y0: c_int, src_x1: c_int, src_y1: c_int, dst_x0: c_int, dst_y0: c_int, dst_x1: c_int, dst_y1: c_int, mask: c_uint, filter: c_uint));
gl_fn!(glRenderbufferStorageMultisample(target: c_uint, samples: c_int, internal_format: c_uint, width: c_int, height: c_int));
gl_fn!(glFramebufferTextureLayer(target: c_uint, attachment: c_uint, texture: c_uint, level: c_int, layer: c_int));
gl_fn!(glMapBufferRange(target: c_uint, offset: isize, length: isize, access: c_uint) -> *mut c_void);
gl_fn!(glFlushMappedBufferRange(target: c_uint, offset: isize, length: isize));
gl_fn!(glMemoryBarrier(barriers: c_uint));
gl_fn!(glTexStorage2D(target: c_uint, levels: c_int, internal_format: c_uint, width: c_int, height: c_int));
gl_fn!(glTexStorage3D(target: c_uint, levels: c_int, internal_format: c_uint, width: c_int, height: c_int, depth: c_int));
gl_fn!(glGetInternalformativ(target: c_uint, internal_format: c_uint, pname: c_uint, count: c_int, params: *mut c_int));
gl_fn!(glInvalidateFramebuffer(target: c_uint, count: c_int, attachments: *const c_uint));
gl_fn!(glInvalidateSubFramebuffer(target: c_uint, count: c_int, attachments: *const c_uint, x: c_int, y: c_int, width: c_int, height: c_int));
gl_fn!(glBindVertexArray(array: c_uint));
gl_fn!(glDeleteVertexArrays(count: c_int, arrays: *const c_uint));
gl_fn!(glGenVertexArrays(count: c_int, arrays: *mut c_uint));
gl_fn!(glIsVertexArray(array: c_uint) -> c_uchar);
gl_fn!(glDrawArraysInstanced(mode: c_uint, first: c_int, count: c_int, instance_count: c_int));
gl_fn!(glDrawElementsInstanced(mode: c_uint, count: c_int, type_: c_uint, indices: *const c_void, instance_count: c_int));
gl_fn!(glCopyBufferSubData(read_target: c_uint, write_target: c_uint, read_offset: isize, write_offset: isize, size: isize));
gl_fn!(glPrimitiveRestartIndex(index: c_uint));
gl_fn!(glTexBuffer(target: c_uint, internal_format: c_uint, buffer: c_uint));
gl_fn!(glGetUniformIndices(program: c_uint, uniform_count: c_int, uniform_names: *const *const c_char, uniform_indices: *mut c_uint));
gl_fn!(glGetActiveUniformsiv(program: c_uint, uniform_count: c_int, uniform_indices: *const c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetActiveUniformName(program: c_uint, uniform_index: c_uint, buffer_size: c_int, length: *mut c_int, uniform_name: *mut c_char));
gl_fn!(glGetUniformBlockIndex(program: c_uint, uniform_block_name: *const c_char) -> c_uint);
gl_fn!(glGetActiveUniformBlockiv(program: c_uint, uniform_block_index: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetActiveUniformBlockName(program: c_uint, uniform_block_index: c_uint, buffer_size: c_int, length: *mut c_int, uniform_block_name: *mut c_char));
gl_fn!(glUniformBlockBinding(program: c_uint, uniform_block_index: c_uint, uniform_block_binding: c_uint));
gl_fn!(glDrawElementsBaseVertex(mode: c_uint, count: c_int, type_: c_uint, indices: *const c_void, base_vertex: c_int));
gl_fn!(glDrawRangeElementsBaseVertex(mode: c_uint, start: c_uint, end: c_uint, count: c_int, type_: c_uint, indices: *const c_void, base_vertex: c_int));
gl_fn!(glDrawElementsInstancedBaseVertex(mode: c_uint, count: c_int, type_: c_uint, indices: *const c_void, instance_count: c_int, base_vertex: c_int));
gl_fn!(glMultiDrawElementsBaseVertex(mode: c_uint, count: *const c_int, type_: c_uint, indices: *const *const c_void, draw_count: c_int, base_vertex: *const c_int));
gl_fn!(glProvokingVertex(mode: c_uint));
gl_fn!(glFenceSync(condition: c_uint, flags: c_uint) -> *mut c_void);
gl_fn!(glIsSync(sync: *mut c_void) -> c_uchar);
gl_fn!(glDeleteSync(sync: *mut c_void));
gl_fn!(glClientWaitSync(sync: *mut c_void, flags: c_uint, timeout: u64) -> c_uint);
gl_fn!(glWaitSync(sync: *mut c_void, flags: c_uint, timeout: u64));
gl_fn!(glGetInteger64v(pname: c_uint, data: *mut i64));
gl_fn!(glGetInteger64i_v(target: c_uint, index: c_uint, data: *mut i64));
gl_fn!(glGetBufferParameteri64v(target: c_uint, pname: c_uint, params: *mut i64));
gl_fn!(glGetSynciv(sync: *mut c_void, pname: c_uint, buffer_size: c_int, length: *mut c_int, values: *mut c_int));
gl_fn!(glTexImage2DMultisample(target: c_uint, samples: c_int, internal_format: c_uint, width: c_int, height: c_int, fixed_sample_locations: c_uchar));
gl_fn!(glTexImage3DMultisample(target: c_uint, samples: c_int, internal_format: c_uint, width: c_int, height: c_int, depth: c_int, fixed_sample_locations: c_uchar));
gl_fn!(glGetMultisamplefv(pname: c_uint, index: c_uint, value: *mut c_float));
gl_fn!(glSampleMaski(mask_number: c_uint, mask: c_uint));
gl_fn!(glFramebufferTexture(target: c_uint, attachment: c_uint, texture: c_uint, level: c_int));
gl_fn!(glBindFragDataLocationIndexed(program: c_uint, color_number: c_uint, index: c_uint, name: *const c_char));
gl_fn!(glGetFragDataIndex(program: c_uint, name: *const c_char) -> c_int);
gl_fn!(glGenSamplers(count: c_int, samplers: *mut c_uint));
gl_fn!(glDeleteSamplers(count: c_int, samplers: *const c_uint));
gl_fn!(glIsSampler(sampler: c_uint) -> c_uchar);
gl_fn!(glBindSampler(unit: c_uint, sampler: c_uint));
gl_fn!(glSamplerParameteri(sampler: c_uint, pname: c_uint, param: c_int));
gl_fn!(glSamplerParameterf(sampler: c_uint, pname: c_uint, param: c_float));
gl_fn!(glSamplerParameteriv(sampler: c_uint, pname: c_uint, params: *const c_int));
gl_fn!(glSamplerParameterfv(sampler: c_uint, pname: c_uint, params: *const c_float));
gl_fn!(glGetSamplerParameteriv(sampler: c_uint, pname: c_uint, params: *mut c_int));
gl_fn!(glGetSamplerParameterfv(sampler: c_uint, pname: c_uint, params: *mut c_float));
gl_fn!(glQueryCounter(id: c_uint, target: c_uint));
gl_fn!(glGetQueryObjecti64v(id: c_uint, pname: c_uint, params: *mut i64));
gl_fn!(glGetQueryObjectui64v(id: c_uint, pname: c_uint, params: *mut u64));
gl_fn!(glVertexAttribDivisor(index: c_uint, divisor: c_uint));
gl_fn!(glVertexAttribDivisorARB(index: c_uint, divisor: c_uint));
gl_fn!(glBlendEquationSeparatei(buffer: c_uint, rgb: c_uint, alpha: c_uint));
gl_fn!(glBlendFuncSeparatei(buffer: c_uint, source_rgb: c_uint, dest_rgb: c_uint, source_alpha: c_uint, dest_alpha: c_uint));
gl_fn!(glObjectLabel(identifier: c_uint, name: c_uint, length: c_int, label: *const c_char));
gl_fn!(glGetObjectLabel(identifier: c_uint, name: c_uint, buffer_size: c_int, length: *mut c_int, label: *mut c_char));
gl_fn!(glDebugMessageControl(source: c_uint, type_: c_uint, severity: c_uint, count: c_int, ids: *const c_uint, enabled: c_uchar));
gl_fn!(glDebugMessageInsert(source: c_uint, type_: c_uint, id: c_uint, severity: c_uint, length: c_int, message: *const c_char));
gl_fn!(glPushDebugGroup(source: c_uint, id: c_uint, length: c_int, message: *const c_char));
gl_fn!(glPopDebugGroup());

type GuestDebugCallback =
    unsafe extern "sysv64" fn(c_uint, c_uint, c_uint, c_uint, c_int, *const c_char, *mut c_void);
type HostDebugCallback =
    unsafe extern "system" fn(c_uint, c_uint, c_uint, c_uint, c_int, *const c_char, *const c_void);

#[derive(Clone, Copy)]
struct DebugCallbackState {
    callback: *mut c_void,
    user: *mut c_void,
}

unsafe impl Send for DebugCallbackState {}
unsafe impl Sync for DebugCallbackState {}

fn debug_callbacks()
-> &'static std::sync::Mutex<std::collections::HashMap<usize, DebugCallbackState>> {
    static CALLBACKS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<usize, DebugCallbackState>>,
    > = OnceLock::new();
    CALLBACKS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

unsafe extern "system" fn host_debug_callback(
    source: c_uint,
    type_: c_uint,
    id: c_uint,
    severity: c_uint,
    length: c_int,
    message: *const c_char,
    context_token: *const c_void,
) {
    let state = debug_callbacks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&(context_token as usize))
        .copied();
    if let Some(state) = state {
        let callback: GuestDebugCallback = unsafe { core::mem::transmute(state.callback) };
        unsafe { callback(source, type_, id, severity, length, message, state.user) };
    }
}

fn precision_token_in(bytes: &[u8], matched: &mut usize) -> bool {
    const TOKEN: &[u8] = b"precision ";
    if *matched == TOKEN.len() {
        return true;
    }
    for &byte in bytes {
        *matched = if byte == TOKEN[*matched] {
            *matched + 1
        } else if byte == TOKEN[0] {
            1
        } else {
            0
        };
        if *matched == TOKEN.len() {
            return true;
        }
    }
    false
}

unsafe fn set_debug_callback(native_symbol: *const u8, callback: *mut c_void, user: *mut c_void) {
    type SetCallback = unsafe extern "system" fn(Option<HostDebugCallback>, *const c_void);
    let address = unsafe { native_gl_address(native_symbol) };
    if !valid_driver_address(address) {
        return;
    }
    let context = CURRENT_CONTEXT.get();
    let function: SetCallback = unsafe { core::mem::transmute(address) };
    if callback.is_null() {
        debug_callbacks()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&context);
        unsafe { function(None, core::ptr::null()) };
    } else {
        debug_callbacks()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(context, DebugCallbackState { callback, user });
        unsafe { function(Some(host_debug_callback), context as *const c_void) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glDebugMessageCallback")]
pub unsafe extern "sysv64" fn glDebugMessageCallback(callback: *mut c_void, user: *mut c_void) {
    unsafe { set_debug_callback(b"glDebugMessageCallback\0".as_ptr(), callback, user) };
}

#[unsafe(export_name = "kinakaze_engine_libGL_glDebugMessageCallbackARB")]
pub unsafe extern "sysv64" fn glDebugMessageCallbackARB(callback: *mut c_void, user: *mut c_void) {
    unsafe { set_debug_callback(b"glDebugMessageCallbackARB\0".as_ptr(), callback, user) };
}

/// GLES permits precision declarations that desktop compatibility-profile GLSL
/// does not parse. Strip only those declarations while preserving the rest of the
/// shader verbatim; `lowp`/`mediump`/`highp` tokens in actual expressions remain
/// available on drivers that advertise the ES compatibility extensions.
#[unsafe(export_name = "kinakaze_engine_libGL_glShaderSource")]
pub unsafe extern "sysv64" fn glShaderSource(
    shader: c_uint,
    count: c_int,
    strings: *const *const c_char,
    lengths: *const c_int,
) {
    if count <= 0 || strings.is_null() {
        return;
    }
    type HostFn = unsafe extern "system" fn(c_uint, c_int, *const *const c_char, *const c_int);
    static FUNCTION: OnceLock<Option<HostFn>> = OnceLock::new();
    let function = FUNCTION.get_or_init(|| unsafe {
        let symbol = b"glShaderSource\0";
        let module = host_module();
        if module.is_null() {
            return None;
        }
        if let Some(pointer) = GetProcAddress(module, symbol.as_ptr()) {
            Some(core::mem::transmute(pointer))
        } else {
            wglGetProcAddress(symbol.as_ptr()).map(|pointer| core::mem::transmute(pointer))
        }
    });
    let Some(function) = *function else { return };

    // Desktop shaders are already accepted verbatim. Detect the one GLES-only
    // construct across the guest's string-vector boundaries and keep the common
    // path allocation- and copy-free.
    let mut precision_match = 0usize;
    let mut has_precision = false;
    for index in 0..count as usize {
        let pointer = unsafe { *strings.add(index) };
        if pointer.is_null() {
            continue;
        }
        let bytes = if lengths.is_null() || unsafe { *lengths.add(index) } < 0 {
            unsafe { CStr::from_ptr(pointer) }.to_bytes()
        } else {
            let length = unsafe { *lengths.add(index) } as usize;
            unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }
        };
        has_precision = precision_token_in(bytes, &mut precision_match);
        if has_precision {
            break;
        }
    }
    if !has_precision {
        unsafe { function(shader, count, strings, lengths) };
        return;
    }

    let mut source = Vec::new();
    for index in 0..count as usize {
        let pointer = unsafe { *strings.add(index) };
        if pointer.is_null() {
            continue;
        }
        let bytes = if lengths.is_null() || unsafe { *lengths.add(index) } < 0 {
            unsafe { CStr::from_ptr(pointer) }.to_bytes()
        } else {
            let length = unsafe { *lengths.add(index) } as usize;
            unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }
        };
        source.extend_from_slice(bytes);
    }
    let text = String::from_utf8_lossy(&source);
    let desktop_source = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("precision "))
        .collect::<Vec<_>>()
        .join("\n");
    let Ok(desktop_source) = CString::new(desktop_source) else {
        return;
    };
    let pointer = desktop_source.as_ptr();
    unsafe { function(shader, 1, &raw const pointer, core::ptr::null()) };
}
gl_fn!(glRectf(x1: c_float, y1: c_float, x2: c_float, y2: c_float));
gl_fn!(glRectd(x1: c_double, y1: c_double, x2: c_double, y2: c_double));
gl_fn!(glRecti(x1: c_int, y1: c_int, x2: c_int, y2: c_int));
gl_fn!(glVertex2i(x: c_int, y: c_int));
gl_fn!(glVertex2d(x: c_double, y: c_double));
gl_fn!(glVertex3d(x: c_double, y: c_double, z: c_double));
gl_fn!(glVertex3i(x: c_int, y: c_int, z: c_int));
gl_fn!(glColor3ub(red: c_uchar, green: c_uchar, blue: c_uchar));
gl_fn!(glColor3d(red: c_double, green: c_double, blue: c_double));
gl_fn!(glColor4d(red: c_double, green: c_double, blue: c_double, alpha: c_double));
gl_fn!(glRotated(angle: c_double, x: c_double, y: c_double, z: c_double));
gl_fn!(glTranslated(x: c_double, y: c_double, z: c_double));
gl_fn!(glScaled(x: c_double, y: c_double, z: c_double));
gl_fn!(glMultMatrixf(m: *const c_float));
gl_fn!(glMultMatrixd(m: *const c_double));
gl_fn!(glLoadMatrixf(m: *const c_float));
gl_fn!(glLoadMatrixd(m: *const c_double));
gl_fn!(glPushAttrib(mask: c_uint));
gl_fn!(glPopAttrib());
gl_fn!(glPushClientAttrib(mask: c_uint));
gl_fn!(glPopClientAttrib());

// ---------------------------------------------------------------------------
// GLX Compatibility and Windowing Integration
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct XVisualInfo {
    pub visual: *mut c_void,
    pub visualid: usize,
    pub screen: c_int,
    pub depth: c_int,
    pub class: c_int,
    pub red_mask: usize,
    pub green_mask: usize,
    pub blue_mask: usize,
    pub colormap_size: c_int,
    pub bits_per_rgb: c_int,
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXQueryExtension")]
pub unsafe extern "sysv64" fn glXQueryExtension(
    _dpy: *mut c_void,
    error_base: *mut c_int,
    event_base: *mut c_int,
) -> c_int {
    if !error_base.is_null() {
        unsafe {
            *error_base = 0;
        }
    }
    if !event_base.is_null() {
        unsafe {
            *event_base = 0;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXQueryVersion")]
pub unsafe extern "sysv64" fn glXQueryVersion(
    _dpy: *mut c_void,
    major: *mut c_int,
    minor: *mut c_int,
) -> c_int {
    if !major.is_null() {
        unsafe {
            *major = 1;
        }
    }
    if !minor.is_null() {
        unsafe {
            *minor = 4;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXQueryExtensionsString")]
pub unsafe extern "sysv64" fn glXQueryExtensionsString(
    _dpy: *mut c_void,
    _screen: c_int,
) -> *const c_char {
    c"GLX_ARB_get_proc_address GLX_ARB_create_context GLX_ARB_create_context_profile GLX_EXT_swap_control GLX_EXT_visual_info GLX_EXT_visual_rating GLX_EXT_texture_from_pixmap".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXQueryServerString")]
pub unsafe extern "sysv64" fn glXQueryServerString(
    _dpy: *mut c_void,
    _screen: c_int,
    name: c_int,
) -> *const c_char {
    match name {
        1 => c"Kinakaze".as_ptr(), // GLX_VENDOR
        2 => c"1.4".as_ptr(),      // GLX_VERSION, matches glXQueryVersion
        3 => unsafe { glXQueryExtensionsString(_dpy, _screen) },
        _ => core::ptr::null(),
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetClientString")]
pub unsafe extern "sysv64" fn glXGetClientString(dpy: *mut c_void, name: c_int) -> *const c_char {
    unsafe { glXQueryServerString(dpy, 0, name) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXIsDirect")]
pub unsafe extern "sysv64" fn glXIsDirect(_dpy: *mut c_void, _ctx: *mut c_void) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetConfig")]
pub unsafe extern "sysv64" fn glXGetConfig(
    _dpy: *mut c_void,
    _vis: *mut XVisualInfo,
    attrib: c_int,
    value: *mut c_int,
) -> c_int {
    if value.is_null() {
        return 0;
    }
    unsafe {
        match attrib {
            1 => *value = 1,   // GLX_USE_GL
            2 => *value = 32,  // GLX_BUFFER_SIZE
            3 => *value = 0,   // GLX_LEVEL
            4 => *value = 1,   // GLX_RGBA
            5 => *value = 1,   // GLX_DOUBLEBUFFER
            6 => *value = 0,   // GLX_STEREO
            7 => *value = 0,   // GLX_AUX_BUFFERS
            8 => *value = 8,   // GLX_RED_SIZE
            9 => *value = 8,   // GLX_GREEN_SIZE
            10 => *value = 8,  // GLX_BLUE_SIZE
            11 => *value = 8,  // GLX_ALPHA_SIZE
            12 => *value = 24, // GLX_DEPTH_SIZE
            13 => *value = 8,  // GLX_STENCIL_SIZE
            _ => *value = 0,
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetFBConfigs")]
pub unsafe extern "sysv64" fn glXGetFBConfigs(
    _dpy: *mut c_void,
    _screen: c_int,
    nelements: *mut c_int,
) -> *mut *mut c_void {
    unsafe { glXChooseFBConfig(_dpy, _screen, core::ptr::null(), nelements) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetFBConfigAttrib")]
pub unsafe extern "sysv64" fn glXGetFBConfigAttrib(
    _dpy: *mut c_void,
    _config: *mut c_void,
    attribute: c_int,
    value: *mut c_int,
) -> c_int {
    if value.is_null() {
        return 0;
    }
    unsafe {
        match attribute {
            // Buffer / Color sizes
            2 | 0x8014 => *value = 32,  // GLX_BUFFER_SIZE
            8 | 0x8020 => *value = 8,   // GLX_RED_SIZE
            9 | 0x8021 => *value = 8,   // GLX_GREEN_SIZE
            10 | 0x8022 => *value = 8,  // GLX_BLUE_SIZE
            11 | 0x8023 => *value = 8,  // GLX_ALPHA_SIZE
            12 | 0x8024 => *value = 24, // GLX_DEPTH_SIZE
            13 | 0x8025 => *value = 8,  // GLX_STENCIL_SIZE
            5 => *value = 1,            // GLX_DOUBLEBUFFER
            6 => *value = 0,            // GLX_STEREO
            7 => *value = 0,            // GLX_AUX_BUFFERS
            14 => *value = 0,           // GLX_ACCUM_RED_SIZE
            15 => *value = 0,           // GLX_ACCUM_GREEN_SIZE
            16 => *value = 0,           // GLX_ACCUM_BLUE_SIZE
            17 => *value = 0,           // GLX_ACCUM_ALPHA_SIZE

            // Extension / Visual attributes
            0x20 => *value = 0x8000,       // GLX_CONFIG_CAVEAT -> GLX_NONE
            0x22 => *value = 0x8002,       // GLX_X_VISUAL_TYPE -> GLX_TRUE_COLOR
            0x23 => *value = 0x8000,       // GLX_TRANSPARENT_TYPE -> GLX_NONE
            0x800B => *value = 1,          // GLX_VISUAL_ID
            0x800C => *value = 0,          // GLX_SCREEN
            0x8010 => *value = 0x7,        // GLX_DRAWABLE_TYPE -> WINDOW | PIXMAP | PBUFFER
            0x8011 => *value = 0x1,        // GLX_RENDER_TYPE -> GLX_RGBA_BIT
            0x8012 => *value = 1,          // GLX_X_RENDERABLE -> True
            0x8013 => *value = 1,          // GLX_FBCONFIG_ID
            100000 => *value = 0,          // GLX_SAMPLE_BUFFERS
            100001 => *value = 0,          // GLX_SAMPLES
            0x20d0 | 0x20d1 => *value = 1, // BIND_TO_TEXTURE_RGB/RGBA_EXT
            0x20d2 => *value = 0,          // BIND_TO_MIPMAP_TEXTURE_EXT
            0x20d3 => *value = 2,          // BIND_TO_TEXTURE_TARGETS_EXT: 2D
            0x20d4 => *value = 1,          // Y_INVERTED_EXT: DIB rows run top to bottom
            _ => *value = 0,
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXChooseVisual")]
pub unsafe extern "sysv64" fn glXChooseVisual(
    _dpy: *mut c_void,
    _screen: c_int,
    _attribList: *mut c_int,
) -> *mut XVisualInfo {
    #[repr(C)]
    struct Visual {
        ext_data: *mut c_void,
        id: usize,
        class: c_int,
        red: usize,
        green: usize,
        blue: usize,
        bits: c_int,
        entries: c_int,
    }
    static mut VISUAL: Visual = Visual {
        ext_data: core::ptr::null_mut(),
        id: 1,
        class: 4,
        red: 0xff0000,
        green: 0xff00,
        blue: 0xff,
        bits: 8,
        entries: 256,
    };
    if _screen != 0 {
        return core::ptr::null_mut();
    }
    let out =
        unsafe { kinakaze_alloc::guest::malloc(size_of::<XVisualInfo>()) }.cast::<XVisualInfo>();
    if out.is_null() {
        return out;
    }
    unsafe {
        out.write(XVisualInfo {
            visual: (&raw mut VISUAL).cast(),
            visualid: 1,
            screen: 0,
            depth: 24,
            class: 4,
            red_mask: 0x00ff0000,
            green_mask: 0x0000ff00,
            blue_mask: 0x000000ff,
            colormap_size: 256,
            bits_per_rgb: 8,
        });
    }
    out
}

struct SurfaceState {
    hwnd: HWND,
    hdc: HDC,
    // Pbuffers own an unmapped native drawable. Window surfaces borrow theirs.
    owned_window: Option<(u64, c_int, c_int)>,
}

unsafe impl Send for SurfaceState {}
unsafe impl Sync for SurfaceState {}

struct ContextState {
    wgl: *mut c_void,
    share: usize,
    attributes: Vec<c_int>,
    display: usize,
    config_id: c_int,
    screen: c_int,
    render_type: c_int,
}

unsafe impl Send for ContextState {}
unsafe impl Sync for ContextState {}

static SURFACES: OnceLock<std::sync::Mutex<std::collections::HashMap<usize, SurfaceState>>> =
    OnceLock::new();
static CONTEXTS: OnceLock<std::sync::Mutex<std::collections::HashMap<usize, ContextState>>> =
    OnceLock::new();

thread_local! {
    static CURRENT_CONTEXT: Cell<usize> = const { Cell::new(0) };
    static CURRENT_DISPLAY: Cell<usize> = const { Cell::new(0) };
    static CURRENT_DRAWABLE: Cell<usize> = const { Cell::new(0) };
    // The HDC is immutable for the lifetime of a bound WGL surface. Caching it
    // beside the GLX binding keeps the presentation hot path thread-local.
    static CURRENT_HDC: Cell<usize> = const { Cell::new(0) };
}

fn clear_current_binding() {
    CURRENT_CONTEXT.set(0);
    CURRENT_DISPLAY.set(0);
    CURRENT_DRAWABLE.set(0);
    CURRENT_HDC.set(0);
}

fn surface_map() -> &'static std::sync::Mutex<std::collections::HashMap<usize, SurfaceState>> {
    SURFACES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn context_map() -> &'static std::sync::Mutex<std::collections::HashMap<usize, ContextState>> {
    CONTEXTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn window_pixel_format() -> PIXELFORMATDESCRIPTOR {
    PIXELFORMATDESCRIPTOR {
        nSize: core::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
        nVersion: 1,
        dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
        iPixelType: PFD_TYPE_RGBA,
        cColorBits: 32,
        cRedBits: 0,
        cRedShift: 0,
        cGreenBits: 0,
        cGreenShift: 0,
        cBlueBits: 0,
        cBlueShift: 0,
        cAlphaBits: 8,
        cAlphaShift: 0,
        cAccumBits: 0,
        cAccumRedBits: 0,
        cAccumGreenBits: 0,
        cAccumBlueBits: 0,
        cAccumAlphaBits: 0,
        cDepthBits: 24,
        cStencilBits: 8,
        cAuxBuffers: 0,
        iLayerType: PFD_MAIN_PLANE as u8,
        bReserved: 0,
        dwLayerMask: 0,
        dwVisibleMask: 0,
        dwDamageMask: 0,
    }
}

fn ensure_surface(drawable: usize) -> Option<SurfaceState> {
    let hwnd = u32::try_from(drawable)
        .ok()
        .and_then(kinakaze_libdisplay::window::x11::native_handle)
        .or_else(|| kinakaze_libdisplay::window::native_handle(drawable as u64))
        .map(|n| n as HWND)
        .unwrap_or(drawable as HWND);
    if hwnd.is_null() {
        gl_trace!("[libGL] ensure_surface: hwnd is null for drawable {drawable:#x}");
        return None;
    }
    let hdc = unsafe { GetDC(hwnd) };
    if hdc.is_null() {
        gl_trace!("[libGL] ensure_surface: GetDC({hwnd:?}) failed");
        return None;
    }

    // Pixel format is immutable for a window.  Reusing the one already selected
    // is required when a GLX drawable is rebound to a context.
    let cur_fmt = unsafe { GetPixelFormat(hdc) };
    gl_trace!("[libGL] ensure_surface: hwnd={hwnd:?} hdc={hdc:?} cur_fmt={cur_fmt}");
    if cur_fmt == 0 {
        let descriptor = window_pixel_format();
        let format = unsafe { ChoosePixelFormat(hdc, &raw const descriptor) };
        gl_trace!("[libGL] ensure_surface: ChoosePixelFormat -> {format}");
        if format == 0 || unsafe { SetPixelFormat(hdc, format, &raw const descriptor) } == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            gl_trace!(
                "[libGL] ensure_surface: SetPixelFormat failed for format={format} last_error={err}"
            );
            unsafe { ReleaseDC(hwnd, hdc) };
            return None;
        }
    }
    Some(SurfaceState {
        hwnd,
        hdc,
        owned_window: None,
    })
}

fn allocate_context(
    display: *mut c_void,
    config_id: c_int,
    screen: c_int,
    share_list: *mut c_void,
    attributes: Vec<c_int>,
) -> *mut c_void {
    let token = Box::into_raw(Box::new(0u8)) as *mut c_void;
    context_map()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            token as usize,
            ContextState {
                wgl: core::ptr::null_mut(),
                share: share_list as usize,
                attributes,
                display: display as usize,
                config_id,
                screen,
                render_type: 0x8014, // GLX_RGBA_TYPE: the WGL pixel format is RGBA.
            },
        );
    token
}

unsafe fn create_native_context(hdc: HDC, share: *mut c_void, attributes: &[c_int]) -> *mut c_void {
    let context = unsafe { wglCreateContext(hdc) };
    if context.is_null() {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        gl_trace!(
            "[libGL] create_native_context: wglCreateContext failed for hdc={hdc:?} last_error={err}"
        );
        return core::ptr::null_mut();
    }

    if attributes.is_empty() {
        if !share.is_null() && unsafe { wglShareLists(share, context) } == 0 {
            unsafe { wglDeleteContext(context) };
            return core::ptr::null_mut();
        }
        return context;
    }

    // Attempt to create a modern context via wglCreateContextAttribsARB
    let prev_ctx = unsafe { wglGetCurrentContext() };
    let make_ok = unsafe { wglMakeCurrent(hdc, context) };
    if make_ok == 0 {
        gl_trace!(
            "[libGL] create_native_context: wglMakeCurrent bootstrap failed, falling back to base context"
        );
        return context;
    }

    type CreateContextAttribs =
        unsafe extern "system" fn(HDC, *mut c_void, *const c_int) -> *mut c_void;
    let address = unsafe { native_gl_address(b"wglCreateContextAttribsARB\0".as_ptr()) };
    let arb_context = if valid_driver_address(address) {
        let create: CreateContextAttribs = unsafe { core::mem::transmute(address) };
        unsafe { create(hdc, share, attributes.as_ptr()) }
    } else {
        core::ptr::null_mut()
    };

    unsafe { wglMakeCurrent(core::ptr::null_mut(), prev_ctx) };

    if !arb_context.is_null() {
        unsafe { wglDeleteContext(context) };
        arb_context
    } else {
        gl_trace!(
            "[libGL] create_native_context: wglCreateContextAttribsARB returned null, falling back to base context"
        );
        context
    }
}

fn materialize_context(
    contexts: &mut std::collections::HashMap<usize, ContextState>,
    token: usize,
    hdc: HDC,
    depth: usize,
) -> Option<*mut c_void> {
    if depth > 16 {
        return None;
    }
    let (existing, share_token, attributes) = {
        let context = contexts.get(&token)?;
        (context.wgl, context.share, context.attributes.clone())
    };
    if !existing.is_null() {
        return Some(existing);
    }
    let share = if share_token == 0 {
        core::ptr::null_mut()
    } else {
        materialize_context(contexts, share_token, hdc, depth + 1)?
    };
    let native = unsafe { create_native_context(hdc, share, &attributes) };
    if native.is_null() {
        gl_trace!(
            "[libGL] materialize_context: create_native_context failed for token={token:#x} hdc={hdc:?}"
        );
        return None;
    }
    contexts.get_mut(&token)?.wgl = native;
    Some(native)
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXCreateContext")]
pub unsafe extern "sysv64" fn glXCreateContext(
    _dpy: *mut c_void,
    _vis: *mut XVisualInfo,
    share_list: *mut c_void,
    _direct: c_int,
) -> *mut c_void {
    // WGL creates a context against a pixel-formatted DC, while GLX creates the
    // context before it is bound to a drawable.  Return a process-local token now
    // and materialize the HGLRC on the first glXMakeCurrent.
    let screen = if _vis.is_null() {
        0
    } else {
        unsafe { (*_vis).screen }
    };
    if screen != 0 {
        return core::ptr::null_mut();
    }
    allocate_context(_dpy, 1, screen, share_list, Vec::new())
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXCreateContextAttribsARB")]
pub unsafe extern "sysv64" fn glXCreateContextAttribsARB(
    _dpy: *mut c_void,
    _config: *mut c_void,
    share_context: *mut c_void,
    _direct: c_int,
    attributes: *const c_int,
) -> *mut c_void {
    // glXChooseFBConfig currently exposes one RGBA configuration on screen 0.
    if _config as usize != 1 {
        return core::ptr::null_mut();
    }
    let mut native_attributes = Vec::new();
    if !attributes.is_null() {
        for index in 0..64usize {
            let attribute = unsafe { *attributes.add(index * 2) };
            if attribute == 0 {
                native_attributes.push(0);
                break;
            }
            let val = unsafe { *attributes.add(index * 2 + 1) };
            gl_trace!("[libGL] glXCreateContextAttribsARB attrib={attribute:#x} val={val:#x}");
            // Filter out GLX-specific tokens that WGL doesn't recognize
            if attribute == 0x8011 || attribute == 0x8012 {
                // GLX_RENDER_TYPE / GLX_X_RENDERABLE: ignore for WGL
                continue;
            }
            native_attributes.push(attribute);
            native_attributes.push(val);
        }
        if native_attributes.last().copied() != Some(0) {
            native_attributes.push(0);
        }
    }
    let ctx = allocate_context(_dpy, _config as c_int, 0, share_context, native_attributes);
    gl_trace!("[libGL] glXCreateContextAttribsARB -> {ctx:?}");
    ctx
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXDestroyContext")]
pub unsafe extern "sysv64" fn glXDestroyContext(_dpy: *mut c_void, ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let removed = context_map()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&(ctx as usize));
    if let Some(state) = removed {
        debug_callbacks()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(ctx as usize));
        if !state.wgl.is_null() {
            if CURRENT_CONTEXT.get() == ctx as usize {
                unsafe { wglMakeCurrent(core::ptr::null_mut(), core::ptr::null_mut()) };
                clear_current_binding();
            }
            unsafe { wglDeleteContext(state.wgl) };
        }
        unsafe { drop(Box::from_raw(ctx.cast::<u8>())) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXMakeCurrent")]
pub unsafe extern "sysv64" fn glXMakeCurrent(
    _dpy: *mut c_void,
    drawable: usize,
    ctx: *mut c_void,
) -> c_int {
    if ctx.is_null() {
        if unsafe { wglMakeCurrent(core::ptr::null_mut(), core::ptr::null_mut()) } == 0 {
            return 0;
        }
        clear_current_binding();
        return 1;
    }

    let hdc = {
        let mut surfaces = surface_map().lock().unwrap_or_else(|p| p.into_inner());
        let surface = surfaces.entry(drawable).or_insert_with(|| {
            ensure_surface(drawable).unwrap_or(SurfaceState {
                hwnd: core::ptr::null_mut(),
                hdc: core::ptr::null_mut(),
                owned_window: None,
            })
        });
        let hdc = surface.hdc;
        if hdc.is_null() {
            surfaces.remove(&drawable);
            return 0;
        }
        hdc
    };

    let mut contexts = context_map().lock().unwrap_or_else(|p| p.into_inner());
    let Some(wgl) = materialize_context(&mut contexts, ctx as usize, hdc, 0) else {
        return 0;
    };
    let ok = unsafe { wglMakeCurrent(hdc, wgl) } != 0;
    let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    gl_trace!(
        "[libGL] glXMakeCurrent drawable={drawable:#x} hdc={hdc:?} context={wgl:?} ok={ok} last_error={err}"
    );
    if ok {
        CURRENT_CONTEXT.set(ctx as usize);
        CURRENT_DISPLAY.set(_dpy as usize);
        CURRENT_DRAWABLE.set(drawable);
        CURRENT_HDC.set(hdc as usize);
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXMakeContextCurrent")]
pub unsafe extern "sysv64" fn glXMakeContextCurrent(
    dpy: *mut c_void,
    draw: usize,
    read: usize,
    ctx: *mut c_void,
) -> c_int {
    // WGL_ARB_make_current_read is not needed when draw/read are identical,
    // which is the path used by GLFW and EGL window surfaces. Rejecting split
    // draw/read is more accurate than silently binding the wrong read surface.
    if !ctx.is_null() && draw != read {
        return 0;
    }
    unsafe { glXMakeCurrent(dpy, draw, ctx) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXSwapBuffers")]
pub unsafe extern "sysv64" fn glXSwapBuffers(_dpy: *mut c_void, drawable: usize) {
    unsafe { publish::frame(drawable) };
    // The common case is the drawable bound on this thread. It already has a
    // stable HDC, so presenting should not contend on a process-wide map every
    // frame. Keep the map lookup as the GLX-compatible fallback for callers
    // that explicitly swap another drawable.
    let current_hdc = if CURRENT_DRAWABLE.get() == drawable {
        CURRENT_HDC.get() as HDC
    } else {
        core::ptr::null_mut()
    };
    let swapped = if !current_hdc.is_null() {
        Some(unsafe { SwapBuffers(current_hdc) } != 0)
    } else {
        let surfaces = surface_map().lock().unwrap_or_else(|p| p.into_inner());
        surfaces
            .get(&drawable)
            .map(|surface| unsafe { SwapBuffers(surface.hdc) } != 0)
    };
    if swapped == Some(true) {
        kinakaze_libdisplay::ui::presentation::frame_ready(drawable);
    }
    if let Some(swapped) = swapped
        && trace_enabled()
    {
        let error = unsafe { glGetError() };
        gl_trace!("[libGL] glXSwapBuffers ok={swapped} gl_error={error:#x}");
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXQueryDrawable")]
pub unsafe extern "sysv64" fn glXQueryDrawable(
    _dpy: *mut c_void,
    draw: usize,
    attribute: c_int,
    value: *mut c_uint,
) {
    if value.is_null() {
        return;
    }
    if let Some(v) = pixmap::query(draw, attribute) {
        unsafe {
            *value = v;
        }
        return;
    }
    let hwnd = draw as HWND;
    let mut rect: RECT = unsafe { core::mem::zeroed() };
    if !hwnd.is_null() {
        unsafe { GetClientRect(hwnd, &mut rect) };
    }
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    unsafe {
        match attribute {
            0x801d => *value = width,  // GLX_WIDTH
            0x801e => *value = height, // GLX_HEIGHT
            0x801b => *value = 1,      // GLX_EVENT_MASK
            0x8013 => *value = 1,      // GLX_FBCONFIG_ID
            0x8014 => *value = 32,     // GLX_BUFFER_SIZE
            _ => *value = 0,
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXChooseFBConfig")]
pub unsafe extern "sysv64" fn glXChooseFBConfig(
    _dpy: *mut c_void,
    _screen: c_int,
    _attrib_list: *const c_int,
    nelements: *mut c_int,
) -> *mut *mut c_void {
    if !nelements.is_null() {
        unsafe {
            *nelements = 0;
        }
    }
    if _screen != 0 {
        return core::ptr::null_mut();
    }
    let result =
        unsafe { kinakaze_alloc::guest::malloc(size_of::<*mut c_void>()) }.cast::<*mut c_void>();
    if !result.is_null() {
        unsafe {
            result.write(1usize as *mut c_void);
            if !nelements.is_null() {
                *nelements = 1;
            }
        }
    }
    result
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetVisualFromFBConfig")]
pub unsafe extern "sysv64" fn glXGetVisualFromFBConfig(
    dpy: *mut c_void,
    _config: *mut c_void,
) -> *mut XVisualInfo {
    unsafe { glXChooseVisual(dpy, 0, core::ptr::null_mut()) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXCreateWindow")]
pub unsafe extern "sysv64" fn glXCreateWindow(
    _dpy: *mut c_void,
    _config: *mut c_void,
    win: usize,
    _attrib_list: *const c_int,
) -> usize {
    win
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXDestroyWindow")]
pub unsafe extern "sysv64" fn glXDestroyWindow(_dpy: *mut c_void, win: usize) {
    if CURRENT_DRAWABLE.get() == win {
        unsafe { wglMakeCurrent(core::ptr::null_mut(), core::ptr::null_mut()) };
        clear_current_binding();
    }
    if let Some(surface) = surface_map()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&win)
    {
        unsafe { ReleaseDC(surface.hwnd, surface.hdc) };
        if let Some((window, _, _)) = surface.owned_window {
            kinakaze_libdisplay::window::destroy(window);
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXCreateNewContext")]
pub unsafe extern "sysv64" fn glXCreateNewContext(
    dpy: *mut c_void,
    _config: *mut c_void,
    _render_type: c_int,
    share_list: *mut c_void,
    _direct: c_int,
) -> *mut c_void {
    if _config as usize != 1 || _render_type != 0x8014 {
        return core::ptr::null_mut();
    }
    allocate_context(dpy, _config as c_int, 0, share_list, Vec::new())
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXQueryContext")]
pub unsafe extern "sysv64" fn glXQueryContext(
    dpy: *mut c_void,
    ctx: *mut c_void,
    attribute: c_int,
    value: *mut c_int,
) -> c_int {
    let contexts = context_map().lock().unwrap_or_else(|p| p.into_inner());
    let Some(context) = contexts.get(&(ctx as usize)) else {
        return 5; // GLX_BAD_CONTEXT
    };
    if context.display != dpy as usize {
        return 5;
    }
    if value.is_null() {
        return 6; // GLX_BAD_VALUE
    }
    let result = match attribute {
        0x8013 => context.config_id,   // GLX_FBCONFIG_ID
        0x8011 => context.render_type, // GLX_RENDER_TYPE (an enum, not the FBConfig bitmask)
        0x800c => context.screen,      // GLX_SCREEN
        _ => return 2,                 // GLX_BAD_ATTRIBUTE
    };
    unsafe { value.write(result) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetCurrentContext")]
pub unsafe extern "sysv64" fn glXGetCurrentContext() -> *mut c_void {
    CURRENT_CONTEXT.get() as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetCurrentDisplay")]
pub unsafe extern "sysv64" fn glXGetCurrentDisplay() -> *mut c_void {
    CURRENT_DISPLAY.get() as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetCurrentDrawable")]
pub unsafe extern "sysv64" fn glXGetCurrentDrawable() -> usize {
    CURRENT_DRAWABLE.get()
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetCurrentReadDrawable")]
pub unsafe extern "sysv64" fn glXGetCurrentReadDrawable() -> usize {
    CURRENT_DRAWABLE.get()
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXWaitGL")]
pub unsafe extern "sysv64" fn glXWaitGL() -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXWaitX")]
pub unsafe extern "sysv64" fn glXWaitX() -> c_int {
    1
}

fn set_swap_interval(interval: c_int) -> bool {
    type SwapInterval = unsafe extern "system" fn(c_int) -> c_int;
    let address = unsafe { native_gl_address(b"wglSwapIntervalEXT\0".as_ptr()) };
    if !valid_driver_address(address) {
        return false;
    }
    let function: SwapInterval = unsafe { core::mem::transmute(address) };
    unsafe { function(interval) != 0 }
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXSwapIntervalEXT")]
pub unsafe extern "sysv64" fn glXSwapIntervalEXT(
    _dpy: *mut c_void,
    _drawable: usize,
    interval: c_int,
) -> c_int {
    c_int::from(!set_swap_interval(interval))
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXSwapIntervalMESA")]
pub unsafe extern "sysv64" fn glXSwapIntervalMESA(interval: c_uint) -> c_int {
    c_int::from(!set_swap_interval(
        interval.min(c_int::MAX as c_uint) as c_int
    ))
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXSwapIntervalSGI")]
pub unsafe extern "sysv64" fn glXSwapIntervalSGI(interval: c_int) -> c_int {
    c_int::from(!set_swap_interval(interval))
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetProcAddress")]
pub unsafe extern "sysv64" fn glXGetProcAddress(procName: *const c_char) -> *mut c_void {
    if procName.is_null() {
        return core::ptr::null_mut();
    }
    let provider = provider_module();
    if !provider.is_null() {
        let guest_name = unsafe { CStr::from_ptr(procName) };
        let mut target = b"kinakaze_engine_libGL_".to_vec();
        target.extend_from_slice(guest_name.to_bytes_with_nul());
        if let Some(ptr) = unsafe { GetProcAddress(provider, target.as_ptr()) } {
            return ptr as *mut c_void;
        }
    }
    // A raw opengl32/wglGetProcAddress address uses the Windows x64 ABI.  A Linux
    // caller uses SysV AMD64, so returning the native address would work just long
    // enough to corrupt its first call.  Extensions become available only after a
    // System-V wrapper is exported from this provider.
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libGL_glXGetProcAddressARB")]
pub unsafe extern "sysv64" fn glXGetProcAddressARB(procName: *const c_char) -> *mut c_void {
    unsafe { glXGetProcAddress(procName) }
}

// ---------------------------------------------------------------------------
// EGL 1.5 compatibility over the same HWND/HDC/WGL objects used by GLX.
// ---------------------------------------------------------------------------

const EGL_SUCCESS: c_int = 0x3000;
const EGL_BAD_DISPLAY: c_int = 0x3008;
const EGL_BAD_CONFIG: c_int = 0x3005;
const EGL_BAD_CONTEXT: c_int = 0x3006;
const EGL_BAD_NATIVE_WINDOW: c_int = 0x300b;
const EGL_BAD_PARAMETER: c_int = 0x300c;
const EGL_WIDTH: c_int = 0x3057;
const EGL_HEIGHT: c_int = 0x3056;
const EGL_VENDOR: c_int = 0x3053;
const EGL_VERSION: c_int = 0x3054;
const EGL_EXTENSIONS: c_int = 0x3055;
const EGL_CLIENT_APIS: c_int = 0x308d;
const EGL_OPENGL_ES_API: c_uint = 0x30a0;
const EGL_OPENGL_API: c_uint = 0x30a2;

static EGL_DISPLAY_TOKEN: u8 = 0;
static EGL_CONFIG_TOKEN: u8 = 0;

thread_local! {
    static EGL_ERROR: Cell<c_int> = const { Cell::new(EGL_SUCCESS) };
    static EGL_API: Cell<c_uint> = const { Cell::new(EGL_OPENGL_ES_API) };
    static EGL_DRAW_SURFACE: Cell<usize> = const { Cell::new(0) };
    static EGL_READ_SURFACE: Cell<usize> = const { Cell::new(0) };
}

fn egl_display() -> *mut c_void {
    (&raw const EGL_DISPLAY_TOKEN).cast_mut().cast()
}

fn egl_config() -> *mut c_void {
    (&raw const EGL_CONFIG_TOKEN).cast_mut().cast()
}

fn egl_fail(error: c_int) -> c_uint {
    EGL_ERROR.set(error);
    0
}

fn valid_egl_display(display: *mut c_void) -> bool {
    display == egl_display()
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetDisplay")]
pub unsafe extern "sysv64" fn eglGetDisplay(_native_display: *mut c_void) -> *mut c_void {
    egl_display()
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetPlatformDisplay")]
pub unsafe extern "sysv64" fn eglGetPlatformDisplay(
    _platform: c_uint,
    _native_display: *mut c_void,
    _attributes: *const isize,
) -> *mut c_void {
    egl_display()
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetPlatformDisplayEXT")]
pub unsafe extern "sysv64" fn eglGetPlatformDisplayEXT(
    platform: c_uint,
    native_display: *mut c_void,
    attributes: *const c_int,
) -> *mut c_void {
    unsafe { eglGetPlatformDisplay(platform, native_display, attributes.cast::<isize>()) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglInitialize")]
pub unsafe extern "sysv64" fn eglInitialize(
    display: *mut c_void,
    major: *mut c_int,
    minor: *mut c_int,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if !major.is_null() {
        unsafe { *major = 1 };
    }
    if !minor.is_null() {
        unsafe { *minor = 5 };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglTerminate")]
pub unsafe extern "sysv64" fn eglTerminate(display: *mut c_void) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetConfigs")]
pub unsafe extern "sysv64" fn eglGetConfigs(
    display: *mut c_void,
    configs: *mut *mut c_void,
    config_size: c_int,
    num_config: *mut c_int,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if num_config.is_null() {
        return egl_fail(EGL_BAD_PARAMETER);
    }
    unsafe { *num_config = 1 };
    if !configs.is_null() && config_size > 0 {
        unsafe { *configs = egl_config() };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglChooseConfig")]
pub unsafe extern "sysv64" fn eglChooseConfig(
    display: *mut c_void,
    _attributes: *const c_int,
    configs: *mut *mut c_void,
    config_size: c_int,
    num_config: *mut c_int,
) -> c_uint {
    unsafe { eglGetConfigs(display, configs, config_size, num_config) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetConfigAttrib")]
pub unsafe extern "sysv64" fn eglGetConfigAttrib(
    display: *mut c_void,
    config: *mut c_void,
    attribute: c_int,
    value: *mut c_int,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if config != egl_config() {
        return egl_fail(EGL_BAD_CONFIG);
    }
    if value.is_null() {
        return egl_fail(EGL_BAD_PARAMETER);
    }
    let result = match attribute {
        0x3020 => 32,                       // EGL_BUFFER_SIZE
        0x3021 => 8,                        // EGL_ALPHA_SIZE
        0x3022..=0x3024 => 8,               // B/G/R sizes
        0x3025 => 24,                       // EGL_DEPTH_SIZE
        0x3026 => 8,                        // EGL_STENCIL_SIZE
        0x3028 => 1,                        // EGL_CONFIG_ID
        0x3029 => 0,                        // EGL_LEVEL
        0x302d => 1,                        // EGL_NATIVE_RENDERABLE
        0x302e => 1,                        // EGL_NATIVE_VISUAL_ID
        0x302f => 1,                        // EGL_NATIVE_VISUAL_TYPE
        0x3031 | 0x3032 => 0,               // samples / sample buffers
        0x3033 => 0x0004 | 0x0002 | 0x0001, // window + pixmap + pbuffer
        0x302a | 0x302c => 16384,           // pbuffer maximum height / width
        0x302b => 16384 * 16384,            // pbuffer maximum pixels
        0x3040 | 0x3042 => 0x0004,          // ES2 renderable/conformant
        _ => 0,
    };
    unsafe { *value = result };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglBindAPI")]
pub unsafe extern "sysv64" fn eglBindAPI(api: c_uint) -> c_uint {
    if api != EGL_OPENGL_ES_API && api != EGL_OPENGL_API {
        return egl_fail(EGL_BAD_PARAMETER);
    }
    EGL_API.set(api);
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglQueryAPI")]
pub unsafe extern "sysv64" fn eglQueryAPI() -> c_uint {
    EGL_API.get()
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglCreateWindowSurface")]
pub unsafe extern "sysv64" fn eglCreateWindowSurface(
    display: *mut c_void,
    config: *mut c_void,
    native_window: usize,
    _attributes: *const c_int,
) -> *mut c_void {
    if !valid_egl_display(display) {
        egl_fail(EGL_BAD_DISPLAY);
        return core::ptr::null_mut();
    }
    if config != egl_config() {
        egl_fail(EGL_BAD_CONFIG);
        return core::ptr::null_mut();
    }
    if native_window == 0 {
        egl_fail(EGL_BAD_NATIVE_WINDOW);
        return core::ptr::null_mut();
    }
    let mut surfaces = surface_map().lock().unwrap_or_else(|p| p.into_inner());
    let surface = surfaces.entry(native_window).or_insert_with(|| {
        ensure_surface(native_window).unwrap_or(SurfaceState {
            hwnd: core::ptr::null_mut(),
            hdc: core::ptr::null_mut(),
            owned_window: None,
        })
    });
    if surface.hdc.is_null() {
        surfaces.remove(&native_window);
        egl_fail(EGL_BAD_NATIVE_WINDOW);
        return core::ptr::null_mut();
    }
    native_window as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglCreatePbufferSurface")]
pub unsafe extern "sysv64" fn eglCreatePbufferSurface(
    display: *mut c_void,
    config: *mut c_void,
    attributes: *const c_int,
) -> *mut c_void {
    let fail = |error| {
        egl_fail(error);
        core::ptr::null_mut()
    };
    if !valid_egl_display(display) {
        return fail(EGL_BAD_DISPLAY);
    }
    if config != egl_config() {
        return fail(EGL_BAD_CONFIG);
    }
    let (mut width, mut height, mut largest) = (0, 0, false);
    if !attributes.is_null() {
        let mut at = attributes;
        while unsafe { *at } != 0x3038 {
            // EGL_NONE
            let (key, value) = unsafe { (*at, *at.add(1)) };
            match key {
                EGL_WIDTH if value >= 0 => width = value,
                EGL_HEIGHT if value >= 0 => height = value,
                0x3058 if value == 0 || value == 1 => largest = value != 0,
                0x3080 | 0x3081 if value == 0x305c => {} // EGL_NO_TEXTURE
                0x3082 if value == 0 => {}               // no mipmap texture
                EGL_WIDTH | EGL_HEIGHT | 0x3058 => return fail(EGL_BAD_PARAMETER),
                0x3080..=0x3082 => return fail(0x3009), // EGL_BAD_MATCH: no texture binding
                _ => return fail(0x3004),               // EGL_BAD_ATTRIBUTE
            }
            at = unsafe { at.add(2) };
        }
    }
    if width > 16384 || height > 16384 {
        if !largest {
            return fail(0x3003);
        } // EGL_BAD_ALLOC
        width = width.min(16384);
        height = height.min(16384);
    }
    // WGL renders to an unmapped drawable owned exclusively by this surface.
    // It never appears on the desktop and is destroyed with the EGL surface.
    let Some(window) = kinakaze_libdisplay::window::create_unmapped(
        None,
        0,
        0,
        width.max(1),
        height.max(1),
        "EGL offscreen",
    ) else {
        return fail(0x3003);
    };
    let Some(mut backing) = ensure_surface(window as usize) else {
        kinakaze_libdisplay::window::destroy(window);
        return fail(0x3003);
    };
    backing.owned_window = Some((window, width, height));
    surface_map()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(window as usize, backing);
    window as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglCreatePixmapSurface")]
pub unsafe extern "sysv64" fn eglCreatePixmapSurface(
    display: *mut c_void,
    config: *mut c_void,
    native_pixmap: usize,
    attributes: *const c_int,
) -> *mut c_void {
    unsafe { eglCreateWindowSurface(display, config, native_pixmap, attributes) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglDestroySurface")]
pub unsafe extern "sysv64" fn eglDestroySurface(
    display: *mut c_void,
    surface: *mut c_void,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if !surface.is_null() {
        unsafe { glXDestroyWindow(display, surface as usize) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglCreateContext")]
pub unsafe extern "sysv64" fn eglCreateContext(
    display: *mut c_void,
    config: *mut c_void,
    share_context: *mut c_void,
    _attributes: *const c_int,
) -> *mut c_void {
    if !valid_egl_display(display) {
        egl_fail(EGL_BAD_DISPLAY);
        return core::ptr::null_mut();
    }
    if config != egl_config() {
        egl_fail(EGL_BAD_CONFIG);
        return core::ptr::null_mut();
    }
    unsafe { glXCreateContext(display, core::ptr::null_mut(), share_context, 1) }
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglDestroyContext")]
pub unsafe extern "sysv64" fn eglDestroyContext(
    display: *mut c_void,
    context: *mut c_void,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if context.is_null() {
        return egl_fail(EGL_BAD_CONTEXT);
    }
    unsafe { glXDestroyContext(display, context) };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglMakeCurrent")]
pub unsafe extern "sysv64" fn eglMakeCurrent(
    display: *mut c_void,
    draw: *mut c_void,
    read: *mut c_void,
    context: *mut c_void,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if context.is_null() {
        let result = unsafe { glXMakeCurrent(display, 0, core::ptr::null_mut()) };
        EGL_DRAW_SURFACE.set(0);
        EGL_READ_SURFACE.set(0);
        return result as c_uint;
    }
    if draw.is_null() || read.is_null() {
        return egl_fail(EGL_BAD_PARAMETER);
    }
    let result = unsafe { glXMakeCurrent(display, draw as usize, context) };
    if result != 0 {
        EGL_DRAW_SURFACE.set(draw as usize);
        EGL_READ_SURFACE.set(read as usize);
    }
    result as c_uint
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglSwapBuffers")]
pub unsafe extern "sysv64" fn eglSwapBuffers(display: *mut c_void, surface: *mut c_void) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if surface.is_null() {
        return egl_fail(EGL_BAD_PARAMETER);
    }
    unsafe { glXSwapBuffers(display, surface as usize) };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglSwapInterval")]
pub unsafe extern "sysv64" fn eglSwapInterval(display: *mut c_void, interval: c_int) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    c_uint::from(set_swap_interval(interval))
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglQuerySurface")]
pub unsafe extern "sysv64" fn eglQuerySurface(
    display: *mut c_void,
    surface: *mut c_void,
    attribute: c_int,
    value: *mut c_int,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if surface.is_null() || value.is_null() {
        return egl_fail(EGL_BAD_PARAMETER);
    }
    let surfaces = surface_map().lock().unwrap_or_else(|p| p.into_inner());
    let Some(backing) = surfaces.get(&(surface as usize)) else {
        return egl_fail(0x300d);
    };
    let (width, height) = if let Some((_, width, height)) = backing.owned_window {
        (width, height)
    } else {
        let mut rect: RECT = unsafe { core::mem::zeroed() };
        if unsafe { GetClientRect(backing.hwnd, &raw mut rect) } == 0 {
            return egl_fail(EGL_BAD_NATIVE_WINDOW);
        }
        (rect.right - rect.left, rect.bottom - rect.top)
    };
    unsafe {
        *value = match attribute {
            EGL_WIDTH => width,
            EGL_HEIGHT => height,
            _ => 0,
        }
    };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglQueryContext")]
pub unsafe extern "sysv64" fn eglQueryContext(
    display: *mut c_void,
    context: *mut c_void,
    attribute: c_int,
    value: *mut c_int,
) -> c_uint {
    if !valid_egl_display(display) {
        return egl_fail(EGL_BAD_DISPLAY);
    }
    if context.is_null() || value.is_null() {
        return egl_fail(EGL_BAD_CONTEXT);
    }
    unsafe {
        *value = match attribute {
            0x3097 => EGL_API.get() as c_int, // EGL_CONTEXT_CLIENT_TYPE
            0x3098 => 2,                      // EGL_CONTEXT_CLIENT_VERSION
            _ => 0,
        }
    };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglQueryString")]
pub unsafe extern "sysv64" fn eglQueryString(display: *mut c_void, name: c_int) -> *const c_char {
    if display.is_null() && name == EGL_EXTENSIONS {
        // EGL_EXT_client_extensions requires a non-null, stable client string
        // before any display has been initialized. Mesa's eglinfo probes it with
        // strstr(), exactly as permitted by the extension specification.
        return c"EGL_EXT_client_extensions EGL_EXT_platform_base EGL_EXT_platform_x11 EGL_KHR_platform_x11".as_ptr();
    }
    if !valid_egl_display(display) {
        egl_fail(EGL_BAD_DISPLAY);
        return core::ptr::null();
    }
    match name {
        EGL_VENDOR => c"kinakaze WGL EGL compatibility".as_ptr(),
        EGL_VERSION => c"1.5 kinakaze".as_ptr(),
        EGL_EXTENSIONS => {
            c"EGL_KHR_create_context EGL_EXT_client_extensions EGL_EXT_platform_base EGL_EXT_platform_x11 EGL_KHR_platform_x11".as_ptr()
        }
        EGL_CLIENT_APIS => c"OpenGL_ES OpenGL".as_ptr(),
        _ => core::ptr::null(),
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetError")]
pub unsafe extern "sysv64" fn eglGetError() -> c_int {
    EGL_ERROR.replace(EGL_SUCCESS)
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetCurrentDisplay")]
pub unsafe extern "sysv64" fn eglGetCurrentDisplay() -> *mut c_void {
    if CURRENT_CONTEXT.get() == 0 {
        core::ptr::null_mut()
    } else {
        egl_display()
    }
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetCurrentContext")]
pub unsafe extern "sysv64" fn eglGetCurrentContext() -> *mut c_void {
    CURRENT_CONTEXT.get() as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetCurrentSurface")]
pub unsafe extern "sysv64" fn eglGetCurrentSurface(which: c_int) -> *mut c_void {
    let surface = if which == 0x3059 {
        EGL_DRAW_SURFACE.get()
    } else {
        EGL_READ_SURFACE.get()
    };
    surface as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglWaitClient")]
pub unsafe extern "sysv64" fn eglWaitClient() -> c_uint {
    unsafe { glFinish() };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglWaitGL")]
pub unsafe extern "sysv64" fn eglWaitGL() -> c_uint {
    unsafe { glFinish() };
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglReleaseThread")]
pub unsafe extern "sysv64" fn eglReleaseThread() -> c_uint {
    unsafe { wglMakeCurrent(core::ptr::null_mut(), core::ptr::null_mut()) };
    clear_current_binding();
    EGL_DRAW_SURFACE.set(0);
    EGL_READ_SURFACE.set(0);
    1
}

#[unsafe(export_name = "kinakaze_engine_libGL_eglGetProcAddress")]
pub unsafe extern "sysv64" fn eglGetProcAddress(procName: *const c_char) -> *mut c_void {
    unsafe { glXGetProcAddress(procName) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glx_window_context() {
        let logical =
            kinakaze_libdisplay::window::create_unmapped(None, 0, 0, 64, 64, "Test GLX Window")
                .expect("failed to create window");
        let handle = kinakaze_libdisplay::window::native_handle(logical).expect("no native handle");

        let ctx = unsafe {
            glXCreateContext(
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                1,
            )
        };
        assert!(!ctx.is_null());
        let display = 0x4321usize as *mut c_void;
        let ok = unsafe { glXMakeCurrent(display, handle, ctx) };
        println!("glXMakeCurrent result = {}", ok);
        assert_eq!(ok, 1);
        assert_eq!(unsafe { glXGetCurrentDisplay() }, display);
        assert_eq!(
            unsafe { glXMakeCurrent(0x5678usize as *mut c_void, 0, ctx) },
            0
        );
        assert_eq!(unsafe { glXGetCurrentDisplay() }, display);
        std::thread::spawn(|| assert!(unsafe { glXGetCurrentDisplay() }.is_null()))
            .join()
            .unwrap();
        assert_eq!(
            unsafe { glXMakeCurrent(display, 0, core::ptr::null_mut()) },
            1
        );
        assert!(unsafe { glXGetCurrentDisplay() }.is_null());
        unsafe {
            glXDestroyContext(display, ctx);
            glXDestroyWindow(display, handle);
        }
        assert!(kinakaze_libdisplay::window::destroy(logical));
    }

    #[test]
    fn egl_pbuffer_renders_and_owns_its_hidden_drawable() {
        unsafe {
            let display = eglGetDisplay(core::ptr::null_mut());
            assert_eq!(
                eglInitialize(display, core::ptr::null_mut(), core::ptr::null_mut()),
                1
            );
            assert_eq!(eglBindAPI(EGL_OPENGL_API), 1);
            let attributes = [EGL_WIDTH, 16, EGL_HEIGHT, 8, 0x3038];
            let surface = eglCreatePbufferSurface(display, egl_config(), attributes.as_ptr());
            assert!(!surface.is_null());
            let mut size = -1;
            assert_eq!(
                eglQuerySurface(display, surface, EGL_WIDTH, &raw mut size),
                1
            );
            assert_eq!(size, 16);
            assert_eq!(
                eglQuerySurface(display, surface, EGL_HEIGHT, &raw mut size),
                1
            );
            assert_eq!(size, 8);
            let context = eglCreateContext(
                display,
                egl_config(),
                core::ptr::null_mut(),
                core::ptr::null(),
            );
            assert!(!context.is_null());
            assert_eq!(eglMakeCurrent(display, surface, surface, context), 1);
            glClearColor(1.0, 0.0, 0.0, 1.0);
            glClear(0x4000);
            glFinish();
            let mut pixel = [0u8; 4];
            glReadPixels(0, 0, 1, 1, 0x1908, 0x1401, pixel.as_mut_ptr().cast());
            assert_eq!(glGetError(), 0);
            assert_eq!(&pixel[..3], &[255, 0, 0]);
            assert_eq!(
                eglMakeCurrent(
                    display,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut()
                ),
                1
            );
            assert_eq!(eglDestroyContext(display, context), 1);
            assert_eq!(eglDestroySurface(display, surface), 1);
            assert!(kinakaze_libdisplay::window::native_handle(surface as u64).is_none());
            assert_eq!(
                eglQuerySurface(display, surface, EGL_WIDTH, &raw mut size),
                0
            );
            assert_eq!(eglGetError(), 0x300d);
            let invalid = [EGL_WIDTH, -1, 0x3038];
            assert!(eglCreatePbufferSurface(display, egl_config(), invalid.as_ptr()).is_null());
            assert_eq!(eglGetError(), EGL_BAD_PARAMETER);
        }
    }

    #[test]
    fn precision_token_detection_spans_shader_string_boundaries() {
        let mut matched = 0;
        assert!(!precision_token_in(b"#version 300 es\nprec", &mut matched));
        assert!(precision_token_in(b"ision highp float;\n", &mut matched));

        let mut ordinary = 0;
        assert!(!precision_token_in(
            b"#version 330\nvoid main() {}\n",
            &mut ordinary,
        ));
    }
}

mod object_layout;
