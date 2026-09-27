use kinakaze_v2_manager::{PeerIdentity, StateManager};
use kinakaze_v2_protocol::{ClientRole, ErrorCode, Hello, PROTOCOL_VERSION};
fn hello() -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        role: ClientRole::Worker,
        token: "tree".into(),
        adoption_ticket: None,
    }
}
#[test]
fn a_process_tree_never_accepts_a_second_unparented_worker() {
    let mut tree = StateManager::for_process_tree(1, "tree".into());
    let first = PeerIdentity {
        host_pid: 11,
        birth: 101,
    };
    tree.connect(hello(), first).unwrap();
    let second = PeerIdentity {
        host_pid: 12,
        birth: 102,
    };
    assert_eq!(
        tree.connect(hello(), second).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    tree.process_exited_with_status(first, 0);
    assert_eq!(
        tree.connect(hello(), second).unwrap_err().code,
        ErrorCode::Unauthorized
    );
}
