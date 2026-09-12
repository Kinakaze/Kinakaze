//! In-process client properties and permissions. No unrelated fd is exposed
//! as a PipeWire protocol connection when the graph is hosted by WASAPI.
use super::*;
pub(super) const READ: u32 = 0o400;
pub(super) const WRITE: u32 = 0o200;
pub(super) const EXECUTE: u32 = 0o100;
pub(super) const ALL: u32 = 0o730;
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Permission {
    pub(super) id: u32,
    pub(super) permissions: u32,
}
#[repr(C)]
struct Info {
    id: u32,
    change_mask: u64,
    props: *const SpaDict,
}
#[repr(C)]
struct Events {
    version: u32,
    info: Option<unsafe extern "sysv64" fn(*mut c_void, *const Info)>,
    permissions: Option<unsafe extern "sysv64" fn(*mut c_void, u32, u32, *const Permission)>,
}
#[repr(C)]
pub struct Client {
    pub(super) header: ProxyHeader,
    pub(super) core: *mut pw_core,
    listeners: hooks::Hooks,
    dirty: bool,
    requests: std::collections::VecDeque<(u32, u32)>,
}
#[repr(C)]
struct Methods {
    version: u32,
    add_listener:
        unsafe extern "sysv64" fn(*mut c_void, *mut SpaHook, *const Events, *mut c_void) -> i32,
    error: unsafe extern "sysv64" fn(*mut c_void, u32, i32, *const c_char) -> i32,
    update_properties: unsafe extern "sysv64" fn(*mut c_void, *const SpaDict) -> i32,
    get_permissions: unsafe extern "sysv64" fn(*mut c_void, u32, u32) -> i32,
    update_permissions: unsafe extern "sysv64" fn(*mut c_void, u32, *const Permission) -> i32,
}
static METHODS: Methods = Methods {
    version: 0,
    add_listener,
    error,
    update_properties,
    get_permissions,
    update_permissions,
};
pub(super) unsafe fn permissions(core: *mut pw_core, id: u32) -> u32 {
    let values = unsafe { &(*core).permissions };
    values
        .iter()
        .find(|p| p.id == id)
        .or_else(|| values.first())
        .map_or(0, |p| p.permissions)
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_core_get_client")]
pub unsafe extern "sysv64" fn pw_core_get_client(core: *mut pw_core) -> *mut Client {
    if core.is_null() {
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    }
    if unsafe { (*core).client.is_null() } {
        let mut object = Box::new(Client {
            header: ProxyHeader {
                interface: make_interface(
                    c"PipeWire:Interface:Client".as_ptr(),
                    3,
                    (&raw const METHODS).cast(),
                    ptr::null_mut(),
                ),
                kind: PROXY_CLIENT,
                user_data: Vec::new(),
            },
            core,
            listeners: hooks::Hooks::new(),
            dirty: true,
            requests: Default::default(),
        });
        object.header.interface.callbacks.data = (&raw mut *object).cast();
        unsafe {
            (*core).client = Box::into_raw(object);
        }
    }
    unsafe { (*core).client }
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_core_steal_fd")]
pub unsafe extern "sysv64" fn pw_core_steal_fd(core: *mut pw_core) -> i32 {
    if core.is_null() { -22 } else { -95 }
}
unsafe extern "sysv64" fn add_listener(
    object: *mut c_void,
    hook: *mut SpaHook,
    events: *const Events,
    data: *mut c_void,
) -> i32 {
    let client = unsafe { &mut *object.cast::<Client>() };
    let result = unsafe { client.listeners.add(hook, events.cast(), data) };
    if result == 0 {
        schedule_core(unsafe { (*client.core).loop_ });
    }
    result
}
unsafe extern "sysv64" fn error(
    object: *mut c_void,
    id: u32,
    res: i32,
    message: *const c_char,
) -> i32 {
    let core = unsafe { (*object.cast::<Client>()).core };
    if unsafe { permissions(core, 1) } & (WRITE | EXECUTE) != (WRITE | EXECUTE) {
        return -13;
    }
    unsafe { notify_core_error(core, id, res, message) };
    0
}
pub(super) unsafe fn update_dict(props: *mut pw_properties, dict: *const SpaDict) -> i32 {
    if dict.is_null() {
        return -22;
    }
    let dict = unsafe { &*dict };
    if dict.n_items > 4096 || (dict.n_items != 0 && dict.items.is_null()) {
        return -22;
    }
    // Validate and copy every input before changing storage. A caller may pass
    // our own dictionary back, and a malformed later entry must not leave its
    // existing dict pointing into freed strings.
    let mut incoming = Vec::new();
    if incoming.try_reserve_exact(dict.n_items as usize).is_err() {
        return -12;
    }
    for index in 0..dict.n_items as usize {
        let item = unsafe { *dict.items.add(index) };
        if item.key.is_null() {
            return -22;
        }
        incoming.push((
            unsafe { CStr::from_ptr(item.key) }.to_owned(),
            if item.value.is_null() {
                None
            } else {
                Some(unsafe { CStr::from_ptr(item.value) }.to_owned())
            },
        ));
    }
    let mut changes = 0;
    let storage = unsafe { properties_mut(props) };
    for (key, value) in incoming {
        if let Some(index) = storage
            .values
            .iter()
            .position(|(candidate, _)| candidate == &key)
        {
            match value {
                Some(value) if storage.values[index].1 == value => {}
                Some(value) => {
                    storage.values[index].1 = value;
                    changes += 1;
                }
                None => {
                    storage.values.remove(index);
                    changes += 1;
                }
            }
        } else if let Some(value) = value {
            storage.values.push((key, value));
            changes += 1;
        }
    }
    if changes != 0 {
        unsafe { properties_mut(props) }.refresh();
    }
    changes
}
unsafe extern "sysv64" fn update_properties(object: *mut c_void, dict: *const SpaDict) -> i32 {
    let client = unsafe { &mut *object.cast::<Client>() };
    if unsafe { permissions(client.core, 1) } & (WRITE | EXECUTE) != (WRITE | EXECUTE) {
        return -13;
    }
    let changed = unsafe { update_dict((*client.core).props, dict) };
    if changed > 0 {
        client.dirty = true;
        schedule_core(unsafe { (*client.core).loop_ });
    }
    changed
}
unsafe extern "sysv64" fn get_permissions(object: *mut c_void, index: u32, count: u32) -> i32 {
    let client = unsafe { &mut *object.cast::<Client>() };
    if unsafe { permissions(client.core, 1) } & (WRITE | EXECUTE) != (WRITE | EXECUTE) {
        return -13;
    }
    if client.requests.len() >= 1024 {
        return -28;
    }
    client.requests.push_back((index, count));
    schedule_core(unsafe { (*client.core).loop_ });
    0
}
unsafe extern "sysv64" fn update_permissions(
    object: *mut c_void,
    count: u32,
    values: *const Permission,
) -> i32 {
    let client = unsafe { &mut *object.cast::<Client>() };
    if count > 1024 || (count != 0 && values.is_null()) {
        return -22;
    }
    if unsafe { permissions(client.core, 1) } & (WRITE | EXECUTE) != (WRITE | EXECUTE) {
        return -13;
    }
    let mut updated = unsafe { (*client.core).permissions.clone() };
    for index in 0..count as usize {
        let value = unsafe { *values.add(index) };
        if value.permissions & !ALL != 0 {
            return -22;
        }
        if value.id != u32::MAX && value.id > NATIVE_NODE_ID {
            continue;
        }
        let old = updated
            .iter()
            .find(|p| p.id == value.id)
            .or_else(|| updated.first())
            .map_or(0, |p| p.permissions);
        // A client can restrict its own access, not manufacture new grants.
        let value = Permission {
            id: value.id,
            permissions: value.permissions & old,
        };
        if let Some(entry) = updated.iter_mut().find(|entry| entry.id == value.id) {
            *entry = value;
        } else if updated.len() < 4096 {
            updated.push(value);
        } else {
            return -28;
        }
    }
    unsafe {
        (*client.core).permissions = updated;
    }
    schedule_core(unsafe { (*client.core).loop_ });
    0
}
pub(super) unsafe fn dispatch(core: *mut pw_core, loop_: *mut pw_thread_loop, serial: usize) {
    let pointer = unsafe { (*core).client };
    if pointer.is_null() {
        return;
    }
    let hooks = unsafe { (*pointer).listeners.snapshot() };
    let dirty = unsafe { std::mem::replace(&mut (*pointer).dirty, false) };
    let props = unsafe { Box::from_raw(copy_properties((*core).props).cast::<Properties>()) };
    let info = Info {
        id: 1,
        change_mask: 1,
        props: &raw const props.public.dict,
    };
    for (hook, callbacks, initial) in hooks {
        if !core_is_live(loop_, core, serial) || unsafe { (*core).client } != pointer {
            return;
        }
        if !(dirty || initial) || !unsafe { (*pointer).listeners.contains(hook) } {
            continue;
        }
        unsafe {
            hooks::Hooks::initialized(hook);
        }
        if let Some(call) = unsafe { (*callbacks.funcs.cast::<Events>()).info } {
            unsafe { call(callbacks.data, &info) };
        }
    }
    if !core_is_live(loop_, core, serial) || unsafe { (*core).client } != pointer {
        return;
    }
    let requests = unsafe { std::mem::take(&mut (*pointer).requests) };
    for (index, count) in requests {
        let values = unsafe {
            (*core)
                .permissions
                .iter()
                .skip(index as usize)
                .take(count as usize)
                .copied()
                .collect::<Vec<_>>()
        };
        let hooks = unsafe { (*pointer).listeners.snapshot() };
        for (hook, callbacks, _) in hooks {
            if !core_is_live(loop_, core, serial) || unsafe { (*core).client } != pointer {
                return;
            }
            if !unsafe { (*pointer).listeners.contains(hook) } {
                continue;
            }
            if let Some(call) = unsafe { (*callbacks.funcs.cast::<Events>()).permissions } {
                unsafe { call(callbacks.data, index, values.len() as u32, values.as_ptr()) };
            }
        }
        if !core_is_live(loop_, core, serial) || unsafe { (*core).client } != pointer {
            return;
        }
    }
}
pub(super) unsafe fn destroy(pointer: *mut Client) {
    let object = unsafe { Box::from_raw(pointer) };
    unsafe {
        (*object.core).client = ptr::null_mut();
    }
}

pub(super) unsafe fn object_listener(
    proxy: *mut c_void,
    listener: *mut SpaHook,
    events: *const c_void,
    data: *mut c_void,
) {
    unsafe { add_listener(proxy, listener, events.cast(), data) };
}
