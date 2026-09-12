//! POSIX spawn attributes, file actions and managed fork/exec.
//! Published storage is guest-owned; no parent Rust allocation survives fork.
use super::{kinakaze_abi_execve, kinakaze_abi_waitpid};
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use kinakaze_vfs::EINVAL;
mod fresh;
mod storage;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SchedParam {
    pub sched_priority: c_int,
}

#[repr(C)]
#[derive(Clone)]
pub struct PosixSpawnAttr {
    pub flags: i16,
    pub pgroup: c_int,
    pub sigdefault: crate::signal::SigSet,
    pub sigmask: crate::signal::SigSet,
    pub schedparam: SchedParam,
    pub schedpolicy: c_int,
    // Linux x86-64 glibc keeps this inside the former int __pad[16].
    // Preserve its slot even when POSIX_SPAWN_SETCGROUP is not selected.
    pub cgroup: c_int,
    pub _pad: [c_int; 15],
}

#[repr(C)]
pub struct PosixSpawnFileActions {
    pub allocated: c_int,
    pub used: c_int,
    pub actions: *mut c_void,
    pub _pad: [c_int; 16],
}

#[derive(Clone)]
pub enum FileAction {
    Close(c_int),
    Dup2(c_int, c_int),
    Open {
        fd: c_int,
        path: std::ffi::CString,
        oflag: c_int,
        mode: u32,
    },
    Chdir(std::ffi::CString),
    Fchdir(c_int),
    CloseFrom(c_int),
    Tcsetpgrp(c_int),
}

const SPAWN_RESETIDS: i16 = 0x01;
const SPAWN_SETPGROUP: i16 = 0x02;
const SPAWN_SETSIGDEF: i16 = 0x04;
const SPAWN_SETSIGMASK: i16 = 0x08;
const SPAWN_SETSCHEDPARAM: i16 = 0x10;
const SPAWN_SETSCHEDULER: i16 = 0x20;
const SPAWN_USEVFORK: i16 = 0x40;
const SPAWN_SETSID: i16 = 0x80;
const SPAWN_FLAGS: i16 = SPAWN_RESETIDS
    | SPAWN_SETPGROUP
    | SPAWN_SETSIGDEF
    | SPAWN_SETSIGMASK
    | SPAWN_SETSCHEDPARAM
    | SPAWN_SETSCHEDULER
    | SPAWN_USEVFORK
    | SPAWN_SETSID;

fn spawn_error() -> c_int {
    match kinakaze_tls::errno() {
        error if error > 0 => error,
        _ => kinakaze_vfs::EIO,
    }
}

fn spawn_fd_valid(fd: c_int) -> Result<(), c_int> {
    if fd < 0 {
        return Err(kinakaze_vfs::EBADF);
    }
    if fd as usize >= kinakaze_vfs::job::current_nofile_limit()? {
        return Err(kinakaze_vfs::EBADF);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_init(
    attr: *mut PosixSpawnAttr,
) -> c_int {
    if attr.is_null() {
        return EINVAL;
    }
    unsafe { ptr::write_bytes(attr, 0, 1) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_destroy(
    attr: *mut PosixSpawnAttr,
) -> c_int {
    if attr.is_null() { EINVAL } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_setflags(
    attr: *mut PosixSpawnAttr,
    flags: i16,
) -> c_int {
    if attr.is_null() || flags & !SPAWN_FLAGS != 0 {
        return EINVAL;
    }
    unsafe { (*attr).flags = flags };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_setpgroup(
    attr: *mut PosixSpawnAttr,
    pgroup: c_int,
) -> c_int {
    if attr.is_null() {
        return EINVAL;
    }
    unsafe { (*attr).pgroup = pgroup };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_setsigmask(
    attr: *mut PosixSpawnAttr,
    sigmask: *const crate::signal::SigSet,
) -> c_int {
    if attr.is_null() || sigmask.is_null() {
        return EINVAL;
    }
    unsafe { (*attr).sigmask = *sigmask };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_setsigdefault(
    attr: *mut PosixSpawnAttr,
    sigdefault: *const crate::signal::SigSet,
) -> c_int {
    if attr.is_null() || sigdefault.is_null() {
        return EINVAL;
    }
    unsafe { (*attr).sigdefault = *sigdefault };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_setschedparam(
    attr: *mut PosixSpawnAttr,
    schedparam: *const SchedParam,
) -> c_int {
    if attr.is_null() || schedparam.is_null() {
        return EINVAL;
    }
    unsafe { (*attr).schedparam = *schedparam };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnattr_setschedpolicy(
    attr: *mut PosixSpawnAttr,
    schedpolicy: c_int,
) -> c_int {
    if attr.is_null() || !matches!(schedpolicy, 0..=2) {
        return EINVAL;
    }
    unsafe { (*attr).schedpolicy = schedpolicy };
    0
}

macro_rules! spawn_attr_getter {
    ($name:ident, $field:ident, $ty:ty) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $name(
            attr: *const PosixSpawnAttr,
            result: *mut $ty,
        ) -> c_int {
            if attr.is_null() || result.is_null() {
                return EINVAL;
            }
            // SAFETY: the ABI caller supplies a readable attribute and writable result.
            unsafe {
                *result = (*attr).$field;
            }
            0
        }
    };
}

spawn_attr_getter!(kinakaze_abi_posix_spawnattr_getflags, flags, i16);
spawn_attr_getter!(kinakaze_abi_posix_spawnattr_getpgroup, pgroup, c_int);
spawn_attr_getter!(
    kinakaze_abi_posix_spawnattr_getsigmask,
    sigmask,
    crate::signal::SigSet
);
spawn_attr_getter!(
    kinakaze_abi_posix_spawnattr_getsigdefault,
    sigdefault,
    crate::signal::SigSet
);
spawn_attr_getter!(
    kinakaze_abi_posix_spawnattr_getschedparam,
    schedparam,
    SchedParam
);
spawn_attr_getter!(
    kinakaze_abi_posix_spawnattr_getschedpolicy,
    schedpolicy,
    c_int
);

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_init(
    fa: *mut PosixSpawnFileActions,
) -> c_int {
    if fa.is_null() {
        return EINVAL;
    }
    unsafe {
        ptr::write_bytes(fa, 0, 1);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_destroy(
    fa: *mut PosixSpawnFileActions,
) -> c_int {
    if fa.is_null() {
        return EINVAL;
    }
    unsafe { storage::destroy(&mut *fa) }.map_or_else(|error| error, |()| 0)
}

unsafe fn append_spawn_action(fa: *mut PosixSpawnFileActions, action: FileAction) -> c_int {
    if fa.is_null() {
        return EINVAL;
    }
    unsafe { storage::append(&mut *fa, &action) }.map_or_else(|error| error, |()| 0)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_addopen(
    fa: *mut PosixSpawnFileActions,
    fd: c_int,
    path: *const c_char,
    oflag: c_int,
    mode: u32,
) -> c_int {
    if fa.is_null() || path.is_null() {
        return EINVAL;
    }
    if let Err(error) = spawn_fd_valid(fd) {
        return error;
    }
    let path = unsafe { CStr::from_ptr(path) }.to_owned();
    unsafe {
        append_spawn_action(
            fa,
            FileAction::Open {
                fd,
                path,
                oflag,
                mode,
            },
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_addclose(
    fa: *mut PosixSpawnFileActions,
    fd: c_int,
) -> c_int {
    if fa.is_null() {
        return EINVAL;
    }
    if let Err(error) = spawn_fd_valid(fd) {
        return error;
    }
    unsafe { append_spawn_action(fa, FileAction::Close(fd)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_adddup2(
    fa: *mut PosixSpawnFileActions,
    fd: c_int,
    newfd: c_int,
) -> c_int {
    if fa.is_null() {
        return EINVAL;
    }
    if let Err(error) = spawn_fd_valid(fd).and_then(|()| spawn_fd_valid(newfd)) {
        return error;
    }
    unsafe { append_spawn_action(fa, FileAction::Dup2(fd, newfd)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_addchdir_np(
    fa: *mut PosixSpawnFileActions,
    path: *const c_char,
) -> c_int {
    if fa.is_null() || path.is_null() {
        return EINVAL;
    }
    let path = unsafe { CStr::from_ptr(path) }.to_owned();
    unsafe { append_spawn_action(fa, FileAction::Chdir(path)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_addfchdir_np(
    fa: *mut PosixSpawnFileActions,
    fd: c_int,
) -> c_int {
    if let Err(error) = spawn_fd_valid(fd) {
        return error;
    }
    unsafe { append_spawn_action(fa, FileAction::Fchdir(fd)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_addclosefrom_np(
    fa: *mut PosixSpawnFileActions,
    from: c_int,
) -> c_int {
    if from < 0 {
        return EINVAL;
    }
    unsafe { append_spawn_action(fa, FileAction::CloseFrom(from)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn_file_actions_addtcsetpgrp_np(
    fa: *mut PosixSpawnFileActions,
    fd: c_int,
) -> c_int {
    if let Err(error) = spawn_fd_valid(fd) {
        return error;
    }
    unsafe { append_spawn_action(fa, FileAction::Tcsetpgrp(fd)) }
}

fn spawn_scheduler_check(attr: &PosixSpawnAttr) -> Result<(), c_int> {
    if attr.flags & (SPAWN_SETSCHEDPARAM | SPAWN_SETSCHEDULER) == 0 {
        return Ok(());
    }
    let policy = if attr.flags & SPAWN_SETSCHEDULER != 0 {
        attr.schedpolicy
    } else {
        0
    };
    match (policy, attr.schedparam.sched_priority) {
        // Managed guest threads currently use the native ordinary scheduler.
        // Priority zero is its only POSIX sched_priority, so this requests the
        // already active policy. Do not call the legacy no-op sched setters.
        (0, 0) => Ok(()),
        // Realtime policy requests must not silently run as ordinary threads.
        // This runtime does not grant the host realtime scheduling privilege.
        (1 | 2, 1..=99) => Err(kinakaze_vfs::EPERM),
        _ => Err(EINVAL),
    }
}

fn spawn_apply_attributes(attr: &PosixSpawnAttr) -> Result<(), c_int> {
    use kinakaze_vfs::signal::{self, Action, Disposition};
    spawn_scheduler_check(attr)?;
    // Signals were blocked before fork. Reset caught dispositions before the
    // child mask is restored so a parent-installed handler cannot run in the
    // gap before exec. Ignored dispositions survive unless explicitly reset.
    for number in 1..signal::NSIG as c_int {
        if matches!(number, signal::SIGKILL | signal::SIGSTOP) {
            continue;
        }
        let explicit_default = attr.flags & SPAWN_SETSIGDEF != 0
            && attr.sigdefault.bits[0] & (1u64 << (number - 1)) != 0;
        if explicit_default
            || matches!(
                signal::current_action(number)?.disposition,
                Disposition::Handle(_, _)
            )
        {
            signal::sigaction(number, Some(Action::default()))?;
        }
    }
    if attr.flags & SPAWN_SETSID != 0 && crate::userdb::kinakaze_abi_setsid() < 0 {
        return Err(spawn_error());
    }
    if attr.flags & SPAWN_SETPGROUP != 0 && crate::userdb::kinakaze_abi_setpgid(0, attr.pgroup) < 0
    {
        return Err(spawn_error());
    }
    if attr.flags & SPAWN_RESETIDS != 0 {
        if crate::userdb::kinakaze_abi_seteuid(crate::userdb::kinakaze_abi_getuid()) < 0
            || crate::userdb::kinakaze_abi_setegid(crate::userdb::kinakaze_abi_getgid()) < 0
        {
            return Err(spawn_error());
        }
    }
    Ok(())
}

fn spawn_apply_file_actions(actions: &[FileAction], error_fd: c_int) -> Result<(), c_int> {
    for action in actions {
        let result = match action {
            FileAction::Close(fd) => {
                spawn_fd_valid(*fd)?;
                // GNU spawn permits closing a valid-numbered, already closed fd.
                crate::kinakaze_abi_close(*fd);
                0
            }
            FileAction::Dup2(fd, newfd) if fd == newfd => {
                // Unlike dup2(fd, fd), the spawn action clears FD_CLOEXEC.
                let flags = unsafe { crate::fdio::kinakaze_abi_fcntl64(*fd, 1, 0) };
                if flags < 0 {
                    return Err(spawn_error());
                }
                unsafe { crate::fdio::kinakaze_abi_fcntl64(*fd, 2, (flags & !1) as usize) }
            }
            FileAction::Dup2(fd, newfd) => crate::fdio::kinakaze_abi_dup2(*fd, *newfd),
            FileAction::Open {
                fd,
                path,
                oflag,
                mode,
            } => {
                spawn_fd_valid(*fd)?;
                crate::kinakaze_abi_close(*fd);
                let opened =
                    unsafe { crate::fsextra::kinakaze_abi_open64(path.as_ptr(), *oflag, *mode) };
                if opened < 0 {
                    return Err(spawn_error());
                }
                if opened != *fd {
                    let duplicated = crate::fdio::kinakaze_abi_dup2(opened, *fd);
                    let error = spawn_error();
                    let closed = crate::kinakaze_abi_close(opened);
                    if duplicated < 0 {
                        return Err(error);
                    }
                    if closed < 0 {
                        return Err(spawn_error());
                    }
                }
                0
            }
            FileAction::Chdir(path) => unsafe {
                crate::fs::exports::kinakaze_abi_chdir(path.as_ptr())
            },
            FileAction::Fchdir(fd) => crate::fdio::kinakaze_abi_fchdir(*fd),
            FileAction::CloseFrom(from) => {
                // This private CLOEXEC channel must survive until exec/error.
                // Existing entries can exceed a subsequently lowered soft limit.
                for fd in kinakaze_vfs::list_open_fds() {
                    if fd >= *from && fd != error_fd {
                        crate::kinakaze_abi_close(fd);
                    }
                }
                0
            }
            FileAction::Tcsetpgrp(fd) => {
                let pgroup = crate::userdb::kinakaze_abi_getpgrp();
                if pgroup < 0 {
                    return Err(spawn_error());
                }
                crate::term::kinakaze_abi_tcsetpgrp(*fd, pgroup)
            }
        };
        if result < 0 {
            return Err(spawn_error());
        }
    }
    Ok(())
}

fn spawn_error_fd_floor(actions: &[FileAction]) -> Result<c_int, c_int> {
    let mut maximum = 2;
    for action in actions {
        match action {
            FileAction::Dup2(fd, newfd) => {
                spawn_fd_valid(*fd)?;
                spawn_fd_valid(*newfd)?;
                maximum = maximum.max(*fd).max(*newfd);
            }
            FileAction::Close(fd)
            | FileAction::Open { fd, .. }
            | FileAction::Fchdir(fd)
            | FileAction::Tcsetpgrp(fd) => {
                spawn_fd_valid(*fd)?;
                maximum = maximum.max(*fd);
            }
            FileAction::CloseFrom(from) if *from < 0 => return Err(EINVAL),
            FileAction::CloseFrom(_) | FileAction::Chdir(_) => {}
        }
    }
    maximum.checked_add(1).ok_or(kinakaze_vfs::EBADF)
}

fn spawn_candidates(file: &CStr, search_path: Option<&CStr>) -> Vec<std::ffi::CString> {
    let bytes = file.to_bytes();
    if search_path.is_none() || bytes.contains(&b'/') {
        return vec![file.to_owned()];
    }
    search_path
        .unwrap()
        .to_bytes()
        .split(|byte| *byte == b':')
        .map(|directory| {
            // An empty PATH component names the current directory. Resolve it only
            // in the child, after any chdir file actions have run.
            let mut candidate = Vec::with_capacity(directory.len() + bytes.len() + 2);
            if !directory.is_empty() {
                candidate.extend_from_slice(directory);
                candidate.push(b'/');
            }
            candidate.extend_from_slice(bytes);
            // Both inputs are CStr byte slices, so they contain no embedded NUL.
            std::ffi::CString::new(candidate).expect("CStr components contain no NUL")
        })
        .collect()
}

fn spawn_report_error(fd: c_int, error: c_int) -> ! {
    let payload = error.max(1).to_ne_bytes();
    let mut sent = 0;
    while sent < payload.len() {
        let count = unsafe {
            crate::kinakaze_abi_write(fd, payload.as_ptr().add(sent).cast(), payload.len() - sent)
        };
        if count > 0 {
            sent += count as usize;
        } else if count < 0 && kinakaze_tls::errno() == kinakaze_vfs::EINTR {
            continue;
        } else {
            break;
        }
    }
    crate::kinakaze_abi_close(fd);
    crate::process::kinakaze_abi__exit(127)
}

fn spawn_reap_failed(child: c_int) {
    loop {
        let result = unsafe { kinakaze_abi_waitpid(child, ptr::null_mut(), 0) };
        if result >= 0 || kinakaze_tls::errno() != kinakaze_vfs::EINTR {
            break;
        }
    }
}

/// Fork and exec share the runtime's existing logical process authority. The
/// CLOEXEC pipe distinguishes a failed child setup/exec from a successful exec;
/// native Windows PIDs never leak through the public result.
unsafe fn spawn_managed(
    pid: *mut c_int,
    file: *const c_char,
    file_actions: *const PosixSpawnFileActions,
    attrp: *const PosixSpawnAttr,
    argv: *const *const c_char,
    envp: *const *const c_char,
    search: bool,
) -> c_int {
    if file.is_null() || argv.is_null() {
        return EINVAL;
    }
    let file = unsafe { CStr::from_ptr(file) };
    if file.is_empty() {
        return kinakaze_vfs::ENOENT;
    }
    let attr: PosixSpawnAttr = if attrp.is_null() {
        // All-zero is the POSIX default, including empty signal sets.
        unsafe { core::mem::zeroed() }
    } else {
        unsafe { (*attrp).clone() }
    };
    if attr.flags & !SPAWN_FLAGS != 0 {
        return EINVAL;
    }
    if let Err(error) = spawn_scheduler_check(&attr) {
        return error;
    }
    // Fresh native workers can restore a descriptor-only launch directly.
    // File actions and process-group/scheduler changes retain the full path.
    if !search && file.to_bytes().starts_with(b"/") {
        let actions = match unsafe { storage::read(file_actions) } {
            Ok(actions) => actions,
            Err(error) => return error,
        };
        if actions.is_empty()
            && attr.flags & !(SPAWN_SETSIGDEF | SPAWN_SETSIGMASK | SPAWN_USEVFORK) == 0
        {
            if let Some(result) = unsafe { fresh::launch(pid, file, &attr, argv, envp) } {
                return result;
            }
        }
    }
    // Release temporary Rust objects before fork. The published action array
    // is guest-owned, and the child decodes its own copy after restoration.
    let floor = match unsafe { storage::read(file_actions) }
        .and_then(|actions| spawn_error_fd_floor(&actions))
    {
        Ok(floor) => floor,
        Err(error) => return error,
    };
    let path = if search {
        let value = unsafe { crate::process::kinakaze_abi_getenv(c"PATH".as_ptr()) };
        Some(if value.is_null() {
            c"/bin:/usr/bin"
        } else {
            unsafe { CStr::from_ptr(value) }
        })
    } else {
        None
    };
    let candidates = match storage::Candidates::new(&spawn_candidates(file, path)) {
        Ok(candidates) => candidates,
        Err(error) => return error,
    };

    let mut pipe = [-1; 2];
    if unsafe { crate::fdio::kinakaze_abi_pipe2(pipe.as_mut_ptr(), kinakaze_vfs::fs::O_CLOEXEC) }
        < 0
    {
        return spawn_error();
    }
    // Even a close/dup/open action which references a currently unused number
    // must not collide with our error channel. fcntl enforces RLIMIT_NOFILE.
    let writer = if pipe[1] >= floor {
        pipe[1]
    } else {
        let duplicate = unsafe { crate::fdio::kinakaze_abi_fcntl64(pipe[1], 1030, floor as usize) };
        let allocation_error = spawn_error();
        crate::kinakaze_abi_close(pipe[1]);
        if duplicate < 0 {
            crate::kinakaze_abi_close(pipe[0]);
            return allocation_error;
        }
        duplicate
    };
    let old_mask =
        match kinakaze_vfs::signal::sigprocmask(kinakaze_vfs::signal::SIG_BLOCK, u64::MAX) {
            Ok(mask) => mask,
            Err(error) => {
                crate::kinakaze_abi_close(pipe[0]);
                crate::kinakaze_abi_close(writer);
                return error;
            }
        };
    let child = crate::kinakaze_abi_fork();
    if child == 0 {
        crate::kinakaze_abi_close(pipe[0]);
        if let Err(error) = spawn_apply_attributes(&attr).and_then(|()| {
            let actions = unsafe { storage::read(file_actions)? };
            spawn_apply_file_actions(&actions, writer)
        }) {
            spawn_report_error(writer, error);
        }
        let child_mask = if attr.flags & SPAWN_SETSIGMASK != 0 {
            attr.sigmask.bits[0]
        } else {
            old_mask
        };
        if let Err(error) =
            kinakaze_vfs::signal::sigprocmask(kinakaze_vfs::signal::SIG_SETMASK, child_mask)
        {
            spawn_report_error(writer, error);
        }
        let mut access_denied = false;
        let mut error = kinakaze_vfs::ENOENT;
        for candidate in candidates.iter() {
            unsafe { kinakaze_abi_execve(candidate.as_ptr(), argv, envp) };
            error = spawn_error();
            match error {
                kinakaze_vfs::EACCES => access_denied = true,
                kinakaze_vfs::ENOENT | kinakaze_vfs::ENOTDIR => {}
                _ => spawn_report_error(writer, error),
            }
        }
        spawn_report_error(
            writer,
            if access_denied {
                kinakaze_vfs::EACCES
            } else {
                error
            },
        );
    }
    let fork_error = spawn_error();
    let _ = kinakaze_vfs::signal::sigprocmask(kinakaze_vfs::signal::SIG_SETMASK, old_mask);
    crate::kinakaze_abi_close(writer);
    if child < 0 {
        crate::kinakaze_abi_close(pipe[0]);
        return fork_error;
    }

    let mut error_bytes = [0u8; core::mem::size_of::<c_int>()];
    let mut received = 0;
    let mut pipe_error = 0;
    while received < error_bytes.len() {
        let count = unsafe {
            crate::kinakaze_abi_read(
                pipe[0],
                error_bytes.as_mut_ptr().add(received).cast(),
                error_bytes.len() - received,
            )
        };
        if count > 0 {
            received += count as usize;
        } else if count == 0 {
            break;
        } else if kinakaze_tls::errno() == kinakaze_vfs::EINTR {
            continue;
        } else {
            pipe_error = spawn_error();
            break;
        }
    }
    crate::kinakaze_abi_close(pipe[0]);
    if pipe_error != 0 || (received != 0 && received != error_bytes.len()) {
        // An internal channel failure cannot leave an unreported live child.
        crate::signal::kinakaze_abi_kill(child, kinakaze_vfs::signal::SIGKILL);
        spawn_reap_failed(child);
        return if pipe_error != 0 {
            pipe_error
        } else {
            kinakaze_vfs::EIO
        };
    }
    if received != 0 {
        spawn_reap_failed(child);
        let error = c_int::from_ne_bytes(error_bytes);
        return if error > 0 { error } else { kinakaze_vfs::EIO };
    }
    if !pid.is_null() {
        unsafe {
            *pid = child;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawn(
    pid: *mut c_int,
    path: *const c_char,
    file_actions: *const PosixSpawnFileActions,
    attrp: *const PosixSpawnAttr,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    unsafe { spawn_managed(pid, path, file_actions, attrp, argv, envp, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_spawnp(
    pid: *mut c_int,
    file: *const c_char,
    file_actions: *const PosixSpawnFileActions,
    attrp: *const PosixSpawnAttr,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    unsafe { spawn_managed(pid, file, file_actions, attrp, argv, envp, true) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_linux_x64_abi_layout_and_initializers_do_not_overwrite_guest_storage() {
        use core::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<SchedParam>(), 4);
        assert_eq!(size_of::<PosixSpawnAttr>(), 336);
        assert_eq!(align_of::<PosixSpawnAttr>(), 8);
        assert_eq!(offset_of!(PosixSpawnAttr, flags), 0);
        assert_eq!(offset_of!(PosixSpawnAttr, pgroup), 4);
        assert_eq!(offset_of!(PosixSpawnAttr, sigdefault), 8);
        assert_eq!(offset_of!(PosixSpawnAttr, sigmask), 136);
        assert_eq!(offset_of!(PosixSpawnAttr, schedparam), 264);
        assert_eq!(offset_of!(PosixSpawnAttr, schedpolicy), 268);
        assert_eq!(offset_of!(PosixSpawnAttr, cgroup), 272);
        assert_eq!(size_of::<PosixSpawnFileActions>(), 80);
        assert_eq!(offset_of!(PosixSpawnFileActions, allocated), 0);
        assert_eq!(offset_of!(PosixSpawnFileActions, used), 4);
        assert_eq!(offset_of!(PosixSpawnFileActions, actions), 8);
        assert_eq!(offset_of!(PosixSpawnFileActions, _pad), 16);

        const CANARY: u64 = 0x1234_5678_9abc_def0;
        let mut storage = [CANARY; 44];
        let attr = unsafe { storage.as_mut_ptr().add(1).cast::<PosixSpawnAttr>() };
        assert_eq!(unsafe { kinakaze_abi_posix_spawnattr_init(attr) }, 0);
        assert_eq!(storage[0], CANARY);
        assert_eq!(storage[43], CANARY);
        assert!(storage[1..43].iter().all(|word| *word == 0));

        let mut storage = [CANARY; 12];
        let actions = unsafe { storage.as_mut_ptr().add(1).cast::<PosixSpawnFileActions>() };
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawn_file_actions_init(actions) },
            0
        );
        assert_eq!(storage[0], CANARY);
        assert_eq!(storage[11], CANARY);
        assert!(storage[3..11].iter().all(|word| *word == 0));
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawn_file_actions_destroy(actions) },
            0
        );
        assert!(storage[1..11].iter().all(|word| *word == 0));
        assert_eq!(storage[0], CANARY);
        assert_eq!(storage[11], CANARY);
    }

    #[test]
    fn spawn_attribute_masks_flags_and_getters_use_gnu_layout() {
        let mut attr: PosixSpawnAttr = unsafe { core::mem::zeroed() };
        let mut mask = crate::signal::SigSet { bits: [0; 16] };
        mask.bits[0] = 0x800;
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_setsigmask(&mut attr, &mask) },
            0
        );
        mask.bits[0] = 0x1000;
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_setsigdefault(&mut attr, &mask) },
            0
        );
        let bytes = ptr::from_ref(&attr).cast::<u8>();
        assert_eq!(unsafe { bytes.add(8).cast::<u64>().read() }, 0x1000);
        assert_eq!(unsafe { bytes.add(136).cast::<u64>().read() }, 0x800);
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_getsigmask(&attr, &mut mask) },
            0
        );
        assert_eq!(mask.bits[0], 0x800);
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_getsigdefault(&attr, &mut mask) },
            0
        );
        assert_eq!(mask.bits[0], 0x1000);
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_setflags(&mut attr, SPAWN_SETSIGMASK) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_setflags(&mut attr, 0x4000) },
            EINVAL
        );
        let mut flags = -1;
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_getflags(&attr, &mut flags) },
            0
        );
        assert_eq!(flags, SPAWN_SETSIGMASK);
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_getflags(&attr, ptr::null_mut()) },
            EINVAL
        );
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_setschedpolicy(&mut attr, 1) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawnattr_setschedpolicy(&mut attr, 3) },
            EINVAL
        );
        assert_eq!(attr.schedpolicy, 1);
    }

    #[test]
    fn spawn_scheduler_does_not_report_success_for_unapplied_realtime_policy() {
        let mut attr: PosixSpawnAttr = unsafe { core::mem::zeroed() };
        attr.flags = SPAWN_SETSCHEDULER;
        assert_eq!(spawn_scheduler_check(&attr), Ok(()));
        attr.schedparam.sched_priority = 1;
        assert_eq!(spawn_scheduler_check(&attr), Err(EINVAL));
        attr.schedpolicy = 1;
        assert_eq!(spawn_scheduler_check(&attr), Err(kinakaze_vfs::EPERM));
        attr.schedparam.sched_priority = 100;
        assert_eq!(spawn_scheduler_check(&attr), Err(EINVAL));
        attr.flags = SPAWN_SETSCHEDPARAM;
        attr.schedparam.sched_priority = 0;
        assert_eq!(spawn_scheduler_check(&attr), Ok(()));
    }

    #[test]
    fn spawnp_obeys_only_path_and_preserves_empty_components() {
        let paths = spawn_candidates(c"busybox", Some(c":/custom::relative:"));
        let bytes: Vec<&[u8]> = paths.iter().map(|path| path.as_bytes()).collect();
        assert_eq!(
            bytes,
            [
                b"busybox".as_slice(),
                b"/custom/busybox",
                b"busybox",
                b"relative/busybox",
                b"busybox"
            ]
        );
        assert_eq!(
            spawn_candidates(c"sub/tool", Some(c"/other")),
            [c"sub/tool".to_owned()]
        );
        assert_eq!(
            spawn_candidates(c"tool", Some(c"a;b")),
            [c"a;b/tool".to_owned()]
        );
        assert_eq!(spawn_candidates(c"tool", None), [c"tool".to_owned()]);
    }

    #[test]
    fn spawn_file_action_storage_counts_and_copies_paths() {
        let mut fa: PosixSpawnFileActions = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawn_file_actions_init(&mut fa) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawn_file_actions_addclose(&mut fa, -1) },
            kinakaze_vfs::EBADF
        );
        assert_eq!(fa.used, 0);
        let mut path = b"child-dir\0".to_vec();
        assert_eq!(
            unsafe {
                kinakaze_abi_posix_spawn_file_actions_addchdir_np(&mut fa, path.as_ptr().cast())
            },
            0
        );
        path[0] = b'X';
        let entries = unsafe { storage::read(&fa) }.unwrap();
        assert!(
            matches!(&entries[0], FileAction::Chdir(value) if value.as_bytes() == b"child-dir")
        );
        assert_eq!(fa.used, 1);
        assert!(fa.allocated >= fa.used);
        assert_eq!(
            unsafe { kinakaze_abi_posix_spawn_file_actions_destroy(&mut fa) },
            0
        );
    }

    #[test]
    fn spawn_file_actions_duplicate_same_fd_clears_cloexec_and_errors_are_not_ignored() {
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe {
                crate::fdio::kinakaze_abi_pipe2(pipe.as_mut_ptr(), kinakaze_vfs::fs::O_CLOEXEC)
            },
            0
        );
        let actions = [FileAction::Dup2(pipe[1], pipe[1])];
        assert_eq!(
            unsafe { crate::fdio::kinakaze_abi_fcntl64(pipe[1], 1, 0) },
            1
        );
        assert_eq!(spawn_apply_file_actions(&actions, -1), Ok(()));
        assert_eq!(
            unsafe { crate::fdio::kinakaze_abi_fcntl64(pipe[1], 1, 0) },
            0
        );
        assert!(spawn_error_fd_floor(&actions).unwrap() > pipe[1]);
        crate::kinakaze_abi_close(pipe[1]);
        assert_eq!(
            spawn_apply_file_actions(&actions, -1),
            Err(kinakaze_vfs::EBADF)
        );
        assert_eq!(
            spawn_apply_file_actions(&[FileAction::Close(pipe[1])], -1),
            Ok(())
        );
        crate::kinakaze_abi_close(pipe[0]);
    }
}
