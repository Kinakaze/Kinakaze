fn main() {
    let mut args = std::env::args().skip(1);
    let host_pid = args
        .next()
        .expect("pid")
        .parse::<i32>()
        .expect("numeric pid");
    let signal = args
        .next()
        .expect("signal")
        .parse::<i32>()
        .expect("numeric signal");
    println!("visible processes: {:#?}", kinakaze_vfs::job::processes());
    let pid = kinakaze_vfs::job::namespace_pid(host_pid as u32)
        .map(|pid| pid as i32)
        .unwrap_or(host_pid);
    println!("host pid {host_pid} maps to namespace pid {pid}");
    match kinakaze_vfs::job::kill(pid, signal) {
        Ok(()) => println!("sent signal {signal} to {pid}"),
        Err(error) => panic!("signal delivery failed: errno {error}"),
    }
}
