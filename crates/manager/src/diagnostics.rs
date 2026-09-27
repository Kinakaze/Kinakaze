//! Read-only control-plane views; no credentials, tickets or state payloads.
use super::{PrewarmState, ProcessState, StateManager};
use kinakaze_v2_protocol::{ProcessIdentity, Stats};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Active,
    Pending,
    Aborted,
    ExecRetiring,
}

#[derive(Debug, Serialize)]
pub struct ProcessSnapshot {
    pub identity: ProcessIdentity,
    pub host_pid: u32,
    // Keep the full native identity for safe host sampling, not JavaScript numbers.
    #[serde(skip)]
    pub host_birth: u64,
    pub status: ProcessStatus,
    pub standby: bool,
    /// Initial executable only; arguments and environment may contain secrets.
    pub program: Option<String>,
    pub modules: usize,
    pub states: usize,
}

#[derive(Debug, Serialize)]
pub struct ManagerSnapshot {
    pub stats: Stats,
    pub processes: Vec<ProcessSnapshot>,
}

impl StateManager {
    /// Capture a consistent bounded view while the caller holds the manager lock.
    /// Serialization and native resource sampling must happen after releasing it.
    pub fn snapshot(&self) -> ManagerSnapshot {
        ManagerSnapshot {
            stats: self.stats(),
            processes: self
                .processes
                .values()
                .map(|process| ProcessSnapshot {
                    identity: process.identity,
                    host_pid: process.peer.host_pid,
                    host_birth: process.peer.birth,
                    status: match process.state {
                        ProcessState::Active if self.retired_peers.contains(&process.peer) => {
                            ProcessStatus::ExecRetiring
                        }
                        ProcessState::Active => ProcessStatus::Active,
                        ProcessState::Pending(_) => ProcessStatus::Pending,
                        ProcessState::Aborted => ProcessStatus::Aborted,
                    },
                    standby: matches!(
                        process.prewarm,
                        PrewarmState::Ready
                            | PrewarmState::PoolReady
                            | PrewarmState::PoolReserved(_)
                    ),
                    program: match &process.prewarm {
                        PrewarmState::PoolActivated { launch, .. } => {
                            launch.arguments.first().cloned()
                        }
                        _ => None,
                    },
                    modules: process.snapshot.modules.len(),
                    states: process.snapshot.states.len(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PeerIdentity;
    use kinakaze_v2_protocol::{ClientRole, Hello, PROTOCOL_VERSION};

    #[test]
    fn pool_diagnostics_distinguish_standby_without_exposing_launch_secrets() {
        use kinakaze_v2_protocol::{PoolLaunch, Reply, Request};
        let mut manager = StateManager::new(1, "credential".into());
        let connect = |manager: &mut StateManager, role, host_pid| {
            manager
                .connect(
                    Hello {
                        version: PROTOCOL_VERSION,
                        token: "credential".into(),
                        role,
                        adoption_ticket: None,
                    },
                    PeerIdentity {
                        host_pid,
                        birth: u64::MAX - 1,
                    },
                )
                .unwrap()
                .0
        };
        let worker = connect(&mut manager, ClientRole::Worker, 42);
        manager.handle(worker, Request::MarkPoolReady).unwrap();
        assert!(manager.snapshot().processes[0].standby);
        let launcher = connect(&mut manager, ClientRole::Launcher, 43);
        let Reply::PoolWorker { pid, .. } = manager
            .handle(launcher, Request::ReservePoolWorker)
            .unwrap()
        else {
            panic!("pool worker expected")
        };
        assert!(manager.snapshot().processes[0].standby);
        manager
            .handle(
                launcher,
                Request::ActivatePoolWorker {
                    pid,
                    launch: PoolLaunch {
                        arguments: vec!["/bin/example".into(), "private-argument".into()],
                        cwd: "/private-cwd".into(),
                        environment: Some(vec!["TOKEN=private-environment".into()]),
                    },
                },
            )
            .unwrap();
        manager.disconnect(launcher);
        let snapshot = manager.snapshot();
        assert!(!snapshot.processes[0].standby);
        assert_eq!(
            snapshot.processes[0].program.as_deref(),
            Some("/bin/example")
        );
        assert_eq!(snapshot.processes[0].host_birth, u64::MAX - 1);
        let view = format!("{snapshot:?}");
        for private in [
            "credential",
            "private-argument",
            "private-cwd",
            "private-environment",
        ] {
            assert!(!view.contains(private));
        }
    }

    #[test]
    fn snapshot_follows_native_lifetime_not_connection_lifetime() {
        let mut manager = StateManager::new(1, "test-secret".into());
        let peer = PeerIdentity {
            host_pid: 42,
            birth: 100,
        };
        let (client, _) = manager
            .connect(
                Hello {
                    version: PROTOCOL_VERSION,
                    token: "test-secret".into(),
                    role: ClientRole::Worker,
                    adoption_ticket: None,
                },
                peer,
            )
            .unwrap();
        manager.disconnect(client);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.stats.processes, 1);
        assert_eq!(snapshot.processes[0].host_pid, 42);
        assert_eq!(snapshot.processes[0].host_birth, 100);
        assert_eq!(snapshot.processes[0].status, ProcessStatus::Active);
        manager.process_exited(PeerIdentity {
            host_pid: 42,
            birth: 99,
        });
        assert_eq!(manager.snapshot().processes.len(), 1);
        manager.process_exited(peer);
        assert!(manager.snapshot().processes.is_empty());
    }
}
