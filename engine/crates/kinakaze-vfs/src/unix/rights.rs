//! Init-owned SCM_RIGHTS bundles. Descriptor metadata contains bounded resource
//! indices; native values never depend on the sending worker remaining alive.
use super::ancillary::Descriptor;
use super::*;
use crate::fs::object::Object;
use crate::state_codec::{Reader, bytes, word};
use kinakaze_v2_protocol::kernel::{
    KernelCommand, MAX_INLINE_RESOURCES, MAX_NATIVE_RESOURCES, RESOURCE_MAGIC, ResourceInput,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
};

fn request(command: KernelCommand) -> Result<Vec<u64>, i32> {
    (kinakaze_runtime::authority::get().ok_or(EOPNOTSUPP)?.kernel)(command)
}
struct Owner {
    pid: u32,
    process: Object,
}
fn owner() -> Result<Arc<Owner>, i32> {
    static OWNER: Mutex<Option<(u32, u64, Arc<Owner>)>> = Mutex::new(None);
    let local_pid = std::process::id();
    let domain = kinakaze_runtime::authority::domain_id();
    let mut owner = OWNER.lock().map_err(|_| EIO)?;
    if let Some((pid, saved_domain, identity)) = &*owner {
        if *pid == local_pid && *saved_domain == domain {
            return Ok(identity.clone());
        }
    }
    let reply = request(KernelCommand::ResourceOwner)?;
    if reply.len() != 2 {
        return Err(EIO);
    }
    let pid = u32::try_from(reply[0]).map_err(|_| EIO)?;
    let process = Object::owned(unsafe {
        OpenProcess(
            PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    })?;
    if ancillary::creation_time(process.raw())? != reply[1] {
        return Err(EIO);
    }
    let identity = Arc::new(Owner { pid, process });
    *owner = Some((local_pid, domain, identity.clone()));
    Ok(identity)
}

enum Native {
    Handle(Object, u64),
    Socket(crate::socket::RightsSocket, Vec<u8>),
    Wake(Object),
}
#[derive(Default)]
struct Prepared(Vec<Native>);
impl Prepared {
    fn push(&mut self, native: Native) -> Result<u64, i32> {
        if self.0.len() == MAX_NATIVE_RESOURCES {
            return Err(crate::EMSGSIZE);
        }
        self.0.push(native);
        Ok(self.0.len() as u64)
    }
    fn handle(&mut self, raw: u64, dependency: u64) -> Result<u64, i32> {
        self.push(Native::Handle(
            Object::duplicate(raw as HANDLE)?,
            dependency,
        ))
    }
    fn pins(&mut self, pins: impl IntoIterator<Item = Object>) -> Result<(), i32> {
        for pin in pins {
            self.push(Native::Handle(pin, 0))?;
        }
        Ok(())
    }
    fn publish(&self, owner: u64, id: u64) -> Result<(), i32> {
        let mut data = Vec::new();
        word(&mut data, RESOURCE_MAGIC);
        word(&mut data, self.0.len() as u64);
        for native in &self.0 {
            match native {
                Native::Handle(handle, dependency) => {
                    word(&mut data, 0);
                    word(&mut data, handle.raw() as u64);
                    word(&mut data, *dependency);
                }
                Native::Socket(_pin, protocol) => {
                    word(&mut data, 1);
                    bytes(&mut data, protocol);
                }
                Native::Wake(event) => {
                    word(&mut data, 2);
                    word(&mut data, event.raw() as u64);
                }
            }
        }
        if data.len() <= MAX_INLINE_RESOURCES {
            return request(KernelCommand::RetainResources {
                owner,
                id,
                input: ResourceInput::Inline(data),
            })
            .map(|_| ());
        }
        let section = Object::owned(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                data.len() as u32,
                std::ptr::null(),
            )
        })?;
        let view = unsafe { MapViewOfFile(section.raw(), FILE_MAP_ALL_ACCESS, 0, 0, data.len()) };
        if view.Value.is_null() {
            return Err(EIO);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), view.Value.cast(), data.len());
            UnmapViewOfFile(view);
        }
        request(KernelCommand::RetainResources {
            owner,
            id,
            input: ResourceInput::Section {
                source: section.raw() as u64,
                length: data.len() as u64,
            },
        })
        .map(|_| ())
    }
    fn capture(&mut self, fd: i32, target_pid: u32) -> Result<Descriptor, i32> {
        let info = crate::get(fd)?;
        if !ancillary::supported(info.kind) {
            return Err(EOPNOTSUPP);
        }
        let shared = crate::ofd::promote(fd)?;
        let terminal = if matches!(info.kind, FdKind::PtyMaster | FdKind::PtySlave) {
            Some(crate::tty::resolve(fd)?)
        } else {
            None
        };
        let unix = if info.kind == FdKind::UnixSocket {
            Some(snapshot(fd)?)
        } else {
            None
        };
        let synthetic = if matches!(
            info.kind,
            FdKind::Synthetic
                | FdKind::SyntheticDirectory
                | FdKind::CgroupFile
                | FdKind::ProcSysctl
        ) {
            Some(crate::export_synthetic_rights(fd, info)?)
        } else {
            None
        };
        let bpf = if info.kind == FdKind::BpfProgram {
            Some(crate::bpf::rights_reference(fd)?)
        } else {
            None
        };
        let table = crate::table().read().map_err(|_| EIO)?;
        let entry = table.slots.get(fd as usize).and_then(|e| *e).ok_or(EBADF)?;
        if entry.generation != info.generation
            || entry.raw != info.raw
            || unix.as_ref().is_some_and(|s| s.handle != entry.raw)
        {
            return Err(EBADF);
        }
        self.pins([shared.pin()?])?;
        if let Some((terminal, side)) = terminal {
            self.pins(terminal.rights_pins(side)?)?;
        }
        let raw = if entry.kind == FdKind::Socket {
            let pin = crate::socket::import_rights(&crate::socket::export_rights(
                entry.raw,
                std::process::id(),
            )?)?;
            let recipe = crate::socket::export_rights(pin.0, target_pid)?;
            self.push(Native::Socket(pin, recipe))?
        } else if entry.raw != 0 {
            self.handle(entry.raw as u64, 0)?
        } else {
            0
        };
        let pipe = crate::pipe_inode::reference(entry)?;
        let metadata = match entry.kind {
            FdKind::File | FdKind::Directory => {
                let overlay = crate::mount::overlay::reference(entry)?;
                let mut data = Vec::new();
                word(&mut data, u64::from(overlay.is_some()));
                data.extend(if let Some(overlay) = overlay {
                    crate::mount::overlay::export_rights(&overlay, |raw| self.handle(raw, 0))?
                } else {
                    crate::mount::native::export_rights_reference(
                        crate::mount::native::reference(entry)?,
                        |raw| self.handle(raw, 0),
                    )?
                });
                data
            }
            FdKind::Pipe => {
                crate::pipe_inode::export_rights(pipe.clone(), |raw| self.handle(raw, 0))?
            }
            FdKind::Fifo => {
                let marker = crate::fifo::rights_reference(entry)?.ok_or(EBADF)?;
                let mut data = Vec::new();
                word(&mut data, self.handle(marker.raw(), 0)?);
                data
            }
            FdKind::UnixSocket => {
                let socket = unix.as_ref().ok_or(EBADF)?;
                self.pins([socket._network_pin.pin()?])?;
                if let Some(ancillary) = &socket.ancillary {
                    for pin in ancillary.pins() {
                        self.handle(pin.raw() as u64, 0)?;
                    }
                }
                ancillary::export_socket(socket, |raw| {
                    self.handle(
                        raw,
                        if raw == socket.record.pin.raw() as u64 {
                            socket.record.id()
                        } else {
                            0
                        },
                    )
                })?
            }
            FdKind::Socket => {
                let network = crate::usernet::rights_reference(entry)?;
                let mut data = Vec::new();
                word(
                    &mut data,
                    network.as_ref().map_or(0, |n| crate::usernet::rights_id(n)),
                );
                let token = if let Some(network) = network {
                    self.pins(crate::usernet::rights_pins(&network)?)?;
                    crate::usernet::rights_token(&network)?
                        .map(|p| self.handle(p.raw() as u64, 0))
                        .transpose()?
                        .unwrap_or(0)
                } else {
                    0
                };
                word(&mut data, token);
                data
            }
            _ => {
                if let Some(bpf) = bpf {
                    let mut data = Vec::new();
                    word(&mut data, bpf.id() as u64);
                    data
                } else {
                    synthetic.unwrap_or_default()
                }
            }
        };
        let anonymous = if matches!(entry.kind, FdKind::Socket | FdKind::UnixSocket) {
            crate::pipe_inode::export_rights(pipe, |raw| self.handle(raw, 0))?
        } else {
            Vec::new()
        };
        Ok(Descriptor {
            description: entry.description_id,
            raw,
            kind: entry.kind.fork_code(),
            flags: entry.flags.0 & !(FdFlags::CLOSE_ON_EXEC.0 | FdFlags::BORROWED.0),
            shared: shared.id(),
            metadata,
            anonymous,
        })
    }
}

#[derive(Clone)]
pub(super) struct Record {
    owner: u64,
    id: u64,
    descriptors: Vec<Descriptor>,
}
pub(super) struct Pending {
    record: Record,
    committed: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.committed {
            self.record.release();
        }
    }
}
impl Pending {
    pub(super) fn record(&self) -> Record {
        self.record.clone()
    }
    pub(super) fn commit(mut self) {
        self.committed = true;
    }
}
pub(super) fn export(owner_id: u64, rights: &[i32]) -> Result<Pending, i32> {
    if rights.len() > 253 {
        return Err(EINVAL);
    }
    let pid = owner()?.pid;
    let mut prepared = Prepared::default();
    let descriptors = rights
        .iter()
        .map(|fd| prepared.capture(*fd, pid))
        .collect::<Result<Vec<_>, _>>()?;
    let record = Record {
        owner: owner_id,
        id: ancillary::random_id()?,
        descriptors,
    };
    let pending = Pending {
        record,
        committed: false,
    };
    prepared.publish(pending.record.owner, pending.record.id)?;
    Ok(pending)
}
/// Listener admission uses the same native custody without guest fd metadata.
pub(super) fn retain_native(
    owner: u64,
    id: u64,
    handles: &[u64],
    wake: HANDLE,
) -> Result<Pending, i32> {
    let mut prepared = Prepared::default();
    for &handle in handles {
        prepared.handle(handle, 0)?;
    }
    prepared.push(Native::Wake(Object::duplicate(wake)?))?;
    let pending = Pending {
        record: Record {
            owner,
            id,
            descriptors: Vec::new(),
        },
        committed: false,
    };
    prepared.publish(owner, id)?;
    Ok(pending)
}
pub(super) fn release_native(owner: u64, id: u64) {
    let _ = request(KernelCommand::ReleaseResources { owner, id });
}
pub(super) fn duplicate_native(owner_id: u64, id: u64) -> Result<Vec<Object>, i32> {
    let owner = owner()?;
    let handles = request(KernelCommand::Resources {
        owner: owner_id,
        id,
    })?;
    handles
        .into_iter()
        .filter(|raw| *raw != 0)
        .map(|raw| ancillary::duplicate(owner.process.raw(), raw, unsafe { GetCurrentProcess() }))
        .collect()
}
impl Record {
    pub(super) fn read(input: &mut Reader<'_>) -> Result<Self, i32> {
        let owner = input.word()?;
        let id = input.word()?;
        let count = input.word()?;
        if owner == 0 || id == 0 || count > 253 {
            return Err(EIO);
        }
        let descriptors = (0..count)
            .map(|_| Descriptor::read(input))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            owner,
            id,
            descriptors,
        })
    }
    pub(super) fn write(&self, output: &mut Vec<u8>) {
        word(output, self.owner);
        word(output, self.id);
        word(output, self.descriptors.len() as u64);
        for descriptor in &self.descriptors {
            descriptor.write(output);
        }
    }
    pub(super) fn release(&self) {
        let _ = request(KernelCommand::ReleaseResources {
            owner: self.owner,
            id: self.id,
        });
    }
    pub(super) fn receive(&self, capacity: usize, flags: i32) -> (Vec<i32>, bool) {
        if capacity == 0 {
            return (Vec::new(), !self.descriptors.is_empty());
        }
        let result = (|| {
            let owner = owner()?;
            let handles = request(KernelCommand::Resources {
                owner: self.owner,
                id: self.id,
            })?;
            let duplicate = |index: u64| -> Result<Object, i32> {
                let raw = *handles
                    .get(
                        usize::try_from(index)
                            .map_err(|_| EIO)?
                            .checked_sub(1)
                            .ok_or(EIO)?,
                    )
                    .ok_or(EIO)?;
                if raw == 0 {
                    return Err(EIO);
                }
                ancillary::duplicate(owner.process.raw(), raw, unsafe { GetCurrentProcess() })
            };
            let mut received = Vec::new();
            let mut truncated = false;
            for descriptor in &self.descriptors {
                if received.len() == capacity {
                    truncated = true;
                    break;
                }
                let result = (|| {
                    let kind = FdKind::from_fork_code(descriptor.kind);
                    let socket = if kind == FdKind::Socket {
                        let recipe = request(KernelCommand::ResourceSocket {
                            owner: self.owner,
                            id: self.id,
                            index: descriptor.raw.checked_sub(1).ok_or(EIO)? as usize,
                        })?;
                        let recipe = recipe
                            .into_iter()
                            .map(|v| u8::try_from(v).map_err(|_| EIO))
                            .collect::<Result<Vec<_>, _>>()?;
                        Some(crate::socket::import_rights(&recipe)?)
                    } else {
                        None
                    };
                    let object = if socket.is_none() && descriptor.raw != 0 {
                        Some(duplicate(descriptor.raw)?)
                    } else {
                        None
                    };
                    let flags = FdFlags(
                        (descriptor.flags & !FdFlags::CLOSE_ON_EXEC.0)
                            | if flags & 0x40000000 != 0 {
                                FdFlags::CLOSE_ON_EXEC.0
                            } else {
                                0
                            },
                    );
                    let raw = socket
                        .as_ref()
                        .map_or_else(|| object.as_ref().map_or(0, |o| o.raw() as usize), |s| s.0);
                    let fd = crate::install_received(raw, kind, flags, descriptor.description)?;
                    if let Some(object) = object {
                        object.into_raw();
                    }
                    if let Some(socket) = socket {
                        socket.into_raw();
                    }
                    if let Err(error) = ancillary::activate_with(fd, descriptor, duplicate) {
                        let _ = crate::close(fd);
                        return Err(error);
                    }
                    Ok(fd)
                })();
                match result {
                    Ok(fd) => received.push(fd),
                    Err(_) => {
                        truncated = true;
                        break;
                    }
                }
            }
            Ok((received, truncated))
        })();
        result.unwrap_or_else(|_: i32| (Vec::new(), !self.descriptors.is_empty()))
    }
}
