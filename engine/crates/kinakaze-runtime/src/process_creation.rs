//! Console policy shared by fork and exec's native bootstrap processes.
use windows_sys::Win32::Foundation::{ERROR_INVALID_HANDLE, GetLastError};
use windows_sys::Win32::System::Console::{GetConsoleCP, GetConsoleMode};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};

/// Avoid creating a hidden console for a parent that has no console at all.
/// An attached parent keeps the existing policy even if all three standard
/// streams are redirected: other inherited guest descriptors may still refer
/// to its console. Re-evaluate on each launch; attachment can change.
pub fn child_creation_flags(standard_handles: [usize; 3]) -> u32 {
    // SAFETY: both calls query this process's console state and take no pointers.
    if unsafe { GetConsoleCP() } != 0 || unsafe { GetLastError() } != ERROR_INVALID_HANDLE {
        return CREATE_NO_WINDOW;
    }
    for handle in standard_handles {
        let mut mode = 0;
        // SAFETY: GetConsoleMode validates opaque handles; mode is writable.
        if unsafe { GetConsoleMode(handle as _, &mut mode) } != 0 {
            return CREATE_NO_WINDOW;
        }
    }
    DETACHED_PROCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    #[test]
    fn creation_policy_helper() {
        let Ok(expected) = std::env::var("KINAKAZE_CREATION_POLICY_TEST") else {
            return;
        };
        let handles = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|which| unsafe { GetStdHandle(which) } as usize);
        assert_eq!(
            child_creation_flags(handles),
            expected.parse::<u32>().unwrap()
        );
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        assert_eq!(input, "inherited pipe input");
        println!("CREATION_POLICY_STDOUT_OK");
        eprintln!("CREATION_POLICY_STDERR_OK");
    }

    #[test]
    fn detached_and_attached_parents_keep_redirected_stdio() {
        for flags in [DETACHED_PROCESS, CREATE_NO_WINDOW] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "process_creation::tests::creation_policy_helper",
                    "--nocapture",
                ])
                .env("KINAKAZE_CREATION_POLICY_TEST", flags.to_string())
                .creation_flags(flags)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"inherited pipe input")
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("CREATION_POLICY_STDOUT_OK"));
            assert!(String::from_utf8_lossy(&output.stderr).contains("CREATION_POLICY_STDERR_OK"));
        }
    }
}
