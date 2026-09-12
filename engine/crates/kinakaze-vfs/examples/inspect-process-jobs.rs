//! Read-only shared process identity/notification snapshots for explicit hosts.
fn main() {
    use kinakaze_runtime::job;
    for argument in std::env::args().skip(1) {
        let host: u32 = argument.parse().expect("host PID");
        let entry = job::namespace_pid(host).and_then(job::lookup);
        if let Some(e) = entry {
            let target = job::resolve(e.namespace_pid).unwrap_or(e);
            println!(
                "{{\"host\":{},\"pid\":{},\"parent\":{},\"group\":{},\"flags\":{},\"delegate\":{},\"resolvedHost\":{},\"resolvedPid\":{},\"pending\":{},\"visibleSelf\":{}}}",
                host,
                e.namespace_pid,
                e.ppid,
                e.pgid,
                e.flags,
                e.delegate,
                target.pid,
                target.namespace_pid,
                job::notifications::has_pending(target.namespace_pid),
                job::namespaces::visible_from(e.namespace_pid, e.namespace_pid).unwrap_or(0)
            );
        } else {
            println!("{{\"host\":{host},\"missing\":true}}");
        }
    }
}
