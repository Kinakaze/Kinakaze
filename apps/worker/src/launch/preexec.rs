//! Native handles inherited only by a fresh, unpublished exec bootstrap.
use crate::{Result, failure};
use kinakaze_v2_protocol::native_exec::{CAPACITY, Launch};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::{
        Memory::{FILE_MAP_READ, MapViewOfFile, UnmapViewOfFile},
        Threading::{SetEvent, WaitForSingleObject},
    },
};

pub(super) struct Activation {
    ready: OwnedHandle,
    activate: OwnedHandle,
    control: OwnedHandle,
}

impl Activation {
    pub fn parse(arguments: &[std::ffi::OsString]) -> Result<Self> {
        if arguments.len() != 1 {
            return Err(failure("invalid exec activation arguments"));
        }
        let handles = arguments[0]
            .to_str()
            .ok_or("invalid exec activation handles")?
            .split(':')
            .map(str::parse::<usize>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if handles.len() != 3
            || handles.iter().any(|&h| h == 0 || h > isize::MAX as usize)
            || handles
                .iter()
                .enumerate()
                .any(|(i, h)| handles[..i].contains(h))
        {
            return Err(failure("invalid exec activation handles"));
        }
        let mut owned = handles
            .into_iter()
            .map(|h| unsafe { OwnedHandle::from_raw_handle(h as _) });
        Ok(Self {
            ready: owned.next().unwrap(),
            activate: owned.next().unwrap(),
            control: owned.next().unwrap(),
        })
    }

    // Called after DLL loading and symbol resolution, before runtime/session
    // initialization. Idle stock has no Linux identity and performs no polling.
    pub fn wait(self) -> Result<Launch> {
        if unsafe { SetEvent(self.ready.as_raw_handle()) } == 0
            || unsafe { WaitForSingleObject(self.activate.as_raw_handle(), u32::MAX) }
                != WAIT_OBJECT_0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let view =
            unsafe { MapViewOfFile(self.control.as_raw_handle(), FILE_MAP_READ, 0, 0, CAPACITY) };
        if view.Value.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let launch =
            Launch::decode(unsafe { std::slice::from_raw_parts(view.Value.cast(), CAPACITY) });
        unsafe { UnmapViewOfFile(view) };
        launch.ok_or_else(|| failure("invalid exec activation frame"))
    }
}
