//! One hidden broadcast receiver per runtime domain. Windows broadcasts
//! WM_TIMECHANGE on discontinuous wall-clock changes. The shared generation
//! survives fork/exec and receiver-owner exit; a waiting process takes over.
use std::ptr;
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Subscription {
    generation: usize,
    _mapping: usize,
}
static SUBSCRIPTION: OnceLock<Result<Subscription, i32>> = OnceLock::new();

unsafe extern "system" fn window_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_TIMECHANGE {
        let address = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
        if address != 0 {
            unsafe { &*(address as *const AtomicU64) }.fetch_add(1, Ordering::Release);
        }
        return 0;
    }
    unsafe { DefWindowProcW(window, message, w, l) }
}

fn subscribe() -> Result<Subscription, i32> {
    let domain = kinakaze_runtime::authority::domain_id();
    let wide = |name: String| name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let name = wide(format!(r"Local\kinakaze.clock-change.v1.{domain}"));
    let mapping = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            4096,
            name.as_ptr(),
        )
    };
    if mapping.is_null() {
        return Err(crate::EIO);
    }
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
    if view.Value.is_null() {
        unsafe {
            CloseHandle(mapping);
        }
        return Err(crate::EIO);
    }
    let generation = view.Value as usize;
    let leader_name = wide(format!(r"Local\kinakaze.clock-change.owner.v1.{domain}"));
    let leader = unsafe { CreateMutexW(ptr::null(), 0, leader_name.as_ptr()) };
    if leader.is_null() {
        unsafe {
            UnmapViewOfFile(view);
            CloseHandle(mapping);
        }
        return Err(crate::EIO);
    }
    let leader = leader as usize;
    let (ready, started) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("kinakaze-clock-change".into())
        .spawn(move || {
            let leader = leader as HANDLE;
            let mut ready = Some(ready);
            let mut acquired = unsafe { WaitForSingleObject(leader, 0) };
            if acquired == WAIT_TIMEOUT {
                // Another live process already owns the receiver. Remain queued
                // on its abandoned mutex so its exit never disables notifications.
                let _ = ready.take().unwrap().send(Ok(()));
                acquired = unsafe { WaitForSingleObject(leader, INFINITE) };
            }
            if !matches!(acquired, WAIT_OBJECT_0 | WAIT_ABANDONED) {
                if let Some(ready) = ready {
                    let _ = ready.send(Err(crate::EIO));
                }
                unsafe {
                    CloseHandle(leader);
                }
                return;
            }
            let class = wide(format!("KinakazeClockChange{}", std::process::id()));
            let instance = unsafe { GetModuleHandleW(ptr::null()) };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..unsafe { std::mem::zeroed() }
            };
            let registered = unsafe { RegisterClassW(&wc) };
            let window = if registered == 0 {
                ptr::null_mut()
            } else {
                // Hidden top-level window: message-only windows do not receive
                // system broadcasts. Never show or activate this window.
                unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        class.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        instance,
                        ptr::null(),
                    )
                }
            };
            if window.is_null() {
                if let Some(ready) = ready {
                    let _ = ready.send(Err(crate::EIO));
                }
            } else {
                unsafe {
                    SetWindowLongPtrW(window, GWLP_USERDATA, generation as isize);
                }
                if let Some(ready) = ready {
                    let _ = ready.send(Ok(()));
                }
                let mut message = unsafe { std::mem::zeroed() };
                while unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                unsafe {
                    DestroyWindow(window);
                }
            }
            unsafe {
                ReleaseMutex(leader);
                CloseHandle(leader);
            }
        });
    if spawned.is_err() {
        unsafe {
            CloseHandle(leader as HANDLE);
            UnmapViewOfFile(view);
            CloseHandle(mapping);
        }
        return Err(crate::EIO);
    }
    // This mapping remains live while the process and its receiver thread do.
    started.recv().map_err(|_| crate::EIO)??;
    Ok(Subscription {
        generation,
        _mapping: mapping as usize,
    })
}

pub(super) fn generation() -> Result<u64, i32> {
    let subscription = SUBSCRIPTION
        .get_or_init(subscribe)
        .as_ref()
        .map_err(|e| *e)?;
    Ok(unsafe { &*(subscription.generation as *const AtomicU64) }.load(Ordering::Acquire))
}
