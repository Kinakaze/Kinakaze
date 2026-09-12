//! Read-only PID translation for bounded host-driven guest diagnostics.
//! The caller must retain its native process handle to exclude host PID reuse.
fn main() -> std::process::ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(host_pid) = arguments
        .first()
        .filter(|_| arguments.len() == 1)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|&pid| pid != 0)
    else {
        eprintln!("usage: resolve-guest-pid <live-host-pid>");
        return std::process::ExitCode::from(2);
    };
    match kinakaze_runtime::job::namespace_pid(host_pid) {
        Some(pid) => {
            println!("{pid}");
            std::process::ExitCode::SUCCESS
        }
        None => {
            eprintln!("no registered guest for host pid {host_pid}");
            std::process::ExitCode::from(1)
        }
    }
}
