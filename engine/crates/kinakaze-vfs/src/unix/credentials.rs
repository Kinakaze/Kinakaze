//! Socket identity snapshots. PID numbers survive sender exit and exec; user
//! IDs are stored in the initial namespace and translated only on reception.
use crate::state_codec::{Reader, word};
use crate::{EINVAL, EIO, EPERM, ESRCH};
use kinakaze_runtime::job::namespaces as registry;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ucred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    // Most desktop IPC stays in one PID namespace. Nested namespaces spill
    // normally, without changing the serialized identity or visibility rules.
    numbers: smallvec::SmallVec<[(u64, u32); 1]>,
    uid: u32,
    gid: u32,
}
impl Sender {
    fn capture(pid: u32, uid: u32, gid: u32) -> Result<Self, i32> {
        let mut namespace = registry::memberships(pid).ok_or(ESRCH)?[registry::PID];
        let mut numbers = smallvec::SmallVec::new();
        loop {
            numbers.push((
                namespace,
                registry::visible_in(pid, namespace).ok_or(ESRCH)?,
            ));
            if namespace == 1 {
                break;
            }
            namespace = registry::pid_parent(namespace).ok_or(EIO)?;
        }
        Ok(Self { numbers, uid, gid })
    }
    pub(super) fn current(effective: bool) -> Result<Self, i32> {
        let ids = crate::credentials::identity();
        let index = usize::from(effective);
        Self::capture(crate::job::process_id(), ids.uids[index], ids.gids[index])
    }
    pub(super) fn visible(&self) -> Result<Ucred, i32> {
        let namespace = crate::namespaces::current_id(registry::PID)?;
        Ok(Ucred {
            pid: self
                .numbers
                .iter()
                .find(|(id, _)| *id == namespace)
                .map_or(0, |(_, pid)| *pid as i32),
            uid: crate::user_namespace::visible(self.uid, false),
            gid: crate::user_namespace::visible(self.gid, true),
        })
    }
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, i32> {
        let uid = u32::try_from(r.word()?).map_err(|_| EIO)?;
        let gid = u32::try_from(r.word()?).map_err(|_| EIO)?;
        let count = r.word()?;
        if count == 0 || count > 32 || count as usize > r.0.len() / 16 {
            return Err(EIO);
        }
        let mut numbers = smallvec::SmallVec::with_capacity(count as usize);
        for _ in 0..count {
            let namespace = r.word()?;
            let pid = u32::try_from(r.word()?).map_err(|_| EIO)?;
            if namespace == 0 || pid == 0 {
                return Err(EIO);
            }
            numbers.push((namespace, pid));
        }
        Ok(Self { numbers, uid, gid })
    }
    pub(super) fn write(&self, out: &mut impl Extend<u8>) {
        for value in [self.uid as u64, self.gid as u64, self.numbers.len() as u64] {
            word(out, value);
        }
        for &(namespace, pid) in &self.numbers {
            word(out, namespace);
            word(out, pid as u64);
        }
    }
}

/// Validate an explicit SCM_CREDENTIALS before committing any payload or fds.
pub fn validate(value: Ucred) -> Result<Sender, i32> {
    let uid = crate::user_namespace::kernel(value.uid, false).map_err(|_| EINVAL)?;
    let gid = crate::user_namespace::kernel(value.gid, true).map_err(|_| EINVAL)?;
    let own = crate::job::process_id();
    let current = registry::visible(own).ok_or(ESRCH)?;
    let ids = crate::credentials::identity();
    let pid_owner =
        crate::namespaces::owner(registry::PID, crate::namespaces::current_id(registry::PID)?)?;
    if (value.pid as u32 != current && !crate::user_namespace::capable(pid_owner, 21))
        || (!ids.uids.contains(&uid) && !crate::user_namespace::current_capable(7))
        || (!ids.gids.contains(&gid) && !crate::user_namespace::current_capable(6))
    {
        return Err(EPERM);
    }
    let pid = registry::resolve(value.pid as u32).ok_or(ESRCH)?;
    Sender::capture(pid, uid, gid)
}

pub(super) fn absent() -> Ucred {
    Ucred {
        pid: 0,
        uid: 65534,
        gid: 65534,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_encoding_preserves_nested_namespaces_and_clone() {
        for count in [1, 2, 32] {
            let mut bytes = Vec::new();
            for value in [40001, 41002, count] {
                word(&mut bytes, value);
            }
            for index in 0..count {
                word(&mut bytes, count - index);
                word(&mut bytes, index + 17);
            }
            let mut reader = Reader(&bytes);
            let identity = Sender::read(&mut reader).unwrap();
            reader.end().unwrap();
            let copy = identity.clone();
            drop(identity);
            let mut encoded = Vec::new();
            copy.write(&mut encoded);
            assert_eq!(encoded, bytes);
            let mut inline = smallvec::SmallVec::<[u8; 128]>::new();
            copy.write(&mut inline);
            assert_eq!(&inline[..], bytes);
            assert_eq!(inline.spilled(), count == 32);
        }
        for count in [0, 33] {
            let mut bytes = Vec::new();
            for value in [40001, 41002, count] {
                word(&mut bytes, value);
            }
            bytes.resize(24 + 33 * 16, 1);
            assert_eq!(Sender::read(&mut Reader(&bytes)), Err(EIO));
        }
    }
}
