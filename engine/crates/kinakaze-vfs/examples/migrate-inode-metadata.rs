//! Offline, explicit conversion; never invoked as a runtime fallback.
#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 || (args[1] != "--check" && args[1] != "--apply") {
        eprintln!(
            "Usage: migrate-inode-metadata --check|--apply <absolute directory>\nStop all writers before --apply; do not start the new runtime until the entire pass succeeds. Legacy streams are retained as backups."
        );
        return std::process::ExitCode::from(2);
    }
    match kinakaze_vfs::fs::migrate_inode_metadata(
        std::path::Path::new(&args[2]),
        args[1] == "--apply",
    ) {
        Ok(report) => {
            println!("{report:?}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Migration incomplete: Linux errno {error}. Do not start the new runtime.");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(not(windows))]
fn main() {
    panic!("Native inode metadata migration requires Windows");
}
