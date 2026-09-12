//! Descriptor-only spawn of an ELF, using the ordinary init identity transaction.
//! No guest code or parent address-space data executes in the fresh worker.
use super::super::{
    SpawnedProcess, exec_host_path, loader_path, register_waitable_child, spawn_suspended_exec,
    string_vector, trace_spawn_phase,
};
use super::*;

struct Handoff(bool);
impl Drop for Handoff {
    fn drop(&mut self) {
        if self.0 {
            kinakaze_vfs::finish_exec_state(None);
        }
    }
}

struct Candidate {
    child: Option<SpawnedProcess>,
    guest_pid: Option<u32>,
}
impl Drop for Candidate {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(pid) = self.guest_pid {
                kinakaze_runtime::discard_uncommitted_child(pid);
            }
        }
    }
}

/// Unsupported formats or standalone runtime domains keep the general fork path.
pub(super) unsafe fn launch(
    pid: *mut c_int,
    file: &CStr,
    attr: &PosixSpawnAttr,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> Option<c_int> {
    if kinakaze_runtime::authority::get().is_none() {
        return None;
    }
    let path = file.to_str().ok()?;
    let host_path = exec_host_path(path).ok()?;
    let image = kinakaze_vfs::read_guest_image(&host_path).ok()?;
    if kinakaze_elf::ElfFile::parse(&image).is_err() {
        return None;
    }
    let started = std::time::Instant::now();
    for attempt in 0..3 {
        let mut native_created = false;
        let mut stage = "arguments";
        let result = (|| -> Result<u32, c_int> {
            let arguments = unsafe { string_vector(argv) }?;
            let environment = unsafe { string_vector(envp) }?;
            stage = "task-limit";
            let limit = kinakaze_runtime::services::task_creation_errno();
            if limit != 0 {
                return Err(limit);
            }
            if kinakaze_runtime::job::namespaces::child_namespace_dead() {
                return Err(kinakaze_vfs::ENOMEM);
            }
            let _image = kinakaze_runtime::lock_process_image();
            stage = "identity-reservation";
            let mut transaction = kinakaze_runtime::authority::ForkTransaction::prepare(None)?;
            let reservation = *transaction.reservation().ok_or(kinakaze_vfs::EIO)?;
            let ticket = core::str::from_utf8(&reservation.token).map_err(|_| kinakaze_vfs::EIO)?;
            let mask = (attr.flags & SPAWN_SETSIGMASK != 0).then_some(attr.sigmask.bits[0]);
            let defaults = if attr.flags & SPAWN_SETSIGDEF != 0 {
                attr.sigdefault.bits[0]
            } else {
                0
            };
            stage = "descriptor-snapshot";
            let descriptors = kinakaze_vfs::exec_descriptor_snapshot()?;
            stage = "state-serialization";
            let payload =
                kinakaze_vfs::prepare_spawn_state_from_image(&environment, &image, mask, defaults)?;
            let mut handoff_owner = Handoff(true);
            trace_spawn_phase(&started, "posix_spawn", "handoff-serialized");
            let loader = loader_path().map_err(|_| kinakaze_vfs::EIO)?;
            let cwd = kinakaze_vfs::resolve_linux_path(&kinakaze_vfs::fs::getcwd())
                .ok()
                .filter(|p| p.is_dir());
            let mut command = vec![
                loader.display().to_string(),
                "--kinakaze-exec".to_owned(),
                host_path.to_string_lossy().into_owned(),
            ];
            if arguments.is_empty() {
                command.push(path.to_owned());
            } else {
                command.extend(arguments);
            }
            stage = "native-process-create";
            let child = spawn_suspended_exec(
                loader,
                &command,
                cwd.as_deref(),
                Some(ticket),
                Some(&descriptors),
            )
            .map_err(|error| {
                if error.raw_os_error() == Some(1237) {
                    kinakaze_vfs::EAGAIN
                } else {
                    kinakaze_vfs::errno_from_win32(error.raw_os_error().unwrap_or(1) as u32)
                }
            })?;
            native_created = true;
            let mut candidate = Candidate {
                child: Some(child),
                guest_pid: None,
            };
            let child = candidate.child.as_mut().unwrap();
            let native_pid = child.id();
            stage = "handoff-publication";
            let handoff =
                kinakaze_vfs::stage_exec_handoff(native_pid, &payload).ok_or(kinakaze_vfs::EIO)?;
            stage = "process-registration";
            let guest_pid = kinakaze_runtime::job::register_forked_child(native_pid)
                .ok_or(kinakaze_vfs::EIO)?;
            candidate.guest_pid = Some(guest_pid);
            if guest_pid != reservation.child.pid || !register_waitable_child(guest_pid, child) {
                return Err(kinakaze_vfs::EIO);
            }
            // A successful spawn is already an exec for setpgid permission checks.
            kinakaze_runtime::job::update_flags(guest_pid, kinakaze_runtime::job::FLAG_EXECED, 0);
            stage = "resume";
            child.resume()?;
            trace_spawn_phase(&started, "posix_spawn", "process-resumed");
            stage = "handoff-ownership";
            if !handoff.wait_until_owned(child) {
                return Err(kinakaze_vfs::EIO);
            }
            stage = "socket-handoff";
            kinakaze_vfs::finish_exec_state(Some(native_pid));
            handoff_owner.0 = false;
            stage = "commit";
            transaction.commit()?;
            trace_spawn_phase(&started, "posix_spawn", "committed");
            candidate.child.take(); // The wait coordinator retains its own handle.
            Ok(guest_pid)
        })();
        if let Err(error) = result {
            if super::super::spawn_trace_enabled() {
                eprintln!(
                    "kinakaze spawn: pid={} op=posix_spawn phase={stage} error={error}",
                    std::process::id()
                );
            }
        }
        if let Err(error) = result {
            if !native_created {
                // A sibling may close/reuse a descriptor while side tables are
                // serialized. Nothing has run in a child: refresh the entire
                // snapshot, with a bounded retry, then retain the full fork path.
                if attempt < 2
                    && matches!(stage, "state-serialization" | "native-process-create")
                    && matches!(error, kinakaze_vfs::EBADF | kinakaze_vfs::EAGAIN)
                {
                    continue;
                }
                return None;
            }
        }
        return Some(match result {
            Ok(child) => {
                if !pid.is_null() {
                    unsafe {
                        *pid = child as c_int;
                    }
                }
                0
            }
            Err(error) => error,
        });
    }
    unreachable!("the bounded spawn loop returns or retries")
}
