//! Deferred mixer operations for native and foreign (including GLib) mainloops.
use super::*;
use std::ffi::CString;

struct Pending {
    context: *mut pa_context,
    operation: *mut pa_operation,
    action: Option<Box<dyn FnOnce(*mut pa_context)>>,
    counted: bool,
}
unsafe extern "sysv64" fn dispatch(
    api: *mut pa_mainloop_api,
    event: *mut c_void,
    data: *mut c_void,
) {
    let pending = unsafe { &mut *data.cast::<Pending>() };
    unsafe { ((*api).defer_enable)(event, 0) };
    if unsafe { (*pending.operation).state.load(Ordering::Acquire) } == 0
        && unsafe { (*pending.context).state.load(Ordering::Acquire) } != PA_CONTEXT_TERMINATED
    {
        if let Some(action) = pending.action.take() {
            action(pending.context);
        }
        let _ = unsafe {
            (*pending.operation).state.compare_exchange(
                0,
                PA_OPERATION_DONE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
        };
    }
    // Foreign mainloops can destroy a freed defer source on a later iteration.
    // The request is already complete when its callback returns.
    pending.counted = false;
    unsafe { (*pending.context).pending.fetch_sub(1, Ordering::Release) };
    unsafe { ((*api).defer_free)(event) };
}
unsafe extern "sysv64" fn destroy(
    _api: *mut pa_mainloop_api,
    _event: *mut c_void,
    data: *mut c_void,
) {
    let pending = unsafe { Box::from_raw(data.cast::<Pending>()) };
    unsafe {
        if pending.counted {
            (*pending.context).pending.fetch_sub(1, Ordering::Release);
        }
        pa_operation_unref(pending.operation);
        pa_context_unref(pending.context);
    }
}
pub(super) unsafe fn enqueue(
    context: *mut pa_context,
    action: impl FnOnce(*mut pa_context) + 'static,
) -> *mut pa_operation {
    if context.is_null() {
        return ptr::null_mut();
    }
    let api = unsafe { (*context).api };
    if api.is_null() {
        action(context);
        return operation_done();
    }
    let operation = Box::into_raw(Box::new(pa_operation {
        refs: AtomicUsize::new(2),
        state: AtomicI32::new(0),
    }));
    unsafe { (*context).refs.fetch_add(1, Ordering::Relaxed) };
    unsafe { (*context).pending.fetch_add(1, Ordering::Relaxed) };
    let pending = Box::into_raw(Box::new(Pending {
        context,
        operation,
        action: Some(Box::new(action)),
        counted: true,
    }));
    let event = unsafe { ((*api).defer_new)(api, dispatch, pending.cast()) };
    if event.is_null() {
        unsafe {
            destroy(api, event, pending.cast());
            pa_operation_unref(operation);
            (*context).error.store(25, Ordering::Release);
        }
        ptr::null_mut()
    } else {
        unsafe { ((*api).defer_set_destroy)(event, Some(destroy)) };
        operation
    }
}
pub(super) unsafe fn success(
    context: *mut pa_context,
    callback: Option<ContextSuccess>,
    userdata: *mut c_void,
    error: i32,
) -> *mut pa_operation {
    unsafe {
        enqueue(context, move |c| {
            (*c).error.store(error, Ordering::Release);
            if let Some(cb) = callback {
                cb(c, i32::from(error == 0), userdata);
            }
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_is_pending")]
pub unsafe extern "sysv64" fn pa_context_is_pending(context: *mut pa_context) -> c_int {
    i32::from(!context.is_null() && unsafe { (*context).pending.load(Ordering::Acquire) != 0 })
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_sample_info_list")]
pub unsafe extern "sysv64" fn pa_context_get_sample_info_list(
    context: *mut pa_context,
    callback: Option<unsafe extern "sysv64" fn(*mut pa_context, *const c_void, c_int, *mut c_void)>,
    data: *mut c_void,
) -> *mut pa_operation {
    // Playback streams do not populate a server sample cache. Complete the
    // empty enumeration through the same deferred mainloop as other queries.
    unsafe {
        enqueue(context, move |context| {
            if let Some(callback) = callback {
                callback(context, ptr::null(), 1, data);
            }
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_remove_sample")]
pub unsafe extern "sysv64" fn pa_context_remove_sample(
    context: *mut pa_context,
    _name: *const c_char,
    callback: Option<ContextSuccess>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe { success(context, callback, data, 5) } // PA_ERR_NOENTITY: cache is empty.
}
unsafe fn notify_change(c: *mut pa_context, facility: i32, index: u32) {
    if unsafe { (*c).subscribe_mask.load(Ordering::Acquire) } & (1 << facility) == 0 {
        return;
    }
    if let Some(cb) =
        unsafe { callback_from_usize::<SubscribeNotify>((*c).subscribe_cb.load(Ordering::Acquire)) }
    {
        unsafe {
            cb(
                c,
                0x10 | facility,
                index,
                (*c).subscribe_userdata.load(Ordering::Acquire) as *mut c_void,
            )
        };
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_ref")]
pub unsafe extern "sysv64" fn pa_context_ref(c: *mut pa_context) -> *mut pa_context {
    if !c.is_null() {
        unsafe { (*c).refs.fetch_add(1, Ordering::Relaxed) };
    }
    c
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_operation_ref")]
pub unsafe extern "sysv64" fn pa_operation_ref(op: *mut pa_operation) -> *mut pa_operation {
    if !op.is_null() {
        unsafe { (*op).refs.fetch_add(1, Ordering::Relaxed) };
    }
    op
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_operation_cancel")]
pub unsafe extern "sysv64" fn pa_operation_cancel(op: *mut pa_operation) {
    if !op.is_null() {
        let _ = unsafe {
            (*op)
                .state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
        };
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_new_with_proplist")]
pub unsafe extern "sysv64" fn pa_context_new_with_proplist(
    api: *mut c_void,
    name: *const c_char,
    properties: *const pa_proplist,
) -> *mut pa_context {
    let c = unsafe { pa_context_new(api, name) };
    if !properties.is_null() {
        unsafe { (*c).properties = (*properties).clone() };
    }
    c
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_server_protocol_version")]
pub unsafe extern "sysv64" fn pa_context_get_server_protocol_version(c: *const pa_context) -> u32 {
    if unsafe { pa_context_get_state(c) } == PA_CONTEXT_READY {
        35
    } else {
        u32::MAX
    }
}

// waveOut has one default output endpoint and no PulseAudio card profiles.
// Native capture has not been implemented, so no source/output is advertised.
pub type EmptyInfoNotify =
    unsafe extern "sysv64" fn(*mut pa_context, *const c_void, c_int, *mut c_void);
macro_rules! empty_query {
    ($name:ident, $error:expr $(, $arg:ident:$kind:ty)*) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libpulse_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(c:*mut pa_context,$($arg:$kind,)*cb:Option<EmptyInfoNotify>,data:*mut c_void)->*mut pa_operation {
            $(let _=$arg;)*
            unsafe { enqueue(c,move |c| { (*c).error.store($error,Ordering::Release); if let Some(cb)=cb { cb(c,ptr::null(),if $error==0 {1} else {-1},data); } }) }
        }
    }
}
empty_query!(pa_context_get_card_info_list, 0);
empty_query!(pa_context_get_card_info_by_index,5,index:u32);
empty_query!(pa_context_get_source_output_info_list, 0);
empty_query!(pa_context_get_source_output_info,5,index:u32);

#[repr(C)]
pub struct ClientInfo {
    index: u32,
    name: *const c_char,
    owner_module: u32,
    driver: *const c_char,
    proplist: *mut pa_proplist,
}
pub type ClientNotify =
    unsafe extern "sysv64" fn(*mut pa_context, *const ClientInfo, c_int, *mut c_void);
unsafe fn clients(
    c: *mut pa_context,
    index: Option<u32>,
    cb: Option<ClientNotify>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe {
        enqueue(c, move |c| {
            if let Some(cb) = cb {
                let found = index.is_none() || index == Some(0);
                if found {
                    let info = ClientInfo {
                        index: 0,
                        name: (*c).name.as_ptr(),
                        owner_module: u32::MAX,
                        driver: SERVER_NAME.as_ptr().cast(),
                        proplist: &raw mut (*c).properties,
                    };
                    cb(c, &info, 0, data);
                }
                cb(c, ptr::null(), if found { 1 } else { -1 }, data);
            }
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_client_info_list")]
pub unsafe extern "sysv64" fn pa_context_get_client_info_list(
    c: *mut pa_context,
    cb: Option<ClientNotify>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe { clients(c, None, cb, data) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_client_info")]
pub unsafe extern "sysv64" fn pa_context_get_client_info(
    c: *mut pa_context,
    index: u32,
    cb: Option<ClientNotify>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe { clients(c, Some(index), cb, data) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_sink_input_info_list")]
pub unsafe extern "sysv64" fn pa_context_get_sink_input_info_list(
    c: *mut pa_context,
    cb: Option<SinkInputInfoNotify>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe {
        enqueue(c, move |c| {
            if let Some(cb) = cb {
                cb(c, ptr::null(), 1, data);
            }
        })
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_sink_volume_by_index")]
pub unsafe extern "sysv64" fn pa_context_set_sink_volume_by_index(
    c: *mut pa_context,
    index: u32,
    v: *const pa_cvolume,
    cb: Option<ContextSuccess>,
    data: *mut c_void,
) -> *mut pa_operation {
    if index != 0 {
        return unsafe { success(c, cb, data, 5) };
    }
    if unsafe { pa_cvolume_valid(v) } == 0 || unsafe { (*v).channels } != 2 {
        return unsafe { success(c, cb, data, 3) };
    }
    let v = unsafe { *v };
    unsafe {
        enqueue(c, move |c| {
            GLOBAL_SINK_VOLUME.store(pa_cvolume_avg(&v), Ordering::Release);
            *lock(sink_channels()) = v;
            let error = if apply_hardware_volume() { 0 } else { 26 };
            (*c).error.store(error, Ordering::Release);
            if let Some(cb) = cb {
                cb(c, i32::from(error == 0), data);
            }
            if error == 0 {
                notify_change(c, 0, 0);
            }
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_sink_mute_by_index")]
pub unsafe extern "sysv64" fn pa_context_set_sink_mute_by_index(
    c: *mut pa_context,
    index: u32,
    mute: c_int,
    cb: Option<ContextSuccess>,
    data: *mut c_void,
) -> *mut pa_operation {
    if index != 0 {
        return unsafe { success(c, cb, data, 5) };
    }
    unsafe {
        enqueue(c, move |c| {
            GLOBAL_SINK_MUTE.store(i32::from(mute != 0), Ordering::Release);
            let error = if apply_hardware_volume() { 0 } else { 26 };
            (*c).error.store(error, Ordering::Release);
            if let Some(cb) = cb {
                cb(c, i32::from(error == 0), data);
            }
            if error == 0 {
                notify_change(c, 0, 0);
            }
        })
    }
}
macro_rules! unavailable_control {
    ($name:ident $(,$arg:ident:$kind:ty)*) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libpulse_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(c:*mut pa_context,$($arg:$kind,)*cb:Option<ContextSuccess>,data:*mut c_void)->*mut pa_operation {
            $(let _=$arg;)* unsafe { success(c,cb,data,5) }
        }
    }
}
unavailable_control!(pa_context_set_card_profile_by_index,index:u32,profile:*const c_char);
unavailable_control!(pa_context_set_sink_port_by_index,index:u32,port:*const c_char);
unavailable_control!(pa_context_set_source_port_by_index,index:u32,port:*const c_char);
unavailable_control!(pa_context_set_source_volume_by_index,index:u32,volume:*const pa_cvolume);
unavailable_control!(pa_context_set_source_mute_by_index,index:u32,mute:c_int);
unavailable_control!(pa_context_set_source_output_volume,index:u32,volume:*const pa_cvolume);
unavailable_control!(pa_context_set_source_output_mute,index:u32,mute:c_int);
unavailable_control!(pa_context_set_default_source,name:*const c_char);
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_default_sink")]
pub unsafe extern "sysv64" fn pa_context_set_default_sink(
    c: *mut pa_context,
    name: *const c_char,
    cb: Option<ContextSuccess>,
    data: *mut c_void,
) -> *mut pa_operation {
    let valid = !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes_with_nul() == SINK_NAME;
    unsafe { success(c, cb, data, if valid { 0 } else { 5 }) }
}

// The native server has no module-stream-restore. Report the protocol's
// NOEXTENSION response asynchronously, allowing GVC to disable that feature.
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_ext_stream_restore_read")]
pub unsafe extern "sysv64" fn pa_ext_stream_restore_read(
    c: *mut pa_context,
    cb: Option<EmptyInfoNotify>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe {
        enqueue(c, move |c| {
            (*c).error.store(21, Ordering::Release);
            if let Some(cb) = cb {
                cb(c, ptr::null(), -1, data);
            }
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_ext_stream_restore_write")]
pub unsafe extern "sysv64" fn pa_ext_stream_restore_write(
    c: *mut pa_context,
    _mode: c_int,
    _entries: *const c_void,
    _count: u32,
    _apply: c_int,
    cb: Option<ContextSuccess>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe { success(c, cb, data, 21) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_ext_stream_restore_subscribe")]
pub unsafe extern "sysv64" fn pa_ext_stream_restore_subscribe(
    c: *mut pa_context,
    _enable: c_int,
    cb: Option<ContextSuccess>,
    data: *mut c_void,
) -> *mut pa_operation {
    unsafe { success(c, cb, data, 21) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_ext_stream_restore_set_subscribe_cb")]
pub unsafe extern "sysv64" fn pa_ext_stream_restore_set_subscribe_cb(
    c: *mut pa_context,
    cb: Option<ContextNotify>,
    data: *mut c_void,
) {
    if !c.is_null() {
        unsafe {
            (*c).restore_cb
                .store(cb.map_or(0, |cb| cb as usize), Ordering::Release);
            (*c).restore_userdata
                .store(data as usize, Ordering::Release);
        }
    }
}
pub(super) fn context_name(name: *const c_char) -> CString {
    if name.is_null() {
        CString::new("Kinakaze application").unwrap()
    } else {
        unsafe { CStr::from_ptr(name) }.to_owned()
    }
}
