//! The only RPC transport owner loaded into a worker process.
//!
//! C callers must provide live, correctly sized and aligned allocations. An API
//! table borrows its session until `close`; every provider call must finish
//! before closing it. Providers must never retain a table past that point.

use core::{ffi::c_void, mem, ptr, slice};
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Mutex,
        atomic::{AtomicPtr, Ordering},
    },
};

use kinakaze_v2_abi::*;
use kinakaze_v2_host_win::PipeConnection;
use kinakaze_v2_protocol::{
    ClientRole, Hello, MAX_FRAME_SIZE, PROTOCOL_VERSION, Reply, Request, RuntimeOpenConfig,
    WireRequest, WireResponse, read_frame, write_frame,
};

#[cfg(feature = "guest-engine")]
mod guest;
#[cfg(feature = "guest-engine")]
mod helper;

struct Connection {
    pipe: Option<PipeConnection>,
    next_id: u64,
}

impl Connection {
    fn exchange(&mut self, request: Request) -> Result<WireResponse, i32> {
        let id = self.next_id;
        let wire_request = WireRequest { id, request };
        // The wire envelope is larger than the input JSON. Reject a local size
        // error before touching the connection or consuming a sequence number.
        let payload = serde_json::to_vec(&wire_request).map_err(|_| STATUS_INVALID_ARGUMENT)?;
        if payload.len() > MAX_FRAME_SIZE {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        self.next_id = id.checked_add(1).ok_or(STATUS_PROTOCOL)?;
        let pipe = self.pipe.as_mut().ok_or(STATUS_TRANSPORT)?;
        let result = exchange_frame(pipe, wire_request);
        // A partial write/read has an uncertain outcome. Never replay a request
        // on this session; the worker lifecycle, not a DLL retry, owns recovery.
        if result.is_err() {
            self.pipe = None;
        }
        result
    }
}

fn exchange_frame<T: io::Read + io::Write>(
    stream: &mut T,
    request: WireRequest,
) -> Result<WireResponse, i32> {
    write_frame(stream, &request).map_err(|_| STATUS_TRANSPORT)?;
    let response: WireResponse = read_frame(stream).map_err(|_| STATUS_TRANSPORT)?;
    if response.id != request.id {
        return Err(STATUS_PROTOCOL);
    }
    Ok(response)
}

struct Session {
    // Read before any inherited synchronization or HANDLE is touched. The
    // native fork copies this allocation but cannot copy a pipe connection.
    owner: u32,
    connection: mem::ManuallyDrop<Mutex<Connection>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.owner == std::process::id() {
            // SAFETY: only the native owner may destroy its mutex and pipe.
            unsafe { mem::ManuallyDrop::drop(&mut self.connection) };
        }
    }
}

// One transport owner even in the small control-plane build. A fork child
// publishes its fresh session before restoring providers or resuming guests.
static ACTIVE_SESSION: AtomicPtr<Session> = AtomicPtr::new(ptr::null_mut());
// Reserve ownership before connecting: two concurrent opens must never issue
// Worker Hello to two managers. Freshly loaded child DLLs have a fresh gate.
static SESSION_GATE: Mutex<()> = Mutex::new(());

fn open_and_publish(config: RuntimeOpenConfig) -> Result<*mut Session, i32> {
    reserve_session(|| open_session(config))
}

fn reserve_session(
    connect: impl FnOnce() -> Result<Box<Session>, i32>,
) -> Result<*mut Session, i32> {
    let _gate = SESSION_GATE.lock().map_err(|_| STATUS_INTERNAL)?;
    let active = ACTIVE_SESSION.load(Ordering::Acquire);
    if !active.is_null() && unsafe { (*active).owner } == std::process::id() {
        return Err(STATUS_INVALID_ARGUMENT);
    }
    publish_session(connect()?)
}

fn publish_session(session: Box<Session>) -> Result<*mut Session, i32> {
    let pointer = Box::into_raw(session);
    let mut previous = ACTIVE_SESSION.load(Ordering::Acquire);
    loop {
        if !previous.is_null() {
            // SAFETY: live sessions remain published until explicit close;
            // open/close require that no borrowed calls are in flight.
            if unsafe { (*previous).owner } == std::process::id() {
                // SAFETY: this unpublished allocation is still ours.
                drop(unsafe { Box::from_raw(pointer) });
                return Err(STATUS_INVALID_ARGUMENT);
            }
        }
        match ACTIVE_SESSION.compare_exchange(
            previous,
            pointer,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(pointer),
            Err(current) => previous = current,
        }
    }
}

/// Resolve a copied parent context before accessing its mutex. Native HANDLE
/// values and locked mutex words from another worker are never usable here.
unsafe fn resolve_session(context: *mut Session) -> Result<&'static Session, i32> {
    let active = ACTIVE_SESSION.load(Ordering::Acquire);
    if active.is_null() {
        return Err(STATUS_NOT_INITIALIZED);
    }
    // SAFETY: ABI callers retain their borrowed contexts until close; the
    // active context is published only while its native owner retains it.
    let active = unsafe { &*active };
    if active.owner != std::process::id() {
        return Err(STATUS_NOT_INITIALIZED);
    }
    if !ptr::eq(context, active) && unsafe { (*context).owner } == active.owner {
        return Err(STATUS_INVALID_ARGUMENT);
    }
    Ok(active)
}

fn session_api(session: *mut Session) -> RuntimeApiV1 {
    RuntimeApiV1 {
        abi_version: ABI_VERSION,
        struct_size: mem::size_of::<RuntimeApiV1>() as u32,
        context: session.cast(),
        call: runtime_call,
    }
}

fn open_session(config: RuntimeOpenConfig) -> Result<Box<Session>, i32> {
    let pipe = PipeConnection::connect(&config.endpoint).map_err(|_| STATUS_TRANSPORT)?;
    let mut connection = Connection {
        pipe: Some(pipe),
        next_id: 1,
    };
    let response = connection.exchange(Request::Hello(Hello {
        version: PROTOCOL_VERSION,
        token: config.token.clone(),
        role: ClientRole::Worker,
        adoption_ticket: config.adoption_ticket.clone(),
    }))?;
    match response.result {
        Ok(Reply::Hello {
            process: Some(_), ..
        }) => {}
        Ok(_) => return Err(STATUS_PROTOCOL),
        Err(_) => return Err(STATUS_REMOTE),
    }
    Ok(Box::new(Session {
        owner: std::process::id(),
        connection: mem::ManuallyDrop::new(Mutex::new(connection)),
    }))
}

fn guard(action: impl FnOnce() -> Result<(), i32>) -> i32 {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(status)) => status,
        Err(_) => STATUS_INTERNAL,
    }
}

fn aligned<T>(value: *const T) -> bool {
    !value.is_null() && (value as usize).is_multiple_of(mem::align_of::<T>())
}

/// Opens a worker session and writes a borrowed runtime table on success.
///
/// # Safety
/// `config_json` is readable for `config_len` bytes and `api_out` points to a
/// writable aligned table. The configuration must not overlap `api_out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_runtime_open_v1(
    config_json: *const u8,
    config_len: u32,
    api_out: *mut RuntimeApiV1,
) -> i32 {
    #[cfg(feature = "guest-engine")]
    if let Err(status) = guest::bootstrap() {
        return status;
    }
    guard(|| {
        if config_json.is_null()
            || config_len == 0
            || config_len as usize > RPC_BUFFER_SIZE
            || !aligned(api_out)
        {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: The ABI caller guarantees the readable configuration span.
        let bytes = unsafe { slice::from_raw_parts(config_json, config_len as usize) };
        let config: RuntimeOpenConfig =
            serde_json::from_slice(bytes).map_err(|_| STATUS_INVALID_ARGUMENT)?;
        if config.endpoint.is_empty() || config.token.is_empty() {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let session = open_and_publish(config)?;
        let api = session_api(session);
        #[cfg(feature = "guest-engine")]
        guest::bind_session();
        // SAFETY: The output is a writable, aligned table supplied by the caller.
        unsafe { ptr::write(api_out, api) };
        Ok(())
    })
}

/// Releases the session and invalidates this table in place.
///
/// # Safety
/// The table must be the live table produced by `open`, with all calls finished
/// and all borrowed copies retired. It must not be closed through another copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_runtime_close_v1(api: *mut RuntimeApiV1) -> i32 {
    guard(|| {
        if !aligned(api) {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let _gate = SESSION_GATE.lock().map_err(|_| STATUS_INTERNAL)?;
        // SAFETY: The caller guarantees a writable live API allocation.
        let api = unsafe { &mut *api };
        if api.abi_version != ABI_VERSION
            || api.struct_size as usize != mem::size_of::<RuntimeApiV1>()
            || !aligned(api.context.cast::<Session>())
            || !std::ptr::fn_addr_eq(api.call, runtime_call as RuntimeCallV1)
        {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        // The owning table on the copied worker stack now owns the adopted
        // session. Resolve it exactly like a call, and retire only that native
        // allocation. The inherited parent allocation is never destroyed.
        let session = unsafe { resolve_session(api.context.cast())? };
        let context = ptr::from_ref(session).cast_mut();
        ACTIVE_SESSION
            .compare_exchange(
                context,
                ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| STATUS_INVALID_ARGUMENT)?;
        api.context = ptr::null_mut();
        api.abi_version = 0;
        // SAFETY: Only the live table returned from open may be passed here.
        drop(unsafe { Box::from_raw(context) });
        Ok(())
    })
}

unsafe extern "C" fn runtime_call(
    context: *mut c_void,
    request: *const u8,
    request_len: u32,
    response: *mut u8,
    response_capacity: u32,
    response_len: *mut u32,
) -> i32 {
    guard(|| {
        if !aligned(response_len) {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: Caller supplies a writable response length, disjoint from the
        // input/output byte spans. A failure always reports zero output bytes.
        unsafe { ptr::write(response_len, 0) };
        // Reject a capacity probe before even examining or executing a request.
        if (response_capacity as usize) < RPC_BUFFER_SIZE {
            return Err(STATUS_BUFFER_TOO_SMALL);
        }
        if !aligned(context.cast::<Session>())
            || request.is_null()
            || response.is_null()
            || request_len == 0
            || request_len as usize > RPC_BUFFER_SIZE
        {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: The caller supplies a valid request byte span.
        let request_bytes = unsafe { slice::from_raw_parts(request, request_len as usize) };
        let request: Request =
            serde_json::from_slice(request_bytes).map_err(|_| STATUS_INVALID_ARGUMENT)?;
        if matches!(request, Request::Hello(_)) {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: The table context borrows a live session until explicit close.
        let session = unsafe { resolve_session(context.cast())? };
        let mut connection = session.connection.lock().map_err(|_| STATUS_INTERNAL)?;
        let wire_response = connection.exchange(request)?;
        let encoded = serde_json::to_vec(&wire_response.result).map_err(|_| STATUS_PROTOCOL)?;
        if encoded.len() > RPC_BUFFER_SIZE {
            connection.pipe = None;
            return Err(STATUS_PROTOCOL);
        }
        // SAFETY: Output allocation has capacity >= RPC_BUFFER_SIZE and does not
        // overlap the newly owned encoded vector or response_len.
        unsafe {
            ptr::copy_nonoverlapping(encoded.as_ptr(), response, encoded.len());
            ptr::write(response_len, encoded.len() as u32);
        }
        Ok(())
    })
}

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_runtime_abi_version_sysv() -> u32 {
    ABI_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_SESSION_LOCK: Mutex<()> = Mutex::new(());

    fn disconnected_session(owner: u32) -> Box<Session> {
        Box::new(Session {
            owner,
            connection: mem::ManuallyDrop::new(Mutex::new(Connection {
                pipe: None,
                next_id: 1,
            })),
        })
    }

    #[test]
    fn copied_context_uses_one_child_session_and_close_releases_only_its_owner() {
        let _serial = TEST_SESSION_LOCK.lock().unwrap();
        let inherited = disconnected_session(std::process::id().wrapping_add(1));
        // A mutex held by a vanished parent thread must never be acquired by
        // the child call or close path. Keeping this guard makes regressions
        // block deterministically instead of depending on host scheduling.
        let parent_lock = inherited.connection.lock().unwrap();
        let child = publish_session(disconnected_session(std::process::id())).unwrap();
        assert!(publish_session(disconnected_session(std::process::id())).is_err());
        let mut copied_api = session_api(ptr::from_ref(&*inherited).cast_mut());
        let request = serde_json::to_vec(&Request::Identity).unwrap();
        let mut response = vec![0; RPC_BUFFER_SIZE];
        let mut length = 9;
        assert_eq!(
            unsafe {
                runtime_call(
                    copied_api.context,
                    request.as_ptr(),
                    request.len() as u32,
                    response.as_mut_ptr(),
                    response.len() as u32,
                    &mut length,
                )
            },
            STATUS_TRANSPORT
        );
        assert_eq!(length, 0);
        assert_eq!(parent_lock.next_id, 1);
        assert_eq!(unsafe { (*child).connection.lock().unwrap().next_id }, 2);
        assert_eq!(
            unsafe { kinakaze_runtime_close_v1(&mut copied_api) },
            STATUS_OK
        );
        assert!(copied_api.context.is_null());
        assert!(ACTIVE_SESSION.load(Ordering::Acquire).is_null());
        assert_eq!(parent_lock.next_id, 1);
        assert!(unsafe { resolve_session(ptr::from_ref(&*inherited).cast_mut()) }.is_err());
        // Retirement leaves no active pointer that could block a subsequent
        // process-local session or route a late call to freed transport state.
        let next = publish_session(disconnected_session(std::process::id())).unwrap();
        let mut next_api = session_api(next);
        assert_eq!(
            unsafe { kinakaze_runtime_close_v1(&mut next_api) },
            STATUS_OK
        );
        drop(parent_lock);
        assert!(inherited.connection.try_lock().is_ok());
    }

    #[test]
    fn concurrent_opens_reserve_before_transport_side_effects_and_failure_releases_gate() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let _serial = TEST_SESSION_LOCK.lock().unwrap();
        assert!(matches!(
            reserve_session(|| Err(STATUS_TRANSPORT)),
            Err(STATUS_TRANSPORT)
        ));
        let starts = AtomicUsize::new(0);
        let barrier = Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let attempts: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        reserve_session(|| {
                            // Represents connect + Worker Hello. A rejected competing
                            // open must not execute this side-effecting operation.
                            starts.fetch_add(1, Ordering::Relaxed);
                            Ok(disconnected_session(std::process::id()))
                        })
                        .map(|pointer| pointer as usize)
                    })
                })
                .collect();
            attempts
                .into_iter()
                .map(|attempt| attempt.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let pointer = results.into_iter().find_map(Result::ok).unwrap() as *mut Session;
        let mut api = session_api(pointer);
        assert_eq!(unsafe { kinakaze_runtime_close_v1(&mut api) }, STATUS_OK);
    }

    #[test]
    fn insufficient_output_never_touches_session_or_request() {
        let mut output_len = 99;
        // SAFETY: No spans/session are dereferenced for insufficient capacity.
        let status = unsafe {
            runtime_call(
                ptr::null_mut(),
                ptr::null(),
                0,
                ptr::null_mut(),
                0,
                &mut output_len,
            )
        };
        assert_eq!(status, STATUS_BUFFER_TOO_SMALL);
        assert_eq!(output_len, 0);
    }

    #[test]
    fn panics_cannot_cross_abi_guard() {
        assert_eq!(guard(|| panic!("test boundary")), STATUS_INTERNAL);
    }

    struct Duplex {
        incoming: io::Cursor<Vec<u8>>,
        outgoing: Vec<u8>,
    }
    impl io::Read for Duplex {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.incoming.read(bytes)
        }
    }
    impl io::Write for Duplex {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.outgoing.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn response_ids_are_checked() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &WireResponse {
                id: 8,
                result: Ok(Reply::Ok),
            },
        )
        .unwrap();
        let mut duplex = Duplex {
            incoming: io::Cursor::new(bytes),
            outgoing: Vec::new(),
        };
        assert!(matches!(
            exchange_frame(
                &mut duplex,
                WireRequest {
                    id: 7,
                    request: Request::Identity
                }
            ),
            Err(STATUS_PROTOCOL)
        ));
        let sent: WireRequest = read_frame(&mut io::Cursor::new(duplex.outgoing)).unwrap();
        assert_eq!(sent.id, 7);
    }

    #[test]
    fn oversized_envelope_is_rejected_before_connection_access() {
        let request = Request::ReadState {
            module_id: 1,
            name: "x".repeat(65_485),
        };
        assert!(serde_json::to_vec(&request).unwrap().len() <= RPC_BUFFER_SIZE);
        let mut connection = Connection {
            pipe: None,
            next_id: 1,
        };
        assert_eq!(connection.exchange(request), Err(STATUS_INVALID_ARGUMENT));
        assert_eq!(connection.next_id, 1);
    }
}

mod object_layout;
