//! X extension client ABI, shared images and extension-owned display lists.
#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
use core::ffi::{c_char, c_int, c_uint, c_void};
use core::{mem, ptr};
use kinakaze_alloc::guest;
use kinakaze_libX11::{Bool, Display, Status, Visual, Window};
mod dbe;
mod dpms;
mod extension;
mod object_layout;
mod shape;
mod shm;
mod sync;
pub use dbe::*;
pub use extension::*;
pub use shape::*;
pub use shm::*;
pub use sync::*;
