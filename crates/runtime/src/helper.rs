//! Helpers share init's object domain and Job lifetime, never a guest PID.
use super::*;

/// # Safety
/// Configuration must be readable for `config_len` bytes for this call.
/// This entry is only valid in a fresh native helper process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_runtime_helper_run_v1(
    kind: u32,
    config_json: *const u8,
    config_len: u32,
) -> i32 {
    guard(|| {
        if !matches!(kind, HELPER_UNIX_RIGHTS | HELPER_USERNET)
            || config_json.is_null()
            || config_len == 0
            || config_len as usize > RPC_BUFFER_SIZE
            || !ACTIVE_SESSION.load(Ordering::Acquire).is_null()
        {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let config: RuntimeOpenConfig = serde_json::from_slice(unsafe {
            slice::from_raw_parts(config_json, config_len as usize)
        })
        .map_err(|_| STATUS_INVALID_ARGUMENT)?;
        if config.adoption_ticket.is_some() {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let mut connection = Connection {
            pipe: Some(PipeConnection::connect(&config.endpoint).map_err(|_| STATUS_TRANSPORT)?),
            next_id: 1,
        };
        let epoch = match connection
            .exchange(Request::Hello(Hello {
                version: PROTOCOL_VERSION,
                token: config.token,
                role: ClientRole::Helper,
                adoption_ticket: None,
            }))?
            .result
        {
            Ok(Reply::Hello {
                epoch,
                process: None,
            }) if epoch != 0 => epoch,
            _ => return Err(STATUS_REMOTE),
        };
        guest_process::authority::install_helper_domain(epoch).map_err(|_| STATUS_INTERNAL)?;
        // Keep the authenticated connection alive until all retained handles
        // have been released. Init's Job also covers abnormal helper exits.
        let result = match kind {
            HELPER_UNIX_RIGHTS => kinakaze_guest_engine::helpers::unix_rights(),
            HELPER_USERNET => kinakaze_guest_engine::helpers::usernet(),
            _ => unreachable!(),
        };
        drop(connection);
        result.map_err(|errno| -errno)
    })
}
