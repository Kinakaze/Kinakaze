//! CLI supervisor and one managed native worker per Linux process.
use crate::{Result, controller::Controller, failure, process::OwnedProcess};
use kinakaze_v2_abi::*;
use kinakaze_v2_host_win::{Library, StartupSpan, random_token};
use kinakaze_v2_protocol::{Reply, Request, RuntimeOpenConfig};
use std::{
    ffi::OsString,
    mem::{self, MaybeUninit},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

struct Options {
    root: PathBuf,
    dist: PathBuf,
    cwd: String,
    web_address: Option<String>,
    arguments: Vec<String>,
}
fn parse(arguments: Vec<OsString>, pool: bool, prepare: bool) -> Result<Options> {
    let mut args = arguments.into_iter();
    let mut root = None;
    let mut dist = None;
    let mut cwd = "/".to_owned();
    let mut guest = Vec::new();
    let mut web_address = None;
    let mut rootfs_manifest = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--rootfs-manifest") => {
                rootfs_manifest =
                    Some(PathBuf::from(args.next().ok_or("missing rootfs manifest")?));
            }
            Some("--root") => {
                root = Some(PathBuf::from(args.next().ok_or("missing --root value")?))
            }
            Some("--dist") => {
                dist = Some(PathBuf::from(args.next().ok_or("missing --dist value")?))
            }
            Some("--web") => {
                web_address = Some(
                    args.next()
                        .ok_or("missing --web value")?
                        .into_string()
                        .map_err(|_| "web address is not UTF-8")?,
                );
            }
            Some("--cwd") => {
                cwd = args
                    .next()
                    .ok_or("missing --cwd value")?
                    .into_string()
                    .map_err(|_| "cwd is not UTF-8")?
            }
            Some("--") => {
                guest = args
                    .map(|arg| {
                        arg.into_string()
                            .map_err(|_| failure("guest argv must be UTF-8"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                break;
            }
            _ => {
                return Err(failure(
                    "usage: worker run [--root ROOT] [--dist DIST] [--cwd /] [--web 127.0.0.1:PORT] -- /linux/program [args...]",
                ));
            }
        }
    }
    if !(pool && guest.is_empty())
        && !guest
            .first()
            .is_some_and(|program| program.starts_with('/'))
        || !cwd.starts_with('/')
    {
        return Err(failure(
            "an absolute Linux program path and cwd are required",
        ));
    }
    let dist = dist
        .unwrap_or(
            std::env::current_exe()?
                .parent()
                .ok_or("worker has no parent directory")?
                .to_owned(),
        )
        .canonicalize()?;
    let root = root.unwrap_or_else(|| dist.join("rootfs"));
    if prepare {
        kinakaze_v2_rootfs::prepare(&root, &dist, rootfs_manifest.as_deref())?;
    } else if rootfs_manifest.is_some() {
        return Err(failure("rootfs manifest is only accepted by worker run"));
    }
    Ok(Options {
        root: root.canonicalize()?,
        dist,
        cwd,
        web_address,
        arguments: guest,
    })
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

pub fn dispatch(mode: &std::ffi::OsStr, arguments: Vec<OsString>) -> Result<i32> {
    match mode.to_str() {
        Some("run") => supervisor(parse(arguments, false, true)?),
        Some("guest") => guest(parse(arguments, false, false)?, None, false, false),
        Some("guest-prewarm") => guest(parse(arguments, false, false)?, None, true, false),
        Some("guest-pool") => guest(parse(arguments, true, false)?, None, false, true),
        Some("--kinakaze-exec") => {
            let executable = arguments.first().ok_or("exec handoff lacks executable")?;
            let argv = arguments
                .iter()
                .skip(1)
                .map(|arg| {
                    arg.clone()
                        .into_string()
                        .map_err(|_| failure("exec argv must be UTF-8"))
                })
                .collect::<Result<Vec<_>>>()?;
            if argv.is_empty() {
                return Err(failure("exec handoff lacks argv[0]"));
            }
            let options = Options {
                root: PathBuf::from(
                    std::env::var_os("KINAKAZE_V2_ROOT").ok_or("exec root is missing")?,
                ),
                dist: PathBuf::from(
                    std::env::var_os("KINAKAZE_V2_DIST").ok_or("exec distribution is missing")?,
                ),
                cwd: "/".into(),
                web_address: None,
                arguments: argv,
            };
            guest(options, Some(PathBuf::from(executable)), false, false)
        }
        Some("--kinakaze-fork") => {
            // Native engine inspects original command line during bootstrap.
            let options = Options {
                root: PathBuf::from(
                    std::env::var_os("KINAKAZE_V2_ROOT").ok_or("fork root is missing")?,
                ),
                dist: PathBuf::from(
                    std::env::var_os("KINAKAZE_V2_DIST").ok_or("fork distribution is missing")?,
                ),
                cwd: "/".into(),
                web_address: None,
                arguments: vec!["<fork-bootstrap>".into()],
            };
            guest(options, None, false, false)
        }
        _ => Err(failure("invalid native worker launch mode")),
    }
}

fn supervisor(options: Options) -> Result<i32> {
    let _total = StartupSpan::begin("supervisor-total");
    let startup = StartupSpan::begin("manager-start");
    let endpoint = format!(r"\\.\pipe\kinakaze-v2-{}", random_token()?);
    let token = random_token()?;
    let executable = std::env::current_exe()?;
    let mut init_command = Command::new(executable.with_file_name("init.exe"));
    if let Some(address) = &options.web_address {
        init_command.args(["--web", address]);
    }
    init_command
        .args([
            "--pipe",
            &endpoint,
            "--controller-pid",
            &std::process::id().to_string(),
        ])
        .env("KINAKAZE_V2_TOKEN", &token);
    let mut init = OwnedProcess::spawn(&mut init_command, "init")?;
    init.expect_ready()?;
    let mut controller = Controller::connect(&endpoint, token.clone())?;
    drop(startup);
    let spawn = StartupSpan::begin("guest-process-create");
    let mut command = Command::new(executable);
    command
        .arg("guest")
        .arg("--root")
        .arg(&options.root)
        .arg("--dist")
        .arg(&options.dist)
        .arg("--cwd")
        .arg(&options.cwd)
        .arg("--")
        .args(&options.arguments)
        .env("KINAKAZE_V2_ENDPOINT", &endpoint)
        .env("KINAKAZE_V2_TOKEN", token)
        .env("KINAKAZE_V2_ROOT", &options.root)
        .env("KINAKAZE_V2_DIST", &options.dist)
        .env_remove("KINAKAZE_V2_ADOPTION")
        .env_remove("KINAKAZE_V2_ADOPTION_TICKET")
        .creation_flags(kinakaze_v2_host_win::background_creation_flags())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = ChildGuard(command.spawn()?);
    drop(spawn);
    let wait = StartupSpan::begin("guest-process-wait");
    let status = child.0.wait()?;
    drop(wait);
    let _cleanup = StartupSpan::begin("manager-cleanup");
    // This fresh process domain allocates its first worker PID 1. Await the
    // logical process, which can outlive its initial native worker after exec.
    let status = match controller.call(Request::AwaitExit { pid: 1 }) {
        Ok(Reply::Exit { status }) => status,
        Err(_) if !status.success() => status.code().unwrap_or(1),
        Err(error) => return Err(error),
        _ => return Err(failure("invalid process exit reply")),
    };
    controller.call(Request::Shutdown)?;
    init.finish()?;
    Ok(status)
}

fn guest(options: Options, executable: Option<PathBuf>, prewarm: bool, pool: bool) -> Result<i32> {
    let startup = StartupSpan::begin("guest-bootstrap");
    let config = RuntimeOpenConfig {
        endpoint: std::env::var("KINAKAZE_V2_ENDPOINT")?,
        token: std::env::var("KINAKAZE_V2_TOKEN")?,
        adoption_ticket: std::env::var("KINAKAZE_V2_ADOPTION").ok(),
    };
    // A reservation is single-use. Consume it before loading providers or
    // starting guest threads, so later exec/fork children cannot replay it.
    unsafe { std::env::remove_var("KINAKAZE_V2_ADOPTION") };
    let discovery = StartupSpan::begin("worker-discover-modules");
    let runtime = kinakaze_v2_bridge::native::runtime_path(&options.dist.join("rootfs/lib"))?;
    drop(discovery);
    let loading = StartupSpan::begin("worker-open-runtime");
    let library = Library::open(&runtime)?;
    drop(loading);
    // All three operations use the fixed native C ABI and this owning DLL.
    let open: RuntimeOpenV1 =
        unsafe { mem::transmute(library.symbol(c"kinakaze_runtime_open_v1")?) };
    let close: RuntimeCloseV1 =
        unsafe { mem::transmute(library.symbol(c"kinakaze_runtime_close_v1")?) };
    let run: RuntimeGuestRunV1 =
        unsafe { mem::transmute(library.symbol(c"kinakaze_runtime_guest_run_v1")?) };
    let mut api = MaybeUninit::uninit();
    let config = serde_json::to_vec(&config)?;
    let opening = StartupSpan::begin("runtime-session-open");
    let status = unsafe { open(config.as_ptr(), config.len().try_into()?, api.as_mut_ptr()) };
    drop(opening);
    if status != STATUS_OK {
        return Err(failure(format!("runtime session open failed: {status}")));
    }
    let mut api = unsafe { api.assume_init() };
    let executable = executable.unwrap_or_else(|| {
        options.root.join(
            options
                .arguments
                .first()
                .map(String::as_str)
                .unwrap_or("/")
                .trim_start_matches('/'),
        )
    });
    let mut guest_config = serde_json::json!({
        "executable": executable, "arguments": options.arguments,
        "root": options.root, "dist": options.dist, "cwd": options.cwd, "environment": null,
    });
    if prewarm {
        guest_config["prewarm"] = true.into();
    }
    if pool {
        guest_config["pool"] = true.into();
    }
    let config = serde_json::to_vec(&guest_config)?;
    drop(startup);
    let status = unsafe { run(&api, config.as_ptr(), config.len().try_into()?) };
    // A successful native guest usually calls exit and never returns here.
    let closed = unsafe { close(&mut api) };
    if closed != STATUS_OK {
        return Err(failure(format!("runtime close failed: {closed}")));
    }
    Ok(if status < 0 { 125 } else { status })
}
