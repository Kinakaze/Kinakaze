#![cfg(windows)]

//! Exercise the real service and separate Windows workers. The ignored helper is
//! launched as a child test process so no test-runner process joins the worker Job.
use kinakaze_v2_host_win::{PipeConnection, ProcessHandle, random_token};
use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, ForkPolicy, Hello, PROTOCOL_VERSION, Reply, Request, Stats, WireRequest,
    WireResponse, read_frame, write_frame,
};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(10);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct OwnedChild {
    child: Arc<Mutex<Child>>,
    native: Option<ProcessHandle>,
    input: ChildStdin,
    events: mpsc::Receiver<String>,
    cancel_watchdog: mpsc::Sender<()>,
}

impl OwnedChild {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn owned test process");
        let native = ProcessHandle::open(child.id()).expect("pin owned test process");
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                // Ignore libtest's surrounding status output. Private ticket
                // payloads remain captured and never appear in assertion text.
                if let Some(event) = line.strip_prefix("V2TEST:") {
                    if send.send(event.to_owned()).is_err() {
                        break;
                    }
                } else if line == "READY" && send.send(line).is_err() {
                    break;
                }
            }
        });
        let child = Arc::new(Mutex::new(child));
        let watchdog_child = Arc::clone(&child);
        let (cancel_watchdog, canceled) = mpsc::channel();
        std::thread::spawn(move || {
            // Also bounds synchronous pipe reads if the implementation hangs.
            // This handle owns exactly the process launched above.
            if canceled.recv_timeout(Duration::from_secs(30)).is_err() {
                let _ = watchdog_child.lock().unwrap().kill();
            }
        });
        Self {
            child,
            native: Some(native),
            input,
            events,
            cancel_watchdog,
        }
    }

    fn event(&self) -> String {
        self.events
            .recv_timeout(DEADLINE)
            .expect("timely child event")
    }

    fn expect_event(&self, expected: &str) {
        assert!(self.event() == expected, "unexpected child event");
    }

    fn command(&mut self, command: &str) {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }

    fn wait_success(&mut self) {
        let native = self.native.take().expect("wait once");
        let (send, done) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(native.wait());
        });
        done.recv_timeout(DEADLINE)
            .expect("native child exit deadline")
            .unwrap();
        assert!(
            self.child.lock().unwrap().wait().unwrap().success(),
            "child failed"
        );
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.cancel_watchdog.send(());
        let mut child = self.child.lock().unwrap();
        // Child retains its own native handle; this cannot kill a reused PID.
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct Client {
    pipe: PipeConnection,
    sequence: u64,
}
impl Client {
    fn open(
        endpoint: &str,
        token: &str,
        role: ClientRole,
        ticket: Option<String>,
    ) -> (Self, WireResponse) {
        let mut client = Self {
            pipe: PipeConnection::connect(endpoint).unwrap(),
            sequence: 0,
        };
        let response = client.call(Request::Hello(Hello {
            version: PROTOCOL_VERSION,
            token: token.to_owned(),
            role,
            adoption_ticket: ticket,
        }));
        (client, response)
    }

    fn call(&mut self, request: Request) -> WireResponse {
        self.sequence += 1;
        write_frame(
            &mut self.pipe,
            &WireRequest {
                id: self.sequence,
                request,
            },
        )
        .unwrap();
        let response: WireResponse = read_frame(&mut self.pipe).unwrap();
        assert_eq!(response.id, self.sequence);
        response
    }

    fn ok(&mut self, request: Request) -> Reply {
        self.call(request).result.expect("successful RPC")
    }
}

struct Session {
    service: OwnedChild,
    endpoint: String,
    token: String,
    controller: Client,
}
impl Session {
    fn start() -> Self {
        let endpoint = format!(r"\\.\pipe\kinakaze-v2-test-{}", random_token().unwrap());
        let token = random_token().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_init"));
        command
            .args([
                "--pipe",
                &endpoint,
                "--controller-pid",
                &std::process::id().to_string(),
            ])
            .env("KINAKAZE_V2_TOKEN", &token);
        let service = OwnedChild::spawn(command);
        service.expect_event("READY");
        let (controller, response) = Client::open(&endpoint, &token, ClientRole::Controller, None);
        assert!(matches!(
            response.result,
            Ok(Reply::Hello { process: None, .. })
        ));
        Self {
            service,
            endpoint,
            token,
            controller,
        }
    }

    fn helper(&self, mode: &str, ticket: Option<&str>) -> OwnedChild {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "native_worker_helper",
                "--nocapture",
            ])
            .env("V2TEST_MODE", mode)
            .env("V2TEST_ENDPOINT", &self.endpoint)
            .env("V2TEST_TOKEN", &self.token);
        if let Some(ticket) = ticket {
            command.env("V2TEST_TICKET", ticket);
        }
        OwnedChild::spawn(command)
    }

    fn stats(&mut self) -> Stats {
        let Reply::Stats(stats) = self.controller.ok(Request::Stats) else {
            panic!("expected Stats")
        };
        stats
    }

    fn assert_reaped(&mut self) {
        // The native exit wait and the manager's wait thread are independent;
        // Stats is the only observation API, so bound this eventual observation.
        let deadline = Instant::now() + DEADLINE;
        loop {
            let stats = self.stats();
            if stats.processes == 0 {
                assert_eq!(
                    (stats.objects, stats.transactions, stats.clients),
                    (0, 0, 1)
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "manager did not reap native exits"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn shutdown(mut self) {
        assert_eq!(self.controller.ok(Request::Shutdown), Reply::Ok);
        self.service.wait_success();
    }
}

#[test]
fn controller_role_is_bound_to_its_native_process() {
    let mut session = Session::start();
    let mut impostor = session.helper("impostor", None);
    impostor.expect_event("UNAUTHORIZED");
    impostor.wait_success();
    assert_eq!(
        session.stats(),
        Stats {
            processes: 0,
            objects: 0,
            transactions: 0,
            clients: 1
        }
    );
    session.shutdown();
}

#[test]
fn duplicate_sequence_cannot_mutate_and_eof_is_not_process_exit() {
    let mut session = Session::start();
    let mut worker = session.helper("duplicate", None);
    worker.expect_event("DETACHED");
    assert_eq!(
        session.stats(),
        Stats {
            processes: 1,
            objects: 1,
            transactions: 0,
            clients: 1
        }
    );
    worker.command("EXIT");
    worker.wait_success();
    session.assert_reaped();
    session.shutdown();
}

#[test]
fn awaiting_activation_wakes_on_commit_and_abort() {
    for commit in [true, false] {
        let mut session = Session::start();
        let mut parent = session.helper("parent", None);
        let prepared = parent.event();
        let mut parts = prepared.split_whitespace();
        assert!(
            parts.next() == Some("FORK"),
            "expected private fork ticket event"
        );
        let transaction: u64 = parts.next().unwrap().parse().unwrap();
        let token = parts.next().expect("adoption token");
        assert!(parts.next().is_none());
        let mut child = session.helper("adopted", Some(token));
        child.expect_event("AWAIT_SENT");
        assert!(
            matches!(
                child.events.recv_timeout(Duration::from_millis(150)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "activation returned before resolution"
        );
        parent.command(&format!(
            "{} {transaction}",
            if commit { "COMMIT" } else { "ABORT" }
        ));
        parent.expect_event("RESOLVED");
        child.expect_event(if commit { "ACTIVATED" } else { "ABORTED" });
        child.command("EXIT");
        child.wait_success();
        parent.command("EXIT");
        parent.wait_success();
        session.assert_reaped();
        session.shutdown();
    }
}

fn event(value: &str) {
    println!("V2TEST:{value}");
    std::io::stdout().flush().unwrap();
}

fn input() -> String {
    let mut input = String::new();
    assert!(
        std::io::stdin().read_line(&mut input).unwrap() > 0,
        "controller input closed"
    );
    input.trim().to_owned()
}

#[test]
#[ignore = "subprocess entry point used by native lifecycle tests"]
fn native_worker_helper() {
    let Ok(mode) = std::env::var("V2TEST_MODE") else {
        return;
    };
    let endpoint = std::env::var("V2TEST_ENDPOINT").unwrap();
    let token = std::env::var("V2TEST_TOKEN").unwrap();
    let role = if mode == "impostor" {
        ClientRole::Controller
    } else {
        ClientRole::Worker
    };
    let (mut client, response) =
        Client::open(&endpoint, &token, role, std::env::var("V2TEST_TICKET").ok());
    if mode == "impostor" {
        assert_eq!(response.result.unwrap_err().code, ErrorCode::Unauthorized);
        event("UNAUTHORIZED");
        return;
    }
    assert!(matches!(
        response.result,
        Ok(Reply::Hello {
            process: Some(_),
            ..
        })
    ));
    match mode.as_str() {
        "duplicate" => {
            assert_eq!(
                client.ok(Request::RegisterModule {
                    module_id: 1,
                    schema: 1
                }),
                Reply::Ok
            );
            assert!(matches!(
                client.ok(Request::DefineState {
                    module_id: 1,
                    name: "native-retention".into(),
                    initial: 41,
                    fork: ForkPolicy::Copy,
                }),
                Reply::State { value: 41, .. }
            ));
            // A duplicate PrepareFork would allocate a transaction and snapshot
            // if dispatched. The controller later checks neither exists.
            write_frame(
                &mut client.pipe,
                &WireRequest {
                    id: client.sequence,
                    request: Request::PrepareFork { request_key: 88 },
                },
            )
            .unwrap();
            let rejected: WireResponse = read_frame(&mut client.pipe).unwrap();
            assert_eq!(rejected.result.unwrap_err().code, ErrorCode::InvalidRequest);
            assert!(
                read_frame::<_, WireResponse>(&mut client.pipe).is_err(),
                "sequence violation must close session"
            );
            drop(client);
            event("DETACHED");
            assert_eq!(input(), "EXIT");
        }
        "parent" => {
            let Reply::ForkPrepared(ticket) = client.ok(Request::PrepareFork { request_key: 1 })
            else {
                panic!("expected fork reservation")
            };
            event(&format!("FORK {} {}", ticket.transaction, ticket.token));
            let command = input();
            let mut parts = command.split_whitespace();
            let operation = parts.next().unwrap();
            let transaction = parts.next().unwrap().parse().unwrap();
            let request = match operation {
                "COMMIT" => Request::CommitFork { transaction },
                "ABORT" => Request::AbortFork { transaction },
                _ => panic!("unexpected parent command"),
            };
            assert_eq!(client.ok(request), Reply::Ok);
            event("RESOLVED");
            assert_eq!(input(), "EXIT");
        }
        "adopted" => {
            assert_eq!(client.ok(Request::MarkReady), Reply::Ok);
            client.sequence += 1;
            write_frame(
                &mut client.pipe,
                &WireRequest {
                    id: client.sequence,
                    request: Request::AwaitActivation,
                },
            )
            .unwrap();
            event("AWAIT_SENT");
            let response: WireResponse = read_frame(&mut client.pipe).unwrap();
            assert_eq!(response.id, client.sequence);
            match response.result {
                Ok(Reply::Ok) => event("ACTIVATED"),
                Err(error) if error.code == ErrorCode::Aborted => event("ABORTED"),
                _ => panic!("unexpected activation result"),
            }
            assert_eq!(input(), "EXIT");
        }
        _ => panic!("unexpected helper mode"),
    }
}
