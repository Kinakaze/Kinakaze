//! Internal workers enter the owning runtime through its versioned C ABI.
use crate::{Result, failure};
use kinakaze_v2_abi::{HELPER_UNIX_RIGHTS, HELPER_USERNET, RuntimeHelperRunV1, STATUS_OK};
use kinakaze_v2_host_win::Library;
use kinakaze_v2_protocol::RuntimeOpenConfig;
use std::{ffi::OsStr, path::PathBuf};

pub fn run(mode: &OsStr) -> Result<()> {
    let kind = match mode.to_str() {
        Some("--unix-rights-keeper") => HELPER_UNIX_RIGHTS,
        Some("--usernet-broker") => HELPER_USERNET,
        _ => return Err(failure("unknown native helper")),
    };
    let dist =
        PathBuf::from(std::env::var_os("KINAKAZE_V2_DIST").ok_or("helper distribution missing")?);
    let config = serde_json::to_vec(&RuntimeOpenConfig {
        endpoint: std::env::var("KINAKAZE_V2_ENDPOINT")?,
        token: std::env::var("KINAKAZE_V2_TOKEN")?,
        adoption_ticket: None,
    })?;
    let runtime = kinakaze_v2_bridge::native::runtime_path(&dist.join("rootfs/lib"))?;
    let library = Library::open(&runtime)?;
    let run: RuntimeHelperRunV1 =
        unsafe { std::mem::transmute(library.symbol(c"kinakaze_runtime_helper_run_v1")?) };
    let status = unsafe { run(kind, config.as_ptr(), config.len().try_into()?) };
    if status != STATUS_OK {
        return Err(failure(format!("native helper failed: {status}")));
    }
    Ok(())
}
