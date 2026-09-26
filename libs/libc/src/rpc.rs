//! SunRPC compatibility uses the installed libtirpc implementation. Handles,
//! authentication, XDR streams and transport state have one owner; native libc
//! does not maintain an independent RPC service table.
use core::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
type Opaque = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    sec: i64,
    usec: i64,
}

unsafe fn symbol(name: &CStr) -> Opaque {
    use kinakaze_link::process::{kinakaze_process_dlopen, kinakaze_process_dlsym};
    static HANDLE: OnceLock<usize> = OnceLock::new();
    let handle = *HANDLE.get_or_init(|| unsafe {
        kinakaze_process_dlopen(c"libtirpc.so.3".as_ptr(), 2 | 0x1000) as usize
    });
    let address = if handle == 0 {
        core::ptr::null_mut()
    } else {
        unsafe { kinakaze_process_dlsym(handle as Opaque, name.as_ptr()) }
    };
    if address.is_null() {
        // A void XDR initializer cannot report a missing backend safely. Fail
        // explicitly instead of returning an uninitialized stream to its caller.
        unsafe {
            crate::legacy::kinakaze_abi___libc_fatal(
                c"Kinakaze SunRPC requires the installed libtirpc.so.3 backend\n".as_ptr(),
            )
        }
    }
    address
}
macro_rules! forward {
    ($name:ident, $target:literal, ($($arg:ident: $ty:ty),*) -> $result:ty) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $name($($arg: $ty),*) -> $result {
            let address = unsafe { symbol(CStr::from_bytes_with_nul_unchecked(concat!($target, "\0").as_bytes())) };
            let call: unsafe extern "sysv64" fn($($ty),*) -> $result = unsafe { core::mem::transmute(address) };
            unsafe { call($($arg),*) }
        }
    };
}
forward!(kinakaze_abi_xdr_void, "xdr_void", () -> c_int);
forward!(kinakaze_abi_xdr_int, "xdr_int", (stream: Opaque, value: *mut c_int) -> c_int);
forward!(kinakaze_abi_xdr_u_int, "xdr_u_int", (stream: Opaque, value: *mut u32) -> c_int);
forward!(kinakaze_abi_xdr_uint32_t, "xdr_uint32_t", (stream: Opaque, value: *mut u32) -> c_int);
forward!(kinakaze_abi_xdr_u_char, "xdr_u_char", (stream: Opaque, value: *mut u8) -> c_int);
forward!(kinakaze_abi_xdr_bool, "xdr_bool", (stream: Opaque, value: *mut c_int) -> c_int);
forward!(kinakaze_abi_xdr_enum, "xdr_enum", (stream: Opaque, value: *mut c_int) -> c_int);
forward!(kinakaze_abi_xdr_array, "xdr_array", (stream: Opaque, data: *mut Opaque, count: *mut u32, max: u32, size: u32, proc: Opaque) -> c_int);
forward!(kinakaze_abi_xdr_bytes, "xdr_bytes", (stream: Opaque, data: *mut Opaque, count: *mut u32, max: u32) -> c_int);
forward!(kinakaze_abi_xdr_opaque, "xdr_opaque", (stream: Opaque, data: Opaque, size: u32) -> c_int);
forward!(kinakaze_abi_xdr_string, "xdr_string", (stream: Opaque, data: *mut *mut c_char, max: u32) -> c_int);
forward!(kinakaze_abi_xdr_pointer, "xdr_pointer", (stream: Opaque, data: *mut Opaque, size: u32, proc: Opaque) -> c_int);
forward!(kinakaze_abi_xdr_netobj, "xdr_netobj", (stream: Opaque, data: Opaque) -> c_int);
forward!(kinakaze_abi_xdr_sizeof, "xdr_sizeof", (proc: Opaque, data: Opaque) -> u64);
forward!(kinakaze_abi_xdr_free, "xdr_free", (proc: Opaque, data: Opaque) -> ());
forward!(kinakaze_abi_xdrmem_create, "xdrmem_create", (stream: Opaque, data: *mut c_char, size: u32, op: c_int) -> ());
forward!(kinakaze_abi_xdrstdio_create, "xdrstdio_create", (stream: Opaque, file: Opaque, op: c_int) -> ());
forward!(kinakaze_abi_authunix_create_default, "authunix_create_default", () -> Opaque);
forward!(kinakaze_abi_authdes_create, "authdes_create", (name: *const c_char, window: u32, address: Opaque, key: Opaque) -> Opaque);
forward!(kinakaze_abi_authdes_pk_create, "authdes_pk_create", (name: *const c_char, public_key: Opaque, window: u32, address: Opaque, key: Opaque) -> Opaque);
forward!(kinakaze_abi_clnt_create, "clnt_create", (host: *const c_char, program: u64, version: u64, protocol: *const c_char) -> Opaque);
forward!(kinakaze_abi_clnttcp_create, "clnttcp_create", (address: Opaque, program: u64, version: u64, socket: *mut c_int, send: u32, recv: u32) -> Opaque);
forward!(kinakaze_abi_clntudp_create, "clntudp_create", (address: Opaque, program: u64, version: u64, timeout: Timeval, socket: *mut c_int) -> Opaque);
forward!(kinakaze_abi_clnt_pcreateerror, "clnt_pcreateerror", (message: *const c_char) -> ());
forward!(kinakaze_abi_clnt_perror, "clnt_perror", (client: Opaque, message: *const c_char) -> ());
forward!(kinakaze_abi_get_myaddress, "get_myaddress", (address: Opaque) -> ());
forward!(kinakaze_abi_host2netname, "host2netname", (output: *mut c_char, host: *const c_char, domain: *const c_char) -> c_int);
forward!(kinakaze_abi_key_gendes, "key_gendes", (key: Opaque) -> c_int);
forward!(kinakaze_abi_key_secretkey_is_set, "key_secretkey_is_set", () -> c_int);
forward!(kinakaze_abi_svc_register, "svc_register", (transport: Opaque, program: u32, version: u32, dispatch: Opaque, protocol: c_int) -> c_int);
forward!(kinakaze_abi_svc_sendreply, "svc_sendreply", (transport: Opaque, proc: Opaque, data: Opaque) -> c_int);
forward!(kinakaze_abi_svc_getreq_poll, "svc_getreq_poll", (fds: Opaque, count: c_int) -> ());
forward!(kinakaze_abi_svcerr_decode, "svcerr_decode", (transport: Opaque) -> ());
forward!(kinakaze_abi_svcerr_noproc, "svcerr_noproc", (transport: Opaque) -> ());
forward!(kinakaze_abi_svcerr_systemerr, "svcerr_systemerr", (transport: Opaque) -> ());
forward!(kinakaze_abi_svctcp_create, "svctcp_create", (socket: c_int, send: u32, recv: u32) -> Opaque);
forward!(kinakaze_abi_svcudp_bufcreate, "svcudp_bufcreate", (socket: c_int, send: u32, recv: u32) -> Opaque);
forward!(kinakaze_abi_xprt_unregister, "xprt_unregister", (transport: Opaque) -> ());

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___rpc_thread_svc_pollfd() -> *mut Opaque {
    unsafe { symbol(c"svc_pollfd").cast() }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___rpc_thread_svc_max_pollfd() -> *mut c_int {
    unsafe { symbol(c"svc_max_pollfd").cast() }
}

#[repr(C)]
struct Client {
    auth: Opaque,
    ops: *const [Opaque; 6],
    private: Opaque,
}
unsafe fn destroy(client: Opaque) {
    let call: unsafe extern "sysv64" fn(Opaque) =
        unsafe { core::mem::transmute((*(*client.cast::<Client>()).ops)[4]) };
    unsafe {
        call(client);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___libc_clntudp_bufcreate(
    address: Opaque,
    program: u64,
    version: u64,
    timeout: Timeval,
    socket: *mut c_int,
    send: u32,
    recv: u32,
    flags: c_int,
) -> Opaque {
    if socket.is_null() || flags & !(0x80000 | 0x800) != 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return core::ptr::null_mut();
    }
    let created = unsafe { *socket < 0 };
    let call: unsafe extern "sysv64" fn(Opaque, u64, u64, Timeval, *mut c_int, u32, u32) -> Opaque =
        unsafe { core::mem::transmute(symbol(c"clntudp_bufcreate")) };
    let client = unsafe { call(address, program, version, timeout, socket, send, recv) };
    if created && !client.is_null() {
        let result = (|| {
            let fd = unsafe { *socket };
            if flags & 0x80000 != 0 {
                kinakaze_vfs::set_close_on_exec(fd, true)?;
            }
            if flags & 0x800 != 0 {
                let current = unsafe { crate::fdio::kinakaze_abi_fcntl64(fd, 3, 0) };
                if current < 0
                    || unsafe {
                        crate::fdio::kinakaze_abi_fcntl64(fd, 4, (current | 0x800) as usize)
                    } < 0
                {
                    return Err(kinakaze_tls::errno());
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            unsafe {
                destroy(client);
                *socket = -1;
            }
            crate::set_errno(error);
            return core::ptr::null_mut();
        }
    }
    client
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___libc_rpc_getport(
    address: Opaque,
    program: u64,
    version: u64,
    protocol: u32,
    retry_seconds: i64,
    total_seconds: i64,
) -> u16 {
    if address.is_null() || retry_seconds < 0 || total_seconds < 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    }
    // Portmapper v2 GETPORT. Keep the caller's timeout contract rather than
    // substituting libtirpc pmap_getport's fixed retry/total timeouts.
    unsafe {
        address
            .cast::<u8>()
            .add(2)
            .copy_from_nonoverlapping(111u16.to_be_bytes().as_ptr(), 2);
    }
    let mut socket = -1;
    let client = if protocol == 6 {
        unsafe { kinakaze_abi_clnttcp_create(address, 100000, 2, &mut socket, 400, 400) }
    } else {
        unsafe {
            kinakaze_abi___libc_clntudp_bufcreate(
                address,
                100000,
                2,
                Timeval {
                    sec: retry_seconds,
                    usec: 0,
                },
                &mut socket,
                400,
                400,
                0x80000,
            )
        }
    };
    let mut port = 0u16;
    if !client.is_null() {
        let mut parameters = [program, version, protocol as u64, 0];
        let call: unsafe extern "sysv64" fn(
            Opaque,
            u64,
            Opaque,
            Opaque,
            Opaque,
            Opaque,
            Timeval,
        ) -> c_int = unsafe { core::mem::transmute((*(*client.cast::<Client>()).ops)[0]) };
        let status = unsafe {
            call(
                client,
                3,
                symbol(c"xdr_pmap"),
                parameters.as_mut_ptr().cast(),
                symbol(c"xdr_u_short"),
                (&mut port as *mut u16).cast(),
                Timeval {
                    sec: total_seconds,
                    usec: 0,
                },
            )
        };
        if status != 0 || port == 0 {
            // libtirpc uses the RFC's 32-bit rpcprog_t/rpcvers_t. Its create
            // error is 16 bytes, unlike glibc's historical u_long-based layout.
            let current: unsafe extern "sysv64" fn() -> *mut u32 =
                unsafe { core::mem::transmute(symbol(c"__rpc_createerr")) };
            let error = unsafe { current() };
            if !error.is_null() {
                unsafe {
                    if status != 0 {
                        error.write(14); // RPC_PMAPFAILURE
                        let get_error: unsafe extern "sysv64" fn(Opaque, Opaque) =
                            core::mem::transmute((*(*client.cast::<Client>()).ops)[2]);
                        get_error(client, error.add(1).cast());
                    } else {
                        error.write(15);
                    } // RPC_PROGNOTREGISTERED
                }
            }
            port = 0;
        }
        unsafe {
            destroy(client);
        }
    }
    unsafe {
        address.cast::<u8>().add(2).write(0);
        address.cast::<u8>().add(3).write(0);
    }
    port
}
