//! Engine session and init process authority. Libraries own their implementation state.
use super::*;
use guest_process::authority::{ForkReservation, Identity, ProcessAuthority};
use kinakaze_guest_engine::GuestConfig;
use kinakaze_v2_host_win::{ProcessHandle, random_token};
use kinakaze_v2_protocol::ErrorCode;
use serde::Deserialize;
use std::path::PathBuf;

pub(super) fn bind_session() {
    // Seed the engine's immutable logical PID before an ordinary worker calls
    // any facade. Both smoke workers and ELF workers use this same authority.
    guest_process::authority::required_identity();
}

static AUTHORITY: ProcessAuthority = ProcessAuthority {
    identity,
    prepare_fork,
    adopt_fork,
    mark_ready,
    await_activation,
    commit_fork,
    abort_fork,
    prepare_exec,
    commit_exec,
    abort_exec,
    memory_target,
};

pub(super) fn bootstrap() -> Result<(), i32> {
    guest_process::authority::install(&AUTHORITY).map_err(|_| STATUS_INTERNAL)?;
    kinakaze_v2_loader::install(resolve_child_api).map_err(|_| STATUS_INTERNAL)?;
    // Explicitly outside DllMain/Windows loader lock. A fork child resumes the
    // copied parent continuation here and does not open a spurious fresh PID.
    guest_process::bootstrap_fork_child();
    Ok(())
}

fn request(request: Request) -> Result<Reply, i32> {
    let pointer = ACTIVE_SESSION.load(Ordering::Acquire);
    // SAFETY: published live session is retained until native worker teardown.
    let session = unsafe { resolve_session(pointer) }.map_err(|_| 5)?;
    let response = session
        .connection
        .lock()
        .map_err(|_| 5)?
        .exchange(request)
        .map_err(|_| 5)?;
    response.result.map_err(|error| match error.code {
        ErrorCode::Unauthorized => 1,
        ErrorCode::NotFound => 3,
        ErrorCode::NotReady => 11,
        ErrorCode::LimitExceeded => 11,
        ErrorCode::Conflict => 16,
        ErrorCode::InvalidRequest => 22,
        ErrorCode::Aborted => 125,
        _ => 5,
    })
}

fn ok(request_value: Request) -> Result<(), i32> {
    match request(request_value)? {
        Reply::Ok => Ok(()),
        _ => Err(5),
    }
}
fn request_key() -> Result<u64, i32> {
    let token = random_token().map_err(|_| 5)?;
    u64::from_str_radix(&token[..16], 16).map_err(|_| 5)
}
fn identity() -> Result<Identity, i32> {
    match request(Request::Identity)? {
        Reply::Identity(id) => Ok(Identity {
            epoch: id.epoch,
            pid: id.pid,
            parent_pid: id.parent_pid,
        }),
        _ => Err(5),
    }
}
fn prepare_fork(parent_pid: Option<u32>) -> Result<ForkReservation, i32> {
    let request_key = request_key()?;
    let req = match parent_pid {
        Some(parent_pid) => Request::PrepareForkWithParent {
            request_key,
            parent_pid,
        },
        None => Request::PrepareFork { request_key },
    };
    match request(req)? {
        Reply::ForkPrepared(ticket) => {
            let token: [u8; 64] = ticket.token.as_bytes().try_into().map_err(|_| 5)?;
            Ok(ForkReservation {
                transaction: ticket.transaction,
                token,
                child: Identity {
                    epoch: ticket.child.epoch,
                    pid: ticket.child.pid,
                    parent_pid: ticket.child.parent_pid,
                },
            })
        }
        _ => Err(5),
    }
}
fn adopt_fork(reservation: &ForkReservation) -> Result<(), i32> {
    // Credentials remain host-only; the guest environment does not contain them.
    let config = RuntimeOpenConfig {
        endpoint: std::env::var("KINAKAZE_V2_ENDPOINT").map_err(|_| 5)?,
        token: std::env::var("KINAKAZE_V2_TOKEN").map_err(|_| 5)?,
        adoption_ticket: Some(
            std::str::from_utf8(&reservation.token)
                .map_err(|_| 5)?
                .to_owned(),
        ),
    };
    open_and_publish(config).map_err(|_| 5)?;
    bind_session();
    Ok(())
}
fn mark_ready() -> Result<(), i32> {
    ok(Request::MarkReady)
}
fn await_activation() -> Result<(), i32> {
    ok(Request::AwaitActivation)
}
fn commit_fork(transaction: u64) -> Result<(), i32> {
    ok(Request::AwaitForkReady { transaction })?;
    ok(Request::CommitFork { transaction })
}
fn abort_fork(transaction: u64) -> Result<(), i32> {
    ok(Request::AbortFork { transaction })
}
fn prepare_exec(pid: u32) -> Result<u64, i32> {
    let process = ProcessHandle::open(pid).map_err(|_| 3)?;
    match request(Request::PrepareExec {
        request_key: request_key()?,
        replacement_host_pid: pid,
        replacement_birth: process.birth(),
    })? {
        Reply::ExecPrepared { transaction } => Ok(transaction),
        _ => Err(5),
    }
}
fn commit_exec(transaction: u64) -> Result<(), i32> {
    ok(Request::AwaitExecReady { transaction })?;
    ok(Request::CommitExec { transaction })
}
fn abort_exec(transaction: u64) -> Result<(), i32> {
    ok(Request::AbortExec { transaction })
}
fn memory_target(pid: u32, write: bool) -> Result<(u32, u64), i32> {
    match request(Request::ProcessMemoryTarget { pid, write })? {
        Reply::ProcessMemoryTarget { host_pid, birth } => Ok((host_pid, birth)),
        _ => Err(5),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    executable: PathBuf,
    arguments: Vec<String>,
    root: PathBuf,
    cwd: String,
    dist: PathBuf,
    environment: Option<Vec<String>>,
    #[serde(default)]
    prewarm: bool,
    #[serde(default)]
    pool: bool,
}

/// # Safety
/// The live session table and JSON bytes must remain readable for this call.
/// No other guest may execute in this worker. Guest exit ends the worker.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_runtime_guest_run_v1(
    api: *const RuntimeApiV1,
    config_json: *const u8,
    config_len: u32,
) -> i32 {
    match catch_unwind(AssertUnwindSafe(
        || -> Result<i32, Box<dyn std::error::Error>> {
            if !aligned(api)
                || config_json.is_null()
                || config_len == 0
                || config_len as usize > RPC_BUFFER_SIZE
            {
                return Err("invalid guest launch configuration".into());
            }
            let api = unsafe { &*api };
            if api.abi_version != ABI_VERSION
                || !aligned(api.context.cast::<Session>())
                || !ptr::fn_addr_eq(api.call, runtime_call as RuntimeCallV1)
            {
                return Err("guest launch requires a live owning runtime table".into());
            }
            let mut config: Configuration = serde_json::from_slice(unsafe {
                slice::from_raw_parts(config_json, config_len as usize)
            })?;
            if ACTIVE_SESSION.load(Ordering::Acquire) != api.context.cast() {
                return Err("guest launch session does not own this native worker".into());
            }
            let registry = kinakaze_v2_loader::load(&config.dist, api)?;
            if config.pool {
                if config.prewarm {
                    return Err("conflicting prewarm modes".into());
                }
                let priority: u32 = std::env::var("KINAKAZE_V2_POOL_PRIORITY")?.parse()?;
                kinakaze_v2_host_win::set_current_process_priority(priority)?;
                ok(Request::MarkPoolReady)
                    .map_err(|errno| format!("pool readiness failed: {errno}"))?;
                let waiting = kinakaze_v2_host_win::StartupSpan::begin("init-pool-wait");
                let Reply::PoolLaunch(launch) = request(Request::AwaitPoolActivation)
                    .map_err(|errno| format!("pool activation failed: {errno}"))?
                else {
                    return Err("pool activation returned no launch".into());
                };
                drop(waiting);
                if !launch.valid() {
                    return Err("invalid pool launch".into());
                }
                config.executable = config
                    .root
                    .join(launch.arguments[0].trim_start_matches('/'));
                config.arguments = launch.arguments;
                config.cwd = launch.cwd;
                config.environment = launch.environment;
            }
            if config.prewarm {
                // Init owns both barriers. No ELF has been mapped, no IFUNC or
                // application initializer has run, and this worker is used once.
                ok(Request::MarkPrewarmReady)
                    .map_err(|errno| format!("prewarm readiness failed: {errno}"))?;
                let _waiting = kinakaze_v2_host_win::StartupSpan::begin("init-prewarm-wait");
                ok(Request::AwaitPrewarmActivation)
                    .map_err(|errno| format!("prewarm activation failed: {errno}"))?;
            }
            Ok(kinakaze_guest_engine::run(GuestConfig {
                executable: config.executable,
                arguments: config.arguments,
                root: config.root,
                cwd: config.cwd,
                environment: config.environment,
                providers: registry,
            }))
        },
    )) {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            eprintln!("guest launch: {error}");
            STATUS_INTERNAL
        }
        Err(_) => STATUS_INTERNAL,
    }
}

unsafe extern "C" fn resolve_child_api(output: *mut RuntimeApiV1) -> i32 {
    if output.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let pointer = ACTIVE_SESSION.load(Ordering::Acquire);
    let session = match unsafe { resolve_session(pointer) } {
        Ok(session) => session,
        Err(_) => return STATUS_NOT_INITIALIZED,
    };
    // SAFETY: the loader supplies a writable table, after child adoption.
    unsafe { output.write(session_api(ptr::from_ref(session).cast_mut())) };
    STATUS_OK
}
