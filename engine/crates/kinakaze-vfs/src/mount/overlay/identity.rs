//! Persistent backing-file identities. NTFS file IDs include their reuse sequence.
use super::*;
use windows_sys::Win32::Storage::FileSystem::{
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};

pub(super) fn volume_uuid(object: &Object) -> Result<[u8; 16], i32> {
    let path = object.path()?;
    let path = crate::path::wide_path(&path)?;
    let mut root = vec![0u16; 32768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(EOPNOTSUPP);
    }
    let mut name = [0u16; 64];
    if unsafe {
        GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), name.len() as u32)
    } == 0
    {
        return Err(EOPNOTSUPP);
    }
    let text = String::from_utf16(&name[..name.iter().position(|v| *v == 0).ok_or(EIO)?])
        .map_err(|_| EIO)?;
    let text = text
        .split_once('{')
        .and_then(|(_, rest)| rest.split_once('}').map(|v| v.0))
        .ok_or(EIO)?;
    let digits: Vec<_> = text.bytes().filter(|b| *b != b'-').collect();
    if digits.len() != 32 {
        return Err(EIO);
    }
    let mut uuid = [0; 16];
    for (out, pair) in uuid.iter_mut().zip(digits.chunks_exact(2)) {
        *out = ((pair[0] as char).to_digit(16).ok_or(EIO)? * 16
            + (pair[1] as char).to_digit(16).ok_or(EIO)?) as u8;
    }
    Ok(uuid)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub upper: bool,
    pub uuid: [u8; 16],
    pub device: u64,
    pub inode: u64,
}
impl Identity {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut bytes = b"CYOVFH01".to_vec();
        bytes.push(u8::from(self.upper));
        bytes.extend_from_slice(&self.uuid);
        bytes.extend_from_slice(&self.device.to_le_bytes());
        bytes.extend_from_slice(&self.inode.to_le_bytes());
        bytes
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() != 41 || &bytes[..8] != b"CYOVFH01" || bytes[8] > 1 {
            return Err(crate::ESTALE);
        }
        Ok(Self {
            upper: bytes[8] != 0,
            uuid: bytes[9..25].try_into().unwrap(),
            device: u64::from_le_bytes(bytes[25..33].try_into().unwrap()),
            inode: u64::from_le_bytes(bytes[33..41].try_into().unwrap()),
        })
    }
    pub(super) fn key(&self) -> String {
        self.encode().iter().map(|v| format!("{v:02x}")).collect()
    }
}

impl Node {
    pub(super) fn filesystem_id(&self) -> Result<u64, i32> {
        let context = self.context.as_ref().ok_or(EIO)?;
        let flags = context.flags;
        let fallback = self.entries[0].metadata.st_dev;
        if !context.writable || flags & (features::UUID_OFF | features::UUID_NULL) != 0 {
            return Ok(fallback);
        }
        let attrs = unsafe { Attributes::from_handle(self.backing_object().raw(), false)? };
        let name = format!("{}uuid", self.namespace.prefix());
        let uuid = if let Some(uuid) = attrs.snapshot()?.get(name.as_bytes()) {
            uuid.clone()
        } else {
            if flags & features::UUID_ON == 0 && !self.backing_object().entries()?.is_empty() {
                return Ok(fallback);
            }
            let mut uuid = [0; 16];
            if unsafe { crate::BCryptGenRandom(std::ptr::null_mut(), uuid.as_mut_ptr(), 16, 2) } < 0
            {
                return Err(EIO);
            }
            uuid[6] = (uuid[6] & 15) | 0x40;
            uuid[8] = (uuid[8] & 63) | 0x80;
            let writer = unsafe { Attributes::from_handle(self.backing_object().raw(), true)? };
            match writer.set(name.as_bytes(), &uuid, crate::xattr::XATTR_CREATE) {
                Ok(()) => {}
                Err(crate::EEXIST) => {}
                Err(e) => return Err(e),
            }
            writer.get(name.as_bytes())?
        };
        if uuid.len() != 16 {
            return Err(EIO);
        }
        Ok(u64::from_le_bytes(uuid[..8].try_into().unwrap())
            ^ u64::from_le_bytes(uuid[8..].try_into().unwrap()))
    }

    pub(super) fn map_inode(&self, device: u64, inode: u64, overlay_device: u64) -> (u64, u64) {
        let Some(context) = &self.context else {
            return (device, inode);
        };
        let mut devices = Vec::new();
        let visible = context.roots.len() - features::data_count(context.flags);
        for root in &context.roots[..visible] {
            if !devices.contains(&root.metadata.st_dev) {
                devices.push(root.metadata.st_dev);
            }
        }
        if devices.len() == 1 {
            return (overlay_device, inode);
        }
        let enabled = context.flags & features::XINO != 0
            || context.flags & features::XINO_AUTO != 0
                && context.volumes.iter().all(|uuid| *uuid != [0; 16]);
        if enabled && let Some(index) = devices.iter().position(|candidate| *candidate == device) {
            let bits = usize::BITS - devices.len().leading_zeros();
            let shift = 64 - bits;
            if inode < (1u64 << shift) {
                return (overlay_device, inode | (((index + 1) as u64) << shift));
            }
        }
        // Overflow follows Linux's documented fallback: a non-directory keeps
        // its backing device; directories remain on the overlay superblock.
        if self.is_directory() {
            (overlay_device, inode)
        } else {
            (device, inode)
        }
    }
    pub(super) fn origin(&self) -> Result<Identity, i32> {
        let attrs =
            unsafe { Attributes::from_handle(self.backing_object().raw(), false)? }.snapshot()?;
        if let Some(bytes) = attrs.get(format!("{}origin", self.namespace.prefix()).as_bytes()) {
            return Identity::decode(bytes);
        }
        let context = self.context.as_ref().ok_or(EIO)?;
        let entry = &self.entries[0];
        let uuid = *context.volumes.get(entry.layer).ok_or(EIO)?;
        let stat = crate::fs::stat_handle(entry.object.raw(), false)?;
        let ignore_uuid = context.ignore_uuid();
        Ok(Identity {
            upper: entry.layer == 0 && context.writable,
            uuid: if ignore_uuid { [0; 16] } else { uuid },
            device: if ignore_uuid { 0 } else { stat.st_dev },
            inode: stat.st_ino,
        })
    }
}

impl LookupContext {
    pub(super) fn ignore_uuid(&self) -> bool {
        self.flags & features::UUID_OFF != 0
            && self.volumes[usize::from(self.writable)..]
                .windows(2)
                .all(|v| v[0] == v[1])
    }
}
