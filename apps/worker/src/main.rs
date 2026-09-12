//! Reproducible native-process proof of the V2 module/state-management chain.
//! This is a fork-state transaction, not an implementation of Linux fork().

mod controller;
mod helper;
mod launch;
mod process;
mod session;

use controller::Controller;
use kinakaze_v2_abi::{FORK_COPY, FORK_RESET, FORK_SHARE};
use kinakaze_v2_host_win::random_token;
use kinakaze_v2_protocol::{ProcessIdentity, Reply, Request, RuntimeOpenConfig};
use process::OwnedProcess;
use session::Session;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn failure(message: impl Into<String>) -> Box<dyn std::error::Error> {
    io::Error::other(message.into()).into()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("worker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .ok_or_else(|| failure("usage: worker smoke [--dist DIST]"))?;
    if matches!(
        mode.to_str(),
        Some("--unix-rights-keeper" | "--usernet-broker")
    ) {
        if args.next().is_some() {
            return Err(failure("internal helper takes no arguments"));
        }
        return helper::run(&mode);
    }
    if matches!(
        mode.to_str(),
        Some(
            "run"
                | "guest"
                | "guest-prewarm"
                | "guest-pool"
                | "--kinakaze-exec"
                | "--kinakaze-fork"
        )
    ) {
        let status = launch::dispatch(&mode, args.collect())?;
        std::process::exit(status);
    }
    if mode == "--help" || mode == "-h" {
        println!(
            "Usage: worker run [--root ROOT] [--dist DIST] [--cwd /] [--web 127.0.0.1:PORT] -- /linux/program [args...]\n       worker smoke [--dist DIST]\nInternal parent/child modes use the inherited V2 session environment."
        );
        return Ok(());
    }
    let mut dist = std::env::current_exe()?
        .parent()
        .ok_or("worker has no parent directory")?
        .to_owned();
    while let Some(arg) = args.next() {
        if arg != "--dist" {
            return Err(failure("unknown argument; expected --dist DIST"));
        }
        dist = args
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| failure("--dist needs a directory"))?;
    }
    let dist = dist.canonicalize()?;
    match mode.to_str() {
        Some("smoke") => smoke(&dist),
        Some("parent") => parent(&dist),
        Some("child") => child(&dist),
        _ => Err(failure("unknown mode; use smoke [--dist DIST]")),
    }
}

fn config() -> Result<RuntimeOpenConfig> {
    Ok(RuntimeOpenConfig {
        endpoint: required_env("KINAKAZE_V2_ENDPOINT")?,
        token: required_env("KINAKAZE_V2_TOKEN")?,
        adoption_ticket: std::env::var("KINAKAZE_V2_ADOPTION").ok(),
    })
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| failure(format!("missing required environment variable {name}")))
}

fn worker_command(mode: &str, dist: &Path) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg(mode).arg("--dist").arg(dist);
    Ok(command)
}

fn smoke(dist: &Path) -> Result<()> {
    // The coordinator is host infrastructure and never opens a Worker session.
    let endpoint = format!(r"\\.\pipe\kinakaze-v2-{}", random_token()?);
    let token = random_token()?;
    let init_exe = std::env::current_exe()?.with_file_name("init.exe");
    let mut init_command = Command::new(init_exe);
    init_command
        .arg("--pipe")
        .arg(&endpoint)
        .arg("--controller-pid")
        .arg(std::process::id().to_string())
        .env("KINAKAZE_V2_TOKEN", &token);
    let mut init = OwnedProcess::spawn(&mut init_command, "init")?;
    init.expect_ready()?;
    let mut controller = Controller::connect(&endpoint, token.clone())?;

    let mut parent_command = worker_command("parent", dist)?;
    parent_command
        .env("KINAKAZE_V2_ENDPOINT", &endpoint)
        .env("KINAKAZE_V2_TOKEN", &token)
        .env_remove("KINAKAZE_V2_ADOPTION")
        .env_remove("KINAKAZE_V2_EXPECTED_IDENTITY");
    let mut parent = OwnedProcess::spawn(&mut parent_command, "parent worker")?;
    parent.finish()?;

    // Native process monitors race with receiving Child::wait completion. This
    // bounded retry belongs only to the test coordinator, never the runtime.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stats = match controller.call(Request::Stats)? {
            Reply::Stats(stats) => stats,
            _ => return Err(failure("controller received invalid Stats reply")),
        };
        if stats.processes == 0 && stats.objects == 0 && stats.transactions == 0 {
            println!("Native exit cleanup: processes=0, objects=0, transactions=0");
            break;
        }
        if Instant::now() >= deadline {
            return Err(failure(format!("native exit cleanup timed out: {stats:?}")));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    expect_ok(controller.call(Request::Shutdown)?, "shutdown")?;
    init.finish()?;
    println!(
        "PASS: native module binding, shared runtime RPC, two native workers, fork-state Copy/Share/Reset and cleanup. Linux guest-memory fork is not implemented by this smoke."
    );
    Ok(())
}

fn parent(dist: &Path) -> Result<()> {
    let session = Session::load(dist, config()?)?;
    let identity = session.check_guest_exports()?;
    session.libc.define("copy", 10, FORK_COPY)?;
    session.libc.write("copy", 11)?;
    session.libc.define("share", 20, FORK_SHARE)?;
    session.libc.write("share", 21)?;
    session.libc.define("reset", 30, FORK_RESET)?;
    session.libc.write("reset", 31)?;
    // Same state name in a separate DLL must resolve to its own module scope.
    session.libpthread.define("copy", 40, FORK_COPY)?;
    session.libpthread.write("copy", 41)?;
    let ticket = match session
        .runtime
        .call(Request::PrepareFork { request_key: 1 })?
    {
        Reply::ForkPrepared(ticket) => ticket,
        _ => return Err(failure("prepare returned invalid fork ticket reply")),
    };
    let result = (|| {
        // Replay the transaction key before native launch: state and the child
        // identity must be unchanged and no duplicate child may be reserved.
        match session
            .runtime
            .call(Request::PrepareFork { request_key: 1 })?
        {
            Reply::ForkPrepared(replayed) if replayed == ticket => {}
            _ => return Err(failure("PrepareFork replay was not idempotent")),
        }
        if ticket.child.parent_pid != identity.pid || ticket.child.pid == identity.pid {
            return Err(failure("fork reserved an invalid logical child identity"));
        }
        let mut child_command = worker_command("child", dist)?;
        child_command
            .env("KINAKAZE_V2_ADOPTION", &ticket.token)
            .env(
                "KINAKAZE_V2_EXPECTED_IDENTITY",
                serde_json::to_string(&ticket.child)?,
            );
        let mut child = OwnedProcess::spawn(&mut child_command, "child worker")?;
        child.expect_ready()?;
        expect_ok(
            session.runtime.call(Request::CommitFork {
                transaction: ticket.transaction,
            })?,
            "fork commit",
        )?;
        expect_ok(
            session.runtime.call(Request::CommitFork {
                transaction: ticket.transaction,
            })?,
            "fork commit replay",
        )?;
        child.finish()?;
        session.libc.expect("copy", 11)?;
        session.libc.expect("share", 121)?;
        session.libc.expect("reset", 31)?;
        session.libpthread.expect("copy", 41)?;
        println!(
            "Parent worker: host_pid={}, linux_pid={}, libc Copy=11 Share=121 Reset=31; libpthread Copy=41",
            std::process::id(),
            identity.pid
        );
        Ok(())
    })();
    if result.is_err() {
        let _ = session.runtime.call(Request::AbortFork {
            transaction: ticket.transaction,
        });
    }
    result
}

fn child(dist: &Path) -> Result<()> {
    let config = config()?;
    if config.adoption_ticket.is_none() {
        return Err(failure("child requires an adoption ticket"));
    }
    let expected: ProcessIdentity =
        serde_json::from_str(&required_env("KINAKAZE_V2_EXPECTED_IDENTITY")?)?;
    let session = Session::load(dist, config)?;
    if session.runtime.identity()? != expected {
        return Err(failure("adopted child identity mismatch"));
    }
    expect_ok(
        session.runtime.call(Request::MarkReady)?,
        "mark child ready",
    )?;
    println!("READY");
    io::stdout().flush()?;
    // Init waits on a state-change condition variable until the parent commits.
    expect_ok(
        session.runtime.call(Request::AwaitActivation)?,
        "await child activation",
    )?;
    let identity = session.check_guest_exports()?;
    if identity != expected {
        return Err(failure("activated child identity changed"));
    }
    session.libc.expect("copy", 11)?;
    session.libc.expect("share", 21)?;
    session.libc.expect("reset", 30)?;
    session.libpthread.expect("copy", 41)?;
    session.libc.write("copy", 111)?;
    session.libc.write("share", 121)?;
    session.libc.write("reset", 130)?;
    session.libpthread.write("copy", 141)?;
    println!(
        "Child worker: host_pid={}, linux_pid={}, parent_linux_pid={}; inherited libc 11/21/30 and libpthread 41; private/shared writes verified by parent",
        std::process::id(),
        identity.pid,
        identity.parent_pid
    );
    Ok(())
}

fn expect_ok(reply: Reply, operation: &str) -> Result<()> {
    match reply {
        Reply::Ok => Ok(()),
        _ => Err(failure(format!("{operation} returned an unexpected reply"))),
    }
}
