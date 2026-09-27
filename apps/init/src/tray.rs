//! The environment's single notification icon. Closing a terminal is detach.
use crate::desktop::Desktop;
use std::{
    io,
    sync::{Arc, atomic::Ordering, mpsc},
};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Shell::*, WindowsAndMessaging::*},
};
const CALLBACK: u32 = WM_APP + 41;
const OPEN_WEB: u32 = 100;
const STOP: u32 = 101;
const TERMINAL: u32 = 200;
struct Context {
    desktop: Arc<Desktop>,
    names: Vec<String>,
    taskbar: u32,
    closing: bool,
}
pub struct Tray {
    hwnd: isize,
}
impl Tray {
    pub fn close(self) {
        unsafe {
            PostMessageW(self.hwnd as _, WM_CLOSE, 0, 0);
        }
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn icon(hwnd: HWND, action: u32, closing: bool) -> bool {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = CALLBACK;
    data.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    for (out, value) in data.szTip.iter_mut().zip(
        if closing {
            "Kinakaze — 正在关闭进程树…"
        } else {
            "Kinakaze — 运行环境"
        }
        .encode_utf16(),
    ) {
        *out = value;
    }
    let ok = unsafe { Shell_NotifyIconW(action, &data) } != 0;
    if action == NIM_ADD && ok {
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        unsafe {
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }
    ok
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Context;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let context = unsafe { &mut *ptr };
    if msg == context.taskbar {
        unsafe {
            icon(
                hwnd,
                NIM_ADD,
                context.desktop.closing.load(Ordering::Acquire),
            );
        }
        return 0;
    }
    match msg {
        WM_TIMER => {
            let closing = context.desktop.closing.load(Ordering::Acquire);
            if closing != context.closing {
                context.closing = closing;
                unsafe {
                    icon(hwnd, NIM_MODIFY, closing);
                }
            }
            0
        }
        CALLBACK => {
            let event = (lparam as u32) & 0xffff;
            if event == WM_LBUTTONDBLCLK || event == NIN_SELECT || event == NIN_SELECT | 1 {
                if !context.desktop.closing.load(Ordering::Acquire) {
                    let _ = context
                        .desktop
                        .open(&context.desktop.startup.default_terminal);
                }
            } else if event == WM_CONTEXTMENU || event == WM_RBUTTONUP {
                let snapshot = context.desktop.snapshot();
                let closing = snapshot["closing"] == true;
                context.names = snapshot["terminals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["name"].as_str().map(str::to_owned))
                    .collect();
                unsafe {
                    let menu = CreatePopupMenu();
                    for (i, name) in context.names.iter().enumerate() {
                        AppendMenuW(
                            menu,
                            MF_STRING | if closing { MF_GRAYED } else { 0 },
                            TERMINAL as usize + i,
                            wide(&format!("回到终端 · {name}")).as_ptr(),
                        );
                    }
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        OPEN_WEB as usize,
                        wide("打开 WebUI").as_ptr(),
                    );
                    AppendMenuW(
                        menu,
                        MF_STRING | if closing { MF_GRAYED } else { 0 },
                        STOP as usize,
                        wide(if closing {
                            "正在关闭进程树…"
                        } else {
                            "关闭整个进程树"
                        })
                        .as_ptr(),
                    );
                    let mut point = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut point);
                    SetForegroundWindow(hwnd);
                    let selected = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        0,
                        hwnd,
                        std::ptr::null(),
                    );
                    DestroyMenu(menu);
                    if selected != 0 {
                        PostMessageW(hwnd, WM_COMMAND, selected as usize, 0);
                    }
                    PostMessageW(hwnd, WM_NULL, 0, 0);
                }
            }
            0
        }
        WM_COMMAND => {
            let command = (wparam & 0xffff) as u32;
            if command == STOP {
                context.desktop.shutdown();
                context.closing = true;
                unsafe {
                    icon(hwnd, NIM_MODIFY, true);
                }
            } else if command == OPEN_WEB {
                if let Some(url) = context.desktop.web_url.get() {
                    unsafe {
                        ShellExecuteW(
                            hwnd,
                            wide("open").as_ptr(),
                            wide(url).as_ptr(),
                            std::ptr::null(),
                            std::ptr::null(),
                            SW_SHOWNORMAL,
                        );
                    }
                }
            } else if command >= TERMINAL && !context.desktop.closing.load(Ordering::Acquire) {
                if let Some(name) = context.names.get((command - TERMINAL) as usize) {
                    let _ = context.desktop.open(name);
                }
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                KillTimer(hwnd, 1);
                icon(hwnd, NIM_DELETE, context.closing);
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
pub fn start(desktop: Arc<Desktop>) -> io::Result<Tray> {
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("init-tray".into())
        .spawn(move || unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide("Kinakaze.Environment.Tray");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            wc.lpfnWndProc = Some(procedure);
            if RegisterClassW(&wc) == 0 {
                let _ = send.send(Err(io::Error::last_os_error()));
                return;
            }
            let mut context = Context {
                desktop,
                names: vec![],
                taskbar: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                closing: false,
            };
            // A hidden top-level window receives Explorer's TaskbarCreated broadcast.
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Kinakaze").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                (&mut context as *mut Context).cast(),
            );
            if hwnd.is_null() {
                let _ = send.send(Err(io::Error::last_os_error()));
                return;
            }
            if !icon(hwnd, NIM_ADD, false) {
                DestroyWindow(hwnd);
                let _ = send.send(Err(io::Error::other("cannot create notification icon")));
                return;
            }
            SetTimer(hwnd, 1, 200, None);
            let _ = send.send(Ok(hwnd as isize));
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            UnregisterClassW(class.as_ptr(), instance);
        })?;
    Ok(Tray {
        hwnd: receive.recv().map_err(io::Error::other)??,
    })
}
