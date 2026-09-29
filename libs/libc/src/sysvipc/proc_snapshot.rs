use super::*;
use std::fmt::Write;

struct Section {
    mapping: *mut c_void,
    control: *mut c_void,
}

impl Drop for Section {
    fn drop(&mut self) {
        release_section(self.mapping, self.control);
    }
}

fn read() -> Result<Vec<u8>, i32> {
    let mut output =
        String::from("key shmid perms size cpid lpid nattch uid gid cuid cgid atime dtime ctime\n");
    for id in registry::ids(IpcKind::Memory)? {
        let (mapping, control) = match registry::open(IpcKind::Memory, id) {
            Ok(section) => section,
            Err(EINVAL) => {
                registry::forget(IpcKind::Memory, id);
                continue;
            }
            Err(error) => return Err(error),
        };
        let section = Section { mapping, control };
        let name = registry::name(IpcKind::Memory, ipc_namespace(), id);
        let guard = CrossProcessLock::acquire(Some(&format!("{name}.lock")))?;
        let header = unsafe { &mut *section.control.cast::<ShmHeader>() };
        collect_attachments(header);
        let removed = header.removed.load(Ordering::Acquire) != 0;
        if removed && header.nattch == 0 {
            drop(guard);
            registry::forget(IpcKind::Memory, id);
            continue;
        }
        writeln!(
            output,
            "{} {} {:o} {} {} {} {} 0 0 0 0 {} {} {}",
            header.key,
            id,
            header.mode | if removed { 0o1000 } else { 0 },
            header.segsz,
            header.cpid,
            header.lpid,
            header.nattch,
            header.atime,
            header.dtime,
            header.ctime
        )
        .map_err(|_| EIO)?;
    }
    Ok(output.into_bytes())
}

extern "C" fn initialize() {
    kinakaze_vfs::procfs::install_shm_reader(read);
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = initialize;
