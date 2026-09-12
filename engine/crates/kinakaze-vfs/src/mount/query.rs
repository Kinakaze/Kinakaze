//! Binary mount queries use the same live attachment records as mountinfo.
use super::*;
use crate::procfs::mounts::{self, Record};
use crate::{ENOENT, EOVERFLOW};

const IDMAP_MASK: u64 = 0x2000 | 0x4000;
pub const SUPPORTED_MASK: u64 =
    1 | 2 | 8 | 16 | 32 | 64 | 128 | 256 | 512 | 1024 | 4096 | IDMAP_MASK;
pub fn unique_id(namespace: u64, id: u64) -> Result<u64, i32> {
    if namespace > u32::MAX as u64 || id > u32::MAX as u64 {
        return Err(EOVERFLOW);
    }
    Ok((namespace << 32) | id)
}
fn current() -> Result<(u64, Vec<Record>), i32> {
    Ok((namespace_id()?, mounts::records(crate::job::process_id())?))
}
pub fn list(root: u64, last: u64, count: usize, reverse: bool) -> Result<Vec<u64>, i32> {
    if count > 1_000_000 {
        return Err(EOVERFLOW);
    }
    if last != 0 && last <= 1u64 << 32 {
        return Err(EINVAL);
    }
    let (ns, records) = current()?;
    let wanted = if root == u64::MAX {
        None
    } else {
        Some(
            records
                .iter()
                .find(|r| unique_id(ns, r.id) == Ok(root))
                .ok_or(ENOENT)?
                .id,
        )
    };
    let mut selected = std::collections::HashSet::new();
    if let Some(root) = wanted {
        selected.insert(root);
        for r in &records {
            if selected.contains(&r.parent) {
                selected.insert(r.id);
            }
        }
    }
    let mut ids = Vec::new();
    for record in records {
        if wanted == Some(record.id) || wanted.is_some() && !selected.contains(&record.id) {
            continue;
        }
        let id = unique_id(ns, record.id)?;
        if last == 0 || if reverse { id < last } else { id > last } {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    if reverse {
        ids.reverse();
    }
    ids.truncate(count);
    Ok(ids)
}
fn u32_at(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn u64_at(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn append(bytes: &mut Vec<u8>, at: usize, text: &str) -> Result<(), i32> {
    // Offset zero always identifies an empty string, including unset fields.
    if bytes.len() == 512 {
        bytes.push(0);
    }
    let offset = u32::try_from(bytes.len() - 512).map_err(|_| EOVERFLOW)?;
    u32_at(bytes, at, offset);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    Ok(())
}
pub fn stat(id: u64, mask: u64) -> Result<Vec<u8>, i32> {
    let (ns, records) = current()?;
    let record = records
        .iter()
        .find(|r| unique_id(ns, r.id) == Ok(id))
        .ok_or(ENOENT)?;
    encode(record, ns, mask)
}
pub fn stat_fd(fd: i32, mask: u64) -> Result<Vec<u8>, i32> {
    let entry = crate::get(fd)?;
    if entry.kind == crate::FdKind::MountTree || overlay::directory_anchor(fd)?.is_some() {
        let id = api::tree_id(fd, "/")?;
        match stat(id, mask) {
            Ok(bytes) => return Ok(bytes),
            Err(ENOENT) => {}
            Err(e) => return Err(e),
        }
        let mut bytes = encode(&api::tree_record(fd)?, namespace_id()?, mask)?;
        if mask & 64 != 0 {
            u64_at(&mut bytes, 112, 0);
        }
        return Ok(bytes);
    }
    let path = crate::procfs::local_fd_link_target(fd)?;
    stat(id_for_path(&path)?, mask)
}
fn encode(r: &Record, ns: u64, requested: u64) -> Result<Vec<u8>, i32> {
    let caller = if r.idmap.is_some() && requested & IDMAP_MASK != 0 {
        Some(crate::user_namespace::current_map()?)
    } else {
        None
    };
    encode_with_map(r, ns, requested, caller.as_ref())
}
fn encode_with_map(
    r: &Record,
    ns: u64,
    requested: u64,
    caller: Option<&crate::user_namespace::Mapping>,
) -> Result<Vec<u8>, i32> {
    let mut bytes = vec![0; 512];
    let mask = requested
        & SUPPORTED_MASK
        & if r.idmap.is_some() {
            u64::MAX
        } else {
            !IDMAP_MASK
        };
    u64_at(&mut bytes, 8, mask);
    if mask & 1 != 0 {
        u32_at(
            &mut bytes,
            16,
            (((r.device >> 8) & 0xfff) | ((r.device >> 32) & 0xfffff000)) as u32,
        );
        u32_at(
            &mut bytes,
            20,
            ((r.device & 0xff) | ((r.device >> 12) & 0xffffff00)) as u32,
        );
        u64_at(
            &mut bytes,
            24,
            if r.filesystem == "overlay" {
                0x794c7630
            } else {
                0x5346544e
            },
        );
        u32_at(&mut bytes, 32, u32::from(r.super_options.starts_with("ro")));
    }
    if mask & 2 != 0 {
        u64_at(&mut bytes, 40, unique_id(ns, r.id)?);
        u64_at(&mut bytes, 48, unique_id(ns, r.parent)?);
        u32_at(&mut bytes, 56, r.id as u32);
        u32_at(&mut bytes, 60, r.parent as u32);
        let flags = r.flags;
        let attrs = (flags & 15)
            | if flags & MS_IDMAPPED != 0 {
                0x100000
            } else {
                0
            }
            | if flags & 256 != 0 { 0x200000 } else { 0 }
            | if flags & 1024 != 0 { 0x10 } else { 0 }
            | if flags & (1 << 24) != 0 { 0x20 } else { 0 }
            | if flags & 2048 != 0 { 0x80 } else { 0 };
        u64_at(
            &mut bytes,
            64,
            attrs | u64::from(r.options.starts_with("ro")),
        );
        u64_at(
            &mut bytes,
            72,
            if flags & MS_PROPAGATION == 0 {
                MS_PRIVATE
            } else {
                flags & MS_PROPAGATION
            },
        );
        if flags & MS_SHARED != 0 {
            u64_at(&mut bytes, 80, r.peer);
        }
        if flags & MS_SLAVE != 0 {
            u64_at(&mut bytes, 88, r.master);
        }
    }
    if mask & 8 != 0 {
        append(&mut bytes, 104, &r.root)?;
    }
    if mask & 16 != 0 {
        append(&mut bytes, 108, &r.target)?;
    }
    if mask & 32 != 0 {
        append(&mut bytes, 36, &r.filesystem)?;
    }
    if mask & 64 != 0 {
        u64_at(&mut bytes, 112, ns);
    }
    if mask & 128 != 0 {
        append(&mut bytes, 4, &r.super_options)?;
    }
    if mask & 256 != 0 {
        append(&mut bytes, 120, "")?;
    }
    if mask & 512 != 0 {
        append(&mut bytes, 124, &r.source)?;
    }
    if mask & 1024 != 0 {
        let options: Vec<_> = r.super_options.split(',').collect();
        u32_at(&mut bytes, 128, options.len() as u32);
        if bytes.len() == 512 {
            bytes.push(0);
        }
        let offset = (bytes.len() - 512) as u32;
        u32_at(&mut bytes, 132, offset);
        for option in options {
            bytes.extend_from_slice(option.as_bytes());
            bytes.push(0);
        }
    }
    if mask & 4096 != 0 {
        u64_at(&mut bytes, 144, SUPPORTED_MASK);
    }
    if mask & IDMAP_MASK != 0 {
        let mapping = r.idmap.as_ref().ok_or(EIO)?;
        let caller = caller.ok_or(EIO)?;
        for (bit, group, count_at, offset_at, rows) in [
            (0x2000, false, 152, 156, &mapping.uid),
            (0x4000, true, 160, 164, &mapping.gid),
        ] {
            if mask & bit == 0 {
                continue;
            }
            let mut count = 0;
            for row in rows {
                let Some(lower) = caller.range_up(row[1], row[2], group) else {
                    continue;
                };
                if count == 0 && bytes.len() == 512 {
                    bytes.push(0);
                }
                if count == 0 {
                    let offset = u32::try_from(bytes.len() - 512).map_err(|_| EOVERFLOW)?;
                    u32_at(&mut bytes, offset_at, offset);
                }
                bytes.extend_from_slice(format!("{} {} {}", row[0], lower, row[2]).as_bytes());
                bytes.push(0);
                count += 1;
            }
            u32_at(&mut bytes, count_at, count);
        }
    }
    let size = u32::try_from(bytes.len()).map_err(|_| EOVERFLOW)?;
    u32_at(&mut bytes, 0, size);
    Ok(bytes)
}

pub fn id_for_path(path: &str) -> Result<u64, i32> {
    if let Some((fd, tail)) = api::tree_reference(path) {
        return api::tree_id(fd, &format!("/{tail}"));
    }
    let path = namespace_path(&crate::fs::absolute_linux(path))?;
    unique_id(
        namespace_id()?,
        visible_mount_id(&path)?.unwrap_or(ROOT_MOUNT_ID),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_layout_keeps_strings_outside_fixed_header() {
        let r = Record {
            idmap: None,
            peer: 0,
            master: 0,
            id: 64,
            parent: 1,
            device: 0,
            root: "/".into(),
            target: "/mnt/space dir".into(),
            filesystem: "overlay".into(),
            source: "overlay".into(),
            options: "rw".into(),
            super_options: "rw,index=on".into(),
            flags: MS_OVERLAY,
        };
        let b = encode(&r, 1, u64::MAX).unwrap();
        assert_eq!(
            u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize,
            b.len()
        );
        assert_eq!(
            u64::from_le_bytes(b[8..16].try_into().unwrap()),
            SUPPORTED_MASK & !IDMAP_MASK
        );
        let offset = u32::from_le_bytes(b[108..112].try_into().unwrap()) as usize;
        assert!(b[512 + offset..].starts_with(b"/mnt/space dir\0"));
        assert!(b[168..512].iter().all(|b| *b == 0));
    }

    #[test]
    fn idmap_queries_preserve_extent_boundaries_and_filter_callers_invisible_ranges() {
        use crate::user_namespace::Mapping;
        let r = Record {
            idmap: Some(Mapping {
                uid: vec![[0, 1000, 10], [100, 2000, 10]],
                gid: vec![[0, 3000, 20]],
            }),
            peer: 0,
            master: 0,
            id: 64,
            parent: 1,
            device: 0,
            root: "/".into(),
            target: "/mapped".into(),
            filesystem: "overlay".into(),
            source: "overlay".into(),
            options: "rw".into(),
            super_options: "rw".into(),
            flags: MS_OVERLAY | MS_IDMAPPED,
        };
        let caller = Mapping {
            uid: vec![[40, 1000, 10], [60, 2000, 5]],
            gid: vec![[50, 3000, 20]],
        };
        let b = encode_with_map(&r, 1, IDMAP_MASK, Some(&caller)).unwrap();
        let get32 = |at| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(get32(152), 1);
        assert_eq!(get32(160), 1);
        assert!(b[512 + get32(156) as usize..].starts_with(b"0 40 10\0"));
        assert!(b[512 + get32(164) as usize..].starts_with(b"0 50 20\0"));
        assert_eq!(b[512], 0);
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), IDMAP_MASK);
        let b = encode_with_map(&r, 1, IDMAP_MASK, Some(&Mapping::default())).unwrap();
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), IDMAP_MASK);
        assert!(b[152..168].iter().all(|&b| b == 0));
        // An idmapped mount with no visible extents is distinct from no idmap.
        let mut plain = r;
        plain.idmap = None;
        let b = encode_with_map(&plain, 1, IDMAP_MASK, None).unwrap();
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 0);
    }
}
