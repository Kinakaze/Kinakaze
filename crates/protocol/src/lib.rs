//! Bounded, pointer-free messages shared by the runtime and process manager.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_SIZE: usize = 65_536;
pub const MAX_STATE_NAME_BYTES: usize = 128;

/// Bound to a fresh pooled worker only after the controller reserves it.
/// Paths and argv use Linux spelling; root and distribution belong to init.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PoolLaunch {
    pub arguments: Vec<String>,
    pub cwd: String,
    pub environment: Option<Vec<String>>,
}

impl PoolLaunch {
    pub fn valid(&self) -> bool {
        self.arguments
            .first()
            .is_some_and(|program| program.starts_with('/'))
            && self.cwd.starts_with('/')
            && !self.cwd.contains('\0')
            && !self.arguments.iter().any(|value| value.contains('\0'))
            && !self
                .environment
                .as_ref()
                .is_some_and(|values| values.iter().any(|value| value.contains('\0')))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOpenConfig {
    pub endpoint: String,
    pub token: String,
    pub adoption_ticket: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClientRole {
    Worker,
    Controller,
    /// A reconnectable host client, authenticated with a separate launch credential.
    Launcher,
    /// Native infrastructure with no Linux PID and no management privileges.
    Helper,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub version: u32,
    pub token: String,
    pub role: ClientRole,
    pub adoption_ticket: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ForkPolicy {
    Copy,
    Share,
    Reset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Request {
    Hello(Hello),
    Identity,
    /// Resolve an authorized logical process for process_vm_readv/writev.
    ProcessMemoryTarget {
        pid: u32,
        write: bool,
    },
    RegisterModule {
        module_id: u32,
        schema: u32,
    },
    DefineState {
        module_id: u32,
        name: String,
        initial: u64,
        fork: ForkPolicy,
    },
    ReadState {
        module_id: u32,
        name: String,
    },
    WriteState {
        module_id: u32,
        name: String,
        value: u64,
    },
    PrepareFork {
        request_key: u64,
    },
    /// CLONE_PARENT keeps the caller's parent; arbitrary reparenting is rejected.
    PrepareForkWithParent {
        request_key: u64,
        parent_pid: u32,
    },
    MarkReady,
    AwaitActivation,
    AwaitForkReady {
        transaction: u64,
    },
    CommitFork {
        transaction: u64,
    },
    AbortFork {
        transaction: u64,
    },
    /// Reserve a replacement native worker for the caller's existing Linux PID.
    PrepareExec {
        request_key: u64,
        replacement_host_pid: u32,
        replacement_birth: u64,
    },
    AwaitExecReady {
        transaction: u64,
    },
    CommitExec {
        transaction: u64,
    },
    AbortExec {
        transaction: u64,
    },
    /// Controller wait for the final native owner of this Linux PID to exit.
    AwaitExit {
        pid: u32,
    },
    /// A fresh, single-use worker has loaded native providers but no guest code.
    MarkPrewarmReady,
    AwaitPrewarmActivation,
    /// Controller-only barriers for an init-owned prewarmed worker.
    AwaitPrewarmReady {
        pid: u32,
    },
    ActivatePrewarm {
        pid: u32,
    },
    MarkPoolReady,
    AwaitPoolActivation,
    AwaitPoolReady {
        minimum: u32,
    },
    /// Reservation ownership is scoped to this control connection until activation.
    ReservePoolWorker,
    ReleasePoolWorker {
        pid: u32,
    },
    ActivatePoolWorker {
        pid: u32,
        launch: PoolLaunch,
    },
    /// Insert a fresh process under a live process in this init's tree.
    ActivatePoolWorkerUnderParent {
        pid: u32,
        parent_pid: u32,
        launch: PoolLaunch,
    },
    Stats,
    Shutdown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub epoch: u64,
    pub pid: u32,
    pub generation: u32,
    pub parent_pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ForkTicket {
    pub transaction: u64,
    pub token: String,
    pub child: ProcessIdentity,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Stats {
    pub processes: usize,
    pub clients: usize,
    pub objects: usize,
    pub transactions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Hello {
        epoch: u64,
        process: Option<ProcessIdentity>,
    },
    Identity(ProcessIdentity),
    ProcessMemoryTarget {
        host_pid: u32,
        birth: u64,
    },
    Ok,
    PoolWorker {
        pid: u32,
        host_pid: u32,
        birth: u64,
    },
    PoolLaunch(PoolLaunch),
    State {
        object_id: u64,
        value: u64,
    },
    ForkPrepared(ForkTicket),
    ExecPrepared {
        transaction: u64,
    },
    /// Native worker exit code, preserved as signed 32-bit bits, not waitpid bits.
    Exit {
        status: i32,
    },
    Stats(Stats),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidRequest,
    VersionMismatch,
    Unauthorized,
    NotFound,
    Conflict,
    NotReady,
    Aborted,
    LimitExceeded,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireRequest {
    pub id: u64,
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireResponse {
    pub id: u64,
    pub result: Result<Reply, RpcError>,
}

/// A failed read poisons the stream; the caller must close it, not attempt resync.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RPC frame length",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Serialization and size validation finish before any transport bytes are sent.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    struct BoundedPayload(Vec<u8>);
    impl Write for BoundedPayload {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_FRAME_SIZE - self.0.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "RPC frame exceeds size limit",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut payload = BoundedPayload(Vec::new());
    serde_json::to_writer(&mut payload, value).map_err(|error| {
        io::Error::new(
            error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
            error,
        )
    })?;
    let payload = payload.0;
    if payload.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RPC frame exceeds size limit",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn concatenated_frames_round_trip_independently() {
        let requests = [
            WireRequest {
                id: 1,
                request: Request::Identity,
            },
            WireRequest {
                id: 2,
                request: Request::DefineState {
                    module_id: 1,
                    name: "共享状态".into(),
                    initial: u64::MAX,
                    fork: ForkPolicy::Share,
                },
            },
        ];
        let mut bytes = Vec::new();
        for request in &requests {
            write_frame(&mut bytes, request).unwrap();
        }
        let mut reader = Cursor::new(bytes);
        for expected in requests {
            assert_eq!(read_frame::<_, WireRequest>(&mut reader).unwrap(), expected);
        }
    }

    #[test]
    fn fragmented_transport_is_supported() {
        struct Fragments(Cursor<Vec<u8>>);
        impl Read for Fragments {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let length = out.len().min(1);
                self.0.read(&mut out[..length])
            }
        }
        let expected = WireResponse {
            id: 42,
            result: Err(RpcError::new(ErrorCode::Aborted, "child exited")),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        assert_eq!(
            read_frame::<_, WireResponse>(&mut Fragments(Cursor::new(bytes))).unwrap(),
            expected
        );
    }

    #[test]
    fn invalid_lengths_are_rejected_before_payload_allocation_or_read() {
        for length in [0_u32, MAX_FRAME_SIZE as u32 + 1, u32::MAX] {
            let mut bytes = Cursor::new(length.to_le_bytes());
            assert_eq!(
                read_frame::<_, Request>(&mut bytes).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn truncated_prefix_and_payload_fail() {
        for bytes in [vec![1, 0], vec![4, 0, 0, 0, b'{']] {
            assert_eq!(
                read_frame::<_, Request>(&mut Cursor::new(bytes))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }

    #[test]
    fn malformed_json_utf8_and_unknown_fields_fail() {
        for payload in [
            b"{bad}".as_slice(),
            &[255],
            br#"{"id":1,"request":"Identity","extra":true}"#,
            br#"{"id":1,"request":{"RegisterModule":{"module_id":1,"schema":1,"extra":3}}}"#,
        ] {
            let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(payload);
            assert_eq!(
                read_frame::<_, WireRequest>(&mut Cursor::new(frame))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn oversize_write_has_no_partial_transport_effect() {
        let mut bytes = Vec::new();
        assert_eq!(
            write_frame(&mut bytes, &"x".repeat(MAX_FRAME_SIZE))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(bytes.is_empty());
    }
}
