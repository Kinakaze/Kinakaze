//! Load the native runtime API and exercise module state through init RPC.

use crate::{Result, failure};
use kinakaze_v2_abi::*;
use kinakaze_v2_bridge::{Module, ModuleImage, ModuleSet};
use kinakaze_v2_host_win::Library;
use kinakaze_v2_protocol::{
    ForkPolicy, ProcessIdentity, Reply, Request, RpcError, RuntimeOpenConfig,
};
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::path::Path;
use std::sync::Arc;

pub struct Runtime {
    api: RuntimeApiV1,
    close: RuntimeCloseV1,
    // These outlive close(), including when the session is only partly loaded.
    _image: ModuleImage,
    _library: Arc<Library>,
}

impl Runtime {
    fn open(dist: &Path, module: &Module, config: RuntimeOpenConfig) -> Result<Self> {
        let image = ModuleImage::load(dist, module)?;
        let library = image.library();
        // SAFETY: These fixed C and SysV entry points belong to the retained DLL.
        let (open, close, abi_version): (
            RuntimeOpenV1,
            RuntimeCloseV1,
            unsafe extern "sysv64" fn() -> u32,
        ) = unsafe {
            (
                std::mem::transmute::<*mut c_void, RuntimeOpenV1>(
                    library.symbol(c"kinakaze_runtime_open_v1")?,
                ),
                std::mem::transmute::<*mut c_void, RuntimeCloseV1>(
                    library.symbol(c"kinakaze_runtime_close_v1")?,
                ),
                std::mem::transmute::<*mut c_void, unsafe extern "sysv64" fn() -> u32>(
                    image.symbol("kinakaze_runtime_abi_version")?,
                ),
            )
        };
        if unsafe { abi_version() } != ABI_VERSION {
            return Err(failure("runtime ABI mismatch"));
        }
        let config = serde_json::to_vec(&config)?;
        let mut api = MaybeUninit::uninit();
        // SAFETY: Nonoverlapping configuration bytes and writable output table.
        check_status("runtime open", unsafe {
            open(config.as_ptr(), config.len().try_into()?, api.as_mut_ptr())
        })?;
        // SAFETY: Successful open initialized the table.
        let api = unsafe { api.assume_init() };
        Ok(Self {
            api,
            close,
            _image: image,
            _library: library,
        })
    }

    pub fn call(&self, request: Request) -> Result<Reply> {
        call_api(&self.api, request)
    }

    pub fn identity(&self) -> Result<ProcessIdentity> {
        match self.call(Request::Identity)? {
            Reply::Identity(identity) => Ok(identity),
            _ => Err(failure("runtime returned invalid identity reply")),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: Session providers are declared before runtime, so Rust drops
        // all providers and their thunks before dropping this owning table.
        let status = unsafe { (self.close)(&mut self.api) };
        if status != STATUS_OK {
            eprintln!("runtime close failed with ABI status {status}");
        }
    }
}

fn call_api(api: &RuntimeApiV1, request: Request) -> Result<Reply> {
    let request = serde_json::to_vec(&request)?;
    let mut response = vec![0_u8; RPC_BUFFER_SIZE];
    let mut response_len = 0_u32;
    // SAFETY: This live table owns its context; buffers are valid and
    // disjoint, and every call completes before Self is dropped.
    check_status("runtime call", unsafe {
        (api.call)(
            api.context,
            request.as_ptr(),
            request.len().try_into()?,
            response.as_mut_ptr(),
            response.len() as u32,
            &mut response_len,
        )
    })?;
    let length = response_len as usize;
    if length == 0 || length > response.len() {
        return Err(failure("invalid runtime response size"));
    }
    let result: std::result::Result<Reply, RpcError> = serde_json::from_slice(&response[..length])?;
    Ok(result?)
}

pub struct Provider {
    image: ModuleImage,
    api: RuntimeApiV1,
    module_id: u32,
}

impl Provider {
    fn load(dist: &Path, module: &Module, runtime: &Runtime) -> Result<Self> {
        let image = ModuleImage::load(dist, module)?;
        match runtime.call(Request::RegisterModule {
            module_id: module.id,
            schema: 1,
        })? {
            Reply::Ok => {}
            _ => return Err(failure("invalid module registration response")),
        }
        Ok(Self {
            image,
            api: runtime.api,
            module_id: module.id,
        })
    }

    pub fn define(&self, name: &str, initial: u64, fork: u32) -> Result<()> {
        let fork = match fork {
            FORK_COPY => ForkPolicy::Copy,
            FORK_SHARE => ForkPolicy::Share,
            FORK_RESET => ForkPolicy::Reset,
            _ => return Err(failure("invalid module fork policy")),
        };
        match call_api(
            &self.api,
            Request::DefineState {
                module_id: self.module_id,
                name: name.into(),
                initial,
                fork,
            },
        )? {
            Reply::State { .. } => Ok(()),
            _ => Err(failure("invalid define-state response")),
        }
    }

    pub fn read(&self, name: &str) -> Result<u64> {
        match call_api(
            &self.api,
            Request::ReadState {
                module_id: self.module_id,
                name: name.into(),
            },
        )? {
            Reply::State { value, .. } => Ok(value),
            _ => Err(failure("invalid read-state response")),
        }
    }

    pub fn write(&self, name: &str, value: u64) -> Result<()> {
        match call_api(
            &self.api,
            Request::WriteState {
                module_id: self.module_id,
                name: name.into(),
                value,
            },
        )? {
            Reply::Ok => Ok(()),
            _ => Err(failure("invalid write-state response")),
        }
    }

    pub fn expect(&self, name: &str, expected: u64) -> Result<()> {
        let actual = self.read(name)?;
        if actual != expected {
            return Err(failure(format!(
                "state {name}: expected {expected}, received {actual}"
            )));
        }
        Ok(())
    }
}

pub struct Session {
    // Rust drops struct fields in declaration order. Do not reorder runtime
    // ahead of the providers: their borrowed API tables must be retired first.
    pub libc: Provider,
    pub libpthread: Provider,
    libm: ModuleImage,
    pub runtime: Runtime,
}

impl Session {
    pub fn load(dist: &Path, config: RuntimeOpenConfig) -> Result<Self> {
        let modules = ModuleSet::discover(&dist.join("rootfs/lib"))?;
        let module = |name: &str| {
            modules
                .modules
                .iter()
                .find(|module| module.soname == name)
                .ok_or_else(|| failure(format!("required module {name} absent from distribution")))
        };
        let runtime = Runtime::open(
            dist,
            modules
                .modules
                .iter()
                .find(|module| module.id == 0)
                .ok_or("runtime missing")?,
            config,
        )?;
        let libc = Provider::load(dist, module("libc.so.6")?, &runtime)?;
        let libm = ModuleImage::load(dist, module("libm.so.6")?)?;
        let libpthread = Provider::load(dist, module("libpthread.so.0")?, &runtime)?;
        Ok(Self {
            libc,
            libpthread,
            libm,
            runtime,
        })
    }

    pub fn check_guest_exports(&self) -> Result<ProcessIdentity> {
        let identity = self.runtime.identity()?;
        // Process identity is supplied by init. PID namespaces and guest TLS
        // are initialized when executing ELF, not by this management harness.
        let strlen: unsafe extern "sysv64" fn(*const u8) -> usize =
            unsafe { std::mem::transmute(self.libc.image.symbol("strlen")?) };
        let (fabs, copysign): (
            unsafe extern "sysv64" fn(f64) -> f64,
            unsafe extern "sysv64" fn(f64, f64) -> f64,
        ) = unsafe {
            (
                std::mem::transmute::<*mut c_void, unsafe extern "sysv64" fn(f64) -> f64>(
                    self.libm.symbol("fabs")?,
                ),
                std::mem::transmute::<*mut c_void, unsafe extern "sysv64" fn(f64, f64) -> f64>(
                    self.libm.symbol("copysign")?,
                ),
            )
        };
        // SAFETY: Function signatures match exports; the string is NUL terminated.
        if unsafe { strlen(c"Kinakaze V2".as_ptr().cast()) } != 11 {
            return Err(failure("native libc strlen returned an incorrect length"));
        }
        let negative_nan = f64::from_bits(0xfff8_0000_0000_0042);
        if unsafe { fabs(-0.0) }.to_bits() != 0
            || unsafe { fabs(negative_nan) }.to_bits() != 0x7ff8_0000_0000_0042
            || unsafe { copysign(2.5, -0.0) }.to_bits() != (-2.5_f64).to_bits()
            || unsafe { copysign(0.0, -1.0) }.to_bits() != (-0.0_f64).to_bits()
        {
            return Err(failure(
                "libm native export returned incorrect floating-point bits",
            ));
        }
        Ok(identity)
    }
}

fn check_status(operation: &str, status: i32) -> Result<()> {
    if status == STATUS_OK {
        Ok(())
    } else {
        Err(failure(format!(
            "{operation} failed with ABI status {status}"
        )))
    }
}
