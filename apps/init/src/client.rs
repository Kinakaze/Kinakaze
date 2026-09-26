//! Reconnectable launch client; no native worker credentials leave init.
use kinakaze_v2_host_win::{PipeConnection, ProcessHandle, private_file};
use kinakaze_v2_protocol::{
    ClientRole, Hello, PROTOCOL_VERSION, PoolLaunch, Reply, Request, WireRequest, WireResponse,
    read_frame, write_frame,
};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Descriptor {
    pub schema: u32,
    pub endpoint: String,
    pub token: String,
    pub host_pid: u32,
    pub birth: u64,
}

pub(super) struct SessionFile {
    path: PathBuf,
    file: Option<fs::File>,
}

impl SessionFile {
    pub fn create(path: PathBuf, endpoint: &str, token: &str) -> Result<Self> {
        let process = ProcessHandle::open(std::process::id())?;
        let mut owned = Self {
            file: Some(private_file(&path)?),
            path,
        };
        let descriptor = Descriptor {
            schema: 1,
            endpoint: endpoint.into(),
            token: token.into(),
            host_pid: process.pid(),
            birth: process.birth(),
        };
        let file = owned.file.as_mut().unwrap();
        file.write_all(&serde_json::to_vec(&descriptor)?)?;
        file.sync_all()?;
        Ok(owned)
    }
}

impl Drop for SessionFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

struct Client {
    pipe: PipeConnection,
    sequence: u64,
}

impl Client {
    fn call(&mut self, request: Request) -> Result<Reply> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("RPC sequence exhausted")?;
        write_frame(
            &mut self.pipe,
            &WireRequest {
                id: self.sequence,
                request,
            },
        )?;
        let reply: WireResponse = read_frame(&mut self.pipe)?;
        if reply.id != self.sequence {
            return Err("RPC response identity mismatch".into());
        }
        Ok(reply.result?)
    }
}

pub(super) fn launch() -> Result<i32> {
    let mut args = std::env::args().skip(2);
    let mut path = None;
    let mut parent_pid = 0;
    let mut wait = false;
    let mut launch = PoolLaunch {
        arguments: Vec::new(),
        cwd: "/".into(),
        environment: None,
    };
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--session-file" => path = Some(PathBuf::from(args.next().ok_or("missing session file")?)),
            "--parent" => parent_pid = args.next().ok_or("missing parent PID")?.parse()?,
            "--cwd" => launch.cwd = args.next().ok_or("missing cwd")?,
            "--env" => {
                let value = args.next().ok_or("missing environment assignment")?;
                if value.split_once('=').is_none_or(|(name, _)| name.is_empty()) {
                    return Err("--env requires NAME=VALUE".into());
                }
                launch.environment.get_or_insert_with(Vec::new).push(value);
            }
            "--wait" => wait = true,
            "--" => { launch.arguments.extend(args); break; }
            _ => return Err("use init launch --session-file FILE [--parent PID] [--wait] [--cwd /] [--env NAME=VALUE] -- /program [args...]".into()),
        }
    }
    if !launch.valid() {
        return Err("launch requires an absolute Linux program and cwd".into());
    }
    let bytes = fs::read(path.ok_or("missing --session-file")?)?;
    if bytes.len() > 4096 {
        return Err("invalid session descriptor size".into());
    }
    let descriptor: Descriptor = serde_json::from_slice(&bytes)?;
    if descriptor.schema != 1 {
        return Err("unsupported session descriptor schema".into());
    }
    let process = ProcessHandle::open(descriptor.host_pid)?;
    if process.birth() != descriptor.birth {
        return Err("stale init session descriptor".into());
    }
    let pipe = PipeConnection::connect(&descriptor.endpoint)?;
    if pipe.peer_pid()? != process.pid() {
        return Err("session endpoint belongs to another process".into());
    }
    let mut client = Client { pipe, sequence: 0 };
    match client.call(Request::Hello(Hello {
        version: PROTOCOL_VERSION,
        token: descriptor.token,
        role: ClientRole::Launcher,
        adoption_ticket: None,
    }))? {
        Reply::Hello { process: None, .. } => {}
        _ => return Err("invalid launch handshake".into()),
    }
    let Reply::PoolWorker { pid, .. } = client.call(Request::ReservePoolWorker)? else {
        return Err("invalid pool reservation".into());
    };
    let result = client.call(Request::ActivatePoolWorkerUnderParent {
        pid,
        parent_pid,
        launch,
    });
    if result.is_err() {
        let _ = client.call(Request::ReleasePoolWorker { pid });
    }
    if result? != Reply::Ok {
        return Err("invalid launch response".into());
    }
    println!("{pid}");
    if wait {
        let Reply::Exit { status } = client.call(Request::AwaitExit { pid })? else {
            return Err("invalid process exit response".into());
        };
        Ok(status)
    } else {
        Ok(0)
    }
}
