//! Platform-independent ownership and fork-transaction state machine.
//!
//! The server serializes calls under one mutex and wakes activation waiters after
//! mutations. Native process handles belong to the transport, which calls
//! `process_exited` only after verifying PID and creation time together.

use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, ForkPolicy, ForkTicket, Hello, MAX_STATE_NAME_BYTES, PROTOCOL_VERSION,
    PoolLaunch, ProcessIdentity, Reply, Request, RpcError, Stats,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod diagnostics;
pub use diagnostics::{ManagerSnapshot, ProcessSnapshot, ProcessStatus};

pub type ClientId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerIdentity {
    pub host_pid: u32,
    pub birth: u64,
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_clients: usize,
    pub max_processes: usize,
    pub max_modules: usize,
    pub max_states_per_process: usize,
    pub max_state_name_bytes: usize,
    pub max_request_keys_per_process: usize,
    pub max_transactions: usize,
    pub max_objects: usize,
    pub max_exit_records: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_clients: 2048,
            max_processes: 1024,
            max_modules: 128,
            max_states_per_process: 4096,
            max_state_name_bytes: MAX_STATE_NAME_BYTES,
            max_request_keys_per_process: 1024,
            max_transactions: 4096,
            max_objects: 262_144,
            max_exit_records: 4096,
        }
    }
}

struct Client {
    peer: PeerIdentity,
    kind: ClientKind,
}

#[derive(Clone, Copy)]
enum ClientKind {
    Helper,
    Controller,
    Launcher,
    Worker { pid: u32 },
    ExecCandidate { pid: u32, transaction: u64 },
    Retired { pid: u32, transaction: u64 },
}

type StateKey = (u32, String);

#[derive(Clone)]
struct Snapshot {
    modules: BTreeMap<u32, u32>,
    states: BTreeMap<StateKey, u64>,
}

#[derive(Clone, Copy)]
enum ProcessState {
    Active,
    Pending(u64),
    Aborted,
}

struct Process {
    identity: ProcessIdentity,
    peer: PeerIdentity,
    snapshot: Snapshot,
    state: ProcessState,
    request_keys: BTreeMap<u64, u64>,
    exec_request_keys: BTreeMap<u64, u64>,
    prewarm: PrewarmState,
}

#[derive(Clone, PartialEq, Eq)]
enum PrewarmState {
    Fresh,
    Ready,
    Activated,
    PoolReady,
    PoolReserved(ClientId),
    PoolActivated {
        launch: Box<PoolLaunch>,
        owner: ClientId,
        parent_pid: u32,
    },
}

#[derive(Clone, Copy)]
struct Object {
    value: u64,
    initial: u64,
    fork: ForkPolicy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransactionState {
    Prepared,
    Adopted,
    Ready,
    Committed,
    Aborted,
}

struct Transaction {
    parent: u32,
    requested_parent: u32,
    ticket: ForkTicket,
    snapshot: Option<Snapshot>,
    state: TransactionState,
}

struct ExecTransaction {
    process: u32,
    owner: ClientId,
    owner_peer: PeerIdentity,
    target: PeerIdentity,
    candidate: Option<ClientId>,
    state: TransactionState,
    // Commit is irreversible, but only a native handle death notification may
    // activate the replacement. EOF cannot prove all old guest threads stopped.
    retiring: bool,
    candidate_exit: Option<i32>,
}

pub struct StateManager {
    epoch: u64,
    token: String,
    limits: Limits,
    clients: BTreeMap<ClientId, Client>,
    processes: BTreeMap<u32, Process>,
    modules: BTreeMap<u32, u32>,
    objects: BTreeMap<u64, Object>,
    transactions: BTreeMap<u64, Transaction>,
    exec_transactions: BTreeMap<u64, ExecTransaction>,
    // EOF does not prove an old worker died. Keep it from reconnecting after an
    // exec commit until the transport observes the pinned native handle exit.
    retired_peers: BTreeSet<PeerIdentity>,
    exit_records: BTreeMap<u32, i32>,
    exit_order: VecDeque<u32>,
    next_client: u64,
    next_pid: u32,
    next_object: u64,
    next_transaction: u64,
    shutdown_requested: bool,
}

type RpcResult<T> = Result<T, RpcError>;

fn error(code: ErrorCode, message: &str) -> RpcError {
    RpcError::new(code, message)
}

fn limit(message: &str) -> RpcError {
    error(ErrorCode::LimitExceeded, message)
}

// Equal-length credentials are compared without data-dependent early return.
fn secret_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn secure_ticket() -> RpcResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| error(ErrorCode::Internal, "OS entropy unavailable"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(result)
}

impl StateManager {
    pub fn new(epoch: u64, token: String) -> Self {
        Self::with_limits(epoch, token, Limits::default())
    }

    pub fn with_limits(epoch: u64, token: String, limits: Limits) -> Self {
        Self {
            epoch,
            token,
            limits,
            clients: BTreeMap::new(),
            processes: BTreeMap::new(),
            modules: BTreeMap::new(),
            objects: BTreeMap::new(),
            transactions: BTreeMap::new(),
            exec_transactions: BTreeMap::new(),
            retired_peers: BTreeSet::new(),
            exit_records: BTreeMap::new(),
            exit_order: VecDeque::new(),
            next_client: 1,
            next_pid: 1,
            next_object: 1,
            next_transaction: 1,
            shutdown_requested: false,
        }
    }

    pub fn connect(&mut self, hello: Hello, peer: PeerIdentity) -> RpcResult<(ClientId, Reply)> {
        if hello.version != PROTOCOL_VERSION {
            return Err(error(
                ErrorCode::VersionMismatch,
                "unsupported protocol version",
            ));
        }
        if self.token.is_empty()
            || hello.token.len() > 512
            || !secret_equal(&hello.token, &self.token)
        {
            return Err(error(ErrorCode::Unauthorized, "invalid session credential"));
        }
        if peer.host_pid == 0 || peer.birth == 0 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "invalid native process identity",
            ));
        }
        if self.shutdown_requested {
            return Err(error(ErrorCode::Aborted, "manager is shutting down"));
        }
        if self.clients.len() >= self.limits.max_clients || self.next_client == u64::MAX {
            return Err(limit("client limit reached"));
        }
        let reserved_exec = self
            .exec_transactions
            .iter()
            .find_map(|(&id, transaction)| (transaction.target == peer).then_some(id));
        let known_worker = self.processes.values().any(|process| process.peer == peer)
            || self.clients.values().any(|client| {
                client.peer == peer
                    && !matches!(client.kind, ClientKind::Controller | ClientKind::Launcher)
            })
            || self.retired_peers.contains(&peer);
        let (process, kind) = match hello.role {
            ClientRole::Helper => {
                if hello.adoption_ticket.is_some()
                    || known_worker
                    || reserved_exec.is_some()
                    || self.clients.values().any(|client| client.peer == peer)
                {
                    return Err(error(
                        ErrorCode::Unauthorized,
                        "helper requires a fresh native identity",
                    ));
                }
                (None, ClientKind::Helper)
            }
            ClientRole::Controller | ClientRole::Launcher => {
                if hello.adoption_ticket.is_some() || known_worker || reserved_exec.is_some() {
                    return Err(error(
                        ErrorCode::Unauthorized,
                        "worker identities cannot open controller connections",
                    ));
                }
                (
                    None,
                    if hello.role == ClientRole::Launcher {
                        ClientKind::Launcher
                    } else {
                        ClientKind::Controller
                    },
                )
            }
            ClientRole::Worker => {
                if known_worker {
                    return Err(error(
                        ErrorCode::Conflict,
                        "native worker already owns a Linux process",
                    ));
                }
                if self.clients.values().any(|client| client.peer == peer) {
                    return Err(error(
                        ErrorCode::Conflict,
                        "controller cannot become a worker",
                    ));
                }
                if let Some(transaction) = reserved_exec {
                    if hello.adoption_ticket.is_some() {
                        return Err(error(
                            ErrorCode::Conflict,
                            "exec candidate cannot adopt a fork",
                        ));
                    }
                    let identity = self.adopt_exec(transaction, self.next_client)?;
                    (
                        Some(identity),
                        ClientKind::ExecCandidate {
                            pid: identity.pid,
                            transaction,
                        },
                    )
                } else {
                    let identity = match hello.adoption_ticket {
                        Some(token) => self.adopt(&token, peer)?,
                        None => {
                            self.check_process_capacity()?;
                            let identity = self.allocate_identity(0)?;
                            self.processes.insert(
                                identity.pid,
                                Process {
                                    identity,
                                    peer,
                                    snapshot: Snapshot {
                                        modules: BTreeMap::new(),
                                        states: BTreeMap::new(),
                                    },
                                    state: ProcessState::Active,
                                    request_keys: BTreeMap::new(),
                                    exec_request_keys: BTreeMap::new(),
                                    prewarm: PrewarmState::Fresh,
                                },
                            );
                            identity
                        }
                    };
                    (Some(identity), ClientKind::Worker { pid: identity.pid })
                }
            }
        };
        let client = self.next_client;
        self.next_client += 1;
        self.clients.insert(client, Client { peer, kind });
        Ok((
            client,
            Reply::Hello {
                epoch: self.epoch,
                process,
            },
        ))
    }

    pub fn handle(&mut self, client: ClientId, request: Request) -> RpcResult<Reply> {
        let connected = self
            .clients
            .get(&client)
            .ok_or_else(|| error(ErrorCode::Unauthorized, "unknown or disconnected client"))?;
        let kind = connected.kind;
        let peer = connected.peer;
        if matches!(kind, ClientKind::Helper) {
            return Err(error(
                ErrorCode::Unauthorized,
                "helper has no management or guest process privileges",
            ));
        }
        if matches!(request, Request::Hello(_)) {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Hello is only valid when opening a connection",
            ));
        }
        if matches!(kind, ClientKind::Launcher)
            && !matches!(
                request,
                Request::Stats
                    | Request::AwaitExit { .. }
                    | Request::ReservePoolWorker
                    | Request::ReleasePoolWorker { .. }
                    | Request::ActivatePoolWorker { .. }
                    | Request::ActivatePoolWorkerUnderParent { .. }
            )
        {
            return Err(error(
                ErrorCode::Unauthorized,
                "launcher can only launch and observe processes",
            ));
        }
        if matches!(kind, ClientKind::Controller | ClientKind::Launcher) {
            return match request {
                Request::Stats => Ok(Reply::Stats(self.stats())),
                Request::AwaitExit { pid } => self.await_exit(pid),
                Request::AwaitPrewarmReady { pid } => self.prewarm_ready(pid),
                Request::ActivatePrewarm { pid } => self.activate_prewarm(pid),
                Request::AwaitPoolReady { minimum } => {
                    if minimum == 0 {
                        return Err(error(
                            ErrorCode::InvalidRequest,
                            "pool readiness count must be positive",
                        ));
                    }
                    if self.pool_ready_count() < minimum as usize {
                        Err(error(ErrorCode::NotReady, "pool is preparing"))
                    } else {
                        Ok(Reply::Ok)
                    }
                }
                Request::ReservePoolWorker => self.reserve_pool_worker(client),
                Request::ReleasePoolWorker { pid } => self.release_pool_worker(client, pid),
                Request::ActivatePoolWorker { pid, launch } => {
                    self.activate_pool_worker(client, pid, 0, launch)
                }
                Request::ActivatePoolWorkerUnderParent {
                    pid,
                    parent_pid,
                    launch,
                } => self.activate_pool_worker(client, pid, parent_pid, launch),
                Request::Shutdown => {
                    self.shutdown_requested = true;
                    Ok(Reply::Ok)
                }
                _ => Err(error(
                    ErrorCode::Unauthorized,
                    "controller has no guest process",
                )),
            };
        }
        if matches!(
            request,
            Request::AwaitExit { .. }
                | Request::AwaitPrewarmReady { .. }
                | Request::ActivatePrewarm { .. }
                | Request::AwaitPoolReady { .. }
                | Request::ReservePoolWorker
                | Request::ReleasePoolWorker { .. }
                | Request::ActivatePoolWorker { .. }
                | Request::ActivatePoolWorkerUnderParent { .. }
        ) {
            return Err(error(
                ErrorCode::Unauthorized,
                "only controller can await process exit",
            ));
        }
        let pid = match kind {
            ClientKind::Worker { pid } => pid,
            ClientKind::ExecCandidate { pid, transaction } => {
                return self.handle_exec_candidate(client, pid, transaction, request);
            }
            ClientKind::Retired { pid, transaction } => {
                return match request {
                    Request::CommitExec { transaction: id } if id == transaction => {
                        self.commit_exec(client, pid, id)
                    }
                    Request::AbortExec { transaction: id } if id == transaction => {
                        self.abort_exec(client, pid, id)
                    }
                    _ => Err(error(
                        ErrorCode::Unauthorized,
                        "worker was replaced by exec",
                    )),
                };
            }
            ClientKind::Controller | ClientKind::Launcher | ClientKind::Helper => {
                unreachable!("handled before dispatch")
            }
        };
        let process = self
            .processes
            .get(&pid)
            .ok_or_else(|| error(ErrorCode::Unauthorized, "worker process has exited"))?;
        if process.peer != peer {
            return Err(error(
                ErrorCode::Unauthorized,
                "worker no longer owns this process",
            ));
        }
        if matches!(request, Request::Identity) {
            return Ok(Reply::Identity(process.identity));
        }
        match process.state {
            ProcessState::Aborted => {
                return Err(error(
                    ErrorCode::Aborted,
                    "child fork transaction was aborted",
                ));
            }
            ProcessState::Pending(_) => {
                if !matches!(
                    request,
                    Request::RegisterModule { .. } | Request::MarkReady | Request::AwaitActivation
                ) {
                    return Err(error(ErrorCode::NotReady, "child must await parent commit"));
                }
            }
            ProcessState::Active => {}
        }
        match request {
            Request::MarkPrewarmReady => {
                let process = self.processes.get_mut(&pid).unwrap();
                if process.identity.parent_pid != 0
                    || process.identity.generation != 1
                    || !matches!(process.prewarm, PrewarmState::Fresh | PrewarmState::Ready)
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "only an unused root worker can prewarm",
                    ));
                }
                process.prewarm = PrewarmState::Ready;
                Ok(Reply::Ok)
            }
            Request::AwaitPrewarmActivation => match self.processes[&pid].prewarm {
                PrewarmState::Activated => Ok(Reply::Ok),
                PrewarmState::Ready => {
                    Err(error(ErrorCode::NotReady, "awaiting prewarm activation"))
                }
                _ => Err(error(ErrorCode::Conflict, "worker has not prewarmed")),
            },
            Request::MarkPoolReady => {
                let process = self.processes.get_mut(&pid).unwrap();
                if process.identity.parent_pid != 0
                    || process.identity.generation != 1
                    || !matches!(
                        process.prewarm,
                        PrewarmState::Fresh | PrewarmState::PoolReady
                    )
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "only an unused root worker can join the pool",
                    ));
                }
                process.prewarm = PrewarmState::PoolReady;
                Ok(Reply::Ok)
            }
            Request::AwaitPoolActivation => match &self.processes[&pid].prewarm {
                PrewarmState::PoolActivated { launch, .. } => {
                    Ok(Reply::PoolLaunch((**launch).clone()))
                }
                PrewarmState::PoolReady | PrewarmState::PoolReserved(_) => {
                    Err(error(ErrorCode::NotReady, "awaiting pool assignment"))
                }
                _ => Err(error(ErrorCode::Conflict, "worker is not in the pool")),
            },
            Request::ProcessMemoryTarget { pid: target, write } => {
                self.process_memory_target(pid, target, write)
            }
            Request::RegisterModule { module_id, schema } => {
                self.register_module(pid, module_id, schema)
            }
            Request::DefineState {
                module_id,
                name,
                initial,
                fork,
            } => self.define_state(pid, module_id, name, initial, fork),
            Request::ReadState { module_id, name } => self.read_state(pid, module_id, name),
            Request::WriteState {
                module_id,
                name,
                value,
            } => self.write_state(pid, module_id, name, value),
            Request::PrepareFork { request_key } => self.prepare_fork(pid, request_key, pid),
            Request::PrepareForkWithParent {
                request_key,
                parent_pid,
            } => {
                if pid == 1 {
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "PID 1 cannot use CLONE_PARENT",
                    ));
                }
                if self.processes[&pid].identity.parent_pid != parent_pid {
                    return Err(error(
                        ErrorCode::Unauthorized,
                        "CLONE_PARENT must retain the caller's parent",
                    ));
                }
                self.prepare_fork(pid, request_key, parent_pid)
            }
            Request::MarkReady => self.mark_ready(pid),
            Request::AwaitActivation => match self.processes[&pid].state {
                ProcessState::Active => Ok(Reply::Ok),
                _ => Err(error(ErrorCode::NotReady, "awaiting parent commit")),
            },
            Request::CommitFork { transaction } => self.commit_fork(pid, transaction),
            Request::AwaitForkReady { transaction } => {
                match self.owned_transaction(pid, transaction)?.state {
                    TransactionState::Ready | TransactionState::Committed => Ok(Reply::Ok),
                    TransactionState::Aborted => {
                        Err(error(ErrorCode::Aborted, "fork transaction was aborted"))
                    }
                    _ => Err(error(
                        ErrorCode::NotReady,
                        "child has not acknowledged readiness",
                    )),
                }
            }
            Request::AbortFork { transaction } => self.abort_fork(pid, transaction),
            Request::PrepareExec {
                request_key,
                replacement_host_pid,
                replacement_birth,
            } => self.prepare_exec(
                client,
                pid,
                request_key,
                PeerIdentity {
                    host_pid: replacement_host_pid,
                    birth: replacement_birth,
                },
            ),
            Request::AwaitExecReady { transaction } => {
                self.await_exec_ready(client, pid, transaction)
            }
            Request::CommitExec { transaction } => self.commit_exec(client, pid, transaction),
            Request::AbortExec { transaction } => self.abort_exec(client, pid, transaction),
            Request::Stats => Ok(Reply::Stats(self.stats())),
            Request::Shutdown => Err(error(
                ErrorCode::Unauthorized,
                "only controller can shut down manager",
            )),
            Request::Identity
            | Request::Hello(_)
            | Request::AwaitExit { .. }
            | Request::AwaitPrewarmReady { .. }
            | Request::ActivatePrewarm { .. }
            | Request::AwaitPoolReady { .. }
            | Request::ReservePoolWorker
            | Request::ReleasePoolWorker { .. }
            | Request::ActivatePoolWorker { .. }
            | Request::ActivatePoolWorkerUnderParent { .. } => {
                unreachable!("handled before dispatch")
            }
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            processes: self.processes.len(),
            clients: self.clients.len(),
            objects: self.objects.len(),
            transactions: self.transactions.len() + self.exec_transactions.len(),
        }
    }

    fn prewarm_ready(&self, pid: u32) -> RpcResult<Reply> {
        if pid == self.next_pid {
            return Err(error(ErrorCode::NotReady, "prewarmed worker is connecting"));
        }
        let process = self.processes.get(&pid).ok_or_else(|| {
            error(
                ErrorCode::NotFound,
                "prewarmed worker has exited or does not exist",
            )
        })?;
        match process.prewarm {
            PrewarmState::Fresh => Err(error(ErrorCode::NotReady, "worker is still preparing")),
            PrewarmState::Ready | PrewarmState::Activated => Ok(Reply::Ok),
            _ => Err(error(
                ErrorCode::Conflict,
                "worker belongs to the generic pool",
            )),
        }
    }

    fn activate_prewarm(&mut self, pid: u32) -> RpcResult<Reply> {
        let process = self
            .processes
            .get_mut(&pid)
            .ok_or_else(|| error(ErrorCode::NotFound, "unknown prewarmed worker"))?;
        if !matches!(
            process.prewarm,
            PrewarmState::Ready | PrewarmState::Activated
        ) {
            return Err(error(ErrorCode::NotReady, "worker is not prewarmed"));
        }
        process.prewarm = PrewarmState::Activated;
        Ok(Reply::Ok)
    }

    pub fn pool_ready_count(&self) -> usize {
        self.processes
            .values()
            .filter(|process| process.prewarm == PrewarmState::PoolReady)
            .count()
    }

    pub fn process_peer(&self, pid: u32) -> Option<PeerIdentity> {
        self.processes.get(&pid).map(|process| process.peer)
    }

    pub fn pool_peer_is_prepared(&self, peer: PeerIdentity) -> bool {
        self.processes.values().any(|process| {
            process.peer == peer
                && matches!(
                    process.prewarm,
                    PrewarmState::PoolReady | PrewarmState::PoolReserved(_)
                )
        })
    }

    fn reserve_pool_worker(&mut self, client: ClientId) -> RpcResult<Reply> {
        let process = self
            .processes
            .values_mut()
            .find(|process| process.prewarm == PrewarmState::PoolReady)
            .ok_or_else(|| error(ErrorCode::NotReady, "no prepared pool worker"))?;
        process.prewarm = PrewarmState::PoolReserved(client);
        Ok(Reply::PoolWorker {
            pid: process.identity.pid,
            host_pid: process.peer.host_pid,
            birth: process.peer.birth,
        })
    }

    fn release_pool_worker(&mut self, client: ClientId, pid: u32) -> RpcResult<Reply> {
        let process = self
            .processes
            .get_mut(&pid)
            .ok_or_else(|| error(ErrorCode::NotFound, "unknown pool worker"))?;
        if process.prewarm != PrewarmState::PoolReserved(client) {
            return Err(error(
                ErrorCode::Conflict,
                "connection does not hold this pool reservation",
            ));
        }
        process.prewarm = PrewarmState::PoolReady;
        Ok(Reply::Ok)
    }

    fn activate_pool_worker(
        &mut self,
        client: ClientId,
        pid: u32,
        parent_pid: u32,
        launch: PoolLaunch,
    ) -> RpcResult<Reply> {
        if !launch.valid() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "pool launch requires absolute Linux argv[0]/cwd and NUL-free strings",
            ));
        }
        if parent_pid == pid {
            return Err(error(
                ErrorCode::InvalidRequest,
                "process cannot be its own parent",
            ));
        }
        let process = self
            .processes
            .get(&pid)
            .ok_or_else(|| error(ErrorCode::NotFound, "unknown pool worker"))?;
        if let PrewarmState::PoolActivated {
            launch: previous,
            owner,
            parent_pid: previous_parent,
        } = &process.prewarm
        {
            return if **previous == launch && *owner == client && *previous_parent == parent_pid {
                Ok(Reply::Ok)
            } else {
                Err(error(
                    ErrorCode::Conflict,
                    "pool worker already has a different launch",
                ))
            };
        }
        if process.prewarm != PrewarmState::PoolReserved(client) {
            return Err(error(
                ErrorCode::Conflict,
                "connection does not hold this pool reservation",
            ));
        }
        if parent_pid != 0 {
            let parent = self
                .processes
                .get(&parent_pid)
                .ok_or_else(|| error(ErrorCode::NotFound, "parent process does not exist"))?;
            if !matches!(parent.state, ProcessState::Active)
                || matches!(
                    parent.prewarm,
                    PrewarmState::Ready | PrewarmState::PoolReady | PrewarmState::PoolReserved(_)
                )
                || self.retired_peers.contains(&parent.peer)
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "parent is not an active application",
                ));
            }
        }
        let process = self.processes.get_mut(&pid).unwrap();
        process.identity.parent_pid = parent_pid;
        process.prewarm = PrewarmState::PoolActivated {
            launch: Box::new(launch),
            owner: client,
            parent_pid,
        };
        Ok(Reply::Ok)
    }

    fn process_memory_target(&self, caller: u32, target: u32, _write: bool) -> RpcResult<Reply> {
        if target == 0 || target > i32::MAX as u32 {
            return Err(error(ErrorCode::NotFound, "unknown Linux process"));
        }
        let process = self
            .processes
            .get(&target)
            .ok_or_else(|| error(ErrorCode::NotFound, "unknown Linux process"))?;
        if !matches!(process.state, ProcessState::Active)
            || self.retired_peers.contains(&process.peer)
        {
            return Err(error(
                ErrorCode::NotFound,
                "Linux process has no active native owner",
            ));
        }
        // The initial domain uses ptrace_scope=1's ancestry restriction. This
        // intentionally does not grant the controller guest memory authority.
        // UID/dumpability and PR_SET_PTRACER exceptions must extend this check
        // when those credentials are represented by the common manager.
        let mut ancestor = target;
        for _ in 0..=self.processes.len() {
            if ancestor == caller {
                return Ok(Reply::ProcessMemoryTarget {
                    host_pid: process.peer.host_pid,
                    birth: process.peer.birth,
                });
            }
            ancestor = self
                .processes
                .get(&ancestor)
                .map_or(0, |entry| entry.identity.parent_pid);
            if ancestor == 0 {
                break;
            }
        }
        Err(error(
            ErrorCode::Unauthorized,
            "memory access requires a descendant process",
        ))
    }

    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// EOF revokes the connection and pending transactions, but is not proof of death.
    pub fn disconnect(&mut self, client: ClientId) {
        for process in self.processes.values_mut() {
            if process.prewarm == PrewarmState::PoolReserved(client) {
                process.prewarm = PrewarmState::PoolReady;
            }
        }
        let Some(client) = self.clients.remove(&client) else {
            return;
        };
        match client.kind {
            ClientKind::Worker { pid } => {
                if self
                    .processes
                    .get(&pid)
                    .is_some_and(|process| process.peer == client.peer)
                {
                    self.abort_pending_for(pid);
                    self.abort_pending_exec_for(pid);
                }
            }
            ClientKind::ExecCandidate { transaction, .. } => {
                self.abort_exec_transaction(transaction);
            }
            ClientKind::Controller
            | ClientKind::Launcher
            | ClientKind::Helper
            | ClientKind::Retired { .. } => {}
        }
        self.collect_objects();
    }

    /// Compatibility entry point for callers without a native exit code.
    pub fn process_exited(&mut self, peer: PeerIdentity) {
        self.process_exited_with_status(peer, 127);
    }

    /// Record exit only for the matching native owner of a current Process.
    ///
    /// A retired exec worker cannot finish its replacement's PID, and stale
    /// notifications with another native birth cannot reclaim its resources.
    /// `status` is the native exit code, not Linux waitpid's encoded status.
    pub fn process_exited_with_status(&mut self, peer: PeerIdentity, status: i32) {
        let mut deferred_exits = Vec::new();
        for transaction in self.exec_transactions.values_mut() {
            if transaction.retiring && transaction.target == peer {
                transaction.candidate_exit.get_or_insert(status);
            }
            if transaction.retiring && transaction.owner_peer == peer {
                transaction.retiring = false;
                if let Some(process) = self.processes.get_mut(&transaction.process) {
                    process.peer = transaction.target;
                }
                if let Some(candidate) = transaction
                    .candidate
                    .and_then(|id| self.clients.get_mut(&id))
                {
                    candidate.kind = ClientKind::Worker {
                        pid: transaction.process,
                    };
                }
                if let Some(status) = transaction.candidate_exit {
                    deferred_exits.push((transaction.target, status));
                }
            }
        }
        let exec_targets: Vec<_> = self
            .exec_transactions
            .iter()
            .filter_map(|(&id, transaction)| (transaction.target == peer).then_some(id))
            .collect();
        for id in exec_targets {
            self.abort_exec_transaction(id);
        }
        let clients: Vec<_> = self
            .clients
            .iter()
            .filter_map(|(&id, client)| (client.peer == peer).then_some(id))
            .collect();
        for client in clients {
            self.disconnect(client);
        }
        let pids: Vec<_> = self
            .processes
            .iter()
            .filter_map(|(&pid, process)| (process.peer == peer).then_some(pid))
            .collect();
        for pid in pids {
            self.abort_pending_for(pid);
            self.abort_pending_exec_for(pid);
            self.processes.remove(&pid);
            self.record_exit(pid, status);
            for child in self.processes.values_mut() {
                if child.identity.parent_pid == pid {
                    child.identity.parent_pid = 0;
                }
            }
            // CLONE_PARENT can outlive its logical parent while the transaction
            // creator stays alive. A reserved, not-yet-attached child must see
            // the same reparenting as an attached child.
            for transaction in self.transactions.values_mut() {
                if transaction.ticket.child.parent_pid == pid {
                    transaction.ticket.child.parent_pid = 0;
                }
            }
            // Terminal transactions no longer need replay records after their owner dies.
            self.transactions
                .retain(|_, transaction| transaction.parent != pid);
            self.exec_transactions
                .retain(|_, transaction| transaction.process != pid);
        }
        self.retired_peers.remove(&peer);
        self.collect_objects();
        // A candidate which died past the irreversible commit completes the
        // logical PID only after every thread of the original worker is dead.
        for (target, status) in deferred_exits {
            self.process_exited_with_status(target, status);
        }
    }

    fn await_exit(&self, pid: u32) -> RpcResult<Reply> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(error(ErrorCode::InvalidRequest, "invalid Linux process ID"));
        }
        if let Some(&status) = self.exit_records.get(&pid) {
            return Ok(Reply::Exit { status });
        }
        if self.processes.contains_key(&pid) {
            return Err(error(
                ErrorCode::NotReady,
                "process native owner has not exited",
            ));
        }
        Err(error(
            ErrorCode::NotFound,
            "unknown process or expired exit record",
        ))
    }

    fn record_exit(&mut self, pid: u32, status: i32) {
        if self.limits.max_exit_records == 0 {
            return;
        }
        while self.exit_records.len() >= self.limits.max_exit_records {
            let expired = self
                .exit_order
                .pop_front()
                .expect("exit record has retention order");
            self.exit_records.remove(&expired);
        }
        self.exit_records.insert(pid, status);
        self.exit_order.push_back(pid);
    }

    fn check_process_capacity(&self) -> RpcResult<()> {
        let reserved = self
            .transactions
            .values()
            .filter(|tx| tx.state == TransactionState::Prepared)
            .count();
        if self.processes.len().saturating_add(reserved) >= self.limits.max_processes {
            Err(limit("process limit reached"))
        } else {
            Ok(())
        }
    }

    fn allocate_identity(&mut self, parent_pid: u32) -> RpcResult<ProcessIdentity> {
        // Linux pid_t is a signed 32-bit integer; zero is reserved for no parent.
        if self.next_pid > i32::MAX as u32 {
            return Err(limit("guest PID space exhausted"));
        }
        let pid = self.next_pid;
        self.next_pid += 1;
        // PIDs are never reused within an epoch, therefore generation stays one.
        Ok(ProcessIdentity {
            epoch: self.epoch,
            pid,
            generation: 1,
            parent_pid,
        })
    }

    fn adopt(&mut self, token: &str, peer: PeerIdentity) -> RpcResult<ProcessIdentity> {
        if token.len() != 64 {
            return Err(error(ErrorCode::Unauthorized, "invalid adoption ticket"));
        }
        let id = self
            .transactions
            .iter()
            .find_map(|(&id, transaction)| {
                secret_equal(&transaction.ticket.token, token).then_some(id)
            })
            .ok_or_else(|| error(ErrorCode::Unauthorized, "unknown adoption ticket"))?;
        let transaction = self.transactions.get_mut(&id).unwrap();
        match transaction.state {
            TransactionState::Prepared => {}
            TransactionState::Aborted => {
                return Err(error(ErrorCode::Aborted, "adoption ticket was revoked"));
            }
            _ => {
                return Err(error(
                    ErrorCode::Conflict,
                    "adoption ticket was already consumed",
                ));
            }
        }
        let identity = transaction.ticket.child;
        let snapshot = transaction
            .snapshot
            .take()
            .expect("prepared transaction owns snapshot");
        transaction.state = TransactionState::Adopted;
        self.processes.insert(
            identity.pid,
            Process {
                identity,
                peer,
                snapshot,
                state: ProcessState::Pending(id),
                request_keys: BTreeMap::new(),
                exec_request_keys: BTreeMap::new(),
                prewarm: PrewarmState::Fresh,
            },
        );
        Ok(identity)
    }

    fn register_module(&mut self, pid: u32, module_id: u32, schema: u32) -> RpcResult<Reply> {
        let process = &self.processes[&pid];
        if let Some(existing) = process.snapshot.modules.get(&module_id) {
            return if *existing == schema {
                Ok(Reply::Ok)
            } else {
                Err(error(
                    ErrorCode::Conflict,
                    "module schema differs from registered schema",
                ))
            };
        }
        if matches!(process.state, ProcessState::Pending(_)) {
            return Err(error(
                ErrorCode::NotReady,
                "child may only validate inherited modules before commit",
            ));
        }
        if let Some(existing) = self.modules.get(&module_id) {
            if *existing != schema {
                return Err(error(
                    ErrorCode::Conflict,
                    "module schema differs from manager schema",
                ));
            }
        } else if self.modules.len() >= self.limits.max_modules {
            return Err(limit("module limit reached"));
        }
        self.modules.insert(module_id, schema);
        self.processes
            .get_mut(&pid)
            .unwrap()
            .snapshot
            .modules
            .insert(module_id, schema);
        Ok(Reply::Ok)
    }

    fn validate_name(&self, name: &str) -> RpcResult<()> {
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(error(
                ErrorCode::InvalidRequest,
                "state name is empty or contains control characters",
            ));
        }
        if name.len() > self.limits.max_state_name_bytes.min(MAX_STATE_NAME_BYTES) {
            return Err(limit("state name exceeds byte limit"));
        }
        Ok(())
    }

    fn check_object_capacity(&self, count: usize) -> RpcResult<()> {
        if self
            .objects
            .len()
            .checked_add(count)
            .is_none_or(|total| total > self.limits.max_objects)
            || self.next_object.checked_add(count as u64).is_none()
        {
            return Err(limit("state object limit reached"));
        }
        Ok(())
    }

    fn insert_object(&mut self, object: Object) -> u64 {
        let object_id = self.next_object;
        self.next_object += 1;
        self.objects.insert(object_id, object);
        object_id
    }

    fn define_state(
        &mut self,
        pid: u32,
        module_id: u32,
        name: String,
        initial: u64,
        fork: ForkPolicy,
    ) -> RpcResult<Reply> {
        self.validate_name(&name)?;
        let process = &self.processes[&pid];
        if !process.snapshot.modules.contains_key(&module_id) {
            return Err(error(
                ErrorCode::NotFound,
                "module must register before defining state",
            ));
        }
        let key = (module_id, name);
        if let Some(&object_id) = process.snapshot.states.get(&key) {
            let object = self.objects[&object_id];
            if object.initial != initial || object.fork != fork {
                return Err(error(
                    ErrorCode::Conflict,
                    "state definition differs from existing policy or initial value",
                ));
            }
            return Ok(Reply::State {
                object_id,
                value: object.value,
            });
        }
        if process.snapshot.states.len() >= self.limits.max_states_per_process {
            return Err(limit("per-process state limit reached"));
        }
        self.check_object_capacity(1)?;
        let object_id = self.insert_object(Object {
            value: initial,
            initial,
            fork,
        });
        self.processes
            .get_mut(&pid)
            .unwrap()
            .snapshot
            .states
            .insert(key, object_id);
        Ok(Reply::State {
            object_id,
            value: initial,
        })
    }

    fn state_object(&self, pid: u32, module_id: u32, name: String) -> RpcResult<u64> {
        self.validate_name(&name)?;
        self.processes[&pid]
            .snapshot
            .states
            .get(&(module_id, name))
            .copied()
            .ok_or_else(|| error(ErrorCode::NotFound, "state is not defined in this process"))
    }

    fn read_state(&self, pid: u32, module_id: u32, name: String) -> RpcResult<Reply> {
        let object_id = self.state_object(pid, module_id, name)?;
        Ok(Reply::State {
            object_id,
            value: self.objects[&object_id].value,
        })
    }

    fn write_state(
        &mut self,
        pid: u32,
        module_id: u32,
        name: String,
        value: u64,
    ) -> RpcResult<Reply> {
        let object_id = self.state_object(pid, module_id, name)?;
        self.objects.get_mut(&object_id).unwrap().value = value;
        Ok(Reply::Ok)
    }

    fn prepare_fork(&mut self, pid: u32, request_key: u64, parent_pid: u32) -> RpcResult<Reply> {
        let process = &self.processes[&pid];
        if let Some(id) = process.request_keys.get(&request_key) {
            let transaction = &self.transactions[id];
            if transaction.requested_parent != parent_pid {
                return Err(error(
                    ErrorCode::Conflict,
                    "fork request key reused with different parent",
                ));
            }
            return if transaction.state == TransactionState::Aborted {
                Err(error(
                    ErrorCode::Aborted,
                    "fork request was aborted; use a new request key",
                ))
            } else {
                Ok(Reply::ForkPrepared(transaction.ticket.clone()))
            };
        }
        if process.request_keys.len() + process.exec_request_keys.len()
            >= self.limits.max_request_keys_per_process
        {
            return Err(limit("fork request-key limit reached"));
        }
        if self.transactions.len() + self.exec_transactions.len() >= self.limits.max_transactions
            || self.next_transaction == u64::MAX
        {
            return Err(limit("fork transaction limit reached"));
        }
        self.check_process_capacity()?;
        let mut snapshot = process.snapshot.clone();
        let copies = snapshot
            .states
            .values()
            .filter(|id| self.objects[id].fork != ForkPolicy::Share)
            .count();
        self.check_object_capacity(copies)?;
        let token = secure_ticket()?;
        // All fallible validation and entropy acquisition precede state mutation.
        let child = self.allocate_identity(parent_pid)?;
        for object_id in snapshot.states.values_mut() {
            let mut object = self.objects[object_id];
            match object.fork {
                ForkPolicy::Share => continue,
                ForkPolicy::Copy => {}
                ForkPolicy::Reset => object.value = object.initial,
            }
            *object_id = self.insert_object(object);
        }
        let transaction = self.next_transaction;
        self.next_transaction += 1;
        let ticket = ForkTicket {
            transaction,
            token,
            child,
        };
        self.transactions.insert(
            transaction,
            Transaction {
                parent: pid,
                requested_parent: parent_pid,
                ticket: ticket.clone(),
                snapshot: Some(snapshot),
                state: TransactionState::Prepared,
            },
        );
        self.processes
            .get_mut(&pid)
            .unwrap()
            .request_keys
            .insert(request_key, transaction);
        Ok(Reply::ForkPrepared(ticket))
    }

    fn mark_ready(&mut self, pid: u32) -> RpcResult<Reply> {
        match self.processes[&pid].state {
            ProcessState::Pending(id) => {
                let transaction = self.transactions.get_mut(&id).unwrap();
                transaction.state = TransactionState::Ready;
                Ok(Reply::Ok)
            }
            ProcessState::Active => Ok(Reply::Ok),
            ProcessState::Aborted => Err(error(ErrorCode::Aborted, "fork was aborted")),
        }
    }

    fn owned_transaction(&self, pid: u32, id: u64) -> RpcResult<&Transaction> {
        let transaction = self
            .transactions
            .get(&id)
            .ok_or_else(|| error(ErrorCode::NotFound, "unknown fork transaction"))?;
        if transaction.parent != pid {
            return Err(error(
                ErrorCode::Unauthorized,
                "only transaction parent may commit or abort",
            ));
        }
        Ok(transaction)
    }

    fn commit_fork(&mut self, pid: u32, id: u64) -> RpcResult<Reply> {
        let transaction = self.owned_transaction(pid, id)?;
        match transaction.state {
            TransactionState::Committed => return Ok(Reply::Ok),
            TransactionState::Aborted => {
                return Err(error(ErrorCode::Aborted, "fork transaction was aborted"));
            }
            TransactionState::Prepared | TransactionState::Adopted => {
                return Err(error(
                    ErrorCode::NotReady,
                    "child has not acknowledged readiness",
                ));
            }
            TransactionState::Ready => {}
        }
        let child = transaction.ticket.child.pid;
        self.processes
            .get_mut(&child)
            .expect("ready child must exist")
            .state = ProcessState::Active;
        self.transactions.get_mut(&id).unwrap().state = TransactionState::Committed;
        Ok(Reply::Ok)
    }

    fn abort_fork(&mut self, pid: u32, id: u64) -> RpcResult<Reply> {
        let transaction = self.owned_transaction(pid, id)?;
        if transaction.state == TransactionState::Committed {
            return Err(error(
                ErrorCode::Conflict,
                "committed fork cannot be aborted",
            ));
        }
        self.abort_transaction(id);
        self.collect_objects();
        Ok(Reply::Ok)
    }

    fn abort_transaction(&mut self, id: u64) {
        let Some(transaction) = self.transactions.get_mut(&id) else {
            return;
        };
        if matches!(
            transaction.state,
            TransactionState::Committed | TransactionState::Aborted
        ) {
            return;
        }
        transaction.state = TransactionState::Aborted;
        transaction.snapshot = None;
        if let Some(child) = self.processes.get_mut(&transaction.ticket.child.pid) {
            // Keep attached child references until its real native exit notification.
            child.state = ProcessState::Aborted;
        }
    }

    fn abort_pending_for(&mut self, pid: u32) {
        let pending: Vec<_> = self
            .transactions
            .iter()
            .filter_map(|(&id, transaction)| {
                ((transaction.parent == pid || transaction.ticket.child.pid == pid)
                    && !matches!(
                        transaction.state,
                        TransactionState::Committed | TransactionState::Aborted
                    ))
                .then_some(id)
            })
            .collect();
        for id in pending {
            self.abort_transaction(id);
        }
    }

    fn prepare_exec(
        &mut self,
        owner: ClientId,
        pid: u32,
        request_key: u64,
        target: PeerIdentity,
    ) -> RpcResult<Reply> {
        let process = &self.processes[&pid];
        if let Some(&id) = process.exec_request_keys.get(&request_key) {
            let transaction = &self.exec_transactions[&id];
            if transaction.target != target || transaction.owner != owner {
                return Err(error(
                    ErrorCode::Conflict,
                    "exec request key reused with different replacement or owner",
                ));
            }
            return if transaction.state == TransactionState::Aborted {
                Err(error(
                    ErrorCode::Aborted,
                    "exec request was aborted; use a new request key",
                ))
            } else {
                Ok(Reply::ExecPrepared { transaction: id })
            };
        }
        if target.host_pid == 0 || target.birth == 0 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "invalid replacement process identity",
            ));
        }
        if self.exec_transactions.values().any(|transaction| {
            transaction.process == pid
                && !matches!(
                    transaction.state,
                    TransactionState::Committed | TransactionState::Aborted
                )
        }) {
            return Err(error(
                ErrorCode::Conflict,
                "process already has a pending exec",
            ));
        }
        if self
            .processes
            .values()
            .any(|process| process.peer == target)
            || self.clients.values().any(|client| client.peer == target)
            || self.retired_peers.contains(&target)
            || self
                .exec_transactions
                .values()
                .any(|transaction| transaction.target == target)
        {
            return Err(error(
                ErrorCode::Conflict,
                "replacement native worker is already in use",
            ));
        }
        if process.request_keys.len() + process.exec_request_keys.len()
            >= self.limits.max_request_keys_per_process
        {
            return Err(limit("exec request-key limit reached"));
        }
        let pending = self
            .exec_transactions
            .values()
            .filter(|transaction| {
                !matches!(
                    transaction.state,
                    TransactionState::Committed | TransactionState::Aborted
                )
            })
            .count();
        if self.transactions.len() + self.exec_transactions.len() >= self.limits.max_transactions
            || self.retired_peers.len() + pending >= self.limits.max_transactions
            || self.next_transaction == u64::MAX
        {
            return Err(limit("exec transaction or native retirement limit reached"));
        }
        let transaction = self.next_transaction;
        self.next_transaction += 1;
        self.exec_transactions.insert(
            transaction,
            ExecTransaction {
                process: pid,
                owner,
                owner_peer: process.peer,
                target,
                candidate: None,
                state: TransactionState::Prepared,
                retiring: false,
                candidate_exit: None,
            },
        );
        self.processes
            .get_mut(&pid)
            .unwrap()
            .exec_request_keys
            .insert(request_key, transaction);
        Ok(Reply::ExecPrepared { transaction })
    }

    fn adopt_exec(&mut self, id: u64, client: ClientId) -> RpcResult<ProcessIdentity> {
        let transaction = &self.exec_transactions[&id];
        match transaction.state {
            TransactionState::Prepared => {}
            TransactionState::Aborted => {
                return Err(error(ErrorCode::Aborted, "exec replacement was revoked"));
            }
            _ => {
                return Err(error(
                    ErrorCode::Conflict,
                    "exec replacement was already attached",
                ));
            }
        }
        let identity = self
            .processes
            .get(&transaction.process)
            .ok_or_else(|| error(ErrorCode::Aborted, "exec owner has exited"))?
            .identity;
        let transaction = self.exec_transactions.get_mut(&id).unwrap();
        transaction.candidate = Some(client);
        transaction.state = TransactionState::Adopted;
        Ok(identity)
    }

    fn handle_exec_candidate(
        &mut self,
        client: ClientId,
        pid: u32,
        id: u64,
        request: Request,
    ) -> RpcResult<Reply> {
        let transaction = self
            .exec_transactions
            .get(&id)
            .ok_or_else(|| error(ErrorCode::Aborted, "exec owner has exited"))?;
        if transaction.state == TransactionState::Aborted {
            return Err(error(ErrorCode::Aborted, "exec replacement was revoked"));
        }
        if transaction.process != pid || transaction.candidate != Some(client) {
            return Err(error(
                ErrorCode::Unauthorized,
                "client does not own exec replacement",
            ));
        }
        let process = self
            .processes
            .get(&pid)
            .ok_or_else(|| error(ErrorCode::Aborted, "exec owner has exited"))?;
        match request {
            Request::Identity => Ok(Reply::Identity(process.identity)),
            Request::RegisterModule { module_id, schema } => {
                match process.snapshot.modules.get(&module_id) {
                    Some(existing) if *existing == schema => Ok(Reply::Ok),
                    Some(_) => Err(error(
                        ErrorCode::Conflict,
                        "replacement module schema differs",
                    )),
                    None => Err(error(
                        ErrorCode::NotReady,
                        "replacement may only validate inherited modules before commit",
                    )),
                }
            }
            Request::MarkReady => {
                let transaction = self.exec_transactions.get_mut(&id).unwrap();
                if transaction.state != TransactionState::Committed {
                    transaction.state = TransactionState::Ready;
                }
                Ok(Reply::Ok)
            }
            Request::AwaitActivation => {
                Err(error(ErrorCode::NotReady, "awaiting exec owner commit"))
            }
            Request::Shutdown | Request::Stats => Err(error(
                ErrorCode::Unauthorized,
                "exec replacement has no controller authority",
            )),
            _ => Err(error(
                ErrorCode::NotReady,
                "replacement must await exec commit",
            )),
        }
    }

    fn owned_exec(&self, owner: ClientId, pid: u32, id: u64) -> RpcResult<&ExecTransaction> {
        let transaction = self
            .exec_transactions
            .get(&id)
            .ok_or_else(|| error(ErrorCode::NotFound, "unknown exec transaction"))?;
        if transaction.process != pid || transaction.owner != owner {
            return Err(error(
                ErrorCode::Unauthorized,
                "only exec owner may control the replacement",
            ));
        }
        Ok(transaction)
    }

    fn await_exec_ready(&self, owner: ClientId, pid: u32, id: u64) -> RpcResult<Reply> {
        match self.owned_exec(owner, pid, id)?.state {
            TransactionState::Ready | TransactionState::Committed => Ok(Reply::Ok),
            TransactionState::Aborted => {
                Err(error(ErrorCode::Aborted, "exec replacement was revoked"))
            }
            _ => Err(error(
                ErrorCode::NotReady,
                "replacement has not acknowledged readiness",
            )),
        }
    }

    fn commit_exec(&mut self, owner: ClientId, pid: u32, id: u64) -> RpcResult<Reply> {
        let transaction = self.owned_exec(owner, pid, id)?;
        match transaction.state {
            TransactionState::Committed => return Ok(Reply::Ok),
            TransactionState::Aborted => {
                return Err(error(ErrorCode::Aborted, "exec replacement was revoked"));
            }
            TransactionState::Prepared | TransactionState::Adopted => {
                return Err(error(
                    ErrorCode::NotReady,
                    "replacement has not acknowledged readiness",
                ));
            }
            TransactionState::Ready => {}
        }
        let process = self
            .processes
            .get_mut(&pid)
            .expect("ready exec owns process");
        self.retired_peers.insert(process.peer);
        self.clients
            .get_mut(&owner)
            .expect("exec owner must be connected")
            .kind = ClientKind::Retired {
            pid,
            transaction: id,
        };
        let transaction = self.exec_transactions.get_mut(&id).unwrap();
        transaction.state = TransactionState::Committed;
        transaction.retiring = true;
        Ok(Reply::Ok)
    }

    fn abort_exec(&mut self, owner: ClientId, pid: u32, id: u64) -> RpcResult<Reply> {
        if self.owned_exec(owner, pid, id)?.state == TransactionState::Committed {
            return Err(error(
                ErrorCode::Conflict,
                "committed exec cannot be aborted",
            ));
        }
        self.abort_exec_transaction(id);
        Ok(Reply::Ok)
    }

    fn abort_exec_transaction(&mut self, id: u64) {
        let Some(transaction) = self.exec_transactions.get_mut(&id) else {
            return;
        };
        if matches!(
            transaction.state,
            TransactionState::Committed | TransactionState::Aborted
        ) {
            return;
        }
        if transaction.candidate.is_some() {
            self.retired_peers.insert(transaction.target);
        }
        transaction.state = TransactionState::Aborted;
    }

    fn abort_pending_exec_for(&mut self, pid: u32) {
        let pending: Vec<_> = self
            .exec_transactions
            .iter()
            .filter_map(|(&id, transaction)| (transaction.process == pid).then_some(id))
            .collect();
        for id in pending {
            self.abort_exec_transaction(id);
        }
    }

    fn collect_objects(&mut self) {
        let mut live = BTreeSet::new();
        for process in self.processes.values() {
            live.extend(process.snapshot.states.values().copied());
        }
        for transaction in self.transactions.values() {
            if let Some(snapshot) = &transaction.snapshot {
                live.extend(snapshot.states.values().copied());
            }
        }
        self.objects.retain(|id, _| live.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_pid_boundary_rejects_connect_and_fork_without_mutation() {
        let mut manager = StateManager::new(1, "test-credential".into());
        let hello = Hello {
            version: PROTOCOL_VERSION,
            token: "test-credential".into(),
            role: ClientRole::Worker,
            adoption_ticket: None,
        };
        let (parent, _) = manager
            .connect(
                hello.clone(),
                PeerIdentity {
                    host_pid: 1,
                    birth: 1,
                },
            )
            .unwrap();
        manager
            .handle(
                parent,
                Request::RegisterModule {
                    module_id: 1,
                    schema: 1,
                },
            )
            .unwrap();
        manager
            .handle(
                parent,
                Request::DefineState {
                    module_id: 1,
                    name: "copy".into(),
                    initial: 10,
                    fork: ForkPolicy::Copy,
                },
            )
            .unwrap();
        manager.next_pid = i32::MAX as u32;
        let ticket = match manager
            .handle(parent, Request::PrepareFork { request_key: 1 })
            .unwrap()
        {
            Reply::ForkPrepared(ticket) => ticket,
            other => panic!("unexpected reply: {other:?}"),
        };
        assert_eq!(ticket.child.pid, i32::MAX as u32);
        assert_eq!(manager.next_pid, i32::MAX as u32 + 1);
        let before = manager.stats();
        let counters = (
            manager.next_client,
            manager.next_pid,
            manager.next_object,
            manager.next_transaction,
        );
        assert_eq!(
            manager
                .handle(parent, Request::PrepareFork { request_key: 2 })
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(
            manager
                .connect(
                    hello,
                    PeerIdentity {
                        host_pid: 2,
                        birth: 2
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(manager.stats(), before);
        assert_eq!(
            (
                manager.next_client,
                manager.next_pid,
                manager.next_object,
                manager.next_transaction
            ),
            counters
        );
        assert_eq!(manager.processes[&1].request_keys.len(), 1);
        assert!(!manager.processes[&1].request_keys.contains_key(&2));
    }
}
